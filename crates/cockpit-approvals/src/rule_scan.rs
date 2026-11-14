//! The rule scan (#1388): before the owner reads an `ask run` request, its command line, and
//! each shell script it names, is read for known-risky patterns (`rules.rs`): a download or
//! decoded text run as code, secrets read, persistence points written, setuid bits, root's
//! files, deletion at or above home, and #1383's files sudo runs that the session can edit.
//! Each finding names its rule, the text that matched and, in a script, the file and line.
//!
//! It warns and never approves. Obfuscated or downloaded code passes any static check, so a
//! clean scan reads "no known risky pattern", never "safe"; what cannot be resolved without
//! running the command (an expansion where the program or its code is, `eval`, code from a
//! pipe) is a finding of its own, "can't tell what runs".
//!
//! What it reads: the command line's pipes, lists, subshells, `-c` strings, here-documents
//! and substitutions, through `sudo`, `env` and the usual wrappers; the shell scripts the
//! command line runs, as a program, a shell's script, its input or a `source`. Not read: what
//! a script runs in turn, scripts of other interpreters, and programs found on `PATH`, which
//! are installed rather than written by the session.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::hints::program_name;
use crate::rules::{
    DOWNLOADERS, Invocation, Pattern, Place, RULES, Rule, Severity, Source, WRITERS, Written,
};
use crate::shell::{
    DirectoryChange, InterpreterCode, ShellCode, Simple, Stdin, Word, absolute, directory_change,
    interpreter, interpreter_code, is_shell, lex, logical_directory, shell_arguments,
    skip_assignments, skip_keywords, skip_wrappers, sudo_options,
};
use crate::sudo_editable::{Lookup, RootRuns, open_regular};

/// One known-risky pattern found in a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule's id (`download-runs`).
    pub rule: String,
    pub severity: Severity,
    /// The line the owner reads (`runs code it downloads`).
    pub reason: String,
    /// The text that matched, on one line: the command, or the file sudo runs. Empty when
    /// nothing in particular matched.
    pub excerpt: String,
    /// The script it is in; `None` on the command line.
    pub file: Option<PathBuf>,
    /// The script's line it starts on, from 1.
    pub line: Option<usize>,
}

/// The findings of `command`, run in `lookup.cwd` by a session working in `clone`, with what
/// #1383 found its sudo runs. High findings first, each in the order the command reads.
/// Reads the shell scripts the command line names.
pub fn scan(
    command: &str,
    lookup: Lookup<'_>,
    clone: Option<&Path>,
    root_runs: &RootRuns,
) -> Vec<Finding> {
    let mut scan = Scan {
        lookup,
        clone,
        findings: Vec::new(),
    };
    scan.script(command, lookup.cwd, At::CommandLine, 0);
    scan.findings.extend(sudo_findings(root_runs, lookup.cwd));
    in_order(scan.findings)
}

/// The findings of the rules that read #1383's check: a file sudo runs that the session can
/// edit, and what sudo runs that cannot be told.
pub fn sudo_findings(root_runs: &RootRuns, cwd: &Path) -> Vec<Finding> {
    let finding = |is: fn(&Pattern) -> bool, excerpt: String| {
        let rule = rule_where(is)
            .next()
            .expect("the rules hold the sudo rules");
        Finding {
            rule: rule.id.to_owned(),
            severity: rule.severity,
            reason: rule.reason.to_owned(),
            excerpt,
            file: None,
            line: None,
        }
    };
    let mut findings: Vec<Finding> = root_runs
        .editable
        .iter()
        .map(|file| {
            finding(
                |pattern| matches!(pattern, Pattern::SudoRunsEditable),
                path_from(&file.path, cwd),
            )
        })
        .collect();
    if root_runs.cannot_tell {
        findings.push(finding(
            |pattern| matches!(pattern, Pattern::SudoCannotTell),
            String::new(),
        ));
    }
    findings
}

/// The finding recorded when the scan did not end in time.
pub fn scan_unfinished() -> Finding {
    cannot_tell_because("the scan did not end in time")
}

/// The finding recorded when the files a command runs could not all be pinned (#1387): a
/// change to them would not refuse the run. `in_time` is false when pinning did not end.
pub fn pinning_incomplete(in_time: bool) -> Finding {
    cannot_tell_because(if in_time {
        "some files it runs could not be pinned"
    } else {
        "the files it runs could not be pinned in time"
    })
}

/// Whether `finding` says part of what the command runs cannot be told, as root or not: an
/// "always" approval (#1387) could not then bind every file it runs.
pub(crate) fn says_cannot_tell(finding: &Finding) -> bool {
    rule_where(|pattern| matches!(pattern, Pattern::CannotTell | Pattern::SudoCannotTell))
        .any(|rule| rule.id == finding.rule)
}

/// Whether `finding` says the command runs code it downloads or decodes: code no pinned
/// file's hash binds, so an "always" approval (#1387) would run whatever comes next.
pub(crate) fn runs_unhashed_code(finding: &Finding) -> bool {
    rule_where(|pattern| {
        matches!(
            pattern,
            Pattern::RunsOutputOf(_) | Pattern::RunsSavedDownload
        )
    })
    .any(|rule| rule.id == finding.rule)
}

fn cannot_tell_because(why: &str) -> Finding {
    let rule = rule_where(|pattern| matches!(pattern, Pattern::CannotTell))
        .next()
        .expect("the rules hold the cannot-tell rule");
    Finding {
        rule: rule.id.to_owned(),
        severity: rule.severity,
        reason: format!("{}: {why}", rule.reason),
        excerpt: String::new(),
        file: None,
        line: None,
    }
}

