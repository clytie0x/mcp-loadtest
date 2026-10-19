//! [`list_all_tools`] — one session's complete `tools/list`, for callers that
//! choose a tool before they start a run (the CLI's `deadlock-probe` without
//! `--tool`, ADR 0030).

use std::collections::HashSet;
use std::path::PathBuf;

use mcp_loadtest_core::config::{Config, ScenarioConfig, ServerConfig};
use mcp_loadtest_protocol::mcp::Tool;
use mcp_loadtest_protocol::session::{Session, SessionError};
use mcp_loadtest_protocol::transport::TransportError;
use tokio::time::Instant as TokioInstant;

use super::RunError;
use super::connect::{build_session, prepare_oauth, shutdown_after_session_error};
use super::executor::{redacted_server_error, redacted_stderr_path};
use crate::scenario::{ScenarioOutcome, teardown};

/// Open one session to `server` the way a run opens its first one, read every
/// `tools/list` page (following `nextCursor`), and shut the session down
/// before returning. Connect and every page share one
/// `server.startup_timeout` budget, as a run's startup `tools/list` does.
///
/// The tools come back in the server's listed order. A teardown failure is an
/// error: the caller is about to start more sessions against the same server
/// and must not leave a stray process behind.
///
/// With `redact_server_identity` (the composite Action), a stdio child's
/// stderr is discarded and startup and teardown errors are made generic, as
/// [`super::Run::with_redacted_server_identity`] does for the run itself.
pub async fn list_all_tools(
    server: &ServerConfig,
    redact_server_identity: bool,
) -> Result<Vec<Tool>, RunError> {
    // Validation is defined on the whole config; the scenario only has to
    // name a known kind (as in `open_session`).
    let config = Config::new(
        server.clone(),
        ScenarioConfig::new("sustained", serde_json::json!({})),
    );
    config
        .validate()
        .map_err(|error| RunError::Config(error.to_string()))?;

    let stderr_log: Option<PathBuf> =
        (redact_server_identity && server.transport == "stdio").then(redacted_stderr_path);
    let startup_deadline = TokioInstant::now()
        .checked_add(server.startup_timeout)
        .ok_or_else(|| RunError::Config("server.startup_timeout is too large".into()))?;
    let redact = |error: RunError| {
        if redact_server_identity {
            redacted_server_error()
        } else {
            error
        }
    };

    let oauth = prepare_oauth(&config, startup_deadline)
        .await
        .map_err(redact)?;
    let mut session = build_session(
        &config,
        stderr_log.as_deref(),
        false,
        None,
        startup_deadline,
        oauth,
    )
    .await
    .map_err(redact)?;

    let listed = match tokio::time::timeout_at(startup_deadline, every_page(&mut session)).await {
        Ok(listed) => listed,
        Err(_) => Err(SessionError::StartupTimeout(server.startup_timeout)),
    };
    let tools = match listed {
        Ok(tools) => tools,
        Err(error) => {
            let error =
                shutdown_after_session_error(session, error, "tools/list discovery cleanup").await;
            return Err(redact(RunError::Session(error)));
        }
    };

    let mut cleanup = ScenarioOutcome::default();
    teardown::shutdown_session(session, &mut cleanup, "tools/list discovery session").await;
    if cleanup.teardown_failure_count > 0 {
        return Err(redact(RunError::Session(SessionError::Transport(
            TransportError::Other(cleanup.notes.join("; ")),
        ))));
    }
    Ok(tools)
}

/// Read `tools/list` pages until the server stops returning `nextCursor`.
async fn every_page(session: &mut Session) -> Result<Vec<Tool>, SessionError> {
    let mut tools = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let (page, next) = session.list_tools_page(cursor.as_deref()).await?;
        tools.extend(page);
        match next {
            None => return Ok(tools),
            Some(next) => {
                // A cursor seen before would loop until the deadline; say so.
                if !seen.insert(next.clone()) {
                    return Err(SessionError::Transport(TransportError::Other(format!(
                        "`tools/list` returned nextCursor `{next}` twice"
                    ))));
                }
                cursor = Some(next);
            }
        }
    }
}
