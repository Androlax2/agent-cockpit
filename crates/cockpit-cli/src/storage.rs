//! Registered-clone and session temporary folder storage reporting, and explicit owner
//! target-pruning decisions.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    CloneStorageView, Command, DatabaseStorageView, EndSweepView, OutputRetentionView,
    PayloadRetentionView, PruneVerdictView, Reply, SessionTempStorageView, StorageAreasView,
    TargetSizeView,
};

use crate::daemon::{BULK_DEADLINE, request, unexpected};

pub fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if arguments
        .first()
        .is_some_and(|argument| argument == "outputs")
    {
        return outputs(&arguments[1..]);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "database")
    {
        return database(&arguments[1..]);
    }
    if !arguments.is_empty() {
        return prune(arguments);
    }
    let reply = match request(Command::StorageReport, BULK_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::Storage {
        clones,
        truncated,
        end_sweeps,
        session_temps,
        session_temps_truncated,
        session_temps_unavailable,
        areas,
    } = &reply
    else {
        return Ok(unexpected(&reply));
    };
    outln!(
        "target/ apparent bytes: regular-file lengths per entry, including hardlinks; not allocated or reclaimable space; concurrent changes may affect results"
    );
    outln!("checkout\tproject\tincarnation\tbytes\tstatus\tholder\tprune\ttarget\treason");
    for clone in clones {
        outln!("{}", row(clone));
    }
    outln!("{}", eligible_total(clones));
    if *truncated {
        outln!("incomplete report: further registered clones omitted by the row limit");
    }
    if !end_sweeps.is_empty() {
        outln!("swept at session end, newest first:");
        outln!("swept at\tcheckout\tproject\tsession\tclone\tpath\treason\ttarget");
        for sweep in end_sweeps {
            outln!("{}", end_sweep_row(sweep));
        }
    }
    outln!("");
    outln!("session temp folders: each session's TMPDIR, apparent bytes as above");
    outln!("session\tbytes\tstatus\tfolder\treason");
    let reported_temps = session_temps.as_deref().unwrap_or_default();
    for temp in reported_temps {
        outln!("{}", temp_row(temp));
    }
    if session_temps.is_none() {
        outln!("incomplete report: the daemon reported no session temp folders");
    }
    if *session_temps_truncated {
        outln!("incomplete report: further session temp folders omitted by the row limit");
    }
    if let Some(reason) = session_temps_unavailable {
        outln!(
            "incomplete report: session temp folders unavailable: {}",
            cell(reason)
        );
    }
    let (clone_bytes, clones_complete) = observed(clones.iter().map(|clone| &clone.measurement));
    let (temp_bytes, temps_complete) =
        observed(reported_temps.iter().map(|temp| &temp.measurement));
    let mut rows = vec![
        AreaRow {
            area: "build folders",
            bytes: Some(clone_bytes),
            retires: Some(eligible(clones).1),
            is_complete: clones_complete && !*truncated,
            freed_by: "torlek storage prune CHECKOUT".to_owned(),
        },
        AreaRow {
            area: "session temp folders",
            bytes: session_temps.as_ref().map(|_| temp_bytes),
            retires: session_temps.as_ref().map(|_| 0),
            is_complete: temps_complete
                && session_temps.is_some()
                && !*session_temps_truncated
                && session_temps_unavailable.is_none(),
            freed_by: "removed when its session ends".to_owned(),
        },
    ];
    let mut explanations = Vec::new();
    match areas {
        Some(areas) => {
            rows.extend(area_rows(areas));
            explanations.extend(area_explanations(areas));
        }
        None => {
            rows.extend(
                ["check outputs", "database", "backups", "logs"].map(|area| AreaRow {
                    area,
                    bytes: None,
                    retires: None,
                    is_complete: false,
                    freed_by: "-".to_owned(),
                }),
            );
            explanations
                .push("the daemon reported no check outputs, database, backups or logs".to_owned());
        }
    }
    outln!("");
    outln!(
        "storage by area: apparent bytes, and what each area's prune would retire now; database pages it retires are reused, the file does not shrink"
    );
    outln!("area\tbytes\tretires\tstatus\tfreed by");
    for row in &rows {
        outln!("{}", area_row(row));
    }
    outln!("{}", area_row(&total(&rows)));
    for explanation in &explanations {
        outln!("incomplete report: {}", cell(explanation));
    }
    let incomplete = rows.iter().any(|row| !row.is_complete);
    Ok(if incomplete {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The bytes, status and reason cells of `measurement`; `missing` explains an absent folder.
fn measurement_cells<'view>(
    measurement: &'view TargetSizeView,
    missing: &'view str,
) -> (String, &'static str, &'view str) {
    match measurement {
        TargetSizeView::Complete { apparent_bytes } => (apparent_bytes.to_string(), "complete", ""),
        TargetSizeView::Missing => ("0".to_owned(), "missing", missing),
        TargetSizeView::Incomplete {
            apparent_bytes,
            reason,
        } => (apparent_bytes.to_string(), "incomplete", reason.as_str()),
        TargetSizeView::Unavailable { reason } => ("-".to_owned(), "unavailable", reason.as_str()),
    }
}

fn temp_row(temp: &SessionTempStorageView) -> String {
    let (bytes, status, reason) = measurement_cells(&temp.measurement, "folder absent");
    format!(
        "{}\t{bytes}\t{status}\t{}\t{}",
        cell(&temp.session),
        cell(&temp.path),
        cell(reason)
    )
}

fn row(clone: &CloneStorageView) -> String {
    let (bytes, status, reason) = measurement_cells(&clone.measurement, "target directory absent");
    let (prune, reason) = match &clone.prune {
        Some(PruneVerdictView::Eligible {
            confirmation_required: true,
        }) => ("eligible", reason),
        Some(PruneVerdictView::Eligible { .. }) => ("eligible-auto", reason),
        Some(PruneVerdictView::Kept { reason }) => ("kept", reason.as_str()),
        None => ("-", "the daemon reported no prune verdict"),
    };
    format!(
        "{}\t{}\t{}\t{bytes}\t{status}\t{}\t{prune}\t{}\t{}",
        clone.checkout,
        cell(&clone.project),
        clone.incarnation,
        cell(clone.holder.as_deref().unwrap_or("-")),
        cell(&clone.target),
        cell(reason)
    )
}

fn end_sweep_row(sweep: &EndSweepView) -> String {
    let swept_at = chrono::DateTime::from_timestamp(sweep.swept_at, 0).map_or_else(
        || sweep.swept_at.to_string(),
        |at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        },
    );
    let (clone, reason) = match &sweep.kept {
        Some(reason) => ("kept", reason.as_str()),
        None => ("removed", "-"),
    };
    format!(
        "{swept_at}\t{}\t{}\t{}\t{clone}\t{}\t{}\t{}",
        sweep.checkout,
        cell(&sweep.project),
        cell(&sweep.session),
        cell(&sweep.path),
        cell(reason),
        cell(sweep.target.as_deref().unwrap_or("-"))
    )
}

/// How many clones prune would accept, and their apparent bytes. Only complete measurements
/// count: a partial size is not what prune would free.
fn eligible(clones: &[CloneStorageView]) -> (usize, u64) {
    clones
        .iter()
        .filter_map(|clone| match (&clone.prune, &clone.measurement) {
            (
                Some(PruneVerdictView::Eligible { .. }),
                TargetSizeView::Complete { apparent_bytes },
            ) => Some(*apparent_bytes),
            _ => None,
        })
        .fold((0, 0), |(count, bytes), clone_bytes| {
            (count + 1, bytes + clone_bytes)
        })
}

fn eligible_total(clones: &[CloneStorageView]) -> String {
    let (count, bytes) = eligible(clones);
    format!(
        "eligible for prune: {count} of {} clones, {bytes} apparent bytes",
        clones.len()
    )
}

/// One area of the summary; `None` bytes when the daemon could not read it at all.
struct AreaRow {
    area: &'static str,
    bytes: Option<u64>,
    retires: Option<u64>,
    is_complete: bool,
    freed_by: String,
}

/// The bytes these measurements observed, partial ones included, and whether all were whole.
fn observed<'view>(measurements: impl Iterator<Item = &'view TargetSizeView>) -> (u64, bool) {
    measurements.fold(
        (0, true),
        |(bytes, is_complete), measurement| match measurement {
            TargetSizeView::Complete { apparent_bytes } => (bytes + apparent_bytes, is_complete),
            TargetSizeView::Missing => (bytes, is_complete),
            TargetSizeView::Incomplete { apparent_bytes, .. } => (bytes + apparent_bytes, false),
            TargetSizeView::Unavailable { .. } => (bytes, false),
        },
    )
}

