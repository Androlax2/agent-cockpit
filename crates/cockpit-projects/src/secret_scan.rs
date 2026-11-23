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

/// How long `gitleaks version` may take.
const VERSION_LIMIT: Duration = Duration::from_secs(10);

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

    /// What `gitleaks version` prints, or why it does not run: readiness reports it for a
    /// project whose agents push (#694).
    pub fn version(&self) -> Result<String, String> {
        let path = std::env::var_os("PATH")
            .ok_or_else(|| "the daemon has no PATH to find gitleaks on".to_owned())?;
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .env("PATH", path)
            .arg("version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let asked = format!("`{} version`", self.program.display());
        match run_within(command, &self.program, VERSION_LIMIT)
            .map_err(|error| error.to_string())?
        {
            Ran::NotFound => Err(format!(
                "{} is not on the daemon's PATH",
                self.program.display()
            )),
            Ran::TimedOut => Err(format!(
                "{asked} gave no answer within {} s",
                VERSION_LIMIT.as_secs()
            )),
            Ran::Exited {
                code: Some(0),
                stdout: Some(stdout),
                ..
            } if !stdout.trim().is_empty() => Ok(stdout.trim().to_owned()),
            Ran::Exited {
                code: Some(0),
                stdout: Some(_),
                ..
            } => Err(format!("{asked} printed no version")),
            Ran::Exited {
                code: Some(0),
                stdout: None,
                ..
            } => Err(format!(
                "{asked} exited, and its output stayed open past the limit, so no version was read"
            )),
            Ran::Exited { code, stderr, .. } => Err(format!(
                "{asked} exited with {}",
                ending(code, stderr.as_deref())
            )),
        }
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

/// The refs the scanned repository holds the remote's tips under, and the log options that
/// leave out what they reach: refs rather than arguments, since a remote's thousands of
/// branches would pass the length one argument may have.
const REMOTE_TIPS: &str = "refs/remote-tips";
const EXCLUDE_REMOTE_TIPS: &str = "--not --glob=refs/remote-tips/*";

/// What gitleaks finds in the lines the commits `commit` reaches and none of `remote_tips`
/// does add: the range [`crate::attribution`] checks, as the push sends it.
///
/// A merge of two parents counts its changes to Git's automatic merge
/// (`--diff-merges=remerge`), so merging a branch the remote holds brings none of that
/// branch's lines. Git has no such diff for a merge of more parents, so its whole change
/// against its first parent is scanned: that may name a line a merged branch brought, which
/// holds the push for the owner rather than let a line through unread.
pub(crate) fn scan(
    git: &Git,
    scanner: &SecretScanner,
    checkout: &Path,
    remote_tips: &[&str],
    commit: &str,
) -> Result<Vec<SecretFinding>, Error> {
    let path = std::env::var_os("PATH")
        .ok_or_else(|| scan_failed("the daemon has no PATH to find gitleaks and Git on"))?;
    let scratch = Scratch::create("cockpit-secret-scan")?;
    let repository = borrowing_repository(git, checkout, &scratch.path)?;
    hold_remote_tips(git, &repository, remote_tips)?;
    let configuration = scratch.path.join("gitleaks.toml");
    write(&configuration, CONFIGURATION)?;
    let scan = Scan {
        scanner,
        scratch: &scratch.path,
        repository: &repository,
        configuration: &configuration,
        path: &path,
    };
    let mut findings = scan.run(
        "two-parents",
        &format!(
            "--no-textconv --diff-merges=remerge --max-parents=2 {commit} {EXCLUDE_REMOTE_TIPS}"
        ),
    )?;
    findings.extend(scan.run(
        "octopus",
        &format!(
            "--no-textconv --diff-merges=first-parent --min-parents=3 {commit} \
             {EXCLUDE_REMOTE_TIPS}"
        ),
    )?);
    Ok(findings)
}

/// One gitleaks run's settings, all of them Torlek's.
struct Scan<'a> {
    scanner: &'a SecretScanner,
    scratch: &'a Path,
    repository: &'a Path,
    configuration: &'a Path,
    path: &'a std::ffi::OsStr,
}

impl Scan<'_> {
    /// gitleaks over the commits `git log` lists with `log_options`, reporting to
    /// `report-<name>.csv`.
    fn run(&self, name: &str, log_options: &str) -> Result<Vec<SecretFinding>, Error> {
        let report = self.scratch.join(format!("report-{name}.csv"));
        let mut command = Command::new(&self.scanner.program);
        command
            .env_clear()
            .env("PATH", self.path)
            .env("HOME", self.scratch)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(self.scratch)
            .args(["git", "--no-banner", "--log-level", "error", "--redact"])
            .args(["--ignore-gitleaks-allow", "--exit-code"])
            .arg(FINDINGS_STATUS.to_string())
            .arg("--config")
            .arg(self.configuration)
            .arg("--gitleaks-ignore-path")
            .arg(self.scratch)
            .args(["--report-format", "csv", "--report-path"])
            .arg(&report)
            .arg("--log-opts")
            .arg(log_options)
            .arg(self.repository)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let (code, stderr) = match run_within(command, &self.scanner.program, SCAN_LIMIT)? {
            Ran::NotFound => {
                return Err(Error::Refused {
                    reason: format!(
                        "the pre-push secret scan needs {GITLEAKS} on the daemon's PATH ({} is \
                         not there), and an agent's push is never sent unscanned; the owner \
                         installs gitleaks",
                        self.scanner.program.display()
                    ),
                });
            }
            Ran::TimedOut => {
                return Err(scan_failed(&format!(
                    "gitleaks gave no answer within {} s",
                    SCAN_LIMIT.as_secs()
                )));
            }
            Ran::Exited { code, stderr, .. } => (code, stderr),
        };
        match code {
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
                "gitleaks exited with {}",
                ending(code, stderr.as_deref())
            ))),
        }
    }
}

