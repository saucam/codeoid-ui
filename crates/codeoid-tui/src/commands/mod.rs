//! Slash-command parser. Turns the raw prompt text into a typed
//! `SlashCommand` enum when it starts with `/`, so the app reducer can
//! dispatch session-lifecycle operations without a dedicated modal.
//!
//! Lives here (not in state/) because parsing and dispatch are orthogonal
//! to state — commands transform into `ClientMessage`s or UI actions.

use codeoid_protocol::SessionMode;

/// A parsed slash-command. Dispatched by the app reducer in
/// [`App::submit_prompt`]. Anything that can't be cleanly mapped to a
/// daemon request or a UI-local action is rejected in the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// `/new <name> [workdir]` — create a new session. If `workdir` is
    /// missing, the caller supplies `std::env::current_dir()`.
    New {
        name: String,
        workdir: Option<String>,
    },
    /// `/rename <new-name>` — rename the focused session. `new-name` may
    /// contain spaces; everything after the command is treated as the
    /// full name (trimmed).
    Rename { name: String },
    /// `/destroy` — destroy the currently focused session.
    Destroy,
    /// `/interrupt` — interrupt the currently focused session.
    Interrupt,
    /// `/approve` — approve the latest pending tool request.
    Approve,
    /// `/deny` — deny the latest pending tool request.
    Deny,
    /// `/mode <interactive|auto-allow|autonomous>` — switch execution mode.
    SetMode(SessionMode),
    /// `/model [value]` — with an argument, switch the focused session's
    /// model; with none, list the available models. The value is validated
    /// against the fetched catalog by the reducer.
    Model(Option<String>),
    /// `/who` — show the authenticated ZeroID identity + scopes.
    Who,
    /// `/rotate` — rotate the backing Claude Code context.
    Rotate,
    /// `/help` — open the help modal.
    Help,
    /// `/clear` — clear the prompt buffer.
    Clear,
    /// `/agents` `/skills` `/mcp` `/hooks` — open the capabilities
    /// modal scrolled to the relevant tab.
    Capabilities(CapabilitiesTab),
    /// `/export [path]` — write the focused session to a JSON bundle
    /// (under `~/.codeoid/exports/` by default; or to the given path).
    Export { path: Option<String> },
    /// `/import <bundle.json> <target-workdir>` — fork a bundle into a
    /// fresh session anchored at `target-workdir`.
    Import {
        bundle_path: String,
        target_workdir: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitiesTab {
    Agents,
    Skills,
    Mcp,
    Hooks,
}

/// Errors returned by [`parse`]. Distinguish "not a command" (returns `Ok(None)`
/// via the outer API) from "looks like a command but malformed" (returns
/// `Err`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("unknown command: /{0} — try /help")]
    Unknown(String),

    #[error("/new requires a session name: /new <name> [workdir]")]
    NewMissingName,

    #[error("/rename requires a new name: /rename <new-name>")]
    RenameMissingName,

    #[error("/mode requires one of: interactive, auto-allow, autonomous")]
    ModeMissingArg,

    #[error("/mode: '{0}' is not a valid mode — use interactive, auto-allow, or autonomous")]
    ModeInvalid(String),

    #[error("/import requires: /import <bundle.json> <target-workdir>")]
    ImportMissingArgs,
}