/// `path` as the owner reads it: `./x.sh` inside `cwd`, else absolute.
pub fn path_from(path: &Path, cwd: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(inside) => format!("./{}", inside.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Each finding once; "can't tell" left out where another finding names the same text; high
/// first, then in the order of [`RULES`], then as the command reads.
fn in_order(findings: Vec<Finding>) -> Vec<Finding> {
    let is_cannot_tell = |finding: &Finding| {
        rule_where(|pattern| matches!(pattern, Pattern::CannotTell))
            .any(|rule| rule.id == finding.rule)
    };
    let mut kept: Vec<Finding> = Vec::new();
    for finding in &findings {
        let is_covered = is_cannot_tell(finding)
            && findings.iter().any(|other| {
                !is_cannot_tell(other)
                    && other.excerpt == finding.excerpt
                    && other.file == finding.file
                    && other.line == finding.line
            });
        if !is_covered && !kept.contains(finding) {
            kept.push(finding.clone());
        }
    }
    let rule_at = |finding: &Finding| RULES.iter().position(|rule| rule.id == finding.rule);
    kept.sort_by_key(|finding| (finding.severity, rule_at(finding)));
    kept
}

fn rule_where(is: impl Fn(&Pattern) -> bool) -> impl Iterator<Item = &'static Rule> {
    RULES
        .iter()
        .filter(move |rule| rule.patterns.iter().any(&is))
}

/// How deep `-c` strings, substitutions and here-documents are read.
const DEPTH_MAX: usize = 8;

/// The most of a script read; a longer one is a "can't tell".
const SCRIPT_BYTES_MAX: u64 = 1024 * 1024;

/// The most of a command kept as a finding's excerpt.
const EXCERPT_CHARS_MAX: usize = 300;

/// Where a finding is: its script and line.
type Location = (Option<PathBuf>, Option<usize>);

/// Where the code being read comes from.
#[derive(Clone, Copy)]
enum At<'p> {
    /// The request's command line.
    CommandLine,
    /// A script the command line runs: each command at its own line.
    File(&'p Path),
    /// Code inside a command (a `-c` string, a substitution), found where that command is.
    Inside(Option<&'p Path>, Option<usize>),
}

impl<'p> At<'p> {
    fn location(self, command: &Simple) -> Location {
        match self {
            At::CommandLine => (None, None),
            At::File(file) => (Some(file.to_path_buf()), Some(command.line)),
            At::Inside(file, line) => (file.map(Path::to_path_buf), line),
        }
    }

    fn inside(self, command: &Simple) -> At<'p> {
        match self {
            At::CommandLine => At::Inside(None, None),
            At::File(file) => At::Inside(Some(file), Some(command.line)),
            inside @ At::Inside(..) => inside,
        }
    }

    /// Whether the scripts this code runs are read: only those the command line names.
    fn reads_scripts(self) -> bool {
        matches!(self, At::CommandLine | At::Inside(None, _))
    }
}

/// One lexed script: its characters, for excerpts, and its commands.
struct Script {
    characters: Vec<char>,
    commands: Vec<Simple>,
}

impl Script {
    /// The text from the start of the command at `first` to the end of the one at `last`.
    fn text(&self, first: usize, last: usize) -> String {
        let start = self.commands[first].span.start;
        let end = self.commands[last].span.end;
        one_line(&self.characters[start..end.max(start)])
    }
}

/// `characters` on one line, blanks collapsed, at most [`EXCERPT_CHARS_MAX`].
fn one_line(characters: &[char]) -> String {
    let text: String = characters.iter().collect();
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= EXCERPT_CHARS_MAX {
        return joined;
    }
    let kept: String = joined.chars().take(EXCERPT_CHARS_MAX - 1).collect();
    format!("{kept}…")
}

/// What a script being read has done so far: where it is, and the files downloaders saved.
struct State {
    cwd: PathBuf,
    /// Each saved file, with the command that saved it.
    saved: Vec<(PathBuf, usize)>,
}

/// The program a command runs, once keywords, assignments, wrappers and `sudo` are left out.
enum Resolved<'w> {
    Program {
        program: &'w Word,
        arguments: &'w [Word],
        cwd: PathBuf,
    },
    /// No program: assignments alone, or a `sudo` that runs nothing.
    Nothing,
    CannotTell,
}

/// Where the code a shell or an interpreter runs comes from.
enum Code<'w> {
    /// A shell's `-c` string.
    Shell(&'w Word),
    /// Another interpreter's inline code.
    Inline(Word),
    Script {
        word: &'w Word,
        is_shell: bool,
    },
    Input {
        is_shell: bool,
    },
    /// `eval`'s words.
    Eval,
    CannotTell,
}

/// How a file is run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Runs {
    /// As a program: a shell reads it when it has no `#!` line or names a shell there.
    AsProgram,
    ByShell,
    ByInterpreter,
}

struct Scan<'a> {
    lookup: Lookup<'a>,
    clone: Option<&'a Path>,
    findings: Vec<Finding>,
}