fn area_rows(areas: &StorageAreasView) -> Vec<AreaRow> {
    let outputs = areas.outputs.as_deref();
    let database = areas.database.as_ref();
    let (log_bytes, logs_complete) = observed(areas.logs.iter().map(|log| &log.measurement));
    vec![
        AreaRow {
            area: "check outputs",
            bytes: outputs
                .map(|projects| projects.iter().map(|project| project.apparent_bytes).sum()),
            retires: outputs
                .map(|projects| projects.iter().map(|project| project.selected_bytes).sum()),
            is_complete: outputs.is_some_and(|projects| {
                projects
                    .iter()
                    .all(|project| project.unmeasured == 0 && project.failures.is_empty())
            }) && !areas.outputs_truncated,
            freed_by: "torlek storage outputs prune".to_owned(),
        },
        AreaRow {
            area: "database",
            bytes: database.map(|database| database.file_bytes + database.wal_bytes),
            retires: database.map(|database| {
                database.sessions.selected_bytes + database.run_outputs.selected_bytes
            }),
            is_complete: database
                .is_some_and(|database| !database.truncated && database.failures.is_empty()),
            freed_by: "torlek storage database prune".to_owned(),
        },
        AreaRow {
            area: "backups",
            bytes: database
                .map(|database| database.backups.iter().map(|backup| backup.bytes).sum()),
            retires: database.map(|_| 0),
            is_complete: database.is_some_and(|database| database.failures.is_empty()),
            freed_by: database.map_or_else(
                || "-".to_owned(),
                |database| format!("rotation keeps the newest {}", database.backups_kept),
            ),
        },
        AreaRow {
            area: "logs",
            bytes: areas.logs_unavailable.is_none().then_some(log_bytes),
            retires: areas.logs_unavailable.is_none().then_some(0),
            is_complete: logs_complete && !areas.logs_truncated && areas.logs_unavailable.is_none(),
            freed_by: "rotation by size and age".to_owned(),
        },
    ]
}