/// Attempt to parse `text` as a slash-command.
///
/// Returns:
/// * `Ok(Some(cmd))` — valid slash-command
/// * `Ok(None)` — plain text (not a slash-command)
/// * `Err(e)` — looks like a slash-command but is malformed
pub fn parse(text: &str) -> Result<Option<SlashCommand>, ParseError> {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Ok(None);
    };

    // `/` alone isn't a command, just a stray slash — treat as plain text.
    if rest.is_empty() {
        return Ok(None);
    }

    let mut parts = rest.split_whitespace();
    let name = parts.next().unwrap_or_default().to_ascii_lowercase();
    let rest_of_line: Vec<&str> = parts.collect();

    let cmd = match name.as_str() {
        "new" => {
            let name = rest_of_line
                .first()
                .copied()
                .ok_or(ParseError::NewMissingName)?
                .to_string();
            let workdir = rest_of_line.get(1..).and_then(|w| {
                if w.is_empty() {
                    None
                } else {
                    Some(w.join(" "))
                }
            });
            SlashCommand::New { name, workdir }
        }
        "rename" | "mv" => {
            // Everything after the command is the full name — users may
            // want spaces or punctuation in labels. Reject empty / pure
            // whitespace explicitly so the daemon isn't called for a
            // no-op that would return an error anyway.
            let name = rest_of_line.join(" ").trim().to_string();
            if name.is_empty() {
                return Err(ParseError::RenameMissingName);
            }
            SlashCommand::Rename { name }
        }
        "destroy" | "close" | "delete" => SlashCommand::Destroy,
        "interrupt" | "stop" | "cancel" => SlashCommand::Interrupt,
        "approve" | "yes" | "y" => SlashCommand::Approve,
        "deny" | "reject" | "no" | "n" => SlashCommand::Deny,
        "rotate" => SlashCommand::Rotate,
        "model" | "m" => {
            // `/model` lists; `/model <value>` switches. The value is a
            // single token (model ids have no spaces).
            let value = rest_of_line.first().map(|s| (*s).to_string());
            SlashCommand::Model(value)
        }
        "who" | "whoami" => SlashCommand::Who,
        "help" | "h" | "?" => SlashCommand::Help,
        "clear" | "cls" => SlashCommand::Clear,
        "agents" | "agent" => SlashCommand::Capabilities(CapabilitiesTab::Agents),
        "skills" | "skill" => SlashCommand::Capabilities(CapabilitiesTab::Skills),
        "mcp" => SlashCommand::Capabilities(CapabilitiesTab::Mcp),
        "hooks" | "hook" => SlashCommand::Capabilities(CapabilitiesTab::Hooks),
        "export" | "share" => {
            let path = if rest_of_line.is_empty() {
                None
            } else {
                Some(rest_of_line.join(" "))
            };
            SlashCommand::Export { path }
        }
        "import" | "fork" => {
            if rest_of_line.len() < 2 {
                return Err(ParseError::ImportMissingArgs);
            }
            // Bundle path is the first token; everything after is the
            // workdir (so it can contain spaces).
            let bundle_path = rest_of_line[0].to_string();
            let target_workdir = rest_of_line[1..].join(" ");
            SlashCommand::Import {
                bundle_path,
                target_workdir,
            }
        }
        "mode" => {
            let arg = rest_of_line
                .first()
                .copied()
                .ok_or(ParseError::ModeMissingArg)?;
            let mode = match arg {
                "interactive" | "i" => SessionMode::Interactive,
                "auto-allow" | "auto" | "a" => SessionMode::AutoAllow,
                "autonomous" | "auto-pilot" | "pilot" => SessionMode::Autonomous,
                other => return Err(ParseError::ModeInvalid(other.to_string())),
            };
            SlashCommand::SetMode(mode)
        }
        other => return Err(ParseError::Unknown(other.to_string())),
    };

    Ok(Some(cmd))
}

/// Static catalog — what the command palette displays. Keep in sync with
/// the `parse` match arms above. Each entry is (`usage`, `description`).
pub const CATALOG: &[(&str, &str)] = &[
    ("/new <name> [workdir]", "create a new session"),
    ("/rename <new-name>", "rename the focused session"),
    ("/destroy", "destroy the focused session"),
    ("/interrupt", "stop the running agent"),
    ("/approve", "approve the pending tool"),
    ("/deny", "deny the pending tool"),
    ("/mode <mode>", "interactive | auto | autonomous"),
    (
        "/model [value]",
        "list models, or switch the focused session",
    ),
    ("/who", "show your ZeroID identity + scopes"),
    ("/rotate", "rotate the backing context"),
    ("/help", "show the help modal"),
    ("/clear", "clear the prompt"),
    ("/agents", "list subagents loaded for this session"),
    ("/skills", "list slash-skill commands"),
    ("/mcp", "list MCP servers wired in"),
    ("/hooks", "list PreToolUse / PostToolUse hooks"),
    ("/export [path]", "export session as a portable bundle"),
    (
        "/import <bundle> <workdir>",
        "fork a bundle into a new session",
    ),
];

