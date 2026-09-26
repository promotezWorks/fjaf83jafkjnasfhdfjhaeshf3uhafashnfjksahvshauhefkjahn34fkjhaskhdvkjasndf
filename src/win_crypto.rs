// language: Rust, file: src/win_crypto.rs, target: Windows
// Shared Windows crypto helpers: DPAPI unwrap + AES-256-GCM decrypt.
#![allow(dead_code)]

#[cfg(windows)]
pub fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
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

#[cfg(not(windows))]
pub fn dpapi_unprotect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

/// Decrypt `nonce[0..12] || ciphertext || tag[last 16]` with AES-256-GCM.
pub fn aes_gcm_decrypt(key: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    if blob.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher
        .decrypt(Nonce::from_slice(&blob[..12]), &blob[12..])
        .ok()
}
