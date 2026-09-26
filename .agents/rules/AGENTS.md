# stoat-rat — Agent Context

## Goal

Maintain and extend `stoat-rat`, a Rust remote-administration agent whose C2 transport is a Stoat
bot session (gateway websocket + REST + Autumn uploads).

## Ground Rules

- Target platform is **Windows** (`x86_64-pc-windows-msvc`). Windows-specific code lives in
  `src/input.rs` and `src/host.rs`, gated with `#![cfg(windows)]`.
- Keep the command surface in `src/commands.rs` as the single dispatch table; add new capabilities
  there and put platform logic in `host.rs` / `capture.rs` / `input.rs`.
- Long or blocking work must run under `tokio::task::spawn_blocking` so the gateway loop keeps
  draining events.
- Never hardcode per-target values in source; they belong in `config.json` / env.

## Key API Facts (verified)

- API base: `https://api.stoat.chat`, auth header `X-Bot-Token`
- Gateway: `wss://events.stoat.chat`; authenticate with `{"type":"Authenticate","token":"..."}`
- Upload host: `https://cdn.stoatusercontent.com` (from `features.autumn.url`); `POST /{tag}`,
  multipart field `file`, tag `attachments`
- Message event fields used: `type`, `author`, `channel`, `content`, `attachments[].{_id,tag,filename}`
- `Bulk` events carry an array of events under `v`

## Build / Verify

```powershell
cargo build --release
cargo test
```
