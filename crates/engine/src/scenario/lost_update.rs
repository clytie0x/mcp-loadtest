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
//! 3. One read on the run session. Every marker whose write was acknowledged,
//!    the control's included, must appear in it; each missing one counts as a
//!    lost update.
//!
//! A concurrent write the server refuses with an `isError` result was never
//! acknowledged, so it counts as refused, not as an error or a loss
//! (ADR 0032). A server that rejects conflicting writes is doing its job.
//!
//! With `pipelined` (ADR 0033) there is one session and one server
//! process: each of `writes_per_worker` rounds sends `concurrent` writes
//! back to back before reading any reply, the way a client that runs tool
//! calls in parallel does. `{worker}` is the write's slot in its round.
//!
//! Because step 1 already proved that state is shared, a final read that
//! shows none of the acknowledged markers (for example, a store corrupted by
//! interleaved writes) counts every one of them as lost.
//!
//! With `recheck_losses` (set when the CLI picked the read tool, ADR 0035),
//! a loss is confirmed before it counts: a few more writes, one at a time,
//! and a second read must show them and everything the first read showed.
//! A read that caps, pages or sorts its output fails that and leaves the
//! check inconclusive instead of reporting writes it merely did not list.

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
use mcp_loadtest_protocol::mcp::{CallToolResult, Content};

/// Placeholder replaced by a run-unique, fixed-width marker per write.
pub const MARKER_PLACEHOLDER: &str = "{marker}";

/// Length of every marker substituted for [`MARKER_PLACEHOLDER`], for
/// callers that check a string field can hold one.
pub const MARKER_LEN: usize = 22;

/// Workers and writes per worker that keep their indexes within the
/// markers' four-digit fields.
const MAX_FOUR_DIGITS: u32 = 10_000;

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
    /// Send each round's writes pipelined on the run session instead of
    /// from `concurrent` separate sessions (ADR 0033).
    pub pipelined: bool,
    /// Confirm missing markers with sequential writes and a second read
    /// before counting them as lost (ADR 0035). For a read tool nobody
    /// vouched for: one that does not list everything is then reported as
    /// inconclusive instead of as lost updates.
    pub recheck_losses: bool,
}

/// Sequential writes a recheck makes before its second read.
const RECHECK_WRITES: u32 = 3;

