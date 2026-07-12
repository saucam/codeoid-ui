//! Smoke-test the client without a TUI.
//!
//! Resolves credentials exactly like the TUI's `main.rs` — CLI/env →
//! `~/.codeoid/config.json` → defaults — so this also verifies the
//! shared-login path (a key written by `codeoid login` Just Works here):
//!
//!     cargo run -p codeoid-client --example headless
//!     CODEOID_TOKEN=eyJ... cargo run -p codeoid-client --example headless
//!
//! Connects, lists sessions, prints them, then disconnects.

use codeoid_client::{connect, load_file_config, resolve_token, resolve_zeroid_url, ClientHandle};
use codeoid_protocol::{ClientMessage, DaemonMessage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    // Same layering as the TUI: env → config.json → default.
    let file = load_file_config();
    let url = std::env::var("CODEOID_URL")
        .ok()
        .or(file.daemon_url)
        .unwrap_or_else(|| "ws://127.0.0.1:7400".into());
    let zeroid_input = std::env::var("ZEROID_URL")
        .ok()
        .or(file.zeroid_url)
        .unwrap_or_else(|| "highflame".into());
    let zeroid_url = resolve_zeroid_url(&zeroid_input);
    let token = std::env::var("CODEOID_TOKEN").ok();
    let api_key = std::env::var("CODEOID_API_KEY").ok().or(file.api_key);

    let token = resolve_token(token.as_deref(), api_key.as_deref(), &zeroid_url).await?;

    let conn = connect(&url, &token).await?;
    println!(
        "connected as {} (scopes: {:?}) daemon_proto={:?}",
        conn.auth.identity.sub, conn.auth.scopes, conn.auth.protocol_version
    );

    list_sessions(&conn.handle).await?;
    list_models(&conn.handle).await?;

    // Heartbeat probe: stay idle past the 20s ping / 28s dead window so the
    // WS-ping keep-alive fires, then confirm the connection is still live.
    // `CODEOID_IDLE_SECS=30 cargo run --example headless`
    if let Ok(secs) = std::env::var("CODEOID_IDLE_SECS") {
        if let Ok(secs) = secs.parse::<u64>() {
            println!("idling {secs}s to exercise the heartbeat...");
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            println!("re-listing after idle (proves the socket survived):");
            list_sessions(&conn.handle).await?;
        }
    }

    conn.handle.shutdown().await;
    Ok(())
}

async fn list_models(handle: &ClientHandle) -> anyhow::Result<()> {
    let id = ClientHandle::next_request_id();
    let outcome = handle
        .request(ClientMessage::ModelsList { id, provider: None })
        .await?;
    match outcome {
        codeoid_client::request::RequestOutcome::TypedResult(DaemonMessage::ModelsListResult {
            models,
            live,
            ..
        }) => {
            println!(
                "models ({}): {}",
                if live { "live" } else { "fallback" },
                models.len()
            );
            for m in models {
                let def = if m.is_default.unwrap_or(false) {
                    " (default)"
                } else {
                    ""
                };
                println!("  · {} — {}{}", m.value, m.display_name, def);
            }
        }
        other => eprintln!("unexpected models outcome: {other:?}"),
    }
    Ok(())
}

async fn list_sessions(handle: &ClientHandle) -> anyhow::Result<()> {
    let id = ClientHandle::next_request_id();
    let outcome = handle.request(ClientMessage::SessionList { id }).await?;
    match outcome {
        codeoid_client::request::RequestOutcome::TypedResult(
            DaemonMessage::SessionListResult { sessions, .. },
        ) => {
            println!("sessions: {}", sessions.len());
            for s in sessions {
                println!("  · {} [{}] — {:?}", s.name, s.id, s.status);
            }
        }
        other => eprintln!("unexpected outcome: {other:?}"),
    }
    Ok(())
}
