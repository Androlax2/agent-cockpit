//! Check-output retention (#406): retires the `stdout`/`stderr` files of check runs that are
//! older than the owner's retention or past a project's budget. Each receipt and its output
//! tail stay; each check's newest run, a live session's latest output and an output someone
//! reads keep their files. Decided in docs/decisions/2026-11-04-406-run-output-retention.md.
use crate::routes::WorkClass;
use crate::storage::Context;
use cockpit_processes::{ExecutionId, OutputFacts, RetirableOutput};
use cockpit_protocol::{ErrorCode, Outcome, OutputRetentionView, ProtocolError, Reply};
use cockpit_turso::sql::Connection;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PAGE: u32 = 256;
/// Outputs one pass considers; the rest wait for the next pass and the reply says so.
const PASS_LIMIT: usize = 16_384;
const HOUR_MS: i64 = 60 * 60 * 1000;

/// The owner's `[storage] output_*` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputPolicy {
    pub(crate) retention_ms: i64,
    pub(crate) budget_bytes: u64,
    pub(crate) grace_ms: i64,
    pub(crate) automatic: bool,
    pub(crate) projects: Vec<String>,
    pub(crate) interval: Duration,
}

impl OutputPolicy {
    pub(crate) fn from_settings(settings: &cockpit_config::StorageSettings) -> Self {
        Self {
            retention_ms: i64::from(settings.output_retention_days.value) * 24 * HOUR_MS,
            budget_bytes: settings.output_budget_mib.value * 1024 * 1024,
            grace_ms: i64::from(settings.output_budget_grace_hours.value) * HOUR_MS,
            automatic: settings.output_prune_automatic.value,
            projects: settings.output_prune_projects.value.clone(),
            interval: Duration::from_secs(
                u64::from(settings.output_prune_interval_minutes.value) * 60,
            ),
        }
    }
}

/// One output as a pass judged it, in the pass's read snapshot.
#[derive(Debug, Clone)]
struct Judged {
    output: RetirableOutput,
    /// `None` when its files could not be measured: never selected, never counted as zero.
    bytes: Option<u64>,
    /// Why it keeps its files whatever its age, if it does.
    protection: Option<String>,
}

fn internal(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, error.to_string())
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_millis()).expect("milliseconds since 1970 fit in i64")
}

/// Why `facts` keeps its files, read on the caller's connection: under the writer before a
/// retirement, so nothing changes between this check and the commit.
async fn protection(
    connection: &Connection,
    epoch: &str,
    newest: &HashSet<i64>,
    facts: &OutputFacts,
) -> Result<Option<String>, ProtocolError> {
    let id = facts.execution;
    if newest.contains(&id.0) {
        return Ok(Some("its check's newest run".to_owned()));
    }
    let Some(project) = facts.project.as_deref() else {
        return Ok(Some("no project".to_owned()));
    };
    let run = format!("execution-{epoch}-{}", id.0);
    let sealed = cockpit_verification::pending_result(connection, project, &run)
        .await
        .map_err(internal)?
        .is_some_and(|identity| {
            identity.sealed && identity.execution == id.0 && identity.origin_epoch == epoch
        });
    if !sealed {
        return Ok(Some("not a check run with a recorded receipt".to_owned()));
    }
    if cockpit_sessions::protects_output(connection, facts)
        .await
        .map_err(internal)?
    {
        return Ok(Some("a live session's latest output".to_owned()));
    }
    if cockpit_processes::output_users(epoch, id) != 0 {
        return Ok(Some("being read".to_owned()));
    }
    Ok(None)
}

async fn newest_runs(connection: &Connection, epoch: &str) -> Result<HashSet<i64>, ProtocolError> {
    Ok(cockpit_verification::newest_runs(connection, epoch)
        .await
        .map_err(internal)?
        .into_iter()
        .collect())
}

/// Which of one project's outputs, oldest first, the policy retires: each unprotected,
/// measured output older than the retention; then, while the project's measured total is over
/// its budget, the oldest past the grace period.
fn select(outputs: &[Judged], policy: &OutputPolicy, now_ms: i64) -> Vec<bool> {
    let is_candidate = |judged: &Judged| judged.protection.is_none() && judged.bytes.is_some();
    let mut selected: Vec<bool> = outputs
        .iter()
        .map(|judged| {
            is_candidate(judged) && now_ms - judged.output.ended_at >= policy.retention_ms
        })
        .collect();
    let mut kept_bytes: u64 = outputs
        .iter()
        .zip(&selected)
        .filter(|(_, is_selected)| !**is_selected)
        .filter_map(|(judged, _)| judged.bytes)
        .sum();
    for (judged, is_selected) in outputs.iter().zip(selected.iter_mut()) {
        if kept_bytes <= policy.budget_bytes {
            break;
        }
        if *is_selected
            || !is_candidate(judged)
            || now_ms - judged.output.ended_at < policy.grace_ms
        {
            continue;
        }
        *is_selected = true;
        kept_bytes -= judged.bytes.unwrap_or(0);
    }
    selected
}

