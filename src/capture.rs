// language: Rust, file: src/capture.rs, target: Windows
// Screen capture (xcap -> JPEG) and microphone capture (cpal -> 16-bit WAV).
#![allow(dead_code)]
use anyhow::{bail, Context, Result};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Capture the entire virtual desktop (all monitors) as one JPEG.
pub fn screenshot_jpeg() -> Result<Vec<u8>> {
    let monitors = xcap::Monitor::all().context("enumerate monitors")?;
    if monitors.is_empty() {
        bail!("no monitors found");
    }
    let min_x = monitors.iter().map(|m| m.x()).min().unwrap_or(0);
    let min_y = monitors.iter().map(|m| m.y()).min().unwrap_or(0);
    let max_x = monitors
        .iter()
        .map(|m| m.x() + m.width() as i32)
        .max()
        .unwrap_or(0);
    let max_y = monitors
        .iter()
        .map(|m| m.y() + m.height() as i32)
        .max()
        .unwrap_or(0);
    let w = (max_x - min_x).max(1) as u32;
    let h = (max_y - min_y).max(1) as u32;

    let mut canvas = image::RgbaImage::new(w, h);
    for m in &monitors {
        let img = m.capture_image().context("capture_image")?;
        let ox = (m.x() - min_x) as i64;
        let oy = (m.y() - min_y) as i64;
        image::imageops::overlay(&mut canvas, &img, ox, oy);
    }
    encode_jpeg_rgba(&canvas.into_raw(), w, h)
}

/// JPEG has no alpha channel — drop RGBA to RGB8 before encoding.
fn encode_jpeg_rgba(raw: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let rgba = image::RgbaImage::from_raw(w, h, raw.to_vec()).context("bad image buffer")?;
    let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::io::Cursor::new(&mut out),
            72,
        );
        enc.encode(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .context("jpeg encode")?;
    }
    Ok(out)
}

/// Record `secs` seconds from the default input device, return WAV bytes.
pub fn record_wav(secs: u64) -> Result<Vec<u8>> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SizedSample};

    fn build<T>(
        device: &cpal::Device,
        cfg: &cpal::StreamConfig,
        buf: Arc<Mutex<Vec<f32>>>,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        let sink = buf.clone();
        let stream = device.build_input_stream(
            cfg,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut g = sink.lock().unwrap();
                for &v in data {
                    g.push(f32::from_sample(v));
                }
            },
            |e| eprintln!("mic: {e}"),
            None,
        )?;
        Ok(stream)
    }

    let host = cpal::default_host();
    let device = host.default_input_device().context("no input device")?;
    let supported = device.default_input_config().context("input config")?;
    let sample_format = supported.sample_format();
    let cfg: cpal::StreamConfig = supported.into();
    let channels = cfg.channels as usize;
    let sample_rate = cfg.sample_rate.0;
    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &cfg, buf.clone())?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &cfg, buf.clone())?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &cfg, buf.clone())?,
        cpal::SampleFormat::I32 => build::<i32>(&device, &cfg, buf.clone())?,
        other => bail!("unsupported sample format {other:?}"),
    };
    stream.play().context("start stream")?;
    std::thread::sleep(Duration::from_secs(secs.clamp(1, 300)));
    drop(stream);

    let data = buf.lock().unwrap().clone();
    let spec = hound::WavSpec {
        channels: channels.max(1) as u16,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = Vec::new();
    {
        let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec)?;
        for s in data {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            w.write_sample(v)?;
        }
        w.finalize()?;
    }
    Ok(out)
}

/// Grab one frame from the default webcam and encode it as JPEG.
#[cfg(windows)]
pub fn webcam_jpeg() -> Result<Vec<u8>> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
    use nokhwa::Camera;

    let requested =
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut camera = Camera::new(CameraIndex::Index(0), requested)?;
    camera.open_stream()?;
    let frame = camera.frame()?;
    let img = frame.decode_image::<RgbFormat>()?;
    let (w, h) = (img.width(), img.height());
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::io::Cursor::new(&mut out),
            75,
        );
        enc.encode(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)?;
    }
    camera.stop_stream().ok();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_rgba_source_as_jpeg() {
        let raw = vec![128u8; 4 * 16 * 16];
        let out = encode_jpeg_rgba(&raw, 16, 16).expect("encode");
        assert!(out.len() > 100);
        assert_eq!(&out[..2], &[0xFF, 0xD8]); // JPEG SOI marker
    }
}

/// Record `secs` of system output audio (WASAPI loopback) as WAV.
#[cfg(windows)]
pub fn record_loopback_wav(secs: u64) -> Result<Vec<u8>> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let pwfx = client.GetMixFormat()?;
        let channels = (*pwfx).nChannels as usize;
        let rate = (*pwfx).nSamplesPerSec;
        let bits = (*pwfx).wBitsPerSample;
        let is_float = bits == 32;

        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            pwfx,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let mut samples: Vec<f32> = Vec::new();
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(secs.clamp(1, 300));
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let size = capture.GetNextPacketSize().unwrap_or(0);
            if size == 0 {
                continue;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                .is_err()
            {
                continue;
            }
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 && !data.is_null() {
                let count = frames as usize * channels;
                if is_float {
                    let f = std::slice::from_raw_parts(data as *const f32, count);
                    samples.extend_from_slice(f);
                } else if bits == 16 {
                    let i = std::slice::from_raw_parts(data as *const i16, count);
                    samples.extend(i.iter().map(|v| *v as f32 / 32768.0));
                }
            }
            let _ = capture.ReleaseBuffer(frames);
        }
        client.Stop()?;

        let spec = hound::WavSpec {
            channels: channels.max(1) as u16,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut out = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec)?;
            for s in samples {
                w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
            }
            w.finalize()?;
        }
        Ok(out)
    }
}
