//! Subcommand dispatch: routes each parsed [`Cmd`] to its handler and funnels
//! errors through hints.

use anyhow::{Context, Result};

use mcp_loadtest_cli::repro::ReproRequest;
use mcp_loadtest_cli::{cmd_compare, cmd_deadlock, cmd_detect, cmd_doctor, cmd_run};

use crate::Cmd;
use crate::cmd_replay;

/// Stream a rendered report to stdout (pipe-friendly), ensuring a trailing newline.
fn print_stdout(rendered: &str) {
    print!("{rendered}");
    if !rendered.ends_with('\n') {
        println!();
    }
}

/// Dispatch a parsed subcommand. Split from `main` so the error→`Hint:`
/// rendering is one funnel for every subcommand.
pub(crate) async fn dispatch(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::ExampleConfig => {
            print!("{}", mcp_loadtest::config::example_config());
            Ok(())
        }
        Cmd::ConfigSchema => {
            print_stdout(&mcp_loadtest::config::config_schema_pretty());
            Ok(())
        }
        Cmd::ListScenarios => {
            println!(
                "sustained        — steady workload; also accepts weighted tool_call/pattern configs"
            );
            println!("deadlock_probe   — Vibe-Trading-bug-class detector");
            println!(
                "cold_start       — respawns a fresh server per iteration; measures spawn→initialize handshake + first call"
            );
            println!("race_check       — identical calls + response-divergence detection");
            println!(
                "lost_update      — concurrent uniquely-marked writes, then a read that must show them all"
            );
            println!("pattern          — weighted multi-step tool-call sequences");
            println!(
                "version_matrix   — drives the same server once per MCP protocol revision and diffs the outcomes"
            );
            Ok(())
        }
        Cmd::Run {
            config,
            capture_stderr,
            tee_stderr,
            trace,
            action_output_dir,
            action_redact_server_identity,
            emit_repro,
            repro_sdk,
        } => {
            let repro = emit_repro
                .map(|path| ReproRequest::new(path, repro_sdk))
                .transpose()?;
            cmd_run::run_from_config_with_repro(
                &config,
                capture_stderr,
                tee_stderr,
                trace,
                action_output_dir,
                action_redact_server_identity,
                repro.as_ref(),
            )
            .await
        }
        Cmd::Replay {
            trace_file,
            server,
            url,
            transport,
            allow_hosts,
        } => {
            let summary = cmd_replay::run(cmd_replay::ReplayArgs {
                trace_file,
                server,
                url,
                transport,
                allow_hosts,
            })
            .await?;
            print_stdout(&summary.rendered);
            if summary.diverged > 0 {
                anyhow::bail!(
                    "replay diverged — {} of {} request frame(s) differ from the recording",
                    summary.diverged,
                    summary.total
                );
            }
            Ok(())
        }
        Cmd::Doctor {
            server,
            runs_dir,
            action_redact_server_identity,
        } => {
            cmd_doctor::run_doctor_with_redaction(server, runs_dir, action_redact_server_identity)
                .await
        }
        Cmd::Compare {
            baseline,
            current,
            format,
            max_p99_regression_pct,
            max_error_rate_regression_pp,
            allow_deadlock_increase,
        } => {
            let fmt = cmd_compare::CompareFormat::parse(&format)?;
            let thresholds = cmd_compare::RegressionThresholds {
                p99_pct: max_p99_regression_pct,
                error_rate_pp: max_error_rate_regression_pp,
                deadlock_zero_tolerance: !allow_deadlock_increase,
            };
            let outcome = cmd_compare::run(&baseline, &current, fmt, &thresholds)?;
            print_stdout(&outcome.rendered);
            cmd_compare::gate(&outcome.report)
        }
        Cmd::DeadlockProbe {
            server,
            tool,
            concurrent,
            hang_threshold,
            grace_period,
            args,
            output_dir,
            action_redact_server_identity,
        } => {
            cmd_deadlock::run_deadlock_probe_with_redaction(
                &server,
                tool.as_deref(),
                concurrent,
                cmd_run::parse_dur_str(&hang_threshold)?,
                cmd_run::parse_dur_str(&grace_period)?,
                args.as_deref(),
                &output_dir,
                action_redact_server_identity,
            )
            .await
        }
        Cmd::RaceCheck {
            server,
            tool,
            concurrent,
            args,
            env,
            startup_timeout,
            output_dir,
        } => {
            let target = cmd_detect::DetectTarget {
                server,
                env,
                startup_timeout: cmd_run::parse_dur_str(&startup_timeout)?,
                redact_server_identity: false,
            };
            cmd_detect::run_race_check(&target, &tool, concurrent, &args, &output_dir).await
        }
        Cmd::LostUpdate {
            server,
            write_tool,
            write_args,
            read_tool,
            read_args,
            concurrent,
            writes_per_worker,
            pipelined,
            emit_repro,
            repro_sdk,
            env,
            startup_timeout,
            output_dir,
            action_redact_server_identity,
        } => {
            let repro = emit_repro
                .map(|path| ReproRequest::new(path, repro_sdk))
                .transpose()?;
            let target = cmd_detect::DetectTarget {
                server,
                env,
                startup_timeout: cmd_run::parse_dur_str(&startup_timeout)?,
                redact_server_identity: action_redact_server_identity,
            };
            cmd_detect::run_lost_update(
                &target,
                write_tool.as_deref(),
                write_args.as_deref(),
                read_tool.as_deref(),
                read_args.as_deref(),
                concurrent,
                writes_per_worker,
                pipelined,
                repro.as_ref(),
                &output_dir,
            )
            .await
        }
        Cmd::Serve { mcp } => {
            if !mcp {
                anyhow::bail!("only --mcp (stdio) is supported; HTTP/SSE serve modes deferred",);
            }
            mcp_loadtest::serve::McpServer::new()
                .run_stdio()
                .await
                .context("running mcp-loadtest serve --mcp")?;
            Ok(())
        }
    }
}
