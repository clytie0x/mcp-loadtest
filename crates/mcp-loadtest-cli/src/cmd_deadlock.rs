//! `mcp-loadtest deadlock-probe` — convenience wrapper around
//! `scenario::DeadlockProbe` that doesn't require a TOML config.
//!
//! Without `--tool` the probe lists the server's tools and picks one the
//! server marks read-only that takes no required arguments (ADR 0030).

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use mcp_loadtest::Report;
use mcp_loadtest::config::{
    Config, OutputConfig, ScenarioConfig, ServerConfig, split_server_command,
};
use mcp_loadtest::protocol::mcp::Tool;
use mcp_loadtest::run::{Run, list_all_tools};
use mcp_loadtest::scenario::deadlock_probe::DeadlockProbe;

use crate::emit::emit_reports;

/// Tools listed when none qualifies for the automatic choice; the rest are
/// counted.
const LISTED_TOOLS_CAP: usize = 20;

/// Drive a `DeadlockProbe` scenario against `server` and fail loudly on any
/// signal of trouble (deadlocks, threshold violations, transport errors).
///
/// `tool: None` picks a tool from the server's `tools/list` (see
/// [`pick_probe_tool`]); `args` (default `{}`) requires a `tool`.
pub async fn run_deadlock_probe(
    server: &str,
    tool: Option<&str>,
    concurrent: u32,
    hang_threshold: Duration,
    grace_period: Duration,
    args: Option<&str>,
    output_dir: &Path,
) -> Result<()> {
    run_deadlock_probe_with_redaction(
        server,
        tool,
        concurrent,
        hang_threshold,
        grace_period,
        args,
        output_dir,
        false,
    )
    .await
}

/// Drive a deadlock probe with optional Action-only server identity redaction.
///
/// When `redact_server_identity` is true, reports, trace identity, child
/// stderr, and parse/spawn diagnostics omit the supplied server command and
/// argv. Ordinary callers should use [`run_deadlock_probe`].
#[allow(clippy::too_many_arguments)] // mirrors the CLI flags plus one private Action control
pub async fn run_deadlock_probe_with_redaction(
    server: &str,
    tool: Option<&str>,
    concurrent: u32,
    hang_threshold: Duration,
    grace_period: Duration,
    args: Option<&str>,
    output_dir: &Path,
    redact_server_identity: bool,
) -> Result<()> {
    if tool.is_none() && args.is_some() {
        anyhow::bail!("--args needs --tool: arguments only make sense for a tool you name");
    }
    let args_value: Value = match args {
        None => json!({}),
        Some(args) if redact_server_identity => serde_json::from_str(args).map_err(|_| {
            anyhow::anyhow!("parsing --args JSON failed (value redacted by Action)")
        })?,
        Some(args) => {
            serde_json::from_str(args).with_context(|| format!("parsing --args JSON: {args}"))?
        }
    };

    let server_parts = split_server_command(server);
    let (command, cmd_args) = if redact_server_identity {
        server_parts
            .map_err(|_| anyhow::anyhow!("parsing --server failed (identity redacted by Action)"))?
    } else {
        server_parts.with_context(|| format!("parsing --server `{server}`"))?
    };
    let server_cfg = ServerConfig::stdio(command, cmd_args);

    let tool = match tool {
        Some(tool) => tool.to_owned(),
        None => {
            // Same server settings as the probe; the discovery session is
            // shut down before the probe starts its own.
            let tools = list_all_tools(&server_cfg, redact_server_identity)
                .await
                .context("listing the server's tools to choose one (no --tool given)")?;
            let Some(chosen) = pick_probe_tool(&tools) else {
                anyhow::bail!("{}", no_qualifying_tool_message(&tools));
            };
            eprintln!(
                "no --tool given; probing `{}` (the server marks it read-only and it takes no \
                 required arguments). These are the server's own hints; pass --tool to choose.",
                printable(&chosen.name)
            );
            chosen.name.clone()
        }
    };

    // Inline scenario; not driven via TOML.
    let scenario = DeadlockProbe {
        concurrent,
        hang_threshold,
        grace_period,
        tool,
        args: args_value,
    };

    let config = Config::new(server_cfg, ScenarioConfig::new("deadlock_probe", json!({})))
        .with_output(OutputConfig::new(
            output_dir.to_path_buf(),
            vec!["terminal".into(), "markdown".into(), "json".into()],
        ));

    let mut run = Run::new(config, Box::new(scenario), output_dir.to_path_buf());
    if redact_server_identity {
        run = run.with_redacted_server_identity();
    }
    let report = run.execute().await?;

    let formats = vec!["terminal".into(), "markdown".into(), "json".into()];
    emit_reports(&report, &formats, output_dir)?;

    // For `deadlock-probe` specifically, ANY signal of trouble = fail:
    //   - deadlock_count > 0 (the headline check)
    //   - hang_count > 0 (a completed call still breached the operator's
    //     diagnostic threshold)
    //   - threshold violations (rolled up by Report::passed())
    //   - error_count > 0 (transport/server errors are bad signals for a
    //     probe whose whole point is "is the server healthy under load?")
    ensure_deadlock_probe_passed(&report)
}

