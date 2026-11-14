//! The shell command lines and scripts the approval checks read (#1383, #1388): a lexer
//! that splits one into simple commands with their words, input and position, and the readers
//! of the programs that run others (`sudo`, `env`, a shell's `-c`, an interpreter's code).
//! It reads; it never runs or expands anything, and a word it cannot resolve without running
//! the shell is marked not literal.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use crate::hints::program_name;

/// `word` as an absolute path from `cwd`, as the kernel opens it: `..` is left for the
/// kernel to resolve through links, so the file hashed is the file run.
pub(crate) fn absolute(cwd: &Path, word: &str) -> PathBuf {
    cwd.join(word)
        .components()
        .filter(|component| *component != Component::CurDir)
        .collect()
}

/// The directory a shell's `cd` moves to: `..` resolved by name, as `cd` does without `-P`.
pub(crate) fn logical_directory(cwd: &Path, word: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for component in cwd.join(word).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                path.pop();
            }
            other => path.push(other),
        }
    }
    path
}

pub(crate) fn is_shell(name: &str) -> bool {
    matches!(
        name,
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "mksh" | "ash" | "yash" | "fish"
    )
}

/// Where a shell reads its code from.
pub(crate) enum ShellCode<'w> {
    /// `-c STRING`, which may be an expansion.
    CommandString(&'w Word),
    /// Its first operand, which may be an expansion.
    Script(&'w Word),
    /// Its input: no operand, `-s` or `-i`.
    Input,
    /// An expansion where its options are, or `-c` without its string.
    CannotTell,
}

pub(crate) struct ShellArguments<'w> {
    pub(crate) code: ShellCode<'w>,
    /// `--rcfile` and `--init-file`.
    pub(crate) start_files: Vec<&'w Word>,
}

pub(crate) fn shell_arguments(arguments: &[Word]) -> ShellArguments<'_> {
    let mut shell = ShellArguments {
        code: ShellCode::Input,
        start_files: Vec::new(),
    };
    let mut is_command_string = false;
    let mut reads_input = false;
    let mut words = arguments.iter();
    let operand = loop {
        let Some(word) = words.next() else {
            break None;
        };
        if !word.is_literal {
            // An expansion is no option the shell can be told to have: its operand.
            break Some(word);
        }
        let text = word.text.as_str();
        match text {
            "--" => break words.next(),
            "--rcfile" | "--init-file" => match words.next() {
                Some(file) if file.is_literal => shell.start_files.push(file),
                _ => {
                    shell.code = ShellCode::CannotTell;
                    return shell;
                }
            },
            "-o" | "+o" | "-O" | "+O" => {
                words.next();
            }
            _ if text.starts_with("--") => {}
            _ if text.len() > 1 && text.starts_with('-') => {
                is_command_string |= text.contains('c');
                reads_input |= text.contains('s') || text.contains('i');
            }
            _ if text.len() > 1 && text.starts_with('+') => {}
            _ => break Some(word),
        }
    };
    shell.code = match operand {
        Some(word) if is_command_string => ShellCode::CommandString(word),
        None if is_command_string => ShellCode::CannotTell,
        Some(word) if !reads_input => ShellCode::Script(word),
        Some(word) if !word.is_literal => ShellCode::CannotTell,
        _ => ShellCode::Input,
    };
    shell
}

pub(crate) const SHELL_BUILTINS: &[&str] = &[
    ":", "true", "false", "echo", "printf", "test", "[", "[[", "exit", "return", "export", "unset",
    "set", "shift", "pwd", "read", "local", "declare", "typeset", "readonly", "alias", "unalias",
    "trap", "wait", "umask", "ulimit", "break", "continue", "let", "shopt", "getopts", "hash",
    "type", "jobs", "fg", "bg", "disown", "kill",
];

/// A non-shell interpreter's options: those that take inline code, those that run a module
/// found on a search path, and those that take another value.
pub(crate) struct Interpreter {
    pub(crate) inline_code: &'static [&'static str],
    pub(crate) runs_module: &'static [&'static str],
    pub(crate) takes_value: &'static [&'static str],
    /// Options whose value, if any, is the rest of their word (`perl -i.bak`).
    pub(crate) attached_value: &'static [&'static str],
}

