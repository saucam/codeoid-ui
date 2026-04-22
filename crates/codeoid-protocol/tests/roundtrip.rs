//! Round-trip tests — parse real daemon JSON, re-serialize, and assert the
//! essential fields survive. Kept small so the test suite stays fast; expand
//! by dropping captured fixtures into `fixtures/` and adding new tests here.

use codeoid_protocol::{DaemonMessage, ErrorCode};

#[test]
fn auth_ok_with_protocol_version() {
    let raw = r#"{
        "type": "auth.ok",
        "identity": { "sub": "spiffe://x/y", "name": "Alice", "type": "human" },
        "scopes": ["sessions.read", "sessions.write"],
        "protocolVersion": 1
    }"#;

    let msg: DaemonMessage = serde_json::from_str(raw).expect("auth.ok parses");
    match msg {
        DaemonMessage::AuthOk(ref ok) => {
            assert_eq!(ok.protocol_version, Some(1));
            assert_eq!(ok.scopes.len(), 2);
        }
        _ => panic!("expected AuthOk"),
    }
}

#[test]
fn auth_ok_without_protocol_version_accepted() {
    // Older daemon that predates the version field — must still parse.
    let raw = r#"{
        "type": "auth.ok",
        "identity": { "sub": "s", "type": "human" },
        "scopes": []
    }"#;

    let msg: DaemonMessage = serde_json::from_str(raw).unwrap();
    match msg {
        DaemonMessage::AuthOk(ok) => assert!(ok.protocol_version.is_none()),
        _ => panic!("expected AuthOk"),
    }
}

#[test]
fn unknown_message_type_falls_through() {
    // Daemon ships a new message kind; old client must not crash.
    let raw = r#"{ "type": "future.thing", "payload": { "anything": true } }"#;
    let msg: DaemonMessage = serde_json::from_str(raw).expect("unknown parses");
    assert!(matches!(msg, DaemonMessage::Unknown));
}

#[test]
fn response_error_parses_with_code() {
    let raw = r#"{
        "type": "response.error",
        "requestId": "req-1",
        "error": "permission denied",
        "code": "forbidden"
    }"#;
    let msg: DaemonMessage = serde_json::from_str(raw).unwrap();
    match msg {
        DaemonMessage::ResponseError { code, .. } => {
            assert_eq!(code, ErrorCode::Forbidden);
        }
        _ => panic!("expected ResponseError"),
    }
}

#[test]
fn session_message_with_tool_roundtrip() {
    let raw = r#"{
        "type": "session.message",
        "sessionId": "s1",
        "messageId": "m1",
        "role": "tool_call",
        "content": "",
        "identity": { "sub": "s", "type": "agent" },
        "tool": {
            "toolId": "t1",
            "name": "Bash",
            "state": {
                "phase": "executing",
                "elapsedMs": 1200
            }
        },
        "timestamp": "2026-04-22T00:00:00.000Z"
    }"#;

    let msg: DaemonMessage = serde_json::from_str(raw).unwrap();
    let DaemonMessage::SessionMessage(sm) = msg else {
        panic!("expected SessionMessage");
    };
    let tool = sm.tool.clone().expect("tool present");
    assert_eq!(tool.name, "Bash");
    assert_eq!(tool.state.phase(), codeoid_protocol::ToolPhase::Executing);

    // And the whole thing re-serializes without losing the tool state.
    let back = serde_json::to_string(&DaemonMessage::SessionMessage(sm)).unwrap();
    assert!(back.contains("\"phase\":\"executing\""));
}

#[test]
fn session_message_delta_tool_replacement() {
    // toolStateUpdate carries full variant — not a field patch.
    let raw = r#"{
        "type": "session.message.delta",
        "sessionId": "s1",
        "messageId": "m1",
        "toolStateUpdate": {
            "phase": "completed",
            "success": true,
            "output": "ok",
            "elapsedMs": 1500
        },
        "timestamp": "2026-04-22T00:00:00.001Z"
    }"#;

    let msg: DaemonMessage = serde_json::from_str(raw).unwrap();
    let DaemonMessage::SessionMessageDelta(d) = msg else {
        panic!("expected delta");
    };
    let state = d.tool_state_update.expect("tool state present");
    assert_eq!(state.phase(), codeoid_protocol::ToolPhase::Completed);
}