/// The tool `deadlock-probe` calls when `--tool` is omitted: the first, in
/// the server's listed order, that the server marks read-only
/// (`annotations.readOnlyHint` explicitly `true`), does not mark destructive
/// (`annotations.destructiveHint` not `true`), and whose `inputSchema` has no
/// `required` properties, so `{}` arguments fit.
///
/// Annotations are the server's own hints, not guarantees: a misconfigured
/// or untrusted server can mark a destructive tool read-only. Nothing else
/// about the tool (its name, its description) is considered.
#[must_use]
pub fn pick_probe_tool(tools: &[Tool]) -> Option<&Tool> {
    tools
        .iter()
        .find(|tool| marked_read_only(tool) && !marked_destructive(tool) && takes_no_args(tool))
}

fn annotation(tool: &Tool, name: &str) -> Option<bool> {
    tool.annotations.as_ref()?.get(name)?.as_bool()
}

fn marked_read_only(tool: &Tool) -> bool {
    annotation(tool, "readOnlyHint") == Some(true)
}

fn marked_destructive(tool: &Tool) -> bool {
    annotation(tool, "destructiveHint") == Some(true)
}

/// `true` when `inputSchema.required` is missing, null, or an empty array.
/// Any other value, a malformed one included, counts as required arguments.
fn takes_no_args(tool: &Tool) -> bool {
    match tool.input_schema.get("required") {
        None | Some(Value::Null) => true,
        Some(Value::Array(required)) => required.is_empty(),
        Some(_) => false,
    }
}

