//! The pre-push secret scan (#694): gitleaks reads every line an agent's push adds, and a
//! finding holds the push for the owner. gitleaks is a required host program: without it an
//! agent's push is refused, never sent unscanned (owner decision, 2026-11-23, option B).
//!
//! gitleaks reads the commits through a private bare repository whose only objects are the
//! checkout's, borrowed as an alternate: no configuration, attributes, `.gitleaks.toml` or
//! `.gitleaksignore` the checkout holds reaches the scan, and its rules are gitleaks' own
//! defaults, from a configuration file Torlek writes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::Error;
use crate::git::{Git, arg, read_all};
use crate::source::Scratch;

/// The program name the daemon runs from its `PATH`.
pub const GITLEAKS: &str = "gitleaks";

/// How long a scan may run before the push is refused for it.
const SCAN_LIMIT: Duration = Duration::from_secs(300);

/// The exit status gitleaks is told to use for findings, apart from its errors' `1`.
const FINDINGS_STATUS: i32 = 3;

/// gitleaks' built-in rules and nothing the scanned repository says.
const CONFIGURATION: &str = "[extend]\nuseDefault = true\n";

/// The gitleaks program an agent's push is scanned with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretScanner {
    program: PathBuf,
}

impl SecretScanner {
    /// `gitleaks` as found on the daemon's `PATH`.
    pub fn from_path() -> SecretScanner {
        SecretScanner {
            program: PathBuf::from(GITLEAKS),
        }
    }

    /// An explicit program, a fake one in fixtures.
    pub fn with_program(program: PathBuf) -> SecretScanner {
        SecretScanner { program }
    }
}

/// One line gitleaks reads as a secret. It never holds the secret itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    pub commit: String,
    pub file: String,
    pub line: String,
    /// The gitleaks rule that matched, such as `generic-api-key`.
    pub rule: String,
}

/// What gitleaks finds in the lines the commits `commit` reaches and none of `remote_tips`
/// does add: the range [`crate::attribution`] checks, as the push sends it. A merge's own
/// lines are its changes to Git's automatic merge (`--diff-merges=remerge`), so a merge of a
/// branch the remote holds brings none of that branch's lines.
pub(crate) fn scan(
    git: &Git,
    scanner: &SecretScanner,
    checkout: &Path,
    remote_tips: &[&str],
    commit: &str,
) -> Result<Vec<SecretFinding>, Error> {
    let scratch = Scratch::create("cockpit-secret-scan")?;
    let repository = borrowing_repository(git, checkout, &scratch.path)?;
    let configuration = scratch.path.join("gitleaks.toml");
    write(&configuration, CONFIGURATION)?;
    let report = scratch.path.join("report.csv");
    let log_options = std::iter::once(format!(
        "--no-textconv --diff-merges=remerge --ignore-missing {commit}"
    ))
    .chain(remote_tips.iter().map(|tip| format!("^{tip}")))
    .collect::<Vec<_>>()
    .join(" ");
    let mut command = Command::new(&scanner.program);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &scratch.path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(&scratch.path)
        .args(["git", "--no-banner", "--log-level", "error", "--redact"])
        .args(["--ignore-gitleaks-allow", "--exit-code"])
        .arg(FINDINGS_STATUS.to_string())
        .arg("--config")
        .arg(&configuration)
        .arg("--gitleaks-ignore-path")
        .arg(&scratch.path)
        .args(["--report-format", "csv", "--report-path"])
        .arg(&report)
        .arg("--log-opts")
        .arg(&log_options)
        .arg(&repository)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let status = run_within(command, &scanner.program, SCAN_LIMIT)?;
    match status.code {
        Some(0) => Ok(Vec::new()),
        Some(FINDINGS_STATUS) => {
            let read = std::fs::read_to_string(&report).map_err(Error::io("read", &report))?;
            let findings = findings_in(&read)?;
            if findings.is_empty() {
                return Err(scan_failed(
                    "it reported findings and its report lists none",
                ));
            }
            Ok(findings)
        }
        code => Err(scan_failed(&format!(
            "gitleaks exited with {}: {}",
            code.map_or("a signal".to_owned(), |code| code.to_string()),
            status.stderr.trim()
        ))),
    }
}

/// A bare repository in `scratch` that reads the checkout's objects, its shallow boundary and
/// grafts, as the push's own walk does, and nothing else of it.
fn borrowing_repository(git: &Git, checkout: &Path, scratch: &Path) -> Result<PathBuf, Error> {
    let listed = git.run(
        checkout,
        &[
            arg("rev-parse"),
            arg("--show-object-format"),
            arg("--path-format=absolute"),
            arg("--git-path"),
            arg("objects"),
            arg("--git-path"),
            arg("shallow"),
            arg("--git-path"),
            arg("info/grafts"),
        ],
    )?;
    let [format, objects, shallow, grafts] = listed.lines().collect::<Vec<_>>()[..] else {
        return Err(scan_failed(&format!(
            "`git rev-parse` named no object format and paths: {listed:?}"
        )));
    };
    let repository = scratch.join("repository");
    let object_format = format!("--object-format={format}");
    git.run(
        scratch,
        &[
            arg("init"),
            arg("--bare"),
            arg("--quiet"),
            arg(&object_format),
            arg("--end-of-options"),
            arg(&repository),
        ],
    )?;
    write(
        &repository.join("objects/info/alternates"),
        &format!("{objects}\n"),
    )?;
    for (borrowed, own) in [(shallow, "shallow"), (grafts, "info/grafts")] {
        let borrowed = Path::new(borrowed);
        if borrowed.exists() {
            let own = repository.join(own);
            std::fs::copy(borrowed, &own).map_err(Error::io("copy", borrowed))?;
        }
    }
    Ok(repository)
}

