# Changelog

All notable changes to **codeoid-ui** (the native Rust client for Codeoid) are
documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning: [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Multi-provider sessions** (pairs with the daemon's pi backend +
  mid-session switching): `/new <name> [workdir] --provider <id>` creates a
  session on a specific backend, `/provider <id>` switches the focused
  session mid-conversation (daemon rejections like mid-turn switches
  surface in the error line), and the transcript header tags non-default
  backends. Protocol: `session.create.providerId`,
  `session.set_provider`, `SessionInfo.providerId`, and
  `AuthOkMsg.providers` (default first).

- **Provider extension surface** (pairs with the daemon's
  provider-extension-surface release) — the TUI now renders what non-Claude
  backends expose through codeoid:
  - **Provider dialogs** — `session.ui_request` opens an interactive modal
    (select with arrows/digits, confirm with y/n, input/editor with a text
    buffer); answers ship as `session.ui_response`, and `session.ui_resolved`
    dismisses copies answered elsewhere. Pending dialogs re-surface on
    attach and queue oldest-first per session. The client now declares the
    `ui.dialogs` + `parts` capabilities (and its protocol version) on the
    auth frame.
  - **Dynamic provider commands** — `session.commands` catalogs (pi
    extension commands, prompt templates, skills) merge into the `/`
    palette, and unknown-but-catalogued verbs pass through as prompt text
    for the provider to expand.
  - **Rich `parts[]` rendering** — the transcript finally renders the
    protocol's `ContentPart` union (text, code, file refs, diffs, trees,
    progress, images, anchors, tables; buttons render as labeled chips —
    activation stays on the web UI until the TUI grows part focus).

## [0.1.0] - 2026-06-23

Initial public release.

### Added

- **`codeoid-tui`** — a native [Ratatui](https://ratatui.rs) terminal cockpit for
  the Codeoid daemon: a true cell-matrix framebuffer, so it stays jitter-free
  under high-frequency streaming deltas.
- **`codeoid-client`** — async WebSocket client (auth handshake, request/response
  correlation, and the unsolicited event stream).
- **`codeoid-protocol`** — serde-compatible Rust port of the daemon's wire
  protocol; `PROTOCOL_VERSION` is negotiated on connect so client and daemon
  versions can move independently.

[Unreleased]: https://github.com/saucam/codeoid-ui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/saucam/codeoid-ui/releases/tag/v0.1.0
