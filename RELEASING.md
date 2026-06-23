# Releasing codeoid-ui

Pushing a `vX.Y.Z` tag publishes the three crates to crates.io and attaches
prebuilt `codeoid-tui` binaries to a GitHub Release — see
[`.github/workflows/release.yml`](.github/workflows/release.yml).

## One-time setup

Add a **`CARGO_REGISTRY_TOKEN`** repository secret (Settings → Secrets and
variables → Actions): crates.io → Account Settings → **API Tokens** → new token
scoped to publish.

(Binary uploads need no extra secret — they use the built-in `GITHUB_TOKEN`.)

## Cutting a release

1. Bump the version in [`Cargo.toml`](Cargo.toml): `[workspace.package].version`
   **and** the `version = "X.Y.Z"` on the internal path-deps in
   `[workspace.dependencies]` (they must match for crates.io).
2. Move the `## [Unreleased]` notes into a new `## [X.Y.Z]` section in
   [`CHANGELOG.md`](CHANGELOG.md).
3. Open a PR, get CI green, merge to `main`.
4. Tag from `main` and push (tags are not branch-protected):

   ```bash
   git checkout main && git pull
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```

The workflow publishes `codeoid-protocol` → `codeoid-client` → `codeoid-tui`
(in dependency order; cargo waits for each to index) and uploads macOS
(arm64 / x64) and Linux (x64) binaries. Install with:

```bash
cargo install codeoid-tui          # from crates.io
# or download a prebuilt binary from the GitHub Release
```

## Notes

- The **wire protocol** version (`PROTOCOL_VERSION` in `codeoid-protocol`) is
  independent of the crate version — bump it only on wire-breaking changes, and
  keep it in lockstep with the daemon's `src/protocol/types.ts` in
  [codeoid](https://github.com/saucam/codeoid). The handshake negotiates
  compatibility, so app versions and the protocol version move independently.
- Both internal crates must be published before `codeoid-tui`; the workflow
  handles the ordering.
