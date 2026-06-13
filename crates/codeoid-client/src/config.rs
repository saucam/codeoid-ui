//! Reads `~/.codeoid/config.json` and resolves the ZeroID issuer, so the
//! TUI shares one credential source with the CLI and web frontends.
//!
//! Precedence (highest first) is owned by the caller in `main.rs`:
//! CLI flag → env var → this file → built-in default. This module only
//! supplies the file layer plus the issuer-preset resolver — it mirrors
//! `codeoid/src/config.ts` (`loadConfig` + `resolveZeroidUrl`) so a key
//! minted via `codeoid login` and written to `config.json` works here too.

use std::path::PathBuf;

use serde::Deserialize;
use tracing::warn;

/// The subset of `~/.codeoid/config.json` the TUI cares about. Unknown
/// fields (memory, compress, telemetry, …) are ignored, so the same file
/// the daemon writes loads cleanly here.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileConfig {
    /// ZeroID API key (`zid_sk_…`) written by `codeoid login`.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Issuer — a preset name (`highflame`/`highflame-dev`/`local`) or a URL.
    #[serde(default)]
    pub zeroid_url: Option<String>,
    /// Daemon WebSocket URL.
    #[serde(default)]
    pub daemon_url: Option<String>,
}

/// Friendly aliases for the ZeroID issuer, mirroring `ZEROID_PRESETS` in
/// `codeoid/src/config.ts`. The shipped default is the Highflame SaaS.
const PRESETS: &[(&str, &str)] = &[
    ("highflame", "https://auth.highflame.ai"),
    ("highflame-dev", "https://auth-dev.highflame.dev"),
    ("local", "http://localhost:8899"),
];

/// Resolve a `zeroidUrl` value to a concrete base URL:
/// a known preset name → its URL; anything with a scheme → used verbatim
/// (trailing slash trimmed); a bare host → assumed `https://`.
#[must_use]
pub fn resolve_zeroid_url(value: &str) -> String {
    let v = value.trim();
    for (name, url) in PRESETS {
        if v == *name {
            return (*url).to_string();
        }
    }
    let stripped = v.trim_end_matches('/');
    if stripped.contains("://") {
        stripped.to_string()
    } else {
        format!("https://{stripped}")
    }
}

/// The config directory, honoring `XDG_CONFIG_HOME` like the TS loader,
/// else `~/.codeoid`.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("codeoid"));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codeoid"))
}

/// Load `config.json` best-effort. A missing file is normal (first run);
/// a malformed file warns and falls back to defaults rather than aborting
/// the TUI — the daemon-side loader fails loudly, but a viewer client
/// degrades gracefully.
#[must_use]
pub fn load_file_config() -> FileConfig {
    let Some(dir) = config_dir() else {
        return FileConfig::default();
    };
    let path = dir.join("config.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return FileConfig::default();
    };
    match serde_json::from_str::<FileConfig>(&raw) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "ignoring malformed config.json");
            FileConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_resolve_to_urls() {
        assert_eq!(resolve_zeroid_url("highflame"), "https://auth.highflame.ai");
        assert_eq!(
            resolve_zeroid_url("highflame-dev"),
            "https://auth-dev.highflame.dev"
        );
        assert_eq!(resolve_zeroid_url("local"), "http://localhost:8899");
    }

    #[test]
    fn full_url_passes_through_trimmed() {
        assert_eq!(
            resolve_zeroid_url("https://zeroid.acme.com"),
            "https://zeroid.acme.com"
        );
        assert_eq!(
            resolve_zeroid_url("https://zeroid.acme.com/"),
            "https://zeroid.acme.com"
        );
        assert_eq!(
            resolve_zeroid_url("  http://10.0.0.1:8899  "),
            "http://10.0.0.1:8899"
        );
    }

    #[test]
    fn bare_host_assumes_https() {
        assert_eq!(
            resolve_zeroid_url("zeroid.acme.com"),
            "https://zeroid.acme.com"
        );
    }

    #[test]
    fn file_config_parses_and_ignores_unknown() {
        let cfg: FileConfig = serde_json::from_str(
            r#"{ "apiKey": "zid_sk_x", "zeroidUrl": "highflame-dev", "memory": { "enabled": true }, "daemonUrl": "ws://x" }"#,
        )
        .unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some("zid_sk_x"));
        assert_eq!(cfg.zeroid_url.as_deref(), Some("highflame-dev"));
        assert_eq!(cfg.daemon_url.as_deref(), Some("ws://x"));
    }
}