impl Scan<'_> {
    /// A finding of each rule holding a pattern `is` accepts.
    fn flag(&mut self, is: impl Fn(&Pattern) -> bool, excerpt: &str, location: Location) {
        for rule in rule_where(is) {
            self.flag_rule(rule, excerpt, &location);
        }
    }

    fn flag_rule(&mut self, rule: &Rule, excerpt: &str, location: &Location) {
        self.findings.push(Finding {
            rule: rule.id.to_owned(),
            severity: rule.severity,
            reason: rule.reason.to_owned(),
            excerpt: excerpt.to_owned(),
            file: location.0.clone(),
            line: location.1,
        });
    }

    fn cannot_tell(&mut self, excerpt: &str, location: Location) {
        self.flag(
            |pattern| matches!(pattern, Pattern::CannotTell),
            excerpt,
            location,
        );
    }

    /// Each command of `code`, read by a shell in `cwd`.
    fn script(&mut self, code: &str, cwd: &Path, at: At<'_>, depth: usize) {
        let first_line = || {
            one_line(
                &code
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .collect::<Vec<_>>(),
            )
        };
        let location = match at {
            At::CommandLine => (None, None),
            At::File(file) => (Some(file.to_path_buf()), None),
            At::Inside(file, line) => (file.map(Path::to_path_buf), line),
        };
        if depth > DEPTH_MAX {
            self.cannot_tell(&first_line(), location);
            return;
        }
        let Some(commands) = lex(code, self.lookup.home) else {
            self.cannot_tell(&first_line(), location);
            return;
        };
        let script = Script {
            characters: code.chars().collect(),
            commands,
        };
        let mut state = State {
            cwd: cwd.to_path_buf(),
            saved: Vec::new(),
        };
        for index in 0..script.commands.len() {
            let command = &script.commands[index];
            match directory_change(command, &state.cwd, self.lookup.home) {
                DirectoryChange::None => self.command(&script, index, &mut state, at, depth),
                DirectoryChange::To(moved) => {
                    self.substitutions(command, &state.cwd, at, depth);
                    state.cwd = moved;
                }
                DirectoryChange::CannotTell => {
                    self.substitutions(command, &state.cwd, at, depth);
                    self.cannot_tell(&script.text(index, index), at.location(command));
                }
            }
        }
    }

    /// The code `command`'s substitutions run, in its words, redirections and input.
    fn substitutions(&mut self, command: &Simple, cwd: &Path, at: At<'_>, depth: usize) {
        let inside = at.inside(command);
        for word in command
            .words
            .iter()
            .chain(&command.writes)
            .chain(stdin_word(&command.stdin))
        {
            for code in &word.substitutions {
                self.script(code, cwd, inside, depth + 1);
            }
        }
    }

    fn command(
        &mut self,
        script: &Script,
        index: usize,
        state: &mut State,
        at: At<'_>,
        depth: usize,
    ) {
        let command = &script.commands[index];
        let excerpt = script.text(index, index);
        let location = at.location(command);
        self.substitutions(command, &state.cwd, at, depth);
        let redirected: Vec<PathBuf> = command
            .writes
            .iter()
            .map(|target| self.path(&target.text, &state.cwd))
            .collect();
        let (program, arguments, cwd) = match self.resolve(&command.words, &state.cwd) {
            Resolved::Program {
                program,
                arguments,
                cwd,
            } => (program, arguments, cwd),
            Resolved::Nothing => {
                self.writes(&redirected, &excerpt, &location);
                return;
            }
            Resolved::CannotTell => {
                self.writes(&redirected, &excerpt, &location);
                self.cannot_tell(&excerpt, location);
                return;
            }
        };
        let name = program_name(&program.text);
        let mut written = redirected.clone();
        written.extend(self.written_by(name, arguments, &cwd));
        self.writes(&written, &excerpt, &location);
        self.program_rules(name, arguments, &command.stdin, &cwd, &excerpt, &location);
        if DOWNLOADERS
            .iter()
            .any(|source| source.programs.contains(&name))
        {
            // The shell opens a redirection where it is, before `sudo -D` moves the program.
            for saved in self
                .saved_by(name, arguments, &cwd)
                .into_iter()
                .chain(redirected)
            {
                state.saved.push((saved, index));
            }
        }
        if program.text.contains('/') {
            let path = self.path(&program.text, &cwd);
            self.runs_file(&path, Runs::AsProgram, script, index, state, at, depth);
        }
        self.code(
            name, program, arguments, script, index, state, &cwd, at, depth,
        );
    }

    /// The program `words` run, through keywords, assignments, wrappers and `sudo`.
    fn resolve<'w>(&self, words: &'w [Word], cwd: &Path) -> Resolved<'w> {
        let mut words = skip_keywords(words);
        let mut cwd = cwd.to_path_buf();
        loop {
            let unwrapped = skip_wrappers(words, &cwd);
            if unwrapped.cannot_tell {
                return Resolved::CannotTell;
            }
            cwd = unwrapped.cwd;
            let Some((program, rest)) = unwrapped.words.split_first() else {
                return Resolved::Nothing;
            };
            if !program.is_literal {
                return Resolved::CannotTell;
            }
            if program_name(&program.text) != "sudo" {
                return Resolved::Program {
                    program,
                    arguments: rest,
                    cwd,
                };
            }
            let Some(options) = sudo_options(rest) else {
                return Resolved::CannotTell;
            };
            if options.runs_nothing {
                return Resolved::Nothing;
            }
            match &options.chdir {
                Some(directory) if !directory.is_literal => return Resolved::CannotTell,
                Some(directory) => cwd = absolute(&cwd, &directory.text),
                None => {}
            }
            words = skip_assignments(&rest[options.command_at..]);
        }
    }

    /// Where the code `program` runs comes from, and what is found in it.
    #[expect(
        clippy::too_many_arguments,
        reason = "one command's parts beside the script it is in; a struct for this one call adds nothing"
    )]
    fn code(
        &mut self,
        name: &str,
        program: &Word,
        arguments: &[Word],
        script: &Script,
        index: usize,
        state: &State,
        cwd: &Path,
        at: At<'_>,
        depth: usize,
    ) {
        let command = &script.commands[index];
        let excerpt = script.text(index, index);
        let location = at.location(command);
        let code = if is_shell(name) {
            match shell_arguments(arguments).code {
                ShellCode::CommandString(word) => Code::Shell(word),
                ShellCode::Script(word) => Code::Script {
                    word,
                    is_shell: true,
                },
                ShellCode::Input => Code::Input { is_shell: true },
                ShellCode::CannotTell => Code::CannotTell,
            }
        } else if program.is_literal && matches!(program.text.as_str(), "source" | ".") {
            match arguments.first() {
                Some(word) => Code::Script {
                    word,
                    is_shell: true,
                },
                None => return,
            }
        } else if name == "eval" {
            Code::Eval
        } else if let Some(interpreter) = interpreter(name) {
            match interpreter_code(&interpreter, arguments) {
                InterpreterCode::Inline(word) => Code::Inline(word),
                InterpreterCode::Script(word) => Code::Script {
                    word,
                    is_shell: false,
                },
                InterpreterCode::Input => Code::Input { is_shell: false },
                InterpreterCode::CannotTell => Code::CannotTell,
            }
        } else {
            return;
        };
        match code {
            Code::CannotTell => self.cannot_tell(&excerpt, location),
            Code::Eval => {
                for word in arguments {
                    self.substituted(word, cwd, &excerpt, &location, depth);
                }
                self.cannot_tell(&excerpt, location);
            }
            Code::Shell(word) if word.is_literal => {
                self.script(&word.text, cwd, at.inside(command), depth + 1);
            }
            Code::Shell(word) => {
                self.substituted_or_cannot_tell(word, cwd, &excerpt, location, depth)
            }
            Code::Inline(word) if word.is_literal => {}
            Code::Inline(word) => {
                self.substituted_or_cannot_tell(&word, cwd, &excerpt, location, depth);
            }
            Code::Script { word, .. } if !word.is_literal => {
                self.substituted_or_cannot_tell(word, cwd, &excerpt, location, depth);
            }
            Code::Script { word, is_shell } => {
                let path = self.path(&word.text, cwd);
                let runs = if is_shell {
                    Runs::ByShell
                } else {
                    Runs::ByInterpreter
                };
                self.runs_file(&path, runs, script, index, state, at, depth);
            }
            Code::Input { is_shell } => match &command.stdin {
                Stdin::Terminal => {}
                Stdin::Pipe => self.piped(script, index, cwd, at, depth),
                Stdin::File(word) if !word.is_literal => {
                    self.substituted_or_cannot_tell(word, cwd, &excerpt, location, depth);
                }
                Stdin::File(word) => {
                    let path = self.path(&word.text, cwd);
                    let runs = if is_shell {
                        Runs::ByShell
                    } else {
                        Runs::ByInterpreter
                    };
                    self.runs_file(&path, runs, script, index, state, at, depth);
                }
                Stdin::Text(word) if !word.substitutions.is_empty() => {
                    self.substituted_or_cannot_tell(word, cwd, &excerpt, location, depth);
                }
                Stdin::Text(word) if is_shell => {
                    self.script(&word.text, cwd, at.inside(command), depth + 1);
                }
                Stdin::Text(word) if !word.is_literal => self.cannot_tell(&excerpt, location),
                Stdin::Text(_) => {}
            },
        }
    }

    /// A shell or an interpreter reading its code from the pipe: what the commands before it
    /// in the pipeline print.
    fn piped(&mut self, script: &Script, index: usize, cwd: &Path, at: At<'_>, depth: usize) {
        let mut first = index;
        while first > 0 && script.commands[first].stdin == Stdin::Pipe {
            first -= 1;
        }
        let excerpt = script.text(first, index);
        let location = at.location(&script.commands[first]);
        let mut sources = Vec::new();
        for upstream in &script.commands[first..index] {
            sources.extend(self.sources_of(upstream, cwd, depth));
        }
        self.flag_sources(&sources, &excerpt, &location);
        if sources.is_empty() {
            self.cannot_tell(&excerpt, location);
        }
    }

    /// Code that is an expansion: the rules of the programs its substitutions run, else
    /// "can't tell".
    fn substituted_or_cannot_tell(
        &mut self,
        word: &Word,
        cwd: &Path,
        excerpt: &str,
        location: Location,
        depth: usize,
    ) {
        if !self.substituted(word, cwd, excerpt, &location, depth) {
            self.cannot_tell(excerpt, location);
        }
    }

    /// Flags the rules whose sources `word`'s substitutions run; whether there was one.
    fn substituted(
        &mut self,
        word: &Word,
        cwd: &Path,
        excerpt: &str,
        location: &Location,
        depth: usize,
    ) -> bool {
        let mut sources = Vec::new();
        for code in &word.substitutions {
            sources.extend(self.sources_in(code, cwd, depth + 1));
        }
        self.flag_sources(&sources, excerpt, location);
        !sources.is_empty()
    }

    fn flag_sources(&mut self, sources: &[&'static Rule], excerpt: &str, location: &Location) {
        for rule in RULES {
            if sources.iter().any(|source| source.id == rule.id) {
                self.flag_rule(rule, excerpt, location);
            }
        }
    }

    /// The rules whose sources the commands of `code` run.
    fn sources_in(&self, code: &str, cwd: &Path, depth: usize) -> Vec<&'static Rule> {
        if depth > DEPTH_MAX {
            return Vec::new();
        }
        lex(code, self.lookup.home)
            .unwrap_or_default()
            .iter()
            .flat_map(|command| self.sources_of(command, cwd, depth))
            .collect()
    }

    /// The rules whose sources `command` runs, itself or in its substitutions.
    fn sources_of(&self, command: &Simple, cwd: &Path, depth: usize) -> Vec<&'static Rule> {
        let mut rules: Vec<&'static Rule> = command
            .words
            .iter()
            .flat_map(|word| &word.substitutions)
            .flat_map(|code| self.sources_in(code, cwd, depth + 1))
            .collect();
        if let Resolved::Program {
            program, arguments, ..
        } = self.resolve(&command.words, cwd)
        {
            let name = program_name(&program.text);
            rules.extend(RULES.iter().filter(|rule| {
                rule.patterns.iter().any(|pattern| match pattern {
                    Pattern::RunsOutputOf(sources) => sources
                        .iter()
                        .any(|source| is_source(source, name, arguments)),
                    _ => false,
                })
            }));
        }
        rules
    }

    /// A file the command runs: a download saved earlier, and a shell script it names.
    #[expect(
        clippy::too_many_arguments,
        reason = "one command's parts beside the script it is in; a struct for this one call adds nothing"
    )]
    fn runs_file(
        &mut self,
        path: &Path,
        runs: Runs,
        script: &Script,
        index: usize,
        state: &State,
        at: At<'_>,
        depth: usize,
    ) {
        let command = &script.commands[index];
        if let Some((_, saved_by)) = state.saved.iter().find(|(saved, _)| saved == path) {
            let excerpt = script.text(*saved_by, index);
            self.flag(
                |pattern| matches!(pattern, Pattern::RunsSavedDownload),
                &excerpt,
                at.location(&script.commands[*saved_by]),
            );
            return;
        }
        if runs == Runs::ByInterpreter || !at.reads_scripts() {
            return;
        }
        let text = match read_script(path) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(()) => {
                self.cannot_tell(&path_from(path, &state.cwd), at.location(command));
                return;
            }
        };
        if runs == Runs::AsProgram && !is_shell_script(&text) {
            return;
        }
        self.script(&text, &state.cwd, At::File(path), depth + 1);
    }

    /// The rules about what a program does with the files it names.
    fn program_rules(
        &mut self,
        name: &str,
        arguments: &[Word],
        stdin: &Stdin,
        cwd: &Path,
        excerpt: &str,
        location: &Location,
    ) {
        let named: Vec<PathBuf> = arguments
            .iter()
            .chain(stdin_word(stdin))
            .flat_map(|word| self.paths_named(word, cwd))
            .collect();
        let reads = |pattern: &Pattern| match pattern {
            Pattern::Reads(places) => named
                .iter()
                .any(|path| places.iter().any(|place| self.is_in(path, place))),
            _ => false,
        };
        let flagged: Vec<&'static Rule> = rule_where(reads).collect();
        for rule in flagged {
            self.flag_rule(rule, excerpt, location);
        }
        let runs = |pattern: &Pattern| match pattern {
            Pattern::Runs(invocations) => invocations
                .iter()
                .any(|invocation| is_invocation(invocation, name, arguments)),
            Pattern::SetsSetuid => name == "chmod" && sets_setuid(arguments),
            Pattern::GivesToRoot => name == "chown" && gives_to_root(arguments),
            Pattern::DeletesAtOrAboveHome => self.deletes_at_or_above_home(name, arguments, cwd),
            _ => false,
        };
        let flagged: Vec<&'static Rule> = rule_where(runs).collect();
        for rule in flagged {
            self.flag_rule(rule, excerpt, location);
        }
    }

    fn writes(&mut self, written: &[PathBuf], excerpt: &str, location: &Location) {
        let writes = |pattern: &Pattern| match pattern {
            Pattern::Writes(places) => written
                .iter()
                .any(|path| places.iter().any(|place| self.is_in(path, place))),
            _ => false,
        };
        let flagged: Vec<&'static Rule> = rule_where(writes).collect();
        for rule in flagged {
            self.flag_rule(rule, excerpt, location);
        }
    }

    /// The paths `name` writes, as one of [`WRITERS`].
    fn written_by(&self, name: &str, arguments: &[Word], cwd: &Path) -> Vec<PathBuf> {
        let Some(writer) = WRITERS.iter().find(|writer| writer.program == name) else {
            return Vec::new();
        };
        let operands = operands(arguments);
        let words: Vec<&str> = match writer.writes {
            Written::Operands => operands.iter().map(|word| word.text.as_str()).collect(),
            Written::OperandsWith(option) if has_option(arguments, option) => {
                operands.iter().map(|word| word.text.as_str()).collect()
            }
            Written::OperandsWith(_) => Vec::new(),
            Written::Destination => match option_value(arguments, 't', "--target-directory") {
                Some(folder) => vec![folder],
                None => operands
                    .last()
                    .map(|word| word.text.as_str())
                    .into_iter()
                    .collect(),
            },
            Written::Value(prefix) => arguments
                .iter()
                .filter_map(|word| word.text.strip_prefix(prefix))
                .collect(),
        };
        words.into_iter().map(|text| self.path(text, cwd)).collect()
    }

    /// The files a downloader saves: `curl -o FILE`, `curl -O URL`, `wget -O FILE`, `wget URL`.
    fn saved_by(&self, name: &str, arguments: &[Word], cwd: &Path) -> Vec<PathBuf> {
        let urls = || {
            arguments
                .iter()
                .filter_map(|word| url_file_name(&word.text))
                .collect::<Vec<_>>()
        };
        let named: Vec<&str> = match name {
            "curl" => {
                let mut named: Vec<&str> = option_values(arguments, 'o', "--output");
                if has_option(arguments, "-O") || has_option(arguments, "--remote-name") {
                    named.extend(urls());
                }
                named
            }
            "wget" => match option_values(arguments, 'O', "--output-document").as_slice() {
                [] => urls(),
                named => named.to_vec(),
            },
            _ => Vec::new(),
        };
        named
            .into_iter()
            .filter(|file| *file != "-")
            .map(|file| self.path(file, cwd))
            .collect()
    }

    /// `rm` or `find -delete` (or `-exec rm`) on a path at or above home or `/`.
    fn deletes_at_or_above_home(&self, name: &str, arguments: &[Word], cwd: &Path) -> bool {
        match name {
            "rm" => operands(arguments)
                .iter()
                .any(|word| self.is_at_or_above_home(&self.path(&word.text, cwd))),
            "find" => {
                let deletes = arguments.iter().enumerate().any(|(at, word)| {
                    word.text == "-delete"
                        || (matches!(word.text.as_str(), "-exec" | "-execdir")
                            && arguments
                                .get(at + 1)
                                .is_some_and(|program| program_name(&program.text) == "rm"))
                });
                if !deletes {
                    return false;
                }
                let starts: Vec<&Word> = find_start_paths(arguments)
                    .iter()
                    .take_while(|word| {
                        !word.text.starts_with('-') && word.text != "(" && word.text != "!"
                    })
                    .collect();
                if starts.is_empty() {
                    return self.is_at_or_above_home(cwd);
                }
                starts
                    .iter()
                    .any(|word| self.is_at_or_above_home(&self.path(&word.text, cwd)))
            }
            _ => false,
        }
    }

    /// `/`, a folder at its top, home or a folder above it, or a glob of what is directly in
    /// one of them (`~/*`).
    fn is_at_or_above_home(&self, path: &Path) -> bool {
        let path = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) if name.chars().all(|character| "*?.[]!".contains(character)) => {
                path.parent().unwrap_or(path)
            }
            _ => path,
        };
        path.components().count() <= 2
            || self.lookup.home.is_some_and(|home| home.starts_with(path))
    }

    fn is_in(&self, path: &Path, place: &Place) -> bool {
        let is_in_clone = self.clone.is_some_and(|clone| path.starts_with(clone));
        match place {
            Place::Home(inside) => self
                .lookup
                .home
                .is_some_and(|home| path.starts_with(home.join(inside))),
            Place::Absolute(prefix) => path.starts_with(prefix),
            Place::NamedOutsideClone(name) => {
                !is_in_clone
                    && path
                        .file_name()
                        .and_then(|file| file.to_str())
                        .is_some_and(|file| file == *name || file.starts_with(&format!("{name}.")))
            }
            Place::InsideOutsideClone(folder) => {
                !is_in_clone && path.ancestors().any(|ancestor| ancestor.ends_with(folder))
            }
        }
    }

    /// The paths a word names: the word, and the value after an `=` (`--file=~/.netrc`).
    fn paths_named(&self, word: &Word, cwd: &Path) -> Vec<PathBuf> {
        let mut texts = vec![word.text.as_str()];
        if let Some((_, value)) = word.text.split_once('=') {
            texts.push(value);
        }
        texts
            .into_iter()
            .filter(|text| !text.is_empty())
            .map(|text| self.path(text, cwd))
            .collect()
    }

    /// `text` as an absolute path from `cwd`, `..` taken back by name; a leading `~`, `$HOME`
    /// or `${HOME}` is home, which a shell expands even where the lexer did not.
    fn path(&self, text: &str, cwd: &Path) -> PathBuf {
        let expanded = self.lookup.home.and_then(|home| {
            ["~", "$HOME", "${HOME}"].iter().find_map(|prefix| {
                let rest = text.strip_prefix(prefix)?;
                (rest.is_empty() || rest.starts_with('/'))
                    .then(|| format!("{}{rest}", home.display()))
            })
        });
        logical_directory(cwd, expanded.as_deref().unwrap_or(text))
    }
}

