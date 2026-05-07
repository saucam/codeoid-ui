//! Wire-format invariants.
//!
//! The TS daemon expects camelCase everywhere except on `type:` discriminators
//! (snake_case) and `SessionMode` (kebab-case). A serde mistake on our side
//! causes silent field drop on receive and silent field drop on send — the
//! single worst class of bug we can ship. This test builds a sample of every
//! type we serialize, dumps it to JSON, and asserts no snake_case leaks.

use codeoid_protocol::{
    Attachment, CancelReason, ClientMessage, ConfirmedBy, ContentPart, DaemonMessage, ErrorCode,
    IdentityType, MessageIdentity, MessageRole, SearchScope, SendPriority, SessionInfo,
    SessionMessage, SessionMessageDelta, SessionMode, SessionStatus, SessionUsage, ToolInfo,
    ToolState,
};
use serde::Serialize;
use serde_json::Value;

/// Keys allowed to contain `_` on the wire. Protocol-level type
/// discriminants carry dotted names (`session.message`) and a handful of
/// snake_case enum values (`waiting_approval`, `tool_call`). These are in
/// VALUES, not KEYS, and are checked separately.
fn is_snake_case_key(k: &str) -> bool {
    k.contains('_')
}

/// Walk a `serde_json::Value` and return every object *key* that contains
/// an underscore. Field names are what the TS daemon matches on — values
/// are allowed to be snake_case (they're enum discriminants).
fn collect_snake_keys(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if is_snake_case_key(k) {
                    out.push((path.to_string(), k.clone()));
                }
                let nested = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                collect_snake_keys(v, &nested, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let nested = format!("{path}[{i}]");
                collect_snake_keys(v, &nested, out);
            }
        }
        _ => {}
    }
}

fn assert_no_snake_case_keys<T: Serialize>(sample: T, label: &str) {
    let value = serde_json::to_value(sample).expect("serializes");
    let mut offenders = Vec::new();
    collect_snake_keys(&value, "", &mut offenders);
    assert!(
        offenders.is_empty(),
        "snake_case field names leaked for {label}:\n  {offenders:#?}\n  serialized = {value:#?}"
    );
}

// -----------------------------------------------------------------------------
// Samples — keep these shaped the way the real daemon emits. If a field gets
// added to the protocol, add it here so the rename-audit covers it.
// -----------------------------------------------------------------------------

fn sample_identity() -> MessageIdentity {
    MessageIdentity {
        sub: "spiffe://x/y".into(),
        name: Some("Alice".into()),
        kind: IdentityType::Human,
    }
}

fn sample_session_info() -> SessionInfo {
    SessionInfo {
        id: "s".into(),
        name: "Demo".into(),
        workdir: "/tmp".into(),
        status: SessionStatus::Working,
        created_by: "me".into(),
        created_at: "2026-04-22T00:00:00Z".into(),
        attached_clients: 1,
        mode: Some(SessionMode::Interactive),
        turns_remaining: Some(10),
        pinned_files: Some(vec!["README.md".into()]),
        agent_uri: None,
        subagents: None,
        usage: Some(SessionUsage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_creation_tokens: 4,
            total_cost_usd: 0.05,
            num_turns: 6,
            duration_ms: 7,
            recent_turns: None,
            peak_input_tokens: Some(8),
            last_turn_input_tokens: Some(9),
            last_turn_output_tokens: Some(10),
            last_turn_cost_usd: Some(0.11),
            last_turn_cache_hit_rate: Some(0.12),
        }),
        rotation: None,
        queued_messages: Some(0),
        model: Some("claude-opus-4-7".into()),
        fallback_model: None,
    }
}