/// Entries whose command name (before the first space) is a prefix match
/// for the user's partial query. An empty query returns every command,
/// preserving the catalog order.
#[must_use]
pub fn filter_catalog(query: &str) -> Vec<&'static (&'static str, &'static str)> {
    let q = query.to_ascii_lowercase();
    CATALOG
        .iter()
        .filter(|(usage, _)| {
            // Strip the leading `/`, then take up to the first space so
            // "/new xyz" filters on "new".
            let name = usage
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            name.starts_with(&q)
        })
        .collect()
}

/// Return the first command name whose prefix matches, or `None` if
/// multiple match (ambiguous) or zero match. Used by Tab-autocomplete.
#[must_use]
pub fn unique_completion(query: &str) -> Option<&'static str> {
    let matches = filter_catalog(query);
    if matches.len() != 1 {
        return None;
    }
    let usage = matches[0].0;
    // Extract just the command name for completion.
    usage.trim_start_matches('/').split_whitespace().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_not_a_command() {
        assert_eq!(parse("hello world"), Ok(None));
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("   "), Ok(None));
    }

    #[test]
    fn bare_slash_is_plain_text() {
        assert_eq!(parse("/"), Ok(None));
    }

    #[test]
    fn new_requires_a_name() {
        assert_eq!(parse("/new"), Err(ParseError::NewMissingName));
        assert_eq!(parse("/new   "), Err(ParseError::NewMissingName));
    }

    #[test]
    fn new_with_name_only() {
        assert_eq!(
            parse("/new demo"),
            Ok(Some(SlashCommand::New {
                name: "demo".into(),
                workdir: None,
            }))
        );
    }

    #[test]
    fn new_with_name_and_workdir() {
        assert_eq!(
            parse("/new demo /tmp/foo"),
            Ok(Some(SlashCommand::New {
                name: "demo".into(),
                workdir: Some("/tmp/foo".into()),
            }))
        );
    }

    #[test]
    fn new_with_name_and_workdir_with_spaces() {
        assert_eq!(
            parse("/new demo /tmp/my dir"),
            Ok(Some(SlashCommand::New {
                name: "demo".into(),
                workdir: Some("/tmp/my dir".into()),
            }))
        );
    }

    #[test]
    fn destroy_aliases() {
        assert_eq!(parse("/destroy"), Ok(Some(SlashCommand::Destroy)));
        assert_eq!(parse("/close"), Ok(Some(SlashCommand::Destroy)));
        assert_eq!(parse("/delete"), Ok(Some(SlashCommand::Destroy)));
    }

    #[test]
    fn interrupt_aliases() {
        assert_eq!(parse("/interrupt"), Ok(Some(SlashCommand::Interrupt)));
        assert_eq!(parse("/stop"), Ok(Some(SlashCommand::Interrupt)));
        assert_eq!(parse("/cancel"), Ok(Some(SlashCommand::Interrupt)));
    }

    #[test]
    fn approve_deny_aliases() {
        assert_eq!(parse("/y"), Ok(Some(SlashCommand::Approve)));
        assert_eq!(parse("/approve"), Ok(Some(SlashCommand::Approve)));
        assert_eq!(parse("/yes"), Ok(Some(SlashCommand::Approve)));
        assert_eq!(parse("/n"), Ok(Some(SlashCommand::Deny)));
        assert_eq!(parse("/deny"), Ok(Some(SlashCommand::Deny)));
        assert_eq!(parse("/reject"), Ok(Some(SlashCommand::Deny)));
        assert_eq!(parse("/no"), Ok(Some(SlashCommand::Deny)));
    }

    #[test]
    fn mode_accepts_all_three_modes_and_shorthands() {
        assert_eq!(
            parse("/mode interactive"),
            Ok(Some(SlashCommand::SetMode(SessionMode::Interactive)))
        );
        assert_eq!(
            parse("/mode auto"),
            Ok(Some(SlashCommand::SetMode(SessionMode::AutoAllow)))
        );
        assert_eq!(
            parse("/mode autonomous"),
            Ok(Some(SlashCommand::SetMode(SessionMode::Autonomous)))
        );
    }

    #[test]
    fn mode_rejects_garbage() {
        assert_eq!(
            parse("/mode wacky"),
            Err(ParseError::ModeInvalid("wacky".into()))
        );
        assert_eq!(parse("/mode"), Err(ParseError::ModeMissingArg));
    }

    #[test]
    fn unknown_command() {
        assert_eq!(parse("/foo"), Err(ParseError::Unknown("foo".into())));
        assert_eq!(parse("/weirdo"), Err(ParseError::Unknown("weirdo".into())));
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(parse("/Help"), Ok(Some(SlashCommand::Help)));
        assert_eq!(
            parse("/NEW demo"),
            Ok(Some(SlashCommand::New {
                name: "demo".into(),
                workdir: None,
            }))
        );
    }

    #[test]
    fn leading_and_trailing_whitespace_tolerated() {
        assert_eq!(parse("  /help  "), Ok(Some(SlashCommand::Help)));
    }

    #[test]
    fn rename_simple() {
        assert_eq!(
            parse("/rename frontend-work"),
            Ok(Some(SlashCommand::Rename {
                name: "frontend-work".into(),
            }))
        );
    }

    #[test]
    fn rename_preserves_spaces() {
        assert_eq!(
            parse("/rename my working session"),
            Ok(Some(SlashCommand::Rename {
                name: "my working session".into(),
            }))
        );
    }

    #[test]
    fn rename_mv_alias() {
        assert_eq!(
            parse("/mv foo"),
            Ok(Some(SlashCommand::Rename { name: "foo".into() }))
        );
    }

    #[test]
    fn rename_requires_name() {
        assert_eq!(parse("/rename"), Err(ParseError::RenameMissingName));
        assert_eq!(parse("/rename    "), Err(ParseError::RenameMissingName));
    }

    #[test]
    fn model_lists_and_switches() {
        assert_eq!(parse("/model"), Ok(Some(SlashCommand::Model(None))));
        assert_eq!(parse("/m"), Ok(Some(SlashCommand::Model(None))));
        assert_eq!(
            parse("/model opus[1m]"),
            Ok(Some(SlashCommand::Model(Some("opus[1m]".into()))))
        );
        assert_eq!(
            parse("/model sonnet"),
            Ok(Some(SlashCommand::Model(Some("sonnet".into()))))
        );
    }

    #[test]
    fn who_aliases() {
        assert_eq!(parse("/who"), Ok(Some(SlashCommand::Who)));
        assert_eq!(parse("/whoami"), Ok(Some(SlashCommand::Who)));
    }

    #[test]
    fn rotate_and_clear() {
        assert_eq!(parse("/rotate"), Ok(Some(SlashCommand::Rotate)));
        assert_eq!(parse("/clear"), Ok(Some(SlashCommand::Clear)));
        assert_eq!(parse("/cls"), Ok(Some(SlashCommand::Clear)));
    }

    #[test]
    fn filter_catalog_empty_query_returns_all() {
        assert_eq!(filter_catalog("").len(), CATALOG.len());
    }

    #[test]
    fn filter_catalog_matches_prefix() {
        let out = filter_catalog("ne");
        assert_eq!(out.len(), 1);
        assert!(out[0].0.starts_with("/new"));
    }

    #[test]
    fn filter_catalog_no_match_returns_empty() {
        assert!(filter_catalog("zzzz").is_empty());
    }

    #[test]
    fn filter_catalog_case_insensitive() {
        let out = filter_catalog("HELP");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "/help");
    }

    #[test]
    fn unique_completion_picks_unambiguous() {
        assert_eq!(unique_completion("ne"), Some("new"));
    }

    #[test]
    fn unique_completion_none_when_ambiguous() {
        // Both /destroy and /deny start with `de` — ambiguous.
        assert_eq!(unique_completion("de"), None);
    }

    #[test]
    fn unique_completion_none_when_no_match() {
        assert_eq!(unique_completion("xzxzxz"), None);
    }
}
