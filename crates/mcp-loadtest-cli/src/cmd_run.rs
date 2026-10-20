//! `mcp-loadtest run --config <path>` — load a TOML config, build the
//! requested scenario, drive a `Run`, and emit reports.
//!
//! Split into focused submodules (kept private; the public surface is the
//! [`run_from_config`] entry, its private-Action output override
//! [`run_from_config_with_output`], the `--emit-repro` form
//! [`run_from_config_with_repro`], plus the re-exported [`parse_dur_str`]
//! helper `main.rs` shares with the `deadlock-probe` subcommand):
//! - `builder` — scenario `kind` → `Box<dyn Scenario>` dispatch
//! - `params` — generic TOML param plucking + duration parsing
//! - `patterns` — weighted multi-step pattern-config parsing

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use mcp_loadtest::StderrCapture;
use mcp_loadtest::config::Config;
use mcp_loadtest::report::failure_reasons;
use mcp_loadtest::run::Run;

use crate::emit::emit_reports;
use crate::repro::{ReproPlan, ReproRequest};

mod builder;
mod params;
mod patterns;

pub(crate) use builder::build_scenario;
pub use params::parse_dur_str;

/// Top-level entry for `Cmd::Run`. Loads `path`, builds the scenario, executes
/// the run, emits reports, and surfaces any threshold violations as an error
/// (non-zero exit code — the CI gating contract; see DESIGN.md §15.4).
///
/// `capture_stderr` / `tee_stderr` map to [`StderrCapture`] (the CLI's
/// `--capture-stderr` / `--tee-stderr` flags, wired in `main.rs` by Agent C).
/// `tee_stderr` wins if both are set (tee is a strict superset of capture).
///
/// `trace` maps to [`Run::with_trace`] (the `--trace <file>` flag): record
/// every JSON-RPC frame of the run as `mcp-trace/1` JSONL (ADR 0021).
pub async fn run_from_config(
    path: &Path,
    capture_stderr: bool,
    tee_stderr: bool,
    trace: Option<PathBuf>,
) -> Result<()> {
    run_from_config_with_output(path, capture_stderr, tee_stderr, trace, None, false).await
}

/// Run a config while optionally overriding its report root and redacting its
/// server identity.
///
/// Both controls exist for the composite Action's private execution mode.
/// Ordinary CLI and library callers should use [`run_from_config`], which
/// preserves `output.report_dir` and keeps reports self-describing.
pub async fn run_from_config_with_output(
    path: &Path,
    capture_stderr: bool,
    tee_stderr: bool,
    trace: Option<PathBuf>,
    action_output_dir: Option<PathBuf>,
    redact_server_identity: bool,
) -> Result<()> {
    run_from_config_with_repro(
        path,
        capture_stderr,
        tee_stderr,
        trace,
        action_output_dir,
        redact_server_identity,
        None,
    )
    .await
}

/// [`run_from_config_with_output`], plus `--emit-repro`: when the config is
/// a pipelined `lost_update` against a stdio server and the run finds lost
/// updates, write a script that reproduces them on the official MCP SDK
/// (ADR 0034). Any other config is refused before the run starts.
pub async fn run_from_config_with_repro(
    path: &Path,
    capture_stderr: bool,
    tee_stderr: bool,
    trace: Option<PathBuf>,
    action_output_dir: Option<PathBuf>,
    redact_server_identity: bool,
    repro: Option<&ReproRequest>,
) -> Result<()> {
    let config_result = Config::from_file(path);
    let mut config = if redact_server_identity {
        config_result.map_err(|_| {
            anyhow::anyhow!("loading Action config failed (server identity redacted)")
        })?
    } else {
        config_result.with_context(|| format!("loading config {}", path.display()))?
    };
    if let Some(output_dir) = action_output_dir {
        config.output.report_dir = output_dir;
    }
    let scenario = build_scenario(&config.scenario.kind, &config.scenario.params)?;
    // The script embeds the server command and env, which Action mode keeps
    // out of every output.
    if repro.is_some() && redact_server_identity {
        anyhow::bail!("--emit-repro is not available in Action mode");
    }
    let repro_plan = match repro {
        Some(request) => {
            let plan = ReproPlan::from_config(&config)?;
            request.check(&plan)?;
            Some(plan)
        }
        None => None,
    };
    let output_dir = config.output.report_dir.clone();
    let formats = config.output.formats.clone();

    let capture = if tee_stderr {
        StderrCapture::Tee
    } else if capture_stderr {
        StderrCapture::Capture
    } else {
        StderrCapture::Off
    };

    let mut run = Run::new(config, scenario, output_dir.clone()).with_stderr_capture(capture);
    if redact_server_identity {
        run = run.with_redacted_server_identity();
    }
    if let Some(trace_path) = trace {
        run = run.with_trace(trace_path);
    }
    let report = run.execute().await?;

    emit_reports(&report, &formats, &output_dir)?;
    if let (Some(request), Some(plan)) = (repro, &repro_plan) {
        if report.scenario_outcome.lost_update_count > 0 {
            request.write(plan)?;
        } else {
            eprintln!(
                "No lost update found, so no repro script was written to {}",
                request.path.display()
            );
        }
    }
    if !report.passed() {
        anyhow::bail!(
            "run failed correctness gate — {}; {}/{} successful calls; see report",
            failure_reasons(&report).join(", "),
            report.scenario_outcome.successful_calls,
            report.scenario_outcome.total_calls,
        );
    }
    Ok(())
}
