//! `torlek storage`: the daemon's bounded, read-only registered-clone target report.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{CloneStorageView, Command, Reply, TargetSizeView};

use crate::daemon::{BULK_DEADLINE, request, unexpected};

pub fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if !arguments.is_empty() {
        return prune(arguments);
    }
    let reply = match request(Command::StorageReport, BULK_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::Storage { clones, truncated } = &reply else {
        return Ok(unexpected(&reply));
    };
    outln!(
        "target/ apparent bytes: regular-file lengths per entry, including hardlinks; not allocated or reclaimable space; concurrent changes may affect results"
    );
    outln!("checkout\tproject\tincarnation\tbytes\tstatus\tholder\ttarget\treason");
    for clone in clones {
        outln!("{}", row(clone));
    }
    if *truncated {
        outln!("incomplete report: further registered clones omitted by the row limit");
    }
    let incomplete = *truncated
        || clones.iter().any(|clone| {
            matches!(
                clone.measurement,
                TargetSizeView::Incomplete { .. } | TargetSizeView::Unavailable { .. }
            )
        });
    Ok(if incomplete {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn row(clone: &CloneStorageView) -> String {
    let (bytes, status, reason) = match &clone.measurement {
        TargetSizeView::Complete { apparent_bytes } => (apparent_bytes.to_string(), "complete", ""),
        TargetSizeView::Missing => ("0".to_owned(), "missing", "target directory absent"),
        TargetSizeView::Incomplete {
            apparent_bytes,
            reason,
        } => (apparent_bytes.to_string(), "incomplete", reason.as_str()),
        TargetSizeView::Unavailable { reason } => ("-".to_owned(), "unavailable", reason.as_str()),
    };
    format!(
        "{}\t{}\t{}\t{bytes}\t{status}\t{}\t{}\t{}",
        clone.checkout,
        cell(&clone.project),
        clone.incarnation,
        cell(clone.holder.as_deref().unwrap_or("-")),
        cell(&clone.target),
        cell(reason)
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
        _ => return Err("storage prune CHECKOUT [--dry-run] | --plan ID --revision N --epoch E --confirm | status ID".to_owned()),
    };
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
        }
        Reply::StoragePruneStatus { status } => {
            outln!("epoch: {}", cell(&status.epoch));
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
        }
        _ => return Ok(unexpected(&reply)),
    }
    Ok(ExitCode::SUCCESS)
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
        };
        assert_eq!(
            row(&clone),
            "2\tapp\t1\t-\tunavailable\tlive\\t\\u{1b}\\n\t/clone/target\tpermission denied"
        );
    }
}