pub(crate) fn interpreter(name: &str) -> Option<Interpreter> {
    let base =
        name.trim_end_matches(|character: char| character.is_ascii_digit() || character == '.');
    Some(match base {
        "python" | "pypy" => Interpreter {
            inline_code: &["-c"],
            runs_module: &["-m"],
            takes_value: &["-W", "-X", "--check-hash-based-pycs"],
            attached_value: &[],
        },
        "perl" => Interpreter {
            inline_code: &["-e", "-E"],
            runs_module: &[],
            takes_value: &["-I", "-M", "-m"],
            attached_value: &["-i", "-0", "-l"],
        },
        "ruby" => Interpreter {
            inline_code: &["-e"],
            runs_module: &[],
            takes_value: &["-I", "-r", "-C"],
            attached_value: &["-i", "-0"],
        },
        "node" | "nodejs" => Interpreter {
            inline_code: &["-e", "--eval", "-p", "--print"],
            runs_module: &[],
            takes_value: &["-r", "--require", "--import", "--loader"],
            attached_value: &[],
        },
        "php" => Interpreter {
            inline_code: &["-r"],
            runs_module: &[],
            takes_value: &["-c", "-d", "-z"],
            attached_value: &[],
        },
        "lua" | "luajit" => Interpreter {
            inline_code: &["-e"],
            runs_module: &["-l"],
            takes_value: &[],
            attached_value: &[],
        },
        "Rscript" => Interpreter {
            inline_code: &["-e"],
            runs_module: &[],
            takes_value: &[],
            attached_value: &[],
        },
        "tclsh" => Interpreter {
            inline_code: &[],
            runs_module: &[],
            takes_value: &[],
            attached_value: &[],
        },
        _ => return None,
    })
}

/// Where a non-shell interpreter reads its code from.
pub(crate) enum InterpreterCode<'w> {
    /// Inline code (`-c`, `-e`, …), which may be an expansion.
    Inline(Word),
    /// Its script operand, which may be an expansion after `--`.
    Script(&'w Word),
    /// Its input: no script, or `-`.
    Input,
    /// A module from a search path (`python3 -m`), or an option that is an expansion.
    CannotTell,
}

/// Where `interpreter`, given `arguments`, reads its code from.
pub(crate) fn interpreter_code<'w>(
    interpreter: &Interpreter,
    arguments: &'w [Word],
) -> InterpreterCode<'w> {
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        if !word.is_literal {
            return InterpreterCode::CannotTell;
        }
        let text = word.text.as_str();
        if text == "--" {
            break;
        }
        if text == "-" {
            return InterpreterCode::Input;
        }
        if interpreter.runs_module.contains(&text) {
            return InterpreterCode::CannotTell;
        }
        if interpreter.inline_code.contains(&text) {
            return match words.next() {
                Some(code) => InterpreterCode::Inline(code.clone()),
                None => InterpreterCode::CannotTell,
            };
        }
        if interpreter.takes_value.contains(&text) {
            words.next();
            continue;
        }
        if text.len() > 2 && text.starts_with('-') && !text.starts_with("--") {
            match grouped_flags(interpreter, &text[1..]) {
                Grouped::Flags => continue,
                Grouped::TakesNext => {
                    words.next();
                    continue;
                }
                Grouped::Code(Some(code)) => return InterpreterCode::Inline(word.with_text(code)),
                Grouped::Code(None) => {
                    return match words.next() {
                        Some(code) => InterpreterCode::Inline(code.clone()),
                        None => InterpreterCode::CannotTell,
                    };
                }
                Grouped::Module => return InterpreterCode::CannotTell,
            }
        }
        if text.starts_with('-') {
            continue;
        }
        return InterpreterCode::Script(word);
    }
    match words.next() {
        Some(script) => InterpreterCode::Script(script),
        None => InterpreterCode::Input,
    }
}

/// What a word of single-letter options grouped behind one `-` (`perl -ne`, `python3 -Bc`)
/// holds, its letters read in turn as the interpreter's own flags.
enum Grouped<'w> {
    /// Flags alone.
    Flags,
    /// The last letter takes the next word as its value.
    TakesNext,
    /// A letter takes inline code: the rest of the word, else (`None`) the next word.
    Code(Option<&'w str>),
    /// A letter runs a module from a search path.
    Module,
}

fn grouped_flags<'w>(interpreter: &Interpreter, letters: &'w str) -> Grouped<'w> {
    let is_flag = |flags: &[&str], letter: char| flags.contains(&format!("-{letter}").as_str());
    for (index, letter) in letters.char_indices() {
        let rest = &letters[index + letter.len_utf8()..];
        let is = |flags: &[&str]| is_flag(flags, letter);
        if is(interpreter.inline_code) {
            // `node -pe CODE`: what follows is more code flags, and the code is the next word.
            let takes_next = rest
                .chars()
                .all(|letter| is_flag(interpreter.inline_code, letter));
            return Grouped::Code((!takes_next).then_some(rest));
        }
        if is(interpreter.attached_value) {
            return Grouped::Flags;
        }
        if is(interpreter.runs_module) {
            return Grouped::Module;
        }
        if is(interpreter.takes_value) {
            return if rest.is_empty() {
                Grouped::TakesNext
            } else {
                Grouped::Flags
            };
        }
    }
    Grouped::Flags
}