fn sample_session_message() -> SessionMessage {
    SessionMessage {
        session_id: "s".into(),
        message_id: "m".into(),
        role: MessageRole::ToolCall,
        content: String::new(),
        parts: Some(vec![
            ContentPart::Text {
                text: "hi".into(),
                markdown: Some(false),
            },
            ContentPart::Code {
                code: "fn x() {}".into(),
                language: Some("rust".into()),
                file_path: Some("lib.rs".into()),
            },
            ContentPart::Diff {
                path: "x".into(),
                added: 1,
                removed: 0,
                original_path: Some("old_x".into()),
            },
            ContentPart::Progress {
                message: "running".into(),
                percent: Some(42),
                elapsed_ms: Some(3000),
            },
        ]),
        identity: sample_identity(),
        tool: Some(ToolInfo {
            tool_id: "t".into(),
            name: "Bash".into(),
            state: ToolState::WaitingConfirmation {
                input: serde_json::json!({"command": "ls"}),
                description: "run ls".into(),
                approval_id: "a1".into(),
            },
        }),
        metadata: None,
        timestamp: "2026-04-22T00:00:00Z".into(),
    }
}

// -----------------------------------------------------------------------------
// Actual tests — one per top-level protocol type. If a new variant lands in
// `ClientMessage` or `DaemonMessage`, add it here.
// -----------------------------------------------------------------------------

#[test]
fn client_messages_are_camel_case_on_wire() {
    let samples: Vec<(&str, ClientMessage)> = vec![
        (
            "SessionCreate",
            ClientMessage::SessionCreate {
                id: "1".into(),
                name: "n".into(),
                workdir: "/".into(),
            },
        ),
        ("SessionList", ClientMessage::SessionList { id: "1".into() }),
        (
            "SessionAttach",
            ClientMessage::SessionAttach {
                id: "1".into(),
                session_id: "s".into(),
            },
        ),
        (
            "SessionDetach",
            ClientMessage::SessionDetach {
                id: "1".into(),
                session_id: "s".into(),
            },
        ),
        (
            "SessionSend",
            ClientMessage::SessionSend {
                id: "1".into(),
                session_id: "s".into(),
                text: "hi".into(),
                attachments: Some(vec![Attachment {
                    path: "README.md".into(),
                    content: Some("x".into()),
                    mime_type: Some("text/plain".into()),
                    data: None,
                }]),
                priority: Some(SendPriority::Now),
            },
        ),
        (
            "SessionInterrupt",
            ClientMessage::SessionInterrupt {
                id: "1".into(),
                session_id: "s".into(),
            },
        ),
        (
            "SessionApprove",
            ClientMessage::SessionApprove {
                id: "1".into(),
                session_id: "s".into(),
                approval_id: "a".into(),
                approved: true,
                updated_input: None,
            },
        ),
        (
            "SessionDestroy",
            ClientMessage::SessionDestroy {
                id: "1".into(),
                session_id: "s".into(),
            },
        ),
        (
            "SessionSetMode",
            ClientMessage::SessionSetMode {
                id: "1".into(),
                session_id: "s".into(),
                mode: SessionMode::Autonomous,
                max_turns: Some(50),
            },
        ),
        (
            "SessionPin",
            ClientMessage::SessionPin {
                id: "1".into(),
                session_id: "s".into(),
                path: "README.md".into(),
            },
        ),
        (
            "SessionUnpin",
            ClientMessage::SessionUnpin {
                id: "1".into(),
                session_id: "s".into(),
                path: "README.md".into(),
            },
        ),
        (
            "SessionRotate",
            ClientMessage::SessionRotate {
                id: "1".into(),
                session_id: "s".into(),
            },
        ),
        (
            "SessionSearch",
            ClientMessage::SessionSearch {
                id: "1".into(),
                query: "bug".into(),
                scope: Some(SearchScope::Workspace),
                workdir: Some("/".into()),
                limit: Some(5),
            },
        ),
        (
            "SessionSetModel",
            ClientMessage::SessionSetModel {
                id: "1".into(),
                session_id: "s".into(),
                model: "opus".into(),
                fallback_model: Some(Some("sonnet".into())),
            },
        ),
        (
            "SessionRename",
            ClientMessage::SessionRename {
                id: "1".into(),
                session_id: "s".into(),
                name: "renamed".into(),
            },
        ),
    ];

    for (label, msg) in samples {
        assert_no_snake_case_keys(msg, label);
    }
}

