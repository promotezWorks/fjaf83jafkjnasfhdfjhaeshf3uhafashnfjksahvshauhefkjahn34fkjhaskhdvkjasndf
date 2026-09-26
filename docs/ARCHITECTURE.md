# stoat-rat — Architecture

## Purpose

A Rust remote-administration agent whose command-and-control transport is a **Stoat** bot
session. The operator issues commands as messages in a control channel; the agent watches the
gateway and posts results back to the originating channel as text or file attachments.

## Module Map

| file | responsibility |
|---|---|
| `src/main.rs` | startup, `/users/@me` identity, gateway connect/authenticate loop, event routing, reconnect/backoff |
| `src/stoat.rs` | REST transport: `X-Bot-Token` auth, `send`, `send_code` (chunked), `upload`, `send_file`, `download`, `raw_get` |
| `src/commands.rs` | command parser + dispatch table |
| `src/capture.rs` | screen capture (xcap → JPEG), microphone capture (cpal → 16-bit PCM WAV) |
| `src/input.rs` | global `WH_KEYBOARD_LL` + `WH_MOUSE_LL` hooks, keylog buffer, freeze via hooks + `BlockInput` |
| `src/host.rs` | shell, detached exec, process list/kill, clipboard, persistence, lock, monitor power, message box, directory listing |
| `src/config.rs` | config resolution (config.json → env), defaults |

## Transport

Stoat exposes a bot gateway and a REST API.

- **Gateway**: `wss://events.stoat.chat`. First frame is
  `{"type":"Authenticate","token":"<bot token>"}`. Server replies `Authenticated`, then `Ready`.
  Incoming events include `Message`, `Bulk` (an array under `v`), `Pong`, `Logout`.
- **REST**: base `https://api.stoat.chat`, auth header `X-Bot-Token`.
  - send message: `POST /channels/{channel}/messages` `{ "content": "..." }`
  - identity: `GET /users/@me`
- **Uploads (Autumn)**: `POST {autumn}/{tag}` with multipart field `file`, header `X-Bot-Token`;
  `tag` is `attachments`. Response `{ "id": "..." }`; reference it in a message via
  `{ "attachments": ["<id>"] }`.
- **Download**: `GET {autumn}/{tag}/{id}`.

## Data Flow

```
operator msg ──► Stoat gateway ──► main::handle_message
                                        │  filter: prefix + operator + channel + not self
                                        ▼
                                   commands::dispatch
                                        │
                    ┌───────────────────┼────────────────────┐
                    ▼                   ▼                    ▼
                capture / input     host actions         stoat REST
                    │                   │                    │
                    └─────────► reply / attachment ◄─────────┘
                                        │
                                        ▼
                              Stoat control channel
```

Long actions (shell, screen, mic) run on `spawn_blocking` so the async gateway loop keeps
draining events and pinging.

## Design Notes

- **Identity filter**: messages authored by the bot's own id are ignored (prevents reply loops),
  and `operator` / `channel` narrow acceptance to a single control point.
- **Freeze**: the low-level hooks swallow input system-wide; `BlockInput(true)` is held on a
  dedicated long-lived thread so the OS block persists for the process lifetime. Ctrl+Alt+Del is
  reserved by Windows and always breaks the freeze.
- **Keylog**: non-injected hook — no DLL, no elevation. Buffer is capped (~1 MB) and rotates.
- **Persistence**: exe copied to `%APPDATA%\Microsoft\Windows`, value name `WindowsUpdate` under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

## Build

```powershell
cargo build --release
cargo test
```

## Hardening Backlog

- `obfstr` / compile-time string obfuscation for the token and API hosts
- scheduler-task persistence variant
- KeyAuth license gate on the agent (portable from the sibling `discord-raider` project)
- `#![windows_subsystem = "windows"]` for a windowless release build
