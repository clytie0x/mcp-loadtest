//! `list_target_tools` tool — the target server's `tools/list`.
//!
//! An agent needs the target's tool names and argument schemas before it can
//! ask for a probe; without this it has to guess. Spawns one session, runs the
//! handshake and `tools/list`, and shuts the session down.

use std::time::Duration;

use serde_json::{Value, json};

use crate::session::Session;

use super::{ToolError, required_str, split_server_command};

/// Upper bound for spawn + `initialize` + `tools/list`. Matches the session's
/// default startup budget plus slack for a slow `tools/list`.
const LIST_BUDGET: Duration = Duration::from_secs(30);

pub(super) fn list_target_tools_def() -> Value {
    json!({
        "name": "list_target_tools",
        "description":
            "Spawn an MCP server over stdio and return its tools/list: each tool's \
             name, description, and input schema, plus the negotiated protocol \
             version. Call this first to choose the `tool` and `args` for \
             deadlock_probe, race_check, cold_start, or sustained_load.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "server_command": {
                    "type": "string",
                    "description": "Shell-split server command (e.g. \"python -m my_mcp\")."
                }
            },
            "required": ["server_command"]
        }
    })
}

pub(super) async fn list_target_tools(args: &Value) -> Result<Value, ToolError> {
    let server_command = required_str(args, "server_command")?;
    let (command, cmd_args) = split_server_command(&server_command)?;

    let listed = tokio::time::timeout(LIST_BUDGET, async {
        let mut session = Session::spawn(&command, &cmd_args).await?;
        let protocol_version = session.server_protocol_version.clone();
        let tools = session.list_tools().await;
        // Shut down even when tools/list failed so no child is left behind.
        let shutdown = session.shutdown().await;
        let tools = tools?;
        shutdown?;
        Ok::<_, crate::session::SessionError>((protocol_version, tools))
    })
    .await
    .map_err(|_| {
        ToolError::Run(format!(
            "target did not finish spawn + tools/list within {}s",
            LIST_BUDGET.as_secs()
        ))
    })?
    .map_err(|e| ToolError::Run(e.to_string()))?;

    let (protocol_version, tools) = listed;
    let tools: Vec<Value> = tools
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect();
    Ok(json!({
        "protocol_version": protocol_version,
        "tool_count": tools.len(),
        "tools": tools,
    }))
}