/// Points a ref under [`REMOTE_TIPS`] in `repository` at each of `remote_tips` the checkout
/// holds as a commit; the rest it lacks, so they reach nothing the push sends.
fn hold_remote_tips(git: &Git, repository: &Path, remote_tips: &[&str]) -> Result<(), Error> {
    if remote_tips.is_empty() {
        return Ok(());
    }
    let asked = remote_tips.join("\n") + "\n";
    let listed = git.run_with_input(
        repository,
        &[
            arg("cat-file"),
            arg("--batch-check=%(objectname) %(objecttype)"),
        ],
        asked.as_bytes(),
    )?;
    let updates: String = listed
        .lines()
        .filter_map(|line| line.strip_suffix(" commit"))
        .enumerate()
        .map(|(index, tip)| format!("create {REMOTE_TIPS}/{index} {tip}\n"))
        .collect();
    if updates.is_empty() {
        return Ok(());
    }
    git.run_with_input(
        repository,
        &[arg("update-ref"), arg("--stdin")],
        updates.as_bytes(),
    )?;
    Ok(())
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

/// How a program run within a limit ended.
enum Ran {
    NotFound,
    /// Killed once the limit passed.
    TimedOut,
    /// Its exit code, `None` for a signal, and what it printed: each `None` when it could not
    /// be read whole within the limit.
    Exited {
        code: Option<i32>,
        stdout: Option<String>,
        stderr: Option<String>,
    },
}

/// `code` and `stderr` as a failure message ends: `2: why` or `2 and printed nothing`.
fn ending(code: Option<i32>, stderr: Option<&str>) -> String {
    let code = code.map_or("a signal".to_owned(), |code| code.to_string());
    match stderr.map(str::trim) {
        Some("") => format!("{code} and printed nothing"),
        Some(stderr) => format!("{code}: {stderr}"),
        None => format!("{code}; its standard error stayed open past the limit, so it is unread"),
    }
}

/// Runs `command`, killed once `limit` passed. Its standard error is read before its standard
/// output, so a failure's reason is kept within the limit.
fn run_within(mut command: Command, program: &Path, limit: Duration) -> Result<Ran, Error> {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Ran::NotFound),
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
            // Kill fails only when it exited meanwhile, which `wait` then reaps. The `git log` it
            // started then ends at its next write, on the pipe gitleaks no longer reads.
            let _ = child.kill();
            child.wait().map_err(Error::io("wait for", program))?;
            return Ok(Ran::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let collect = |reader: std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>| {
        let left = deadline.saturating_duration_since(Instant::now());
        match reader.recv_timeout(left) {
            Ok(Ok(bytes)) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Ok(Err(_)) | Err(_) => None,
        }
    };
    let stderr = collect(stderr);
    Ok(Ran::Exited {
        code: status.code(),
        stdout: collect(stdout),
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