/// Every retirable output of this database epoch, judged in one read snapshot, up to
/// [`PASS_LIMIT`]; `only` limits it to those projects. Returns whether outputs were left out.
async fn judge(
    context: &Context,
    only: Option<&[String]>,
) -> Result<(Vec<Judged>, bool), ProtocolError> {
    let epoch = context.database.epoch();
    let read = context.database.read().await.map_err(internal)?;
    let newest = newest_runs(&read, epoch).await?;
    let mut judged = Vec::new();
    let mut after = ExecutionId(0);
    loop {
        let page = cockpit_processes::retirable_outputs(&read, epoch, after, PAGE)
            .await
            .map_err(internal)?;
        let Some(last) = page.last() else {
            return Ok((judged, false));
        };
        after = last.facts.execution;
        for output in page {
            let project = output.facts.project.as_deref().unwrap_or_default();
            if only.is_some_and(|projects| !projects.iter().any(|name| name == project)) {
                continue;
            }
            if judged.len() == PASS_LIMIT {
                return Ok((judged, true));
            }
            let protection = protection(&read, epoch, &newest, &output.facts).await?;
            judged.push(Judged {
                output,
                bytes: None,
                protection,
            });
        }
    }
}

/// Measures each judged output's files off the async workers.
async fn measure(context: &Context, mut judged: Vec<Judged>) -> Result<Vec<Judged>, ProtocolError> {
    let supervisor = context.supervisor.clone();
    let database = Arc::clone(&context.database);
    tokio::task::spawn_blocking(move || {
        for output in &mut judged {
            output.bytes = supervisor.output_bytes(&database, output.output.facts.execution);
        }
        judged
    })
    .await
    .map_err(internal)
}

/// Retires one selected output: rechecks it under the writer, commits the retirement, removes
/// its files, then records them gone. Returns the bytes removed, or why it kept them.
async fn retire(context: &Context, judged: &Judged) -> Result<u64, String> {
    let id = judged.output.facts.execution;
    let epoch = context.database.epoch();
    let retiring = {
        let write = context.database.write().await.map_err(|e| e.to_string())?;
        let facts = cockpit_processes::output_facts(&write, id)
            .await
            .map_err(|e| e.to_string())?;
        if facts != judged.output.facts {
            return Err("it changed since this pass judged it".to_owned());
        }
        let newest = newest_runs(&write, epoch).await.map_err(|e| e.message)?;
        if let Some(reason) = protection(&write, epoch, &newest, &facts)
            .await
            .map_err(|e| e.message)?
        {
            return Err(format!("kept: {reason}"));
        }
        let retiring =
            cockpit_processes::begin_output_retirement(&write, id, epoch, facts.revision)
                .await
                .map_err(|e| e.to_string())?;
        write.commit().await.map_err(|e| e.to_string())?;
        retiring
    };
    let supervisor = context.supervisor.clone();
    let removing = retiring.clone();
    let removed = tokio::task::spawn_blocking(move || supervisor.remove_output_files(&removing))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}; it stays retiring until a restart finishes it"))?;
    let write = context.database.write().await.map_err(|e| e.to_string())?;
    cockpit_processes::finish_output_retirement(&write, retiring)
        .await
        .map_err(|e| e.to_string())?;
    write.commit().await.map_err(|e| e.to_string())?;
    Ok(removed)
}

/// Reports, and with `apply` retires, what the owner's policy selects; `only` limits it to
/// those projects.
pub(crate) async fn pass(
    context: &Context,
    apply: bool,
    only: Option<&[String]>,
) -> Result<Reply, ProtocolError> {
    let (judged, truncated) = judge(context, only).await?;
    let judged = measure(context, judged).await?;
    let now = now_ms();
    let mut by_project: BTreeMap<String, Vec<Judged>> = BTreeMap::new();
    for output in judged {
        let project = output.output.facts.project.clone().unwrap_or_default();
        by_project.entry(project).or_default().push(output);
    }
    let mut projects = Vec::with_capacity(by_project.len());
    for (project, outputs) in by_project {
        let selected = select(&outputs, &context.outputs, now);
        let mut view = OutputRetentionView {
            project,
            outputs: outputs.len() as u64,
            apparent_bytes: outputs.iter().filter_map(|judged| judged.bytes).sum(),
            unmeasured: outputs
                .iter()
                .filter(|judged| judged.bytes.is_none())
                .count() as u64,
            protected: outputs
                .iter()
                .filter(|judged| judged.protection.is_some())
                .count() as u64,
            selected: 0,
            selected_bytes: 0,
            retired: 0,
            retired_bytes: 0,
            failures: Vec::new(),
        };
        for (judged, _) in outputs.iter().zip(&selected).filter(|(_, is)| **is) {
            view.selected += 1;
            view.selected_bytes += judged.bytes.unwrap_or(0);
            if !apply {
                continue;
            }
            match retire(context, judged).await {
                Ok(removed) => {
                    view.retired += 1;
                    view.retired_bytes += removed;
                }
                Err(reason) => view.failures.push(format!(
                    "execution {}: {reason}",
                    judged.output.facts.execution.0
                )),
            }
        }
        projects.push(view);
    }
    Ok(Reply::StorageOutputs {
        projects,
        applied: apply,
        truncated,
    })
}