/// Why each area of `areas` is incomplete; empty when all are whole.
fn area_explanations(areas: &StorageAreasView) -> Vec<String> {
    let mut explanations = Vec::new();
    if let Some(reason) = &areas.outputs_unavailable {
        explanations.push(format!("check outputs unavailable: {reason}"));
    }
    for project in areas.outputs.iter().flatten() {
        if project.unmeasured > 0 {
            explanations.push(format!(
                "{} check outputs unmeasured in {}",
                project.unmeasured, project.project
            ));
        }
        for failure in &project.failures {
            explanations.push(format!("check outputs of {}: {failure}", project.project));
        }
    }
    if areas.outputs_truncated {
        explanations.push("further check outputs left out by the pass bound".to_owned());
    }
    if let Some(reason) = &areas.database_unavailable {
        explanations.push(format!("database unavailable: {reason}"));
    }
    if let Some(database) = &areas.database {
        if database.truncated {
            explanations.push(
                "the database pass stopped at its bound; further sessions or run outputs uncounted"
                    .to_owned(),
            );
        }
        explanations.extend(database.failures.iter().cloned());
    }
    if let Some(reason) = &areas.logs_unavailable {
        explanations.push(format!("log folders unavailable: {reason}"));
    }
    if areas.logs_truncated {
        explanations.push("further log folders omitted by the row limit".to_owned());
    }
    for log in &areas.logs {
        if let TargetSizeView::Incomplete { reason, .. } | TargetSizeView::Unavailable { reason } =
            &log.measurement
        {
            explanations.push(format!("log folder {}: {reason}", log.process));
        }
    }
    explanations
}

