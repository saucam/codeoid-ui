# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/saucam/codeoid-ui/compare/v0.1.0...v0.1.1) - 2026-07-02

### Added

- high-visibility approval banner for pending tool calls ([#6](https://github.com/saucam/codeoid-ui/pull/6))

### Fixed

- terminal restore on panic, per-session render caches, incremental animation frames, sanitizer gaps ([#12](https://github.com/saucam/codeoid-ui/pull/12))

### Other

- release v0.1.0 ([#7](https://github.com/saucam/codeoid-ui/pull/7))

## [0.1.0](https://github.com/saucam/codeoid-ui/releases/tag/v0.1.0) - 2026-06-23

### Added

- high-visibility approval banner for pending tool calls ([#6](https://github.com/saucam/codeoid-ui/pull/6))
- rename SessionMode AutoAllow -> Guarded (lockstep with codeoid daemon)
- *(tui)* Esc interrupts a busy turn (Claude Code parity)
- *(tui)* bring the Rust TUI to parity with web/Telegram
- *(tui)* AskUserQuestion form modal
- *(tui)* per-block tool-output expand with [/]/Enter navigation
- *(tui)* collapse tool output by default, toggle with v
- *(tui)* render ExitPlanMode plan content; refine-via-typing affordance
- /export and /import slash commands (P7B parity with web)
- surface MCP header keys in capabilities modal
- /agents /skills /mcp /hooks capabilities modal (P7A parity with web)

### Fixed

- *(tui)* mouse wheel scroll, accurate bottom-anchor, Tier-1 scrollback perf cache
- *(tui)* enable bracketed paste so multi-line paste lands as one insert

### Other

- window the transcript render to O(viewport) ([#5](https://github.com/saucam/codeoid-ui/pull/5))
- Set up crates.io + binary releases: publish-on-tag workflow ([#2](https://github.com/saucam/codeoid-ui/pull/2))
- Adding more features and stabilizing
- More changes, towards stability
- Initial draft
