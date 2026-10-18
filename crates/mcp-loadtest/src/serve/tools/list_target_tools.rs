//! `list_target_tools` tool — the target server's `tools/list`.
//!
//! An agent needs the target's tool names and argument schemas before it can
//! ask for a probe; without this it has to guess. Spawns one session, runs the
//! handshake and `tools/list`, and shuts the session down.

use std::time::Duration;

use serde_json::{Value, json};

use super::{ToolError, target};

/// Slack on top of the startup budget for spawn and a slow `tools/list`.
const LIST_SLACK: Duration = Duration::from_secs(20);

pub(super) fn list_target_tools_def() -> Value {
    json!({
        "name": "list_target_tools",
        "description":
            "Connect to an MCP server (stdio command or remote URL) and return its tools/list: each tool's \
             name, description, and input schema, plus the negotiated protocol \
             version. Call this first to choose the `tool` and `args` for \
             deadlock_probe, race_check, lost_update, cold_start, or sustained_load. \
             Each tool's `annotations` (null when absent) are the target's own hints, \
             not guarantees: a misconfigured or untrusted server can mark a \
             destructive tool readOnlyHint. Use them only to narrow the choice \
             (prefer readOnlyHint tools for race_check and sustained_load, skip \
             destructiveHint ones); run tools make real calls, so point them only at \
             disposable data or data you may modify, whatever the annotations say.",
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
    let target = target(args)?;
    let budget = target.startup_timeout() + LIST_SLACK;

    let listed = tokio::time::timeout(budget, async {
        let mut session = target.open().await?;
        let protocol_version = session.server_protocol_version.clone();
        let tools = session.list_tools().await;
        // Shut down even when tools/list failed so no child is left behind.
        let shutdown = session.shutdown().await;
        let tools = tools.map_err(|e| ToolError::Run(e.to_string()))?;
        shutdown.map_err(|e| ToolError::Run(e.to_string()))?;
        Ok::<_, ToolError>((protocol_version, tools))
    })
    .await
    .map_err(|_| {
        ToolError::Run(format!(
            "target did not finish connect + tools/list within {}s",
            budget.as_secs()
        ))
    })??;

    let (protocol_version, tools) = listed;
    let tools: Vec<Value> = tools
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
                "annotations": tool.annotations,
            })
        })
        .collect();
    Ok(json!({
        "protocol_version": protocol_version,
        "tool_count": tools.len(),
        "tools": tools,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MCP forbids basing tool-use decisions on annotations from an untrusted
    /// server, so the agent-facing text must not present them as a safety
    /// control.
    #[test]
    fn description_treats_annotations_as_untrusted_hints() {
        let def = list_target_tools_def();
        let description = def["description"].as_str().expect("description");
        assert!(description.contains("not guarantees"), "{description}");
        assert!(description.contains("disposable data"), "{description}");
    }
}