fn stdin_word(stdin: &Stdin) -> Option<&Word> {
    match stdin {
        Stdin::File(word) | Stdin::Text(word) => Some(word),
        Stdin::Terminal | Stdin::Pipe => None,
    }
}

fn is_source(source: &Source, name: &str, arguments: &[Word]) -> bool {
    source.programs.contains(&name)
        && (source.with_option.is_empty()
            || source.with_option.iter().any(|option| {
                if source.groups_letters {
                    has_option(arguments, option)
                } else {
                    has_exact_option(arguments, option)
                }
            }))
}

fn is_invocation(invocation: &Invocation, name: &str, arguments: &[Word]) -> bool {
    if invocation.program != name
        || invocation
            .unless
            .iter()
            .any(|option| has_option(arguments, option))
    {
        return false;
    }
    match invocation.operand {
        None => true,
        Some(operands_flagged) => operands(arguments)
            .first()
            .is_some_and(|operand| operands_flagged.contains(&operand.text.as_str())),
    }
}

/// The words that are not options; after `--`, every word.
fn operands(arguments: &[Word]) -> Vec<&Word> {
    let mut operands = Vec::new();
    let mut is_after_options = false;
    for word in arguments {
        if !is_after_options && word.text == "--" {
            is_after_options = true;
            continue;
        }
        if is_after_options || !word.text.starts_with('-') || word.text == "-" {
            operands.push(word);
        }
    }
    operands
}