/// What `sudo`'s options say: where the command starts, whether it runs nothing (`-e`, `-l`,
/// `-v`, `-K`, `-V`, `-h`), starts a shell (`-s`, `-i`), or moves (`-D`). `None` for an
/// option that cannot be read (an expansion) or a root directory change (`-R`).
pub(crate) struct SudoOptions {
    pub(crate) command_at: usize,
    pub(crate) runs_nothing: bool,
    pub(crate) starts_shell: bool,
    pub(crate) chdir: Option<Word>,
}

pub(crate) fn sudo_options(arguments: &[Word]) -> Option<SudoOptions> {
    const SHORT_WITH_VALUE: &str = "ugpCDRTUrt";
    const LONG_WITH_VALUE: &[&str] = &[
        "--user",
        "--group",
        "--prompt",
        "--close-from",
        "--chdir",
        "--chroot",
        "--command-timeout",
        "--other-user",
        "--role",
        "--type",
        "--host",
    ];
    let mut options = SudoOptions {
        command_at: arguments.len(),
        runs_nothing: false,
        starts_shell: false,
        chdir: None,
    };
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        let text = word.text.as_str();
        if text == "--" {
            options.command_at = index + 1;
            return Some(options);
        }
        if !text.starts_with('-') || text == "-" {
            options.command_at = index;
            return Some(options);
        }
        if !word.is_literal {
            return None;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let name = format!("--{name}");
            let value = match (LONG_WITH_VALUE.contains(&name.as_str()), inline) {
                (true, Some(value)) => Some(word.with_text(value)),
                (true, None) => {
                    index += 1;
                    Some(arguments.get(index)?.clone())
                }
                (false, _) => None,
            };
            match name.as_str() {
                "--edit" | "--list" | "--validate" | "--remove-timestamp" | "--version"
                | "--help" => options.runs_nothing = true,
                "--shell" | "--login" => options.starts_shell = true,
                "--chdir" => options.chdir = value,
                "--chroot" => return None,
                _ => {}
            }
            index += 1;
            continue;
        }
        for (position, flag) in text[1..].char_indices() {
            match flag {
                'e' | 'l' | 'v' | 'K' | 'V' | 'h' => options.runs_nothing = true,
                's' | 'i' => options.starts_shell = true,
                'R' => return None,
                _ => {}
            }
            if SHORT_WITH_VALUE.contains(flag) {
                let attached = &text[1 + position + flag.len_utf8()..];
                let value = if attached.is_empty() {
                    index += 1;
                    arguments.get(index)?.clone()
                } else {
                    word.with_text(attached)
                };
                if flag == 'D' {
                    options.chdir = Some(value);
                }
                break;
            }
        }
        index += 1;
    }
    // Options and no command: `sudo -v`, `sudo -k`, or `sudo -s` for a shell.
    options.runs_nothing |= !options.starts_shell;
    Some(options)
}

/// The words of a command with `if`, `then`, `!`, `{` and the like before its program left
/// out; a `for`, `case` or `[[` header runs no program.
pub(crate) fn skip_keywords(words: &[Word]) -> &[Word] {
    let mut start = 0;
    while let Some(word) = words.get(start) {
        if !word.is_literal {
            break;
        }
        match word.text.as_str() {
            "!" | "{" | "}" | "if" | "then" | "else" | "elif" | "fi" | "do" | "done" | "while"
            | "until" | "esac" => start += 1,
            "for" | "case" | "select" | "[[" | "function" => return &[],
            _ => break,
        }
    }
    &words[start..]
}

/// The words after leading `NAME=value` assignments.
pub(crate) fn skip_assignments(words: &[Word]) -> &[Word] {
    let start = words
        .iter()
        .position(|word| !is_assignment(word))
        .unwrap_or(words.len());
    &words[start..]
}

fn is_assignment(word: &Word) -> bool {
    word.text.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            && !name.starts_with(|character: char| character.is_ascii_digit())
    })
}