/// The owner's opt-in automatic pass over `output_prune_projects`, on the Bulk lane like an
/// owner request. Never runs unless `output_prune_automatic` is set.
pub(crate) async fn maintain_forever(context: Context, shared: Arc<crate::server::Shared>) {
    if !context.outputs.automatic {
        std::future::pending::<()>().await;
        return;
    }
    let mut ticks = tokio::time::interval(context.outputs.interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        let worker = context.clone();
        let work = Box::pin(async move {
            match pass(&worker, true, Some(&worker.outputs.projects)).await {
                Ok(reply) => Outcome::Ok(reply),
                Err(error) => Outcome::Error(error),
            }
        });
        let outcome =
            match shared
                .lanes
                .admit(WorkClass::Bulk, "storage-outputs-automatic".into(), work)
            {
                Ok(work) => work.await,
                Err(outcome) => outcome,
            };
        match outcome {
            Outcome::Ok(Reply::StorageOutputs { projects, .. }) => {
                for project in projects.iter().filter(|project| project.retired > 0) {
                    tracing::info!(
                        project = project.project,
                        retired = project.retired,
                        bytes = project.retired_bytes,
                        "retired check output"
                    );
                }
                for failure in projects.iter().flat_map(|project| &project.failures) {
                    tracing::warn!(%failure, "automatic output retention kept an output");
                }
            }
            Outcome::Ok(_) => {}
            Outcome::Error(error) => {
                tracing::warn!(error = %error.message, "automatic output retention deferred");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_processes::OutputMode;

    const DAY_MS: i64 = 24 * HOUR_MS;
    const NOW: i64 = 100 * DAY_MS;

    fn policy() -> OutputPolicy {
        OutputPolicy {
            retention_ms: 14 * DAY_MS,
            budget_bytes: 100,
            grace_ms: HOUR_MS,
            automatic: false,
            projects: Vec::new(),
            interval: Duration::from_secs(3600),
        }
    }

    fn output(id: i64, age_ms: i64, bytes: Option<u64>, protection: Option<&str>) -> Judged {
        Judged {
            output: RetirableOutput {
                facts: OutputFacts {
                    execution: ExecutionId(id),
                    origin_epoch: Some("epoch".to_owned()),
                    mode: OutputMode::Files,
                    project: Some("app".to_owned()),
                    session: None,
                    pending_result: false,
                    active: false,
                    revision: 3,
                    retirement: None,
                },
                ended_at: NOW - age_ms,
            },
            bytes,
            protection: protection.map(str::to_owned),
        }
    }

    #[test]
    fn selection_follows_age_and_budget() {
        let cases: [(&str, Vec<Judged>, Vec<bool>); 7] = [
            (
                "an output past retention goes even under budget",
                vec![
                    output(1, 15 * DAY_MS, Some(1), None),
                    output(2, DAY_MS, Some(1), None),
                ],
                vec![true, false],
            ),
            (
                "over budget, the oldest go until the rest fit",
                vec![
                    output(1, 3 * HOUR_MS, Some(60), None),
                    output(2, 2 * HOUR_MS, Some(60), None),
                    output(3, 2 * HOUR_MS, Some(60), None),
                ],
                vec![true, true, false],
            ),
            (
                "an output inside the grace period stays over budget",
                vec![output(1, 0, Some(200), None)],
                vec![false],
            ),
            (
                "a protected output stays past retention and still counts toward the budget",
                vec![
                    output(1, 30 * DAY_MS, Some(90), Some("its check's newest run")),
                    output(2, 2 * HOUR_MS, Some(20), None),
                ],
                vec![false, true],
            ),
            (
                "an unmeasured output is never selected",
                vec![output(1, 30 * DAY_MS, None, None)],
                vec![false],
            ),
            (
                "outputs past retention leave room under budget for the rest",
                vec![
                    output(1, 20 * DAY_MS, Some(150), None),
                    output(2, 2 * HOUR_MS, Some(50), None),
                ],
                vec![true, false],
            ),
            ("nothing to judge", Vec::new(), Vec::new()),
        ];
        for (case, outputs, expected) in cases {
            assert_eq!(select(&outputs, &policy(), NOW), expected, "{case}");
        }
    }
}