/// Whether `option` is given before `--`: a long one exactly or with `=value`, a single
/// letter alone or among others behind one `-` (`-di`), another single-dash one exactly.
fn has_option(arguments: &[Word], option: &str) -> bool {
    arguments
        .iter()
        .take_while(|word| word.text != "--")
        .any(|word| {
            let text = word.text.as_str();
            if option.starts_with("--") {
                return text == option || text.starts_with(&format!("{option}="));
            }
            match option
                .strip_prefix('-')
                .filter(|letter| letter.chars().count() == 1)
            {
                Some(letter) => {
                    text.starts_with('-') && !text.starts_with("--") && text[1..].contains(letter)
                }
                None => text == option,
            }
        })
}

/// Whether `option` is given before `--` as a word of its own, for a program whose options
/// have one dash and several letters (`openssl enc -d`, not `-md`).
fn has_exact_option(arguments: &[Word], option: &str) -> bool {
    arguments
        .iter()
        .take_while(|word| word.text != "--")
        .any(|word| word.text == option)
}

/// The value of the short option `letter` (`-t DIR`, `-tDIR`) or `long` (`--long DIR`,
/// `--long=DIR`), the first one given.
fn option_value<'w>(arguments: &'w [Word], letter: char, long: &str) -> Option<&'w str> {
    option_values(arguments, letter, long).into_iter().next()
}