/// A command with programs that run the next one left out (`env`, `nice`, `nohup`,
/// `timeout`, `setsid`, `stdbuf`, `ionice`, `time`, `command`, `exec`, `builtin`), and the
/// directory `env -C` moves it to.
pub(crate) struct Unwrapped<'w> {
    pub(crate) words: &'w [Word],
    pub(crate) cwd: PathBuf,
    pub(crate) cannot_tell: bool,
}

pub(crate) fn skip_wrappers<'w>(words: &'w [Word], cwd: &Path) -> Unwrapped<'w> {
    let mut unwrapped = Unwrapped {
        words: skip_assignments(words),
        cwd: cwd.to_path_buf(),
        cannot_tell: false,
    };
    loop {
        let Some((program, rest)) = unwrapped.words.split_first() else {
            return unwrapped;
        };
        if !program.is_literal {
            return unwrapped;
        }
        let after = match program_name(&program.text) {
            "env" => skip_env_options(rest, &mut unwrapped),
            "nice" => skip_options(rest, &["-n", "--adjustment"]),
            "nohup" | "builtin" => rest,
            "setsid" | "time" => skip_options(rest, &[]),
            "stdbuf" => skip_options(rest, &["-i", "-o", "-e"]),
            "ionice" => skip_options(rest, &["-c", "-n", "--class", "--classdata"]),
            "exec" => skip_options(rest, &["-a"]),
            "command" => {
                if rest
                    .first()
                    .is_some_and(|word| word.text.starts_with("-v") || word.text.starts_with("-V"))
                {
                    return Unwrapped {
                        words: &[],
                        ..unwrapped
                    };
                }
                skip_options(rest, &[])
            }
            "timeout" => {
                let after = skip_options(rest, &["-s", "-k", "--signal", "--kill-after"]);
                after.get(1..).unwrap_or_default()
            }
            _ => return unwrapped,
        };
        unwrapped.words = skip_assignments(after);
    }
}

/// `env`'s options: `-C` moves the command, `-S` splits a string into it, which is not read.
fn skip_env_options<'w>(words: &'w [Word], unwrapped: &mut Unwrapped<'_>) -> &'w [Word] {
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let text = word.text.as_str();
        match text {
            "--" => return &words[index + 1..],
            "-u" | "--unset" => index += 2,
            "-C" | "--chdir" => {
                match words.get(index + 1) {
                    Some(directory) if directory.is_literal => {
                        unwrapped.cwd = absolute(&unwrapped.cwd, &directory.text);
                    }
                    _ => unwrapped.cannot_tell = true,
                }
                index += 2;
            }
            _ if text.starts_with("-S") || text.starts_with("--split-string") => {
                unwrapped.cannot_tell = true;
                return &[];
            }
            _ if text.starts_with('-') && text.len() > 1 => index += 1,
            _ => return &words[index..],
        }
    }
    &[]
}

/// Words after the options of a wrapper; those in `with_value` take the next word.
fn skip_options<'w>(words: &'w [Word], with_value: &[&str]) -> &'w [Word] {
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let text = word.text.as_str();
        if text == "--" {
            return &words[index + 1..];
        }
        if !text.starts_with('-') || text.len() < 2 {
            break;
        }
        index += if with_value.contains(&text) { 2 } else { 1 };
    }
    words.get(index..).unwrap_or_default()
}

/// One word of a command as the shell passes it on, quotes removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    pub(crate) text: String,
    /// No expansion or glob: `text` is what the program gets.
    pub(crate) is_literal: bool,
    /// The code of each command or process substitution in it (`$(…)`, a backtick pair,
    /// `<(…)`, `>(…)`), which the shell runs to expand it.
    pub(crate) substitutions: Vec<String>,
}

impl Word {
    pub(crate) fn with_text(&self, text: &str) -> Word {
        Word {
            text: text.to_owned(),
            is_literal: self.is_literal,
            substitutions: self.substitutions.clone(),
        }
    }
}

/// Where a command's input comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stdin {
    /// The run's terminal.
    Terminal,
    /// The previous command of a pipeline.
    Pipe,
    /// `< FILE`.
    File(Word),
    /// A here-document or here-string.
    Text(Word),
}

/// One simple command: its words, its input, the files its output goes to and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Simple {
    pub(crate) words: Vec<Word>,
    pub(crate) stdin: Stdin,
    /// The files its output redirections write: `> FILE`, `>> FILE`, `>| FILE`, `&> FILE`.
    pub(crate) writes: Vec<Word>,
    /// Its characters in the script, from its first word or redirection to its last.
    pub(crate) span: Range<usize>,
    /// The line of the script it starts on, from 1.
    pub(crate) line: usize,
}

