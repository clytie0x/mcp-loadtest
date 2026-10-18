//! `lost_update` scenario — detects acknowledged writes that never land.
//!
//! `race_check` sends identical arguments, so it cannot see one writer
//! overwriting another. This scenario gives every write a unique marker:
//!
//! 1. Control: write one marker on a fresh session, then read on the run
//!    session. Seeing it proves both that the read observes writes and that
//!    sessions share state; otherwise the check is inconclusive and stops
//!    before any concurrent write.
//! 2. `concurrent` independent sessions, released through the pool's start
//!    gate, each send `writes_per_worker` writes. `{marker}`, `{worker}` and
//!    `{seq}` inside string values of `write_args` are substituted per call.
//! 3. One read on the run session. Every marker whose write was acknowledged
//!    must appear in it; each missing one counts as a lost update.
//!
//! Because step 1 already proved that state is shared, a final read that
//! shows none of the acknowledged markers (for example, a store corrupted by
//! interleaved writes) counts every one of them as lost.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::scenario::race_check::call_tool_result_to_value;
use crate::scenario::{
    RunContext, Scenario, ScenarioOutcome, classify_error, is_logical_tool_error, pool, teardown,
};
use mcp_loadtest_core::metrics::CallOutcome;
use mcp_loadtest_protocol::Session;
use mcp_loadtest_protocol::hang_detector::{HangOutcome, hang_detect};
use mcp_loadtest_protocol::mcp::CallToolResult;

/// Placeholder replaced by a run-unique, fixed-width marker per write.
pub const MARKER_PLACEHOLDER: &str = "{marker}";

/// Concurrent uniquely-marked writes, then one read that must contain them.
pub struct LostUpdate {
    /// Independent sessions writing at once. Must be at least 2.
    pub concurrent: u32,
    /// Writes each session sends. Must be at least 1.
    pub writes_per_worker: u32,
    /// Tool that records one write.
    pub write_tool: String,
    /// Arguments for `write_tool`; must contain `{marker}` in a string value.
    pub write_args: Value,
    /// Tool that returns the stored state.
    pub read_tool: String,
    /// Arguments for `read_tool`, passed verbatim.
    pub read_args: Value,
}

#[async_trait]
impl Scenario for LostUpdate {
    async fn drive(&self, session: &mut Session, ctx: &RunContext) -> ScenarioOutcome {
        if let Some(message) = self.validation_error() {
            return invalid_plan(message);
        }
        let Some(factory) = ctx.session_factory.clone() else {
            return invalid_plan(
                "a session_factory is required for concurrent sessions \
                 (Run::execute attaches one automatically)",
            );
        };
        let nonce = format!("{:08x}", rand::random::<u32>());
        let mut outcome = ScenarioOutcome::default();

        // 1. Control: a write on a fresh session must be visible to a read on
        //    the run session, which proves both that the read observes writes
        //    and that sessions share state.
        let mut probe = tokio::select! {
            biased;
            _ = ctx.cancel_token.cancelled() => {
                outcome.notes.push("lost_update: cancelled before the control write".to_owned());
                return outcome;
            }
            spawned = factory.spawn() => match spawned {
                Ok(probe) => probe,
                Err(err) => {
                    outcome.error_count += 1;
                    outcome
                        .notes
                        .push(format!("lost_update: control session failed to start: {err}"));
                    return outcome;
                }
            },
        };
        let control = marker(&nonce, "c", 0);
        let control_args = render(&self.write_args, "control", 0, &control);
        let written = call(
            &mut probe,
            &self.write_tool,
            &control_args,
            ctx,
            &mut outcome,
            "control write",
        )
        .await
        .is_some();
        teardown::shutdown_session(probe, &mut outcome, "lost_update control session").await;
        if !written {
            outcome
                .notes
                .push("lost_update: control write failed; cannot run the check".to_owned());
            return outcome;
        }
        match call(
            session,
            &self.read_tool,
            &self.read_args,
            ctx,
            &mut outcome,
            "control read",
        )
        .await
        {
            Some(read) if contains(&read, &control) => {}
            Some(_) => {
                outcome.error_count += 1;
                outcome.notes.push(format!(
                    "lost_update inconclusive: `{}` on one session did not show the marker \
                     `{}` wrote on another. Either the read does not return what the write \
                     stored, or sessions do not share state (for example a stdio server that \
                     keeps data in process memory)",
                    self.read_tool, self.write_tool
                ));
                return outcome;
            }
            None => return outcome,
        }

        // 2. Concurrent uniquely-marked writes across independent sessions.
        let acknowledged = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&acknowledged);
        let write_tool = Arc::new(self.write_tool.clone());
        let write_args = Arc::new(self.write_args.clone());
        let writes = self.writes_per_worker;
        let nonce = Arc::new(nonce);
        let pooled = pool::drive_pooled(ctx, self.concurrent, move |worker, mut session, wctx| {
            let acknowledged = Arc::clone(&shared);
            let write_tool = Arc::clone(&write_tool);
            let write_args = Arc::clone(&write_args);
            let nonce = Arc::clone(&nonce);
            async move {
                let mut out = ScenarioOutcome::default();
                for seq in 0..writes {
                    if wctx.is_cancelled() {
                        out.notes
                            .push(format!("cancelled: worker={worker} seq={seq}"));
                        break;
                    }
                    let mark = marker(&nonce, &worker.to_string(), seq);
                    let args = render(&write_args, &worker.to_string(), seq, &mark);
                    let label = format!("write worker={worker} seq={seq}");
                    if call(&mut session, &write_tool, &args, &wctx, &mut out, &label)
                        .await
                        .is_some()
                    {
                        match acknowledged.lock() {
                            Ok(mut marks) => marks.push(mark),
                            Err(_) => {
                                out.error_count += 1;
                                out.notes.push("marker collector lock poisoned".to_owned());
                            }
                        }
                    }
                }
                teardown::shutdown_session(
                    session,
                    &mut out,
                    format!("lost_update worker {worker}"),
                )
                .await;
                out
            }
        })
        .await;
        merge(&mut outcome, pooled);