/// Every value of the short option `letter`, alone or last among others behind one `-`
/// (`-fsSLo FILE`), or attached (`-oFILE`), and of `long`.
fn option_values<'w>(arguments: &'w [Word], letter: char, long: &str) -> Vec<&'w str> {
    let mut values = Vec::new();
    let mut words = arguments.iter().take_while(|word| word.text != "--");
    while let Some(word) = words.next() {
        let text = word.text.as_str();
        if text == long {
            values.extend(words.next().map(|value| value.text.as_str()));
        } else if let Some(value) = text
            .strip_prefix(long)
            .and_then(|rest| rest.strip_prefix('='))
        {
            values.push(value);
        } else if let Some(at) = text
            .strip_prefix('-')
            .filter(|letters| !letters.starts_with('-'))
            .and_then(|letters| letters.find(letter))
        {
            let rest = &text[1 + at + letter.len_utf8()..];
            if rest.is_empty() {
                values.extend(words.next().map(|value| value.text.as_str()));
            } else {
                values.push(rest);
            }
        }
    }
    values
}

/// The file name a URL's path ends in, as a downloader saves it; `None` for another word.
fn url_file_name(word: &str) -> Option<&str> {
    let (_, rest) = word.split_once("://")?;
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let (_, name) = path.rsplit_once('/')?;
    (!name.is_empty()).then_some(name)
}