#[test]
fn daemon_messages_are_camel_case_on_wire() {
    use codeoid_protocol::AuthOkMsg;
    let samples: Vec<(&str, DaemonMessage)> = vec![
        (
            "AuthOk",
            DaemonMessage::AuthOk(AuthOkMsg {
                identity: sample_identity(),
                scopes: vec!["session:list".into()],
                protocol_version: Some(1),
            }),
        ),
        (
            "ResponseOk",
            DaemonMessage::ResponseOk {
                request_id: "r".into(),
                data: Some(serde_json::json!({"okKey": true})),
            },
        ),
        (
            "ResponseError",
            DaemonMessage::ResponseError {
                request_id: "r".into(),
                error: "nope".into(),
                code: ErrorCode::Forbidden,
            },
        ),
        (
            "SessionListResult",
            DaemonMessage::SessionListResult {
                request_id: "r".into(),
                sessions: vec![sample_session_info()],
            },
        ),
        (
            "SessionStatusChange",
            DaemonMessage::SessionStatusChange {
                session_id: "s".into(),
                status: SessionStatus::WaitingApproval,
                timestamp: "t".into(),
            },
        ),
        (
            "SessionInfoUpdate",
            DaemonMessage::SessionInfoUpdate {
                session: sample_session_info(),
                timestamp: "t".into(),
            },
        ),
        (
            "ScrollbackReplay",
            DaemonMessage::ScrollbackReplay {
                session_id: "s".into(),
                messages: vec![sample_session_message()],
            },
        ),
        (
            "SessionMessage",
            DaemonMessage::SessionMessage(sample_session_message()),
        ),
        (
            "SessionMessageDelta",
            DaemonMessage::SessionMessageDelta(SessionMessageDelta {
                session_id: "s".into(),
                message_id: "m".into(),
                content_append: Some("x".into()),
                parts_append: None,
                parts_update: None,
                tool_state_update: Some(ToolState::Completed {
                    success: true,
                    output: Some("done".into()),
                    elapsed_ms: Some(200),
                    confirmed_by: Some(ConfirmedBy::User),
                }),
                timestamp: "t".into(),
            }),
        ),
    ];

    for (label, msg) in samples {
        assert_no_snake_case_keys(msg, label);
    }
}

#[test]
fn tool_state_cancelled_has_camel_case_fields() {
    let state = ToolState::Cancelled {
        reason: CancelReason::Denied,
        message: Some("no".into()),
    };
    assert_no_snake_case_keys(state, "ToolState::Cancelled");
}

#[test]
fn waiting_confirmation_roundtrips_approval_id() {
    // Regression test for the bug where `approval_id` serialized as
    // snake_case and incoming `approvalId` from the daemon failed to
    // deserialize — swallowing every approval gate.
    let raw = r#"{
        "phase": "waiting_confirmation",
        "input": {"command": "ls"},
        "description": "list files",
        "approvalId": "a-42"
    }"#;
    let state: ToolState = serde_json::from_str(raw).expect("parses");
    match state {
        ToolState::WaitingConfirmation { approval_id, .. } => {
            assert_eq!(approval_id, "a-42");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_executing_roundtrips_elapsed_ms() {
    let raw = r#"{ "phase": "executing", "elapsedMs": 1234 }"#;
    let state: ToolState = serde_json::from_str(raw).unwrap();
    match state {
        ToolState::Executing { elapsed_ms, .. } => {
            assert_eq!(elapsed_ms, Some(1234));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_completed_roundtrips_confirmed_by() {
    let raw = r#"{
        "phase": "completed",
        "success": true,
        "elapsedMs": 500,
        "confirmedBy": "auto"
    }"#;
    let state: ToolState = serde_json::from_str(raw).unwrap();
    match state {
        ToolState::Completed {
            elapsed_ms,
            confirmed_by,
            ..
        } => {
            assert_eq!(elapsed_ms, Some(500));
            assert_eq!(confirmed_by, Some(ConfirmedBy::Auto));
        }
        _ => panic!("wrong variant"),
    }
}