        // 3. One read; every acknowledged marker must be in it.
        let acknowledged = match acknowledged.lock() {
            Ok(marks) => marks.clone(),
            Err(_) => {
                outcome.error_count += 1;
                outcome
                    .notes
                    .push("lost_update: marker collector lock poisoned".to_owned());
                return outcome;
            }
        };
        if acknowledged.is_empty() {
            outcome.error_count += 1;
            outcome
                .notes
                .push("lost_update inconclusive: no concurrent write was acknowledged".to_owned());
            return outcome;
        }
        let Some(read) = call(
            session,
            &self.read_tool,
            &self.read_args,
            ctx,
            &mut outcome,
            "final read",
        )
        .await
        else {
            return outcome;
        };
        let missing: Vec<&String> = acknowledged
            .iter()
            .filter(|mark| !contains(&read, mark))
            .collect();
        outcome.lost_update_count = missing.len() as u64;
        if !missing.is_empty() {
            let preview: Vec<&str> = missing.iter().take(5).map(|m| m.as_str()).collect();
            outcome.notes.push(format!(
                "lost updates: {} of {} acknowledged writes to `{}` are missing from `{}` \
                 (first: {})",
                missing.len(),
                acknowledged.len(),
                self.write_tool,
                self.read_tool,
                preview.join(", ")
            ));
        }
        outcome
    }

    fn config_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "concurrent": {
                    "type": "integer",
                    "minimum": 2,
                    "default": 10,
                    "description": "Independent sessions writing at once."
                },
                "writes_per_worker": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 5,
                    "description": "Writes each session sends."
                },
                "write_tool": {
                    "type": "string",
                    "description": "Tool that records one write."
                },
                "write_args": {
                    "type": "object",
                    "description": "Arguments for write_tool. String values may use {marker} (required), {worker}, and {seq}."
                },
                "read_tool": {
                    "type": "string",
                    "description": "Tool whose result must contain every acknowledged marker."
                },
                "read_args": {
                    "type": "object",
                    "description": "Arguments for read_tool, passed verbatim."
                }
            },
            "required": ["write_tool", "write_args", "read_tool"]
        })
    }

    fn name(&self) -> &'static str {
        "lost_update"
    }
}

impl LostUpdate {
    /// Why this plan cannot run, if it cannot.
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.concurrent < 2 {
            Some("concurrent must be >= 2")
        } else if self.writes_per_worker == 0 {
            Some("writes_per_worker must be >= 1")
        } else if !has_marker(&self.write_args) {
            Some("write_args must contain {marker} in a string value")
        } else {
            None
        }
    }
}