/// The names in `inputSchema.required`, for the no-qualifying-tool list.
fn required_names(tool: &Tool) -> String {
    match tool.input_schema.get("required") {
        None | Some(Value::Null) => "none".to_owned(),
        Some(Value::Array(required)) if required.is_empty() => "none".to_owned(),
        Some(Value::Array(required)) => required
            .iter()
            .map(|name| match name.as_str() {
                Some(name) => printable(name),
                None => printable(&name.to_string()),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Some(other) => format!("unreadable ({})", printable(&other.to_string())),
    }
}

/// The error for a server where no tool qualifies: why, a short list of its
/// tools, and how to choose one.
fn no_qualifying_tool_message(tools: &[Tool]) -> String {
    let mut message = String::from(
        "no --tool given and no tool qualifies to be picked automatically. A tool qualifies \
         only when the server marks it read-only and it has no required arguments.",
    );
    if tools.is_empty() {
        message.push_str("\nThe server lists no tools.");
    } else {
        message.push_str("\nThe server's tools:");
        for tool in tools.iter().take(LISTED_TOOLS_CAP) {
            let mut marks = vec![if marked_read_only(tool) {
                "read-only"
            } else {
                "not read-only"
            }];
            if marked_destructive(tool) {
                marks.push("destructive");
            }
            let _ = write!(
                message,
                "\n  - {}: {}; required arguments: {}",
                printable(&tool.name),
                marks.join(", "),
                required_names(tool),
            );
        }
        if tools.len() > LISTED_TOOLS_CAP {
            let _ = write!(
                message,
                "\n  ... and {} more",
                tools.len() - LISTED_TOOLS_CAP
            );
        }
    }
    message.push_str("\nHint: pass --tool NAME and --args '{...}'.");
    message
}

/// Escape control characters in a server-supplied string before it reaches
/// the terminal.
fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

fn ensure_deadlock_probe_passed(report: &Report) -> Result<()> {
    let dc = report.scenario_outcome.deadlock_count;
    let hc = report.scenario_outcome.hang_count;
    let ec = report.scenario_outcome.error_count;
    let tv = report.threshold_violations.len();
    let tf = report.scenario_outcome.teardown_failure_count;
    if !report.passed() || ec > 0 {
        if dc > 0 {
            anyhow::bail!(
                "DEADLOCK DETECTED — {dc} deadlock(s), {hc} slow response(s), {ec} error(s), \
                 {tf} teardown failure(s), {tv} threshold violation(s)"
            );
        }
        anyhow::bail!(
            "deadlock-probe failed — {hc} slow response(s), {ec} error(s), \
             {tf} teardown failure(s), {tv} threshold violation(s)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_loadtest::{ProcessStats, ScenarioMetrics, ScenarioOutcome, ServerInfo};

    #[tokio::test]
    async fn action_mode_spawn_error_omits_server_identity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sentinel = "ACTION_SERVER_SECRET_7F3B";
        let server = format!("no-such-deadlock-binary --token {sentinel}");
        let error = run_deadlock_probe_with_redaction(
            &server,
            Some("echo"),
            1,
            Duration::from_millis(10),
            Duration::from_millis(10),
            None,
            tmp.path(),
            true,
        )
        .await
        .expect_err("missing server must fail");
        let diagnostic = format!("{error:#}");
        assert!(
            !diagnostic.contains(&server) && !diagnostic.contains(sentinel),
            "redacted spawn error leaked server identity: {diagnostic}"
        );
        assert!(diagnostic.contains("identity redacted"), "{diagnostic}");
    }

    fn tools(raw: Value) -> Vec<Tool> {
        serde_json::from_value(raw).expect("tool list")
    }

    #[test]
    fn pick_requires_explicit_read_only_no_destructive_and_no_required_args() {
        let listed = tools(json!([
            {"name": "unannotated", "inputSchema": {"type": "object"}},
            {"name": "read_only_false", "inputSchema": {"type": "object"},
             "annotations": {"readOnlyHint": false}},
            {"name": "read_only_string", "inputSchema": {"type": "object"},
             "annotations": {"readOnlyHint": "true"}},
            {"name": "needs_query", "inputSchema": {"type": "object", "required": ["q"]},
             "annotations": {"readOnlyHint": true}},
            {"name": "malformed_required", "inputSchema": {"type": "object", "required": "q"},
             "annotations": {"readOnlyHint": true}},
            {"name": "wipe", "inputSchema": {"type": "object"},
             "annotations": {"readOnlyHint": true, "destructiveHint": true}},
            {"name": "get_status", "inputSchema": {"type": "object", "required": []},
             "annotations": {"readOnlyHint": true, "destructiveHint": false}},
            {"name": "later", "inputSchema": {"type": "object"},
             "annotations": {"readOnlyHint": true}}
        ]));
        let chosen = pick_probe_tool(&listed).expect("a tool qualifies");
        assert_eq!(
            chosen.name, "get_status",
            "first qualifying tool in listed order"
        );

        // No name heuristics: a read-only tool called `delete_all` still
        // qualifies on its hints, and `echo` without hints does not.
        let by_hints_only = tools(json!([
            {"name": "echo", "inputSchema": {"type": "object"}},
            {"name": "delete_all", "inputSchema": {},
             "annotations": {"readOnlyHint": true}}
        ]));
        assert_eq!(
            pick_probe_tool(&by_hints_only).map(|tool| tool.name.as_str()),
            Some("delete_all")
        );
        assert!(pick_probe_tool(&listed[..6]).is_none());
    }

    #[test]
    fn no_qualifying_tool_message_lists_tools_and_the_hint() {
        let listed = tools(json!([
            {"name": "write\u{1b}[31m", "inputSchema": {"type": "object"}},
            {"name": "search", "inputSchema": {"type": "object", "required": ["query", "limit"]},
             "annotations": {"readOnlyHint": true}},
            {"name": "wipe", "inputSchema": {"type": "object"},
             "annotations": {"readOnlyHint": true, "destructiveHint": true}}
        ]));
        let message = no_qualifying_tool_message(&listed);
        assert!(
            message.contains("marks it read-only and it has no required arguments"),
            "{message}"
        );
        assert!(
            message.contains("write\\u{1b}[31m: not read-only; required arguments: none"),
            "control characters are escaped: {message}"
        );
        assert!(
            message.contains("search: read-only; required arguments: query, limit"),
            "{message}"
        );
        assert!(
            message.contains("wipe: read-only, destructive; required arguments: none"),
            "{message}"
        );
        assert!(
            message.ends_with("Hint: pass --tool NAME and --args '{...}'."),
            "{message}"
        );
        assert!(no_qualifying_tool_message(&[]).contains("The server lists no tools."));
    }

    #[tokio::test]
    async fn args_without_tool_is_rejected_before_spawning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = run_deadlock_probe(
            "no-such-deadlock-binary",
            None,
            1,
            Duration::from_millis(10),
            Duration::from_millis(10),
            Some("{}"),
            tmp.path(),
        )
        .await
        .expect_err("--args without --tool must fail");
        assert!(error.to_string().contains("--args needs --tool"), "{error}");
    }

    #[test]
    fn mixed_success_and_slow_probe_is_a_cli_failure() {
        let report = Report {
            run_id: "01TEST".into(),
            started_at: std::time::SystemTime::UNIX_EPOCH,
            duration: Duration::from_millis(25),
            scenario_name: "deadlock_probe".into(),
            server_info: ServerInfo {
                command: "fixture".into(),
                args: Vec::new(),
                pid: None,
                protocol_version: None,
            },
            metrics: ScenarioMetrics {
                outcomes: mcp_loadtest::OutcomeCounts {
                    success: 1,
                    hang: 1,
                    ..mcp_loadtest::OutcomeCounts::default()
                },
                ..ScenarioMetrics::default()
            },
            process: ProcessStats::default(),
            scenario_outcome: ScenarioOutcome {
                total_calls: 2,
                successful_calls: 1,
                hang_count: 1,
                ..ScenarioOutcome::default()
            },
            trace_path: None,
            threshold_violations: Vec::new(),
            coverage: None,
            per_tool: Default::default(),
        };

        let error = ensure_deadlock_probe_passed(&report)
            .expect_err("one slow cohort member must make the CLI fail");
        assert!(
            error.to_string().contains("1 slow response(s)"),
            "CLI diagnostic must identify the breached hang threshold: {error}"
        );
    }
}