/// `chmod`'s mode sets the setuid or setgid bit: `u+s`, `g=s`, `+s`, `4755`, `2755`.
/// Whether `chmod`'s mode, its first operand, sets the setuid or setgid bit.
fn sets_setuid(arguments: &[Word]) -> bool {
    let Some(mode) = chmod_mode(arguments) else {
        return false;
    };
    is_octal_setuid(&mode.text) || mode.text.split(',').any(is_symbolic_setuid)
}

/// `chmod`'s mode: its first word that is not an option, a mode taking away (`-s`) included,
/// or none with `--reference`, which takes the mode from a file.
fn chmod_mode(arguments: &[Word]) -> Option<&Word> {
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        let text = word.text.as_str();
        if text == "--" {
            return words.next();
        }
        if text.starts_with("--reference") {
            return None;
        }
        let is_option = text.starts_with("--")
            || (text.len() > 1
                && text.starts_with('-')
                && text[1..].chars().all(|option| "cfvR".contains(option)));
        if !is_option {
            return Some(word);
        }
    }
    None
}

/// `find`'s words after its leading options (`-H`, `-L`, `-P`, `-D debug`, `-O3`), where its
/// start paths begin.
fn find_start_paths(arguments: &[Word]) -> &[Word] {
    let mut at = 0;
    while let Some(word) = arguments.get(at) {
        let Some(option) = word.text.strip_prefix('-') else {
            break;
        };
        match option {
            "H" | "L" | "P" => at += 1,
            "D" => at += 2,
            level if level.starts_with('O') => at += 1,
            _ => break,
        }
    }
    arguments.get(at..).unwrap_or_default()
}

fn is_octal_setuid(text: &str) -> bool {
    text.len() >= 4
        && text.bytes().all(|byte| (b'0'..=b'7').contains(&byte))
        && (text.as_bytes()[text.len() - 4] - b'0') & 0o6 != 0
}

fn is_symbolic_setuid(clause: &str) -> bool {
    let actions = clause.trim_start_matches(['u', 'g', 'o', 'a']);
    if !actions.starts_with(['+', '-', '=']) {
        return false;
    }
    let mut adds = false;
    let mut sets = false;
    for character in actions.chars() {
        match character {
            '+' | '=' => adds = true,
            '-' => adds = false,
            's' => sets |= adds,
            'r' | 'w' | 'x' | 'X' | 't' | 'u' | 'g' | 'o' => {}
            _ => return false,
        }
    }
    sets
}