fn write(path: &Path, contents: &str) -> Result<(), Error> {
    std::fs::write(path, contents).map_err(Error::io("write", path))
}

fn scan_failed(why: &str) -> Error {
    Error::Refused {
        reason: format!(
            "the pre-push secret scan failed, and an agent's push is never sent unscanned: \
             {why}"
        ),
    }
}

struct Finished {
    code: Option<i32>,
    stderr: String,
}

/// Runs `command`, killed once `limit` passed. A missing program refuses the push, naming it.
fn run_within(mut command: Command, program: &Path, limit: Duration) -> Result<Finished, Error> {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::Refused {
                reason: format!(
                    "the pre-push secret scan needs {GITLEAKS} on the daemon's PATH ({} is not \
                     there), and an agent's push is never sent unscanned; the owner installs \
                     gitleaks, which `torlek doctor` reports",
                    program.display()
                ),
            });
        }
        Err(error) => return Err(Error::io("run", program)(error)),
    };
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(Error::io("wait for", program))? {
            break status;
        }
        if Instant::now() >= deadline {
            // Kill fails only when it exited meanwhile, which `wait` then reaps.
            let _ = child.kill();
            child.wait().map_err(Error::io("wait for", program))?;
            return Err(scan_failed(&format!(
                "gitleaks gave no answer within {} s",
                limit.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Its output is drained so it never stalls on a full pipe; only standard error is kept.
    drop(stdout);
    let left = deadline.saturating_duration_since(Instant::now());
    let stderr = match stderr.recv_timeout(left) {
        Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
        Ok(Err(error)) => return Err(Error::io("read the output of", program)(error)),
        Err(_) => String::new(),
    };
    Ok(Finished {
        code: status.code(),
        stderr,
    })
}

/// The findings of a gitleaks CSV report: its header names the columns.
fn findings_in(report: &str) -> Result<Vec<SecretFinding>, Error> {
    let mut rows = csv_rows(report).into_iter();
    let Some(header) = rows.next() else {
        return Ok(Vec::new());
    };
    let column = |name: &str| {
        header
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| scan_failed(&format!("gitleaks' report has no {name} column")))
    };
    let (rule, commit, file, line) = (
        column("RuleID")?,
        column("Commit")?,
        column("File")?,
        column("StartLine")?,
    );
    rows.map(|row| {
        let field = |index: usize| {
            row.get(index)
                .cloned()
                .ok_or_else(|| scan_failed("a row of gitleaks' report is short of its header"))
        };
        Ok(SecretFinding {
            commit: field(commit)?,
            file: field(file)?,
            line: field(line)?,
            rule: field(rule)?,
        })
    })
    .collect()
}

/// RFC 4180 rows, as Go's `encoding/csv` writes them: a field with a comma, quote or line
/// break is quoted, and a quote in it doubled. Blank lines are no rows.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut is_quoted = false;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match (is_quoted, character) {
            (true, '"') if characters.peek() == Some(&'"') => {
                characters.next();
                field.push('"');
            }
            (true, '"') => is_quoted = false,
            (true, other) => field.push(other),
            (false, '"') => is_quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') if characters.peek() == Some(&'\n') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                let row = std::mem::take(&mut row);
                if row != [""] {
                    rows.push(row);
                }
            }
            (false, other) => field.push(other),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "RuleID,Commit,File,SymlinkFile,Secret,Match,StartLine,EndLine,\
                          StartColumn,EndColumn,Author,Message,Date,Email,Fingerprint,Tags\n";

    #[test]
    fn a_report_names_each_findings_rule_file_line_and_commit() {
        let report = format!(
            "{HEADER}generic-api-key,c1,src/a.rs,,REDACTED,REDACTED,3,3,1,9,A,\"work, \
             \"\"quoted\"\"\nsecond line\",d,e,f,\n"
        );

        assert_eq!(
            findings_in(&report).unwrap(),
            [SecretFinding {
                commit: "c1".to_owned(),
                file: "src/a.rs".to_owned(),
                line: "3".to_owned(),
                rule: "generic-api-key".to_owned(),
            }]
        );
    }

    #[test]
    fn an_empty_report_has_no_findings() {
        assert_eq!(findings_in("").unwrap(), []);
    }

    #[test]
    fn a_report_without_a_column_it_needs_is_refused() {
        assert!(findings_in("RuleID,File\nx,y\n").is_err());
    }

    #[test]
    fn a_quoted_field_keeps_its_commas_quotes_and_line_breaks() {
        assert_eq!(
            csv_rows("a,\"b,\"\"c\"\"\nd\",e\r\nf,g\n"),
            [
                vec!["a".to_owned(), "b,\"c\"\nd".to_owned(), "e".to_owned()],
                vec!["f".to_owned(), "g".to_owned()]
            ]
        );
    }
}