/// Run one call through the hang detector, recording metrics and outcome
/// counters. Returns the result only for a timely, non-error response.
async fn call(
    session: &mut Session,
    tool: &str,
    args: &Value,
    ctx: &RunContext,
    out: &mut ScenarioOutcome,
    label: &str,
) -> Option<CallToolResult> {
    let started = Instant::now();
    let result = hang_detect(
        session.call_tool(tool, args),
        ctx.hang_threshold,
        ctx.grace_period,
    )
    .await;
    out.total_calls += 1;
    match result {
        HangOutcome::Ok { result, duration } if !is_logical_tool_error(&result) => {
            out.successful_calls += 1;
            ctx.metrics
                .record_tool(tool, duration, CallOutcome::Success);
            Some(result)
        }
        HangOutcome::Ok { duration, .. } => {
            out.error_count += 1;
            ctx.metrics
                .record_tool(tool, duration, CallOutcome::ServerError);
            out.notes
                .push(format!("{label}: `{tool}` returned isError"));
            None
        }
        HangOutcome::Slow { duration, .. } => {
            out.hang_count += 1;
            ctx.metrics.record_tool(tool, duration, CallOutcome::Hang);
            out.notes
                .push(format!("{label}: `{tool}` exceeded the hang threshold"));
            None
        }
        HangOutcome::Deadlock { hung_for } => {
            out.deadlock_count += 1;
            out.hung_for_ms.push(hung_for.as_millis());
            ctx.metrics
                .record_tool(tool, hung_for, CallOutcome::Deadlock);
            out.notes.push(format!(
                "deadlock: {label} tool={tool} hung_for={}ms",
                hung_for.as_millis()
            ));
            None
        }
        HangOutcome::Err(err) => {
            out.error_count += 1;
            ctx.metrics
                .record_tool(tool, started.elapsed(), classify_error(&err));
            out.notes.push(format!("{label}: `{tool}` failed: {err}"));
            None
        }
        other => {
            out.error_count += 1;
            out.notes.push(format!(
                "{label}: unexpected hang outcome for `{tool}`: {other:?}"
            ));
            None
        }
    }
}

/// Fixed-width so no marker is a prefix of another.
fn marker(nonce: &str, worker: &str, seq: u32) -> String {
    format!("mlt{nonce}w{worker:0>4}s{seq:04}x")
}

/// Substitute `{marker}`, `{worker}` and `{seq}` in every string value.
fn render(template: &Value, worker: &str, seq: u32, mark: &str) -> Value {
    match template {
        Value::String(s) => Value::String(
            s.replace(MARKER_PLACEHOLDER, mark)
                .replace("{worker}", worker)
                .replace("{seq}", &seq.to_string()),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| render(item, worker, seq, mark))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), render(v, worker, seq, mark)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn has_marker(template: &Value) -> bool {
    match template {
        Value::String(s) => s.contains(MARKER_PLACEHOLDER),
        Value::Array(items) => items.iter().any(has_marker),
        Value::Object(map) => map.values().any(has_marker),
        _ => false,
    }
}

fn contains(result: &CallToolResult, mark: &str) -> bool {
    call_tool_result_to_value(result).to_string().contains(mark)
}

fn merge(into: &mut ScenarioOutcome, from: ScenarioOutcome) {
    into.total_calls += from.total_calls;
    into.successful_calls += from.successful_calls;
    into.hang_count += from.hang_count;
    into.deadlock_count += from.deadlock_count;
    into.error_count += from.error_count;
    into.divergence_count += from.divergence_count;
    into.lost_update_count += from.lost_update_count;
    into.incomplete_worker_count += from.incomplete_worker_count;
    into.teardown_failure_count += from.teardown_failure_count;
    into.notes.extend(from.notes);
    into.hung_for_ms.extend(from.hung_for_ms);
}

fn invalid_plan(message: &str) -> ScenarioOutcome {
    ScenarioOutcome {
        error_count: 1,
        notes: vec![format!("lost_update: invalid plan — {message}")],
        ..ScenarioOutcome::default()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn render_substitutes_every_placeholder_in_nested_strings() {
        let template = json!({
            "entity": "e{worker}",
            "observations": [{ "text": "{marker}", "n": 3 }],
            "label": "w{worker}-s{seq}"
        });
        let out = render(&template, "7", 2, "MARK");
        assert_eq!(
            out,
            json!({
                "entity": "e7",
                "observations": [{ "text": "MARK", "n": 3 }],
                "label": "w7-s2"
            })
        );
    }

    #[test]
    fn markers_are_fixed_width_and_never_prefixes_of_each_other() {
        let short = marker("abcd1234", "1", 1);
        let long = marker("abcd1234", "1", 10);
        assert_eq!(short.len(), long.len());
        assert!(!long.contains(&short) && !short.contains(&long));
    }

    #[test]
    fn plan_requires_a_marker_two_sessions_and_one_write() {
        let mut plan = LostUpdate {
            concurrent: 2,
            writes_per_worker: 1,
            write_tool: "append".into(),
            write_args: json!({ "item": "{marker}" }),
            read_tool: "read".into(),
            read_args: json!({}),
        };
        assert_eq!(plan.validation_error(), None);
        plan.write_args = json!({ "item": "fixed" });
        assert!(plan.validation_error().unwrap().contains("{marker}"));
        plan.write_args = json!({ "item": "{marker}" });
        plan.concurrent = 1;
        assert!(plan.validation_error().unwrap().contains("concurrent"));
        plan.concurrent = 2;
        plan.writes_per_worker = 0;
        assert!(
            plan.validation_error()
                .unwrap()
                .contains("writes_per_worker")
        );
    }
}