/// The sum of what each row could read; complete only when every row is.
fn total(rows: &[AreaRow]) -> AreaRow {
    AreaRow {
        area: "total",
        bytes: Some(rows.iter().filter_map(|row| row.bytes).sum()),
        retires: Some(rows.iter().filter_map(|row| row.retires).sum()),
        is_complete: rows.iter().all(|row| row.is_complete),
        freed_by: "-".to_owned(),
    }
}

fn area_row(row: &AreaRow) -> String {
    let number =
        |value: Option<u64>| value.map_or_else(|| "-".to_owned(), |value| value.to_string());
    let status = match (row.bytes, row.is_complete) {
        (None, _) => "unavailable",
        (Some(_), true) => "complete",
        (Some(_), false) => "incomplete",
    };
    format!(
        "{}\t{}\t{}\t{status}\t{}",
        row.area,
        number(row.bytes),
        number(row.retires),
        row.freed_by
    )
}

/// A path or holder is data, even when it contains a terminal escape or a table delimiter.
fn cell(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}

fn prune(arguments: &[OsString]) -> Result<ExitCode, String> {
    let args: Vec<_> = arguments.iter().map(|s| s.to_str()).collect();
    let positive = |value: &str| {
        value
            .parse::<i64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| "storage identity/revision must be positive".to_owned())
    };
    let command = match args.as_slice() {
        [Some("prune"), Some(id), Some("--dry-run")] => Command::StoragePrunePreview { checkout: positive(id)?, prepare: false },
        [Some("prune"), Some(id)] => Command::StoragePrunePreview { checkout: positive(id)?, prepare: true },
        [Some("prune"), Some("--plan"), Some(id), Some("--revision"), Some(revision), Some("--epoch"), Some(epoch), Some("--confirm")] =>
            Command::StoragePruneApply { plan: positive(id)?, revision: positive(revision)?, epoch: (*epoch).to_owned() },
        [Some("prune"), Some("status"), Some(id)] => Command::StoragePruneStatus { operation: positive(id)? },
        [Some("prune"), Some("abandon"), Some(id), Some("--revision"), Some(revision), Some("--epoch"), Some(epoch)] =>
            Command::StoragePruneAbandon { operation: positive(id)?, revision: positive(revision)?, epoch: (*epoch).to_owned() },
        _ => return Err("storage prune CHECKOUT [--dry-run] | --plan ID --revision N --epoch E --confirm | status ID | abandon ID --revision N --epoch E".to_owned()),
    };
    let applying = matches!(command, Command::StoragePruneApply { .. });
    let reply = match request(command, BULK_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    match &reply {
        Reply::StoragePrunePreview { preview } => {
            outln!(
                "{}: {}",
                if preview.dry_run {
                    "dry-run"
                } else {
                    "prepared; nothing deleted"
                },
                cell(&preview.target)
            );
            match &preview.measurement {
                TargetSizeView::Complete { apparent_bytes } => {
                    outln!("{apparent_bytes} apparent bytes; physical space freed is not promised")
                }
                TargetSizeView::Missing => outln!("target is absent"),
                TargetSizeView::Incomplete {
                    apparent_bytes,
                    reason,
                } => outln!(
                    "at least {apparent_bytes} apparent bytes; incomplete: {}",
                    cell(reason)
                ),
                TargetSizeView::Unavailable { reason } => {
                    outln!("size unavailable: {}", cell(reason))
                }
            }
            if let Some(reason) = &preview.kept_reason {
                outln!("kept: {}", cell(reason));
            }
            if let Some(eligibility) = &preview.eligibility {
                outln!("eligibility: {}", cell(eligibility));
            }
            if let Some(plan) = &preview.plan {
                outln!(
                    "Confirm only if this literal target is disposable and no unmanaged writer can mutate it:"
                );
                outln!(
                    "torlek storage prune --plan {} --revision {} --epoch {} --confirm",
                    plan.operation,
                    plan.revision,
                    cell(&plan.epoch)
                );
            }
            if matches!(
                preview.measurement,
                TargetSizeView::Incomplete { .. } | TargetSizeView::Unavailable { .. }
            ) {
                return Ok(ExitCode::FAILURE);
            }
        }
        Reply::StoragePruneStatus { status } => {
            outln!("plan epoch: {}", cell(&status.epoch));
            outln!("current database epoch: {}", cell(&status.database_epoch));
            outln!("target: {}", cell(&status.target));
            outln!(
                "quarantine: {} (recorded location; existence/contents may be uncertain)",
                status
                    .quarantine
                    .as_deref()
                    .map(cell)
                    .unwrap_or_else(|| "not reserved".into())
            );
            outln!(
                "operation {}: {}; {} removed apparent bytes, {} absent bytes with unconfirmed attribution",
                status.operation,
                cell(&status.phase),
                status.removed_apparent_bytes,
                status.absent_unconfirmed_bytes
            );
            if let Some(reason) = &status.reason {
                outln!("{}", cell(reason));
            }
            if !matches!(status.phase.as_str(), "complete" | "abandoned") {
                outln!("To abandon once no filesystem step is active, retaining all files:");
                outln!(
                    "torlek storage prune abandon {} --revision {} --epoch {}",
                    status.operation,
                    status.revision,
                    cell(&status.database_epoch)
                );
            }
            if applying && status.phase != "complete" {
                return Ok(ExitCode::FAILURE);
            }
        }
        _ => return Ok(unexpected(&reply)),
    }
    Ok(ExitCode::SUCCESS)
}