impl Simple {
    fn new(stdin: Stdin) -> Simple {
        Simple {
            words: Vec::new(),
            stdin,
            writes: Vec::new(),
            span: 0..0,
            line: 0,
        }
    }
}

/// The simple commands of a shell command line, in order; `None` when it does not lex (an
/// unterminated quote or expansion).
pub(crate) fn lex(script: &str, home: Option<&Path>) -> Option<Vec<Simple>> {
    let characters: Vec<char> = script.chars().collect();
    let line_starts = std::iter::once(0)
        .chain(
            characters
                .iter()
                .enumerate()
                .filter(|(_, character)| **character == '\n')
                .map(|(at, _)| at + 1),
        )
        .collect();
    Lexer {
        characters,
        line_starts,
        position: 0,
        home,
        commands: Vec::new(),
        current: Simple::new(Stdin::Terminal),
        current_start: None,
        pending_heredocs: Vec::new(),
    }
    .run()
}

struct PendingHeredoc {
    delimiter: String,
    strips_tabs: bool,
    expands: bool,
    /// Whether this command reads it as its input.
    is_stdin: bool,
    command: usize,
}

struct Lexer<'h> {
    characters: Vec<char>,
    /// Where each line starts.
    line_starts: Vec<usize>,
    position: usize,
    home: Option<&'h Path>,
    commands: Vec<Simple>,
    current: Simple,
    /// Where the current command's first word or redirection starts.
    current_start: Option<usize>,
    pending_heredocs: Vec<PendingHeredoc>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Redirect {
    Input,
    Heredoc { strips_tabs: bool },
    HereString,
    Output,
    Other,
}

