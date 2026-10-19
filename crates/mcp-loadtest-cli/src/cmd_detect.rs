//! `mcp-loadtest race-check` and `mcp-loadtest lost-update` — convenience
//! wrappers around the `RaceCheck` and `LostUpdate` scenarios that don't
//! require a TOML config.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use mcp_loadtest::config::{
    Config, OutputConfig, ScenarioConfig, ServerConfig, split_server_command,
};
use mcp_loadtest::report::failure_reasons;
use mcp_loadtest::run::Run;
use mcp_loadtest::scenario::Scenario;
use mcp_loadtest::scenario::lost_update::LostUpdate;
use mcp_loadtest::scenario::race_check::RaceCheck;

use crate::emit::emit_reports;

/// The stdio server a detector subcommand spawns for every session.
#[derive(Debug, Clone)]
pub struct DetectTarget {
    /// Server command, parsed shell-style.
    pub server: String,
    /// `KEY=VALUE` pairs added to every server process's environment.
    pub env: Vec<String>,
    /// Budget for each session's initialize handshake.
    pub startup_timeout: Duration,
}

/// Run `race_check`: `concurrent` sessions send one identical call to
/// `tool`, and any divergence between the responses fails.
pub async fn run_race_check(
    target: &DetectTarget,
    tool: &str,
    concurrent: u32,
    args: &str,
    output_dir: &Path,
) -> Result<()> {
    if concurrent < 2 {
        anyhow::bail!("--concurrent must be at least 2 to compare responses");
    }
    let scenario = RaceCheck {
        concurrent,
        tool: tool.to_owned(),
        args: parse_json_object("--args", args)?,
    };
    execute(target, "race_check", Box::new(scenario), output_dir).await
}

/// Run `lost_update`: `concurrent` sessions each send `writes_per_worker`
/// uniquely-marked writes, and one read must return every acknowledged one.
/// With `pipelined`, one session sends each round's writes back to back.
#[allow(clippy::too_many_arguments)] // mirrors the CLI flags
pub async fn run_lost_update(
    target: &DetectTarget,
    write_tool: &str,
    write_args: &str,
    read_tool: &str,
    read_args: &str,
    concurrent: u32,
    writes_per_worker: u32,
    pipelined: bool,
    output_dir: &Path,
) -> Result<()> {
    let scenario = LostUpdate {
        concurrent,
        writes_per_worker,
        write_tool: write_tool.to_owned(),
        write_args: parse_json_object("--write-args", write_args)?,
        read_tool: read_tool.to_owned(),
        read_args: parse_json_object("--read-args", read_args)?,
        pipelined,
    };
    if let Some(message) = scenario.validation_error() {
        anyhow::bail!("invalid lost-update plan: {message}");
    }
    execute(target, "lost_update", Box::new(scenario), output_dir).await
}

fn parse_json_object(flag: &str, raw: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(raw).with_context(|| format!("parsing {flag} JSON: {raw}"))?;
    if !value.is_object() {
        anyhow::bail!("{flag} must be a JSON object");
    }
    Ok(value)
}

fn parse_env(pairs: &[String]) -> Result<BTreeMap<String, String>> {
    pairs
        .iter()
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((key.to_owned(), value.to_owned())),
            _ => anyhow::bail!("--env expects KEY=VALUE, got `{pair}`"),
        })
        .collect()
}

/// Drive `scenario`, write the terminal, markdown, and JSON reports, and fail
/// with the report's reasons when it did not pass.
async fn execute(
    target: &DetectTarget,
    kind: &str,
    scenario: Box<dyn Scenario>,
    output_dir: &Path,
) -> Result<()> {
    let (command, cmd_args) = split_server_command(&target.server)
        .with_context(|| format!("parsing --server `{}`", target.server))?;
    let mut server = ServerConfig::stdio(command, cmd_args);
    server.env = parse_env(&target.env)?;
    server.startup_timeout = target.startup_timeout;

    let formats: Vec<String> = vec!["terminal".into(), "markdown".into(), "json".into()];
    let config = Config::new(server, ScenarioConfig::new(kind, json!({})))
        .with_output(OutputConfig::new(output_dir.to_path_buf(), formats.clone()));
    let report = Run::new(config, scenario, output_dir.to_path_buf())
        .execute()
        .await?;
    emit_reports(&report, &formats, output_dir)?;
    if !report.passed() {
        anyhow::bail!(
            "{} failed — {}; {}/{} successful calls; see report",
            kind.replace('_', "-"),
            failure_reasons(&report).join(", "),
            report.scenario_outcome.successful_calls,
            report.scenario_outcome.total_calls,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_pairs_split_on_the_first_equals() {
        let env = parse_env(&["A=1".into(), "URL=http://x/?a=b".into()]).expect("valid pairs");
        assert_eq!(env["A"], "1");
        assert_eq!(env["URL"], "http://x/?a=b");
        assert!(parse_env(&["NOVALUE".into()]).is_err());
        assert!(parse_env(&["=1".into()]).is_err());
    }

    #[test]
    fn json_flags_must_be_objects() {
        assert!(parse_json_object("--args", "{\"x\":1}").is_ok());
        assert!(parse_json_object("--args", "[1]").is_err());
        assert!(parse_json_object("--args", "not json").is_err());
    }
}
