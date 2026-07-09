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

#[test]
fn session_ui_request_parses_from_daemon_json() {
    // Exactly the shape the TS daemon's requestUserInput() broadcasts.
    let raw = r#"{
        "type": "session.ui_request",
        "sessionId": "s1",
        "requestId": "u1",
        "method": "select",
        "title": "Pick one",
        "message": "The extension wants an answer.",
        "options": ["Allow", "Block"],
        "timeoutMs": 30000,
        "timestamp": "2026-07-09T00:00:00Z"
    }"#;
    let msg: DaemonMessage = serde_json::from_str(raw).expect("ui_request parses");
    match msg {
        DaemonMessage::SessionUiRequest(req) => {
            assert_eq!(req.request_id, "u1");
            assert_eq!(req.method, codeoid_protocol::UiRequestMethod::Select);
            assert_eq!(
                req.options.as_deref(),
                Some(&["Allow".to_string(), "Block".to_string()][..])
            );
            assert_eq!(req.timeout_ms, Some(30_000));
        }
        _ => panic!("expected SessionUiRequest"),
    }
}

#[test]
fn session_ui_resolved_unknown_reason_degrades_to_other() {
    // A future reason string must not fail deserialization — every reason
    // means "dismiss the local copy".
    let raw = r#"{
        "type": "session.ui_resolved",
        "sessionId": "s1",
        "requestId": "u1",
        "reason": "superseded_by_something_new",
        "timestamp": "t"
    }"#;
    let msg: DaemonMessage = serde_json::from_str(raw).expect("ui_resolved parses");
    match msg {
        DaemonMessage::SessionUiResolved { reason, .. } => {
            assert_eq!(reason, codeoid_protocol::UiResolvedReason::Other);
        }
        _ => panic!("expected SessionUiResolved"),
    }
}

#[test]
fn session_commands_result_parses_with_optional_fields_absent() {
    let raw = r#"{
        "type": "session.commands.result",
        "requestId": "r1",
        "sessionId": "s1",
        "providerId": "pi",
        "commands": [
            { "name": "review", "description": "Review the diff", "source": "extension" },
            { "name": "fix-tests" }
        ]
    }"#;
    let msg: DaemonMessage = serde_json::from_str(raw).expect("commands.result parses");
    match msg {
        DaemonMessage::SessionCommandsResult {
            provider_id,
            commands,
            ..
        } => {
            assert_eq!(provider_id, "pi");
            assert_eq!(commands.len(), 2);
            assert_eq!(commands[0].name, "review");
            assert!(commands[1].description.is_none());
        }
        _ => panic!("expected SessionCommandsResult"),
    }
}

#[test]
fn session_ui_response_serializes_camel_case() {
    use codeoid_protocol::ClientMessage;
    let msg = ClientMessage::SessionUiResponse {
        id: "1".into(),
        session_id: "s".into(),
        request_id: "u".into(),
        value: None,
        confirmed: Some(true),
        cancelled: None,
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "session.ui_response");
    assert_eq!(json["sessionId"], "s");
    assert_eq!(json["requestId"], "u");
    assert_eq!(json["confirmed"], true);
    // Omitted optionals must not serialize as null (the daemon's zod
    // schema treats explicit null differently from absent).
    assert!(json.get("value").is_none());
    assert!(json.get("cancelled").is_none());
}