impl Lexer<'_> {
    fn peek(&self, offset: usize) -> Option<char> {
        self.characters.get(self.position + offset).copied()
    }

    fn run(mut self) -> Option<Vec<Simple>> {
        while let Some(character) = self.peek(0) {
            match character {
                ' ' | '\t' => self.position += 1,
                '\\' if self.peek(1) == Some('\n') => self.position += 2,
                '\n' => {
                    self.position += 1;
                    self.end_command(Stdin::Terminal);
                    self.read_heredocs()?;
                }
                '#' => {
                    while self.peek(0).is_some_and(|next| next != '\n') {
                        self.position += 1;
                    }
                }
                ';' | '(' | ')' => {
                    self.position += 1;
                    self.end_command(Stdin::Terminal);
                }
                '|' => {
                    self.position += 1;
                    if self.peek(0) == Some('|') {
                        self.position += 1;
                        self.end_command(Stdin::Terminal);
                    } else {
                        if self.peek(0) == Some('&') {
                            self.position += 1;
                        }
                        self.end_command(Stdin::Pipe);
                    }
                }
                '&' if self.peek(1) == Some('>') => {
                    let start = self.position;
                    self.position += 2;
                    if self.peek(0) == Some('>') {
                        self.position += 1;
                    }
                    self.redirect_target(Redirect::Output)?;
                    self.mark(start);
                }
                '&' => {
                    self.position += 1;
                    if self.peek(0) == Some('&') {
                        self.position += 1;
                    }
                    self.end_command(Stdin::Terminal);
                }
                '<' | '>' => {
                    let start = self.position;
                    self.redirect()?;
                    self.mark(start);
                }
                _ => {
                    let start = self.position;
                    let word = self.word()?;
                    self.mark(start);
                    let is_fd = word.is_literal
                        && !word.text.is_empty()
                        && word.text.bytes().all(|byte| byte.is_ascii_digit())
                        && matches!(self.peek(0), Some('<' | '>'));
                    if !is_fd {
                        self.current.words.push(word);
                    }
                }
            }
        }
        if !self.pending_heredocs.is_empty() {
            return None;
        }
        self.end_command(Stdin::Terminal);
        Some(self.commands)
    }

    /// The current command reaches from `start`, or where it began, to here.
    fn mark(&mut self, start: usize) {
        let start = *self.current_start.get_or_insert(start);
        self.current.span = start..self.position;
    }

    fn end_command(&mut self, next_stdin: Stdin) {
        let mut current = std::mem::replace(&mut self.current, Simple::new(next_stdin));
        if let Some(start) = self.current_start.take() {
            current.line = self.line_starts.partition_point(|line| *line <= start);
        }
        if !current.words.is_empty() {
            self.commands.push(current);
        }
    }

    fn redirect(&mut self) -> Option<()> {
        let first = self.peek(0)?;
        self.position += 1;
        let kind = match (first, self.peek(0)) {
            ('<', Some('(')) | ('>', Some('(')) => {
                // A process substitution is a word the shell expands.
                self.position -= 1;
                let word = self.word()?;
                self.current.words.push(word);
                return Some(());
            }
            ('<', Some('<')) => {
                self.position += 1;
                match self.peek(0) {
                    Some('<') => {
                        self.position += 1;
                        Redirect::HereString
                    }
                    Some('-') => {
                        self.position += 1;
                        Redirect::Heredoc { strips_tabs: true }
                    }
                    _ => Redirect::Heredoc { strips_tabs: false },
                }
            }
            ('<', Some('&')) => {
                self.position += 1;
                Redirect::Other
            }
            ('>', Some('&')) => {
                // `>&2` moves a descriptor; `>& FILE` writes the file.
                self.position += 1;
                let rest: String = self.characters[self.position..]
                    .iter()
                    .skip_while(|character| matches!(character, ' ' | '\t'))
                    .take_while(|character| {
                        !matches!(character, ' ' | '\t' | '\n' | ';' | '&' | '|')
                    })
                    .collect();
                if rest == "-" || rest.chars().all(|character| character.is_ascii_digit()) {
                    Redirect::Other
                } else {
                    Redirect::Output
                }
            }
            ('>', Some('>')) | ('>', Some('|')) => {
                self.position += 1;
                Redirect::Output
            }
            ('<', Some('>')) => {
                self.position += 1;
                Redirect::Input
            }
            ('<', _) => Redirect::Input,
            _ => Redirect::Output,
        };
        self.redirect_target(kind)
    }

    fn redirect_target(&mut self, kind: Redirect) -> Option<()> {
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.position += 1;
        }
        let start = self.position;
        let target = self.word()?;
        match kind {
            Redirect::Input => self.current.stdin = Stdin::File(target),
            Redirect::HereString => self.current.stdin = Stdin::Text(target),
            Redirect::Heredoc { strips_tabs } => {
                let is_quoted = self.characters[start..self.position]
                    .iter()
                    .any(|character| matches!(character, '\'' | '"' | '\\'));
                // The last here-document of a command is its input.
                let command = self.commands.len();
                for earlier in &mut self.pending_heredocs {
                    earlier.is_stdin &= earlier.command != command;
                }
                self.pending_heredocs.push(PendingHeredoc {
                    delimiter: target.text,
                    strips_tabs,
                    expands: !is_quoted,
                    is_stdin: true,
                    command,
                });
            }
            Redirect::Output => self.current.writes.push(target),
            Redirect::Other => {}
        }
        Some(())
    }

    /// Reads the bodies of the here-documents the line just ended opened, in order.
    fn read_heredocs(&mut self) -> Option<()> {
        for heredoc in std::mem::take(&mut self.pending_heredocs) {
            let mut body = String::new();
            loop {
                let start = self.position;
                if start >= self.characters.len() {
                    return None;
                }
                while self.peek(0).is_some_and(|next| next != '\n') {
                    self.position += 1;
                }
                let line: String = self.characters[start..self.position].iter().collect();
                self.position += 1;
                let line = if heredoc.strips_tabs {
                    line.trim_start_matches('\t').to_owned()
                } else {
                    line
                };
                if line == heredoc.delimiter {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if !heredoc.is_stdin {
                continue;
            }
            let is_literal = !(heredoc.expands && (body.contains('$') || body.contains('`')));
            let text = Stdin::Text(Word {
                text: body,
                is_literal,
                substitutions: Vec::new(),
            });
            match self.commands.get_mut(heredoc.command) {
                Some(command) => command.stdin = text,
                None => self.current.stdin = text,
            }
        }
        Some(())
    }

    /// One word up to a blank or an operator, quotes removed; an expansion or a glob in it
    /// makes it not literal.
    fn word(&mut self) -> Option<Word> {
        let mut text = String::new();
        let mut is_literal = true;
        let mut substitutions = Vec::new();
        let start = self.position;
        while let Some(character) = self.peek(0) {
            match character {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | ')' => break,
                '(' if self.position == start => break,
                '<' | '>' if self.peek(1) != Some('(') => break,
                '<' | '>' => {
                    is_literal = false;
                    self.position += 1;
                    substitutions.push(self.substitution()?);
                }
                '\\' => {
                    self.position += 1;
                    match self.peek(0) {
                        Some('\n') => {}
                        Some(escaped) => text.push(escaped),
                        None => {}
                    }
                    self.position += 1;
                }
                '\'' => {
                    self.position += 1;
                    loop {
                        match self.peek(0)? {
                            '\'' => break,
                            quoted => text.push(quoted),
                        }
                        self.position += 1;
                    }
                    self.position += 1;
                }
                '"' => {
                    self.position += 1;
                    loop {
                        match self.peek(0)? {
                            '"' => break,
                            '\\' if matches!(self.peek(1), Some('$' | '`' | '"' | '\\' | '\n')) => {
                                if let Some(escaped) = self.peek(1).filter(|next| *next != '\n') {
                                    text.push(escaped);
                                }
                                self.position += 2;
                            }
                            '$' | '`' => {
                                is_literal = false;
                                self.expansion(&mut text, &mut substitutions)?;
                            }
                            quoted => {
                                text.push(quoted);
                                self.position += 1;
                            }
                        }
                    }
                    self.position += 1;
                }
                '$' | '`' => {
                    is_literal = false;
                    self.expansion(&mut text, &mut substitutions)?;
                }
                '*' | '?' | '[' => {
                    is_literal = false;
                    text.push(character);
                    self.position += 1;
                }
                '~' if self.position == start => {
                    self.position += 1;
                    match (self.peek(0), self.home) {
                        (None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|'), Some(home)) => {
                            text.push_str(&home.to_string_lossy());
                        }
                        _ => {
                            is_literal = false;
                            text.push('~');
                        }
                    }
                }
                other => {
                    text.push(other);
                    self.position += 1;
                }
            }
        }
        Some(Word {
            text,
            is_literal,
            substitutions,
        })
    }

    /// The code between a `(` here and its matching `)`, which it skips.
    fn substitution(&mut self) -> Option<String> {
        let open = self.position;
        self.balanced('(', ')')?;
        Some(
            self.characters[open + 1..self.position - 1]
                .iter()
                .collect(),
        )
    }

    /// Skips an expansion at `$` or a backtick, keeping its text for display and the code of
    /// a command substitution.
    fn expansion(&mut self, text: &mut String, substitutions: &mut Vec<String>) -> Option<()> {
        let start = self.position;
        match (self.peek(0)?, self.peek(1)) {
            ('`', _) => {
                self.position += 1;
                loop {
                    match self.peek(0)? {
                        '`' => break,
                        '\\' => self.position += 2,
                        _ => self.position += 1,
                    }
                }
                self.position += 1;
                substitutions.push(
                    self.characters[start + 1..self.position - 1]
                        .iter()
                        .collect(),
                );
            }
            ('$', Some('(')) => {
                self.position += 1;
                substitutions.push(self.substitution()?);
            }
            ('$', Some('{')) => {
                self.position += 1;
                self.balanced('{', '}')?;
            }
            ('$', Some('\'')) => {
                self.position += 2;
                loop {
                    match self.peek(0)? {
                        '\'' => break,
                        '\\' => self.position += 2,
                        _ => self.position += 1,
                    }
                }
                self.position += 1;
            }
            ('$', Some(next)) if next.is_alphanumeric() || next == '_' => {
                self.position += 1;
                while self
                    .peek(0)
                    .is_some_and(|next| next.is_alphanumeric() || next == '_')
                {
                    self.position += 1;
                }
            }
            ('$', Some(next)) if "@*#?$!-".contains(next) => self.position += 2,
            _ => self.position += 1,
        }
        text.extend(&self.characters[start..self.position]);
        Some(())
    }

    /// Skips from an `open` to its matching `close`, quotes inside respected.
    fn balanced(&mut self, open: char, close: char) -> Option<()> {
        let mut depth = 0usize;
        loop {
            let character = self.peek(0)?;
            self.position += 1;
            match character {
                '\\' => self.position += 1,
                '\'' => {
                    while self.peek(0)? != '\'' {
                        self.position += 1;
                    }
                    self.position += 1;
                }
                '"' => loop {
                    match self.peek(0)? {
                        '"' => {
                            self.position += 1;
                            break;
                        }
                        '\\' => self.position += 2,
                        _ => self.position += 1,
                    }
                },
                _ if character == open => depth += 1,
                _ if character == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(());
                    }
                }
                _ => {}
            }
        }
    }
}

