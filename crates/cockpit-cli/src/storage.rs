//! `torlek storage`: the daemon's bounded, read-only registered-clone target report.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{CloneStorageView, Command, Reply, TargetSizeView};

use crate::daemon::{BULK_DEADLINE, request, unexpected};

pub fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if !arguments.is_empty() {
        return Err("storage takes no arguments; it only reports registered clones".to_owned());
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
