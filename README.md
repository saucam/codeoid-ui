# Codeoid UI

Native terminal cockpit for the [Codeoid](../codeoid) daemon, written in Rust + [Ratatui](https://ratatui.rs).

Replaces the Ink/React `codeoid tui` client. The daemon is untouched — this is
a drop-in WebSocket client that speaks the same wire protocol.

## Why

The Ink/React TUI paid the cost of a JavaScript UI framework (React
reconciliation, VDOM diffing, GC pauses) on every keystroke and every LLM
token. For a tool that gets hammered by high-frequency streaming deltas, a
true cell-matrix framebuffer is categorically faster and jitter-free.

## Workspace layout

```
codeoid-ui/
├── Cargo.toml                       # workspace + shared dep versions
├── rust-toolchain.toml              # pin stable
└── crates/
    ├── codeoid-protocol/            # pure serde types — no I/O
    │   ├── src/
    │   │   ├── lib.rs               # PROTOCOL_VERSION + re-exports
    │   │   ├── client.rs            # ClientMessage enum
    │   │   ├── daemon.rs            # DaemonMessage enum (+ Unknown fallback)
    │   │   ├── message.rs           # SessionMessage, ContentPart, delta
    │   │   ├── session.rs           # SessionInfo, usage telemetry
    │   │   └── tool.rs              # ToolState (5-phase tagged enum)
    │   └── tests/roundtrip.rs       # JSON compat tests
    │
    ├── codeoid-client/               # async transport — tokio + tungstenite
    │   ├── src/
    │   │   ├── lib.rs
    │   │   ├── connection.rs        # connect + auth handshake + reader/writer
    │   │   ├── request.rs           # id → oneshot registry
    │   │   └── error.rs
    │   └── examples/headless.rs     # smoke test w/o TUI
    │
    └── codeoid-tui/                 # ratatui app — the UI
        ├── src/
        │   ├── main.rs              # bin entry, terminal setup
        │   ├── app.rs               # reducer: drains unified event stream
        │   ├── event.rs             # AppEvent = Key|Net|Tick
        │   ├── keymap.rs            # keystroke → Action
        │   ├── state/               # session list, message store, modals
        │   ├── ui/                  # widget layout (tabs/scrollback/prompt/status/modal)
        │   ├── render/              # pure data→Line helpers (markdown, diff)
        │   └── commands/            # slash-cmd loader, @file mention scanner
        └── ...
```

### Design principles

- **Protocol crate is pure data.** No tokio, no I/O. Reusable by any future
  Rust client (CLI, headless agents).
- **Client crate owns reconnection.** The TUI sees a clean `Stream<DaemonMessage>`
  and never touches WebSocket frames.
- **Single `AppEvent` enum → single `update()`.** Elm-style, testable
  without a terminal.
- **`render/` vs `ui/`.** `ui/` arranges; `render/` is pure
  protocol-to-`Line` helpers. Easy unit tests for markdown/diff without
  spinning up Ratatui.

## Building

```bash
cd codeoid-ui
cargo build --release
```

The `codeoid-tui` binary drops into `target/release/codeoid-tui`.

## Running

```bash
# daemon must be running at ws://127.0.0.1:7400
CODEOID_TOKEN=$(codeoid auth print-token) \
  cargo run -p codeoid-tui --release
```

Flags:

| Flag            | Env                | Default                  | Meaning                          |
|-----------------|--------------------|--------------------------|----------------------------------|
| `--url`         | `CODEOID_URL`      | `ws://127.0.0.1:7400`    | Daemon WebSocket URL             |
| `--token`       | `CODEOID_TOKEN`    | *(required)*             | ZeroID JWT                        |
| `--log-file`    | `CODEOID_LOG_FILE` | *(none — logs dropped)*  | tracing file sink                 |

## Testing

```bash
cargo test                              # everything
cargo test -p codeoid-protocol          # JSON round-trip tests
cargo test -p codeoid-tui state::       # reducer tests (no Tokio, no Ratatui)
```

## Keybindings

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `Tab` / `i`            | Focus prompt                        |
| `Esc`                  | Blur prompt                         |
| `Enter`                | Send prompt                         |
| `Shift+Enter` / `Ctrl+J` | Newline in prompt                 |
| `←` / `→`, `p` / `n`   | Prev / next session                 |
| `y` / `d`              | Approve / deny pending tool         |
| `Ctrl+X` / `.`         | Interrupt the current session       |
| `m`                    | Cycle execution mode                |
| `PgUp` / `PgDn`        | Scroll transcript                   |
| `?`                    | Toggle keybinding help              |
| `q` / `Ctrl+C`         | Quit                                |

## Protocol versioning

This client bakes in `PROTOCOL_VERSION = 1`. On connect, the daemon's
`auth.ok` includes its own `protocolVersion`; a mismatch pops a warning
modal but doesn't refuse to run — the daemon's wire protocol is
additive-safe. Bump [`crates/codeoid-protocol/src/lib.rs`](crates/codeoid-protocol/src/lib.rs)
and the daemon in lockstep on any breaking change.
