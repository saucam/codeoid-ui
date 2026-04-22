//! Smoke-test the client without a TUI.
//!
//! Usage:
//!     CODEOID_URL=ws://127.0.0.1:7400 CODEOID_TOKEN=eyJ...  cargo run -p codeoid-client --example headless
//!
//! Connects, lists sessions, prints them, then disconnects.

use codeoid_client::{connect, ClientHandle};
use codeoid_protocol::{ClientMessage, DaemonMessage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let url = std::env::var("CODEOID_URL").unwrap_or_else(|_| "ws://127.0.0.1:7400".into());
    let token = std::env::var("CODEOID_TOKEN")
        .map_err(|_| anyhow::anyhow!("CODEOID_TOKEN must be set"))?;

    let conn = connect(&url, &token).await?;
    println!(
        "connected as {} (scopes: {:?}) daemon_proto={:?}",
        conn.auth.identity.sub, conn.auth.scopes, conn.auth.protocol_version
    );
    if let Some(drift) = conn.version_warning {
        eprintln!(
            "⚠️  protocol version drift: client={}, daemon={:?}",
            drift.client, drift.daemon
        );
    }

    list_sessions(&conn.handle).await?;
    conn.handle.shutdown().await;
    Ok(())
}

async fn list_sessions(handle: &ClientHandle) -> anyhow::Result<()> {
    let id = ClientHandle::next_request_id();
    let outcome = handle.request(ClientMessage::SessionList { id }).await?;
    match outcome {
        codeoid_client::request::RequestOutcome::TypedResult(DaemonMessage::SessionListResult {
            sessions,
            ..
        }) => {
            println!("sessions: {}", sessions.len());
            for s in sessions {
                println!("  · {} [{}] — {:?}", s.name, s.id, s.status);
            }
        }
        other => eprintln!("unexpected outcome: {other:?}"),
    }
    Ok(())
}
