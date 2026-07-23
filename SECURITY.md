# Security Policy

codeoid-ui is a terminal client for the [Codeoid](https://github.com/highflame-ai/codeoid)
daemon. It holds credentials (a ZeroID API key or JWT) and connects to the
daemon over WebSocket.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately via GitHub Security Advisories:
<https://github.com/highflame-ai/codeoid-ui/security/advisories/new>

Include repro steps, affected commit, and impact. We'll acknowledge and
coordinate a fix and disclosure with you.

## What to keep in mind

- **Credentials.** The client reads `CODEOID_API_KEY` / `CODEOID_TOKEN` (or
  `~/.codeoid/config.json`). A `zid_sk_…` API key is exchanged with ZeroID for a
  short-lived access token at startup. Don't paste keys into shared terminals or
  commit them; treat `~/.codeoid/config.json` as a secret.
- **Transport.** Defaults to `ws://127.0.0.1:7400` (loopback). Point it at a
  remote daemon only over a trusted/authenticated channel (e.g. `wss://` behind
  your own tunnel) — the daemon authenticates every connection with the token,
  but plaintext `ws://` over an untrusted network exposes it.
- **Logging.** Stderr is reserved for the TUI; logs go to `--log-file` only when
  you set it. Avoid logging tokens; scrub before sharing a log capture.
- **Scope of this client.** It enforces nothing itself — authorization is the
  daemon's job (per-message ZeroID scopes). Report daemon/auth issues against
  [highflame-ai/codeoid](https://github.com/highflame-ai/codeoid).

## Supported versions

Pre-1.0; fixes land on `main`. Build from a recent commit.
