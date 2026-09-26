// language: Rust, file: src/volume.rs, target: Windows
// System output volume / mute via the Core Audio endpoint (IAudioEndpointVolume).
#![cfg(windows)]
#![allow(dead_code)]
use anyhow::Result;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

fn endpoint() -> Result<IAudioEndpointVolume> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let vol: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
        Ok(vol)
    }
}

pub fn get_percent() -> Result<u32> {
    let vol = endpoint()?;
    let v = unsafe { vol.GetMasterVolumeLevelScalar()? };
    Ok((v * 100.0).round() as u32)
}

pub fn get_mute() -> Result<bool> {
    let vol = endpoint()?;
    Ok(unsafe { vol.GetMute()? }.as_bool())
}

pub fn set_percent(p: u32) -> Result<()> {
    let vol = endpoint()?;
    let f = (p.min(100) as f32) / 100.0;
    unsafe { vol.SetMasterVolumeLevelScalar(f, std::ptr::null())? };
    Ok(())
}

pub fn set_mute(m: bool) -> Result<()> {
    let vol = endpoint()?;
    unsafe { vol.SetMute(BOOL::from(m), std::ptr::null())? };
    Ok(())
}