/// `storage outputs [prune]`: what check-output retention selects per project, and with
/// `prune` retires it. Exit 1 when a selected output was kept, an output was unmeasured, or the
/// pass left outputs out.
fn outputs(arguments: &[OsString]) -> Result<ExitCode, String> {
    let apply = match arguments {
        [] => false,
        [prune] if prune == "prune" => true,
        _ => return Err("storage outputs [prune]".to_owned()),
    };
    let reply = match request(Command::StorageOutputs { apply }, BULK_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::StorageOutputs {
        projects,
        applied,
        truncated,
    } = &reply
    else {
        return Ok(unexpected(&reply));
    };
    outln!(
        "check outputs: apparent bytes of each ended check run's stdout and stderr; receipts and their output tails stay"
    );
    outln!(
        "project\toutputs\tbytes\tunmeasured\tprotected\tselected\tselected_bytes\tretired\tretired_bytes"
    );
    for project in projects {
        outln!("{}", output_row(project));
    }
    for project in projects {
        for failure in &project.failures {
            outln!("{}: {}", cell(&project.project), cell(failure));
        }
    }
    if *applied {
        outln!(
            "retired {} outputs, {} apparent bytes",
            projects.iter().map(|project| project.retired).sum::<u64>(),
            projects
                .iter()
                .map(|project| project.retired_bytes)
                .sum::<u64>()
        );
    } else {
        outln!(
            "dry run: nothing retired; `torlek storage outputs prune` retires the selected outputs"
        );
    }
    if *truncated {
        outln!("incomplete pass: further outputs were left out by its bound");
    }
    let incomplete = *truncated
        || projects
            .iter()
            .any(|project| project.unmeasured > 0 || !project.failures.is_empty());
    Ok(if incomplete {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// `storage database [prune]`: what database retention selects, and with `prune` retires it.
/// Exit 1 when the pass stopped early or left sessions out.
fn database(arguments: &[OsString]) -> Result<ExitCode, String> {
    let apply = match arguments {
        [] => false,
        [prune] if prune == "prune" => true,
        _ => return Err("storage database [prune]".to_owned()),
    };
    let reply = match request(Command::StorageDatabase { apply }, BULK_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::StorageDatabase { database, applied } = &reply else {
        return Ok(unexpected(&reply));
    };
    print_database(database, *applied);
    Ok(if database.truncated || !database.failures.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn print_database(database: &DatabaseStorageView, applied: bool) {
    outln!(
        "database: {} bytes, WAL {} bytes (apparent sizes; freed pages are reused, the file does not shrink)",
        database.file_bytes,
        database.wal_bytes
    );
    outln!(
        "retention: payloads of what ended over {} days ago; session, launch, transition, decision and audit rows stay",
        database.retention_days
    );
    outln!("rule	selected	selected_bytes	retired	retired_bytes");
    outln!(
        "{}",
        rule_row("ended sessions' launch context", &database.sessions)
    );
    outln!(
        "{}",
        rule_row("finished command runs' output", &database.run_outputs)
    );
    outln!(
        "change feed: {} captured rows kept now; the hourly trim keeps the newest {}",
        database.feed_changes,
        database.feed_changes_kept
    );
    if database.backups_kept == 0 {
        outln!("backups: none taken (database_backups_kept = 0)");
    } else {
        outln!(
            "backups: {} of the newest {} kept",
            database.backups.len(),
            database.backups_kept
        );
    }
    for backup in &database.backups {
        outln!(
            "{}	{}	{}",
            cell(&backup.path),
            backup.taken_at,
            backup.bytes
        );
    }
    for failure in &database.failures {
        outln!("{}", cell(failure));
    }
    if applied {
        outln!(
            "retired {} sessions' launch context and {} run outputs, {} apparent bytes",
            database.sessions.retired,
            database.run_outputs.retired,
            database.sessions.retired_bytes + database.run_outputs.retired_bytes
        );
    } else {
        outln!(
            "dry run: nothing retired; `torlek storage database prune` retires the selected payloads"
        );
    }
    if database.wal_checkpoint_busy {
        outln!(
            "WAL checkpoint busy: a reader held the WAL, so it did not shrink; the hourly checkpoint finishes it"
        );
    }
    if database.truncated {
        outln!("incomplete pass: its bound left sessions or run outputs for the next pass");
    }
}

fn rule_row(rule: &str, view: &PayloadRetentionView) -> String {
    format!(
        "{rule}	{}	{}	{}	{}",
        view.selected, view.selected_bytes, view.retired, view.retired_bytes
    )
}

fn output_row(project: &OutputRetentionView) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        cell(&project.project),
        project.outputs,
        project.apparent_bytes,
        project.unmeasured,
        project.protected,
        project.selected,
        project.selected_bytes,
        project.retired,
        project.retired_bytes
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unavailable_size_is_not_printed_as_zero_and_names_cannot_add_rows() {
        let clone = CloneStorageView {
            checkout: 2,
            project: "app".to_owned(),
            incarnation: 1,
            target: "/clone/target".to_owned(),
            holder: Some("live\t\x1b\n".to_owned()),
            measurement: TargetSizeView::Unavailable {
                reason: "permission denied".to_owned(),
            },
            prune: Some(PruneVerdictView::Kept {
                reason: "permission denied".to_owned(),
            }),
        };
        assert_eq!(
            row(&clone),
            "2\tapp\t1\t-\tunavailable\tlive\\t\\u{1b}\\n\tkept\t/clone/target\tpermission denied"
        );
    }
}
