use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};

const FILE_BYTES: usize = 4 * 1024 * 1024;
const RECORDS: usize = 32;
const REPORT_BYTES: usize = 256 * 1024;

pub(super) struct TerminationEvidence {
    logs: PathBuf,
    report: PathBuf,
    captured: bool,
}

impl TerminationEvidence {
    pub(super) fn new(logs: PathBuf, report: PathBuf) -> Self {
        Self {
            logs,
            report,
            captured: false,
        }
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        let (evidence, incomplete) = collect(&self.logs);
        OpenOptions::new()
            .append(true)
            .open(&self.report)?
            .write_all(evidence.as_bytes())?;
        self.captured = true;
        if incomplete {
            return Err(io::Error::other(
                "termination evidence incomplete; see report",
            ));
        }
        Ok(())
    }
}

impl Drop for TerminationEvidence {
    fn drop(&mut self) {
        if !self.captured
            && let Err(error) = self.finish()
        {
            // A failed artifact write must never replace the original candidate panic.
            let _ = writeln!(
                io::stderr().lock(),
                "latency harness: termination evidence: {error}"
            );
        }
    }
}

fn collect(logs: &Path) -> (String, bool) {
    let mut text = String::from(
        "\n### Termination evidence\n\nBounded log snapshot; not a lifecycle verdict.\n\n",
    );
    let mut count = 0;
    let mut malformed = 0;
    let mut omitted = 0;
    let mut incomplete = false;
    for name in [
        "cockpit.log",
        "cockpit.log.1",
        "cockpit.log.2",
        "cockpit.log.3",
    ] {
        let file = match File::open(logs.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound && name != "cockpit.log" => {
                continue;
            }
            Err(error) => {
                let _ = writeln!(text, "- {name}: missing/unreadable ({:?}).", error.kind());
                incomplete = true;
                continue;
            }
        };
        let mut bytes = Vec::new();
        if let Err(error) = file.take(FILE_BYTES as u64 + 1).read_to_end(&mut bytes) {
            let _ = writeln!(text, "- {name}: read failed ({:?}).", error.kind());
            incomplete = true;
            continue;
        }
        if bytes.len() > FILE_BYTES {
            bytes.truncate(FILE_BYTES);
            let _ = writeln!(text, "- {name}: input truncated at {FILE_BYTES} bytes.");
            incomplete = true;
        }
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
                malformed += 1;
                continue;
            };
            if !line.ends_with(b"\n") {
                malformed += 1;
                continue;
            }
            let fields = &record["fields"];
            if !matches!(
                fields["message"].as_str(),
                Some(
                    "a session's agent termination is unconfirmed"
                        | "a failed launch's agent may still run; resume stops it first"
                )
            ) {
                continue;
            }
            let (Some(timestamp), Some(session), Some(execution), Some(diagnostics)) = (
                record["timestamp"].as_str(),
                fields["session"].as_str(),
                fields["execution"].as_i64(),
                fields["diagnostics"].as_str(),
            ) else {
                malformed += 1;
                continue;
            };
            let retained = serde_json::json!({
                "timestamp": timestamp, "session": session,
                "execution": execution, "diagnostics": diagnostics,
            })
            .to_string();
            // Reserve space for the bounded file-status lines and final capture status.
            if count == RECORDS || text.len() + retained.len() + 5 > REPORT_BYTES - 4096 {
                omitted += 1;
                continue;
            }
            let _ = writeln!(text, "    {retained}");
            count += 1;
        }
    }
    if malformed > 0 {
        let _ = writeln!(
            text,
            "\n- Malformed, partial or uncorrelated records: {malformed} (raw content omitted)."
        );
        incomplete = true;
    }
    if omitted > 0 {
        let _ = writeln!(
            text,
            "\n- Output truncated: {omitted} matching records omitted (32 records / 256 KiB limits)."
        );
        incomplete = true;
    }
    if incomplete {
        text.push_str(
            "\nEvidence incomplete; missing records cannot establish successful termination.\n",
        );
    } else if count == 0 {
        text.push_str("\nNo matching termination records in the available log files.\n");
    }
    (text, incomplete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    const MESSAGE: &str = "a session's agent termination is unconfirmed";
    const FAILED_LAUNCH: &str = "a failed launch's agent may still run; resume stops it first";
    const VERDICT: &str = "Harness incomplete until the final assertion verdict.\nFailed: 1\n";

    fn record(message: &str, diagnostics: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "timestamp": "2026-11-02T15:00:00Z",
                "fields": {"message": message, "session": "session-537", "execution": 640,
                    "diagnostics": diagnostics, "environment": "must-not-export"},
                "provider_output": "must-not-export"
            })
        )
    }

    #[test]
    fn release_termination_evidence_survives_logger_cleanup_and_report_rewrites() {
        for unwind in [false, true] {
            let artifact = tempfile::tempdir().unwrap();
            let report = artifact.path().join("report.md");
            let mut deleted_home = PathBuf::new();
            let result = catch_unwind(AssertUnwindSafe(|| {
                let home = crate::Home::new();
                deleted_home = home.path();
                let logs = home.state_dir().join("logs/daemon");
                let mut evidence = TerminationEvidence::new(logs.clone(), report.clone());
                let logger = cockpit_diagnostics::start(cockpit_diagnostics::Options::new(
                    logs,
                    cockpit_diagnostics::Directives::from_env_value(Some("warn")).unwrap(),
                ))
                .unwrap();
                tracing::dispatcher::with_default(logger.dispatch(), || {
                    tracing::warn!(
                        session = "session-537",
                        execution = 640,
                        diagnostics = "Unidentified; term: None; kill: None",
                        "a session's agent termination is unconfirmed"
                    );
                });
                assert!(logger.shutdown(Duration::from_secs(1)));
                fs::write(&report, "earlier progress").unwrap();
                fs::write(&report, VERDICT).unwrap();
                if unwind {
                    panic!("original candidate verdict");
                }
                evidence.finish().unwrap();
            }));
            assert!(!deleted_home.exists());
            if unwind {
                assert_eq!(
                    result.unwrap_err().downcast_ref::<&str>(),
                    Some(&"original candidate verdict")
                );
            } else {
                result.unwrap();
            }
            let retained = fs::read_to_string(report).unwrap();
            assert!(retained.starts_with(VERDICT));
            assert!(retained.contains("session-537") && retained.contains("640"));
            assert!(retained.contains("Unidentified; term: None; kill: None"));
            assert_eq!(retained.matches("### Termination evidence").count(), 1);
            assert!(!retained.contains("all required assertions passed"));
        }
    }

    #[test]
    fn release_termination_evidence_exports_only_correlated_allowlisted_fields() {
        let logs = tempfile::tempdir().unwrap();
        let report = logs.path().join("report.md");
        fs::write(&report, VERDICT).unwrap();
        fs::write(
            logs.path().join("cockpit.log"),
            record("unrelated", "must-not-export"),
        )
        .unwrap();
        fs::write(
            logs.path().join("cockpit.log.1"),
            record(MESSAGE, "Unidentified"),
        )
        .unwrap();
        fs::write(
            logs.path().join("cockpit.log.3"),
            record(FAILED_LAUNCH, "Survivor"),
        )
        .unwrap();
        fs::write(
            logs.path().join("cockpit.log.4"),
            record(MESSAGE, "must-not-export"),
        )
        .unwrap();
        TerminationEvidence::new(logs.path().to_owned(), report.clone())
            .finish()
            .unwrap();
        let retained = fs::read_to_string(report).unwrap();
        assert!(retained.contains("Unidentified") && retained.contains("Survivor"));
        assert!(!retained.contains("must-not-export"));
        assert!(!retained.contains("provider_output") && !retained.contains("environment"));
    }

    #[test]
    fn release_termination_evidence_labels_missing_malformed_and_unreadable_logs() {
        for problem in [
            "missing",
            "malformed",
            "partial",
            "uncorrelated",
            "unreadable",
        ] {
            let logs = tempfile::tempdir().unwrap();
            let report = logs.path().join("report.md");
            let current = logs.path().join("cockpit.log");
            fs::write(&report, VERDICT).unwrap();
            match problem {
                "missing" => {}
                "malformed" => fs::write(&current, "not-json\n").unwrap(),
                "partial" => fs::write(&current, "{\"fields\":").unwrap(),
                "uncorrelated" => fs::write(
                    &current,
                    format!("{}\n", serde_json::json!({"fields":{"message":MESSAGE}})),
                )
                .unwrap(),
                "unreadable" => fs::create_dir(&current).unwrap(),
                _ => unreachable!(),
            }
            assert!(
                TerminationEvidence::new(logs.path().to_owned(), report.clone())
                    .finish()
                    .is_err(),
                "{problem}"
            );
            let retained = fs::read_to_string(report).unwrap();
            assert!(retained.starts_with(VERDICT));
            assert!(
                retained.contains("Evidence incomplete"),
                "{problem}: {retained}"
            );
            assert!(!retained.contains("No matching termination records"));
        }
    }

    #[test]
    fn release_termination_evidence_bounds_file_reads_records_and_rendered_output() {
        for content in [
            record(MESSAGE, "Unidentified").repeat(33),
            record(MESSAGE, &"x".repeat(256 * 1024)),
            format!(
                "{}{}",
                record(MESSAGE, "Unidentified"),
                "x".repeat(4 * 1024 * 1024)
            ),
        ] {
            let logs = tempfile::tempdir().unwrap();
            let report = logs.path().join("report.md");
            fs::write(&report, VERDICT).unwrap();
            fs::write(logs.path().join("cockpit.log"), content).unwrap();
            assert!(
                TerminationEvidence::new(logs.path().to_owned(), report.clone())
                    .finish()
                    .is_err()
            );
            let retained = fs::read_to_string(report).unwrap();
            assert!(retained.len() <= VERDICT.len() + 256 * 1024);
            assert!(retained.matches("\"execution\":640").count() <= 32);
            assert!(retained.contains("truncated"));
            assert!(retained.contains("Evidence incomplete"));
        }
    }

    #[test]
    fn release_termination_evidence_reports_a_clean_empty_scan_without_acceptance() {
        let logs = tempfile::tempdir().unwrap();
        let report = logs.path().join("report.md");
        fs::write(&report, VERDICT).unwrap();
        fs::write(
            logs.path().join("cockpit.log"),
            record("unrelated", "secret"),
        )
        .unwrap();
        TerminationEvidence::new(logs.path().to_owned(), report.clone())
            .finish()
            .unwrap();
        let retained = fs::read_to_string(report).unwrap();
        assert!(retained.contains("No matching termination records"));
        assert!(retained.contains("not a lifecycle verdict"));
    }

    #[test]
    fn release_termination_evidence_write_failure_preserves_the_original_panic() {
        let logs = tempfile::tempdir().unwrap();
        fs::write(
            logs.path().join("cockpit.log"),
            record(MESSAGE, "Unidentified"),
        )
        .unwrap();
        let report = logs.path().join("absent/report.md");
        assert!(
            TerminationEvidence::new(logs.path().to_owned(), report.clone())
                .finish()
                .is_err()
        );
        let result = catch_unwind(|| {
            let _evidence = TerminationEvidence::new(logs.path().to_owned(), report);
            panic!("original candidate verdict");
        });
        assert_eq!(
            result.unwrap_err().downcast_ref::<&str>(),
            Some(&"original candidate verdict")
        );
    }
}