#[async_trait]
impl Scenario for LostUpdate {
    async fn drive(&self, session: &mut Session, ctx: &RunContext) -> ScenarioOutcome {
        if let Some(message) = self.validation_error() {
            return invalid_plan(message);
        }
        if self.pipelined {
            return self.drive_pipelined(session, ctx).await;
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
        let written = call_strict(
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
        match call_strict(
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
        let first_refusal: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let shared_refusal = Arc::clone(&first_refusal);
        let write_tool = Arc::new(self.write_tool.clone());
        let write_args = Arc::new(self.write_args.clone());
        let writes = self.writes_per_worker;
        let shared_nonce = Arc::new(nonce.clone());
        let pooled = pool::drive_pooled(ctx, self.concurrent, move |worker, mut session, wctx| {
            let acknowledged = Arc::clone(&shared);
            let first_refusal = Arc::clone(&shared_refusal);
            let write_tool = Arc::clone(&write_tool);
            let write_args = Arc::clone(&write_args);
            let nonce = Arc::clone(&shared_nonce);
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
                    match call(&mut session, &write_tool, &args, &wctx, &mut out, &label).await {
                        Called::Ok(_) => match acknowledged.lock() {
                            Ok(mut marks) => marks.push(mark),
                            Err(_) => {
                                out.error_count += 1;
                                out.notes.push("marker collector lock poisoned".to_owned());
                            }
                        },
                        Called::Refused(text) => {
                            out.refused_write_count += 1;
                            if let Ok(mut first) = first_refusal.lock() {
                                first.get_or_insert(text);
                            }
                        }
                        Called::Failed => {}
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
        let first_refusal = first_refusal.lock().ok().and_then(|first| first.clone());
        self.note_refusals(&mut outcome, first_refusal);

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
        self.verify(session, ctx, outcome, &acknowledged, &control, &nonce)
            .await
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
                },
                "pipelined": {
                    "type": "boolean",
                    "default": false,
                    "description": "Send each round of `concurrent` writes back to back on one session (one server process) instead of from separate sessions."
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
    /// The pipelined mode (ADR 0033): control, then rounds of in-flight
    /// writes, all on the run session.
    async fn drive_pipelined(&self, session: &mut Session, ctx: &RunContext) -> ScenarioOutcome {
        let nonce = format!("{:08x}", rand::random::<u32>());
        let mut outcome = ScenarioOutcome::default();

        // 1. Control: with one process, this proves only that the read
        //    observes writes.
        let control = marker(&nonce, "c", 0);
        let control_args = render(&self.write_args, "control", 0, &control);
        if call_strict(
            session,
            &self.write_tool,
            &control_args,
            ctx,
            &mut outcome,
            "control write",
        )
        .await
        .is_none()
        {
            outcome
                .notes
                .push("lost_update: control write failed; cannot run the check".to_owned());
            return outcome;
        }
        match call_strict(
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
                    "lost_update inconclusive: `{}` did not show the marker `{}` wrote \
                     just before on the same session, so the read does not return what \
                     the write stored",
                    self.read_tool, self.write_tool
                ));
                return outcome;
            }
            None => return outcome,
        }

        // 2. Rounds of `concurrent` writes in flight at once.
        let mut acknowledged = Vec::new();
        let mut first_refusal = None;
        let budget = ctx.hang_threshold.saturating_add(ctx.grace_period);
        for seq in 0..self.writes_per_worker {
            if ctx.is_cancelled() {
                outcome
                    .notes
                    .push(format!("cancelled before pipelined round {seq}"));
                break;
            }
            let marks: Vec<String> = (0..self.concurrent)
                .map(|slot| marker(&nonce, &slot.to_string(), seq))
                .collect();
            let args: Vec<Value> = marks
                .iter()
                .enumerate()
                .map(|(slot, mark)| render(&self.write_args, &slot.to_string(), seq, mark))
                .collect();
            let calls: Vec<(&str, &Value)> = args
                .iter()
                .map(|args| (self.write_tool.as_str(), args))
                .collect();
            let deadline = tokio::time::Instant::now() + budget;
            let replies = match session.call_tools_pipelined(&calls, deadline).await {
                Ok(replies) => replies,
                Err(err) => {
                    outcome.total_calls += calls.len() as u64;
                    outcome.error_count += 1;
                    ctx.metrics
                        .record_tool(&self.write_tool, budget, classify_error(&err));
                    outcome.notes.push(format!(
                        "pipelined round {seq}: `{}` failed: {err}",
                        self.write_tool
                    ));
                    return outcome;
                }
            };
            let mut unanswered = 0u32;
            for (slot, (reply, mark)) in replies.into_iter().zip(marks).enumerate() {
                outcome.total_calls += 1;
                let label = format!("write worker={slot} seq={seq}");
                let tool = self.write_tool.as_str();
                match reply.result {
                    None => {
                        unanswered += 1;
                        outcome.deadlock_count += 1;
                        outcome.hung_for_ms.push(budget.as_millis());
                        ctx.metrics.record_tool(tool, budget, CallOutcome::Deadlock);
                        outcome.notes.push(format!(
                            "deadlock: {label} tool={tool} hung_for={}ms",
                            budget.as_millis()
                        ));
                    }
                    Some(Ok(result)) if is_logical_tool_error(&result) => {
                        outcome.refused_write_count += 1;
                        ctx.metrics
                            .record_tool(tool, reply.elapsed, CallOutcome::ServerError);
                        first_refusal.get_or_insert_with(|| error_text(&result));
                    }
                    Some(Ok(_)) if reply.elapsed >= ctx.hang_threshold => {
                        outcome.hang_count += 1;
                        ctx.metrics
                            .record_tool(tool, reply.elapsed, CallOutcome::Hang);
                        outcome
                            .notes
                            .push(format!("{label}: `{tool}` exceeded the hang threshold"));
                    }
                    Some(Ok(_)) => {
                        outcome.successful_calls += 1;
                        ctx.metrics
                            .record_tool(tool, reply.elapsed, CallOutcome::Success);
                        acknowledged.push(mark);
                    }
                    Some(Err(err)) => {
                        outcome.error_count += 1;
                        ctx.metrics
                            .record_tool(tool, reply.elapsed, classify_error(&err));
                        outcome
                            .notes
                            .push(format!("{label}: `{tool}` failed: {err}"));
                    }
                }
            }
            if unanswered > 0 {
                outcome.notes.push(format!(
                    "lost_update: {unanswered} pipelined write{} still unanswered after {}ms; \
                     stopping, since their replies may still arrive on this session",
                    if unanswered == 1 { "" } else { "s" },
                    budget.as_millis()
                ));
                return outcome;
            }
        }
        self.note_refusals(&mut outcome, first_refusal);

        // 3. One read; every acknowledged marker must be in it.
        self.verify(session, ctx, outcome, &acknowledged, &control, &nonce)
            .await
    }

    /// Step 3: one read on the run session must show every acknowledged
    /// marker and the control.
    async fn verify(
        &self,
        session: &mut Session,
        ctx: &RunContext,
        mut outcome: ScenarioOutcome,
        acknowledged: &[String],
        control: &str,
        nonce: &str,
    ) -> ScenarioOutcome {
        if acknowledged.is_empty() {
            outcome.error_count += 1;
            outcome
                .notes
                .push("lost_update inconclusive: no concurrent write was acknowledged".to_owned());
            return outcome;
        }
        let Some(read) = call_strict(
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
        // The control write was acknowledged and read back before any
        // concurrent write, so losing it now means the store dropped data it
        // already held (for example, a writer that read a truncated file).
        let control_lost = !contains(&read, control);
        outcome.lost_update_count = missing.len() as u64 + u64::from(control_lost);
        let mut loss_notes = Vec::new();
        if control_lost {
            loss_notes.push(format!(
                "lost update: the control write `{control}`, read back before the concurrent \
                 writes began, is missing from `{}`; the store dropped data it already held",
                self.read_tool
            ));
        }
        if !missing.is_empty() {
            let preview: Vec<&str> = missing.iter().take(5).map(|m| m.as_str()).collect();
            loss_notes.push(format!(
                "lost updates: {} of {} acknowledged writes to `{}` are missing from `{}` \
                 (first: {})",
                missing.len(),
                acknowledged.len(),
                self.write_tool,
                self.read_tool,
                preview.join(", ")
            ));
        }
        if self.recheck_losses && outcome.lost_update_count > 0 {
            let shown: Vec<&str> = acknowledged
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(control))
                .filter(|mark| contains(&read, mark))
                .collect();
            let mut missing: Vec<&str> = missing.iter().map(|m| m.as_str()).collect();
            if control_lost {
                missing.push(control);
            }
            return self
                .recheck(
                    session, ctx, outcome, loss_notes, &read, &shown, &missing, nonce,
                )
                .await;
        }
        outcome.notes.extend(loss_notes);
        outcome
    }

    /// Confirm a loss before it counts (ADR 0035): write [`RECHECK_WRITES`]
    /// markers one at a time, then read again. The second read must show
    /// every one of them, every marker the first read showed (`shown`),
    /// every other entry of the first read (`first`), and still none of the
    /// `missing` ones. Otherwise the read is not a
    /// complete listing (it caps, pages or sorts its output) or not a
    /// stable one, and the check is inconclusive rather than failed.
    /// `loss_notes` describe the loss and are kept only when it is
    /// confirmed.
    #[allow(clippy::too_many_arguments)] // the loss as `verify` found it
    async fn recheck(
        &self,
        session: &mut Session,
        ctx: &RunContext,
        mut outcome: ScenarioOutcome,
        loss_notes: Vec<String>,
        first: &CallToolResult,
        shown: &[&str],
        missing: &[&str],
        nonce: &str,
    ) -> ScenarioOutcome {
        let lost = outcome.lost_update_count;
        // A failed recheck call is already counted by `call_strict`; only a
        // read that answered but cannot confirm the loss adds an error.
        let unconfirmed = |outcome: &mut ScenarioOutcome, why: String, counted: bool| {
            outcome.lost_update_count = 0;
            if !counted {
                outcome.error_count += 1;
            }
            outcome.notes.push(format!(
                "lost_update inconclusive: {lost} acknowledged write{} did not show in `{}`, \
                 but a recheck could not confirm the loss: {why}. Name a read tool that \
                 returns everything stored with --read-tool",
                if lost == 1 { "" } else { "s" },
                self.read_tool
            ));
        };

        let mut written = Vec::new();
        for seq in 0..RECHECK_WRITES {
            let mark = marker(nonce, "r", seq);
            let args = render(&self.write_args, "recheck", seq, &mark);
            let label = format!("recheck write {seq}");
            if call_strict(session, &self.write_tool, &args, ctx, &mut outcome, &label)
                .await
                .is_none()
            {
                unconfirmed(&mut outcome, format!("`{}` failed", self.write_tool), true);
                return outcome;
            }
            written.push(mark);
        }
        let Some(read) = call_strict(
            session,
            &self.read_tool,
            &self.read_args,
            ctx,
            &mut outcome,
            "recheck read",
        )
        .await
        else {
            unconfirmed(&mut outcome, "the second read failed".to_owned(), true);
            return outcome;
        };

        let unlisted = written.iter().filter(|mark| !contains(&read, mark)).count();
        let dropped = shown.iter().filter(|mark| !contains(&read, mark)).count();
        let reappeared = missing.iter().filter(|mark| contains(&read, mark)).count();
        // A capped or sorted read of a store that held other data can drop
        // that data, not a marker, to make room for the recheck writes.
        let run_prefix = format!("mlt{nonce}");
        let evicted = evicted_entries(first, &read, &run_prefix);
        if unlisted > 0 || dropped > 0 || evicted > 0 {
            unconfirmed(
                &mut outcome,
                format!(
                    "after {RECHECK_WRITES} more writes made one at a time, the second read \
                     lacked {unlisted} of them, {dropped} marker{} and {evicted} other \
                     entr{} the first read showed, so the read does not list everything \
                     stored (it may cap, page or sort its output)",
                    if dropped == 1 { "" } else { "s" },
                    if evicted == 1 { "y" } else { "ies" }
                ),
                false,
            );
        } else if reappeared > 0 {
            unconfirmed(
                &mut outcome,
                format!(
                    "{reappeared} of the missing markers appeared in a second read, so the \
                     read does not yet show every acknowledged write when it answers"
                ),
                false,
            );
        } else {
            outcome.notes.extend(loss_notes);
            outcome.notes.push(format!(
                "lost_update: confirmed by a recheck: after {RECHECK_WRITES} more writes made \
                 one at a time, `{}` showed all of them and everything it showed before, and \
                 still lacks the missing markers",
                self.read_tool
            ));
        }
        outcome
    }

    /// One note for every write the server refused, quoting the first.
    fn note_refusals(&self, outcome: &mut ScenarioOutcome, first_refusal: Option<String>) {
        if outcome.refused_write_count == 0 {
            return;
        }
        let text = first_refusal.unwrap_or_default();
        let refused = outcome.refused_write_count;
        outcome.notes.push(format!(
            "lost_update: the server refused {refused} concurrent write{} \
             (`{}` returned isError: {text}); a refused write was never acknowledged, \
             so it is not expected in the read",
            if refused == 1 { "" } else { "s" },
            self.write_tool
        ));
    }

    /// Why this plan cannot run, if it cannot.
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.concurrent < 2 {
            Some("concurrent must be >= 2")
        } else if self.writes_per_worker == 0 {
            Some("writes_per_worker must be >= 1")
        } else if self.concurrent > MAX_FOUR_DIGITS || self.writes_per_worker > MAX_FOUR_DIGITS {
            // Worker and sequence numbers must fit their four-digit fields,
            // or markers outgrow MARKER_LEN.
            Some("concurrent and writes_per_worker must be <= 10000")
        } else if !has_marker(&self.write_args) {
            Some("write_args must contain {marker} in a string value")
        } else {
            None
        }
    }
}

/// How one call ended.
enum Called {
    Ok(CallToolResult),
    /// The tool returned `isError`; carries its text. Not yet counted in
    /// `out`, because only the caller knows whether a refusal is an error.
    Refused(String),
    /// Hang, deadlock or call error, already counted and noted.
    Failed,
}

/// Like [`call`], but a refusal is an error: the control write and the
/// reads must succeed for the check to mean anything.
async fn call_strict(
    session: &mut Session,
    tool: &str,
    args: &Value,
    ctx: &RunContext,
    out: &mut ScenarioOutcome,
    label: &str,
) -> Option<CallToolResult> {
    match call(session, tool, args, ctx, out, label).await {
        Called::Ok(result) => Some(result),
        Called::Refused(text) => {
            out.error_count += 1;
            out.notes
                .push(format!("{label}: `{tool}` returned isError: {text}"));
            None
        }
        Called::Failed => None,
    }
}

/// Run one call through the hang detector, recording metrics and outcome
/// counters.
async fn call(
    session: &mut Session,
    tool: &str,
    args: &Value,
    ctx: &RunContext,
    out: &mut ScenarioOutcome,
    label: &str,
) -> Called {
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
            Called::Ok(result)
        }
        HangOutcome::Ok { result, duration } => {
            ctx.metrics
                .record_tool(tool, duration, CallOutcome::ServerError);
            Called::Refused(error_text(&result))
        }
        HangOutcome::Slow { duration, .. } => {
            out.hang_count += 1;
            ctx.metrics.record_tool(tool, duration, CallOutcome::Hang);
            out.notes
                .push(format!("{label}: `{tool}` exceeded the hang threshold"));
            Called::Failed
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
            Called::Failed
        }
        HangOutcome::Err(err) => {
            out.error_count += 1;
            ctx.metrics
                .record_tool(tool, started.elapsed(), classify_error(&err));
            out.notes.push(format!("{label}: `{tool}` failed: {err}"));
            Called::Failed
        }
        other => {
            out.error_count += 1;
            out.notes.push(format!(
                "{label}: unexpected hang outcome for `{tool}`: {other:?}"
            ));
            Called::Failed
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

/// The text a tool error carries, on one line and capped, for a note.
fn error_text(result: &CallToolResult) -> String {
    const MAX_CHARS: usize = 200;
    let text = result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return "(no text)".to_owned();
    }
    match text.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// How many entries of `first` that hold no marker of this run (`run_prefix`)
/// are missing from `second`. An entry is a line of text, or a string inside
/// JSON a text holds; digit runs compare equal, so a count or a timestamp
/// that moved between the reads is not a missing entry.
fn evicted_entries(first: &CallToolResult, second: &CallToolResult, run_prefix: &str) -> usize {
    let counts = |result: &CallToolResult| {
        let mut out = std::collections::HashMap::<String, usize>::new();
        let mut entries = Vec::new();
        let value = call_tool_result_to_value(result);
        // `_meta` and the envelope are not store data.
        collect_entries(&value["content"], &mut entries);
        collect_entries(&value["structuredContent"], &mut entries);
        for entry in entries.into_iter().filter(|e| !e.contains(run_prefix)) {
            *out.entry(without_digits(&entry)).or_default() += 1;
        }
        out
    };
    let after = counts(second);
    counts(first)
        .into_iter()
        .map(|(entry, n)| n.saturating_sub(after.get(&entry).copied().unwrap_or(0)))
        .sum()
}

fn collect_entries(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(inner @ (Value::Array(_) | Value::Object(_))) => collect_entries(&inner, out),
            _ => out.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            ),
        },
        Value::Array(items) => items.iter().for_each(|item| collect_entries(item, out)),
        // Keys too: a store keyed by record ID can lose a key whose value
        // another record repeats.
        Value::Object(map) => {
            for (key, item) in map {
                out.push(key.clone());
                collect_entries(item, out);
            }
        }
        _ => {}
    }
}

fn without_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.push(c);
        }
    }
    out
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
    into.refused_write_count += from.refused_write_count;
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
    fn evicted_entries_counts_other_data_a_read_dropped() {
        let result = |text: &str| CallToolResult {
            meta: None,
            content: vec![Content::Text {
                text: text.to_owned(),
            }],
            is_error: false,
            structured_content: None,
        };
        let prefix = "mltabcd1234";
        // A capped JSON listing: the recheck markers pushed two older
        // records out while every marker the first read showed stayed.
        let first = result(r#"["alpha", "beta", "mltabcd1234w0000s0000x"]"#);
        let second = result(
            r#"["mltabcd1234w0000s0000x", "mltabcd1234w000rs0000x", "mltabcd1234w000rs0001x"]"#,
        );
        assert_eq!(evicted_entries(&first, &second, prefix), 2);
        // A complete listing keeps them; a count or timestamp that moved
        // is not an eviction.
        let first = result("Found 3 items at 10:01\nalpha\nbeta\nmltabcd1234w0000s0000x");
        let second = result(
            "Found 5 items at 10:02\nalpha\nbeta\nmltabcd1234w0000s0000x\n\
             mltabcd1234w000rs0000x\nmltabcd1234w000rs0001x",
        );
        assert_eq!(evicted_entries(&first, &second, prefix), 0);
        // Records keyed by ID with the same value: a dropped key counts.
        let first =
            result(r#"{"alpha": "todo", "beta": "todo", "mltabcd1234w0000s0000x": "todo"}"#);
        let second = result(
            r#"{"beta": "todo", "mltabcd1234w0000s0000x": "todo", "mltabcd1234w000rs0000x": "todo"}"#,
        );
        assert_eq!(evicted_entries(&first, &second, prefix), 1);
        // Repeated entries count: one of two `item` lines gone is one.
        let first = result("item 1\nitem 2");
        let second = result("item 2");
        assert_eq!(evicted_entries(&first, &second, prefix), 1);
    }

    #[test]
    fn error_text_is_one_capped_line() {
        let result = |text: &str| CallToolResult {
            meta: None,
            content: vec![Content::Text {
                text: text.to_owned(),
            }],
            is_error: true,
            structured_content: None,
        };
        assert_eq!(
            error_text(&result("Could not find exact match for edit:\nEND")),
            "Could not find exact match for edit: END"
        );
        assert_eq!(error_text(&result("  ")), "(no text)");
        let long = error_text(&result(&"é".repeat(300)));
        assert_eq!(long.chars().count(), 201);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn markers_are_fixed_width_and_never_prefixes_of_each_other() {
        let short = marker("abcd1234", "1", 1);
        let long = marker("abcd1234", "1", 10);
        assert_eq!(short.len(), long.len());
        assert_eq!(short.len(), MARKER_LEN);
        assert_eq!(marker("abcd1234", "r", 2).len(), MARKER_LEN);
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
            pipelined: false,
            recheck_losses: false,
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
        // Indexes past 9999 would make markers longer than MARKER_LEN.
        plan.writes_per_worker = 10_000;
        plan.concurrent = 10_000;
        assert_eq!(plan.validation_error(), None);
        assert_eq!(marker("abcd1234", "9999", 9999).len(), MARKER_LEN);
        plan.concurrent = 10_001;
        assert!(plan.validation_error().unwrap().contains("<= 10000"));
        plan.concurrent = 2;
        plan.writes_per_worker = 10_001;
        assert!(plan.validation_error().unwrap().contains("<= 10000"));
    }
}
