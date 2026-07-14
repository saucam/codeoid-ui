# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/saucam/codeoid-ui/compare/codeoid-client-v0.1.1...codeoid-client-v0.2.0) - 2026-07-13

### Added

- adopt scrollback.paging — tail-first attach + scroll-up history backfill ([#21](https://github.com/saucam/codeoid-ui/pull/21))
- surface session fork in the TUI (/fork [backend]) ([#17](https://github.com/saucam/codeoid-ui/pull/17))
- multi-provider sessions — /provider switch, /new --provider, backend tags ([#15](https://github.com/saucam/codeoid-ui/pull/15))
- provider extension surface — dialogs, dynamic commands, rich parts rendering ([#14](https://github.com/saucam/codeoid-ui/pull/14))

### Fixed

- model catalog follows the session's backend (TUI) ([#22](https://github.com/saucam/codeoid-ui/pull/22))
- never wedge or eat input — request timeouts, prompt restore, destroy confirm ([#18](https://github.com/saucam/codeoid-ui/pull/18))

## [0.1.1](https://github.com/saucam/codeoid-ui/compare/codeoid-client-v0.1.0...codeoid-client-v0.1.1) - 2026-07-02

### Other

- release v0.1.0 ([#7](https://github.com/saucam/codeoid-ui/pull/7))

## [0.1.0](https://github.com/saucam/codeoid-ui/releases/tag/codeoid-client-v0.1.0) - 2026-06-23

### Added

- *(tui)* Esc interrupts a busy turn (Claude Code parity)
- *(tui)* bring the Rust TUI to parity with web/Telegram
- /export and /import slash commands (P7B parity with web)
- /agents /skills /mcp /hooks capabilities modal (P7A parity with web)
- *(client)* include fs:read in the requested scope set

### Other

- Set up crates.io + binary releases: publish-on-tag workflow ([#2](https://github.com/saucam/codeoid-ui/pull/2))
- More changes, towards stability
- Initial draft