/// What a command does to the directory the commands after it run in.
pub(crate) enum DirectoryChange {
    /// It is not `cd`, `pushd` or `popd`.
    None,
    /// `cd` moves them here.
    To(PathBuf),
    /// Where it moves them cannot be told without running it: an expansion, `cd -`, `pushd`,
    /// `popd`, or `cd` alone without a home.
    CannotTell,
}

/// Where `command` moves the commands after it, from `cwd`.
pub(crate) fn directory_change(
    command: &Simple,
    cwd: &Path,
    home: Option<&Path>,
) -> DirectoryChange {
    let words = skip_assignments(skip_keywords(&command.words));
    let Some((program, rest)) = words.split_first() else {
        return DirectoryChange::None;
    };
    if !program.is_literal {
        return DirectoryChange::None;
    }
    match program.text.as_str() {
        "cd" => {}
        "pushd" | "popd" => return DirectoryChange::CannotTell,
        _ => return DirectoryChange::None,
    }
    // `cd`'s own options, grouped or not: `-P` follows links, `-L` (the default) does not,
    // the last one given wins; `-e` and `-@` change neither.
    let mut is_physical = false;
    let mut operands = rest.iter();
    let directory = loop {
        match operands.next() {
            Some(word) if word.is_literal && word.text == "--" => break operands.next(),
            Some(word)
                if word.is_literal
                    && word.text.len() > 1
                    && word.text.starts_with('-')
                    && word.text[1..].chars().all(|option| "LPe@".contains(option)) =>
            {
                if let Some(last) = word
                    .text
                    .chars()
                    .rev()
                    .find(|option| "LP".contains(*option))
                {
                    is_physical = last == 'P';
                }
            }
            other => break other,
        }
    };
    match (directory, home) {
        (Some(directory), _) if directory.is_literal && directory.text != "-" => {
            DirectoryChange::To(if is_physical {
                let path = absolute(cwd, &directory.text);
                std::fs::canonicalize(&path).unwrap_or(path)
            } else {
                logical_directory(cwd, &directory.text)
            })
        }
        (None, Some(home)) => DirectoryChange::To(home.to_path_buf()),
        _ => DirectoryChange::CannotTell,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(text: &str) -> Word {
        Word {
            text: text.to_owned(),
            is_literal: true,
            substitutions: Vec::new(),
        }
    }

    #[test]
    fn the_lexer_reads_quotes_operators_and_input() {
        let commands = lex("a 'b c' \"d$X\" < in | sudo sh -c 'x; y' && z", None).unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(
            commands[0].words,
            [
                literal("a"),
                literal("b c"),
                Word {
                    text: "d$X".to_owned(),
                    is_literal: false,
                    substitutions: Vec::new(),
                }
            ]
        );
        assert_eq!(commands[0].stdin, Stdin::File(literal("in")));
        assert_eq!(commands[1].stdin, Stdin::Pipe);
        assert_eq!(commands[1].words[3], literal("x; y"));
        assert_eq!(commands[2].words, [literal("z")]);
    }

    #[test]
    fn a_heredoc_is_the_input_of_its_command() {
        let commands = lex("sudo bash <<'EOF'\n./x.sh $Y\nEOF\necho done", None).unwrap();
        assert_eq!(commands[0].stdin, Stdin::Text(literal("./x.sh $Y\n")));
        assert_eq!(commands[1].words, [literal("echo"), literal("done")]);
    }

    #[test]
    fn a_command_knows_its_characters_line_and_output_files() {
        let script = "a > out 2>&1\n  b >> log &> both >&2 >& file\nc";
        let commands = lex(script, None).unwrap();
        let text = |command: &Simple| -> String {
            script.chars().collect::<Vec<_>>()[command.span.clone()]
                .iter()
                .collect()
        };
        assert_eq!(text(&commands[0]), "a > out 2>&1");
        assert_eq!(commands[0].writes, [literal("out")]);
        assert_eq!(commands[1].line, 2);
        assert_eq!(text(&commands[1]), "b >> log &> both >&2 >& file");
        assert_eq!(
            commands[1].writes,
            [literal("log"), literal("both"), literal("file")]
        );
        assert_eq!(commands[2].line, 3);
    }

    #[test]
    fn a_word_keeps_the_code_of_its_substitutions() {
        let commands = lex("sh -c \"$(curl x | sh)\" `id` <(wget y) > >(tee z)", None).unwrap();
        let words = &commands[0].words;
        assert_eq!(words[2].substitutions, ["curl x | sh"]);
        assert_eq!(words[3].substitutions, ["id"]);
        assert_eq!(words[4].substitutions, ["wget y"]);
        assert_eq!(commands[0].writes[0].substitutions, ["tee z"]);
    }

    #[test]
    fn an_unterminated_quote_does_not_lex() {
        assert_eq!(lex("sudo sh -c 'x", None), None);
    }
}
