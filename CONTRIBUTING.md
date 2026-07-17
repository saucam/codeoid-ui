# Contributing to codeoid-ui

Thanks for helping improve the Codeoid terminal cockpit. This is the native
Rust/[Ratatui](https://ratatui.rs) client for the
[Codeoid](https://github.com/highflame-ai/codeoid) daemon.

## Workspace

Three crates (see [README § Workspace layout](README.md#workspace-layout)):

- `codeoid-protocol` — pure serde wire types, **no I/O**. Must stay in sync with
  the daemon's `src/protocol/` in [highflame-ai/codeoid](https://github.com/highflame-ai/codeoid).
- `codeoid-client` — async transport (tokio + tungstenite): connect, auth, reconnect.
- `codeoid-tui` — the Ratatui app (Elm-style: one `AppEvent` → one `update()`).

## Development loop

```bash
cargo build                       # debug build
cargo run -p codeoid-tui          # run against a local daemon (codeoid start)
cargo test                        # everything
cargo test -p codeoid-protocol    # JSON round-trip / wire-compat tests
cargo test -p codeoid-tui state:: # reducer tests (no Tokio, no Ratatui)
cargo fmt                         # rustfmt (run before committing)
cargo clippy --all-targets        # lint
```

A headless smoke client (no terminal) lives at
`crates/codeoid-client/examples/headless.rs` — handy for protocol work.

## Conventions

- **rustfmt + clippy clean.** Run `cargo fmt` and `cargo clippy` before a PR;
  CI expects both green. Keep formatting changes out of feature commits.
- **Keep the layers honest.** `codeoid-protocol` stays I/O-free; `render/` is
  pure protocol→`Line` helpers (unit-testable without a terminal); `ui/` only
  arranges widgets. New keybindings go through `keymap.rs` so the help modal and
  tests stay authoritative — except runtime-conditional bindings (e.g. Esc that
  only interrupts when a turn is live), which are resolved at the dispatch site
  in `app.rs` with a comment explaining why.
- **Prefer reducer tests.** Most behavior is testable via `update()` /
  `state::` without spinning up a terminal — add to that suite.

## Protocol changes

The wire format is shared with the daemon. If you change `codeoid-protocol`,
bump `PROTOCOL_VERSION` (`crates/codeoid-protocol/src/lib.rs`) and update the
daemon's `src/protocol/` in [highflame-ai/codeoid](https://github.com/highflame-ai/codeoid)
in lockstep. The protocol is additive-safe; a version mismatch warns rather than
refuses.

## Reporting bugs & security

- Bugs: open an issue with repro steps and, if relevant, a `--log-file` capture
  (stderr is reserved for the TUI, so file logging is the way to get traces).
- Security issues: see [`SECURITY.md`](SECURITY.md) — don't open a public issue.

## License

By contributing you agree your contributions are licensed under the
[MIT License](LICENSE).

## Signing off your work (DCO)

This project uses the [Developer Certificate of Origin](https://developercertificate.org/) (DCO) rather than a CLA — a lightweight, per-commit attestation that you wrote, or otherwise have the right to submit, the code you contribute.

Sign off every commit:

```bash
git commit -s -m "your message"
```

That appends a trailer derived from your Git identity:

```
Signed-off-by: Your Name <you@example.com>
```

By signing off you agree to the DCO (full text at <https://developercertificate.org/>). If a commit is missing the trailer, amend it with `git commit --amend -s` (or `git rebase --signoff` for a range) before pushing.