/// `chown`'s owner is root: `root`, `root:group`, `0`.
fn gives_to_root(arguments: &[Word]) -> bool {
    operands(arguments).first().is_some_and(|owner| {
        let user = owner.text.split([':', '.']).next().unwrap_or("");
        matches!(user, "root" | "0")
    })
}

/// The text of the script at `path` when it is one: `Ok(None)` when absent or not text,
/// `Err` when it cannot be read whole.
fn read_script(path: &Path) -> Result<Option<String>, ()> {
    let Some(file) = open_regular(path).map_err(|_| ())? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(SCRIPT_BYTES_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    // A program is not a script whatever its size: a large one must not read as "can't tell".
    if bytes.contains(&0) {
        return Ok(None);
    }
    if bytes.len() as u64 > SCRIPT_BYTES_MAX {
        return Err(());
    }
    Ok(String::from_utf8(bytes).ok())
}

/// Whether a shell reads a file run as a program: it has no `#!` line, or its line names a
/// shell, directly or through `env`.
fn is_shell_script(text: &str) -> bool {
    let Some(interpreter_line) = text.lines().next().and_then(|line| line.strip_prefix("#!"))
    else {
        return true;
    };
    let mut words = interpreter_line.split_whitespace();
    let Some(program) = words.next() else {
        return true;
    };
    match program_name(program) {
        "env" => words
            .find(|word| !word.starts_with('-'))
            .is_some_and(|program| is_shell(program_name(program))),
        name => is_shell(name),
    }
}

const STORED_VERSION: &str = "v1";

/// The stored form: a first line `v1`, then one line per finding, its rule, severity, file,
/// line, reason and excerpt separated by tabs, each with `\`, tab and line break escaped.
pub(crate) fn encode(findings: &[Finding]) -> String {
    let mut text = STORED_VERSION.to_owned();
    for finding in findings {
        let file = finding
            .file
            .as_ref()
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = finding
            .line
            .map(|line| line.to_string())
            .unwrap_or_default();
        let fields = [
            finding.rule.as_str(),
            finding.severity.as_str(),
            &file,
            &line,
            &finding.reason,
            &finding.excerpt,
        ];
        text.push('\n');
        text.push_str(
            &fields
                .iter()
                .map(|field| escape(field))
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    text
}

/// The stored form read back; `None` when it is not one [`encode`] writes.
pub(crate) fn decode(text: &str) -> Option<Vec<Finding>> {
    let mut lines = text.split('\n');
    if lines.next()? != STORED_VERSION {
        return None;
    }
    lines
        .map(|line| {
            let fields = line.split('\t').map(unescape).collect::<Option<Vec<_>>>()?;
            let [rule, severity, file, line, reason, excerpt] =
                <[String; 6]>::try_from(fields).ok()?;
            let file = match file.as_str() {
                "" => None,
                path if path.starts_with('/') => Some(PathBuf::from(path)),
                _ => return None,
            };
            let line = match line.as_str() {
                "" => None,
                number => Some(number.parse().ok()?),
            };
            (!rule.is_empty()).then_some(Finding {
                rule,
                severity: Severity::parse(&severity)?,
                reason,
                excerpt,
                file,
                line,
            })
        })
        .collect()
}

fn escape(field: &str) -> String {
    field
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape(field: &str) -> Option<String> {
    let mut text = String::new();
    let mut characters = field.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            text.push(character);
            continue;
        }
        match characters.next()? {
            '\\' => text.push('\\'),
            't' => text.push('\t'),
            'n' => text.push('\n'),
            _ => return None,
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_form_reads_back() {
        let findings = vec![
            Finding {
                rule: "download-runs".to_owned(),
                severity: Severity::High,
                reason: "runs code it downloads".to_owned(),
                excerpt: "curl x\t| sh \\ y\nz".to_owned(),
                file: Some(PathBuf::from("/clone/x y.sh")),
                line: Some(12),
            },
            Finding {
                rule: "sudo-cannot-tell".to_owned(),
                severity: Severity::Warn,
                reason: "can't tell what runs as root".to_owned(),
                excerpt: String::new(),
                file: None,
                line: None,
            },
        ];
        assert_eq!(decode(&encode(&findings)), Some(findings));
        assert_eq!(decode(&encode(&[])), Some(Vec::new()));
        assert_eq!(decode("v2"), None);
        assert_eq!(decode("v1\nrule\tbad\t\t\treason\texcerpt"), None);
        assert_eq!(decode("v1\nrule\thigh\trelative\t\treason\texcerpt"), None);
        assert_eq!(decode("v1\nrule\thigh\t\t\treason"), None);
    }

    #[test]
    fn every_pattern_the_scan_raises_itself_has_one_rule() {
        for is in [
            (|pattern: &Pattern| matches!(pattern, Pattern::CannotTell)) as fn(&Pattern) -> bool,
            |pattern| matches!(pattern, Pattern::SudoRunsEditable),
            |pattern| matches!(pattern, Pattern::SudoCannotTell),
            |pattern| matches!(pattern, Pattern::RunsSavedDownload),
        ] {
            assert_eq!(rule_where(is).count(), 1);
        }
    }

    #[test]
    fn rule_ids_are_distinct() {
        let mut ids: Vec<&str> = RULES.iter().map(|rule| rule.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RULES.len());
    }
}
