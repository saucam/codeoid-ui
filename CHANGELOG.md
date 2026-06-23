# Changelog

All notable changes to **codeoid-ui** (the native Rust client for Codeoid) are
documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning: [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
