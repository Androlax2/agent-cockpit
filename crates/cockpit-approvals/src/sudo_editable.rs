//! "Runs a file this session can edit, as root" (#1383): an approved `sudo ./setup.sh`
//! grants whatever the agent put in `setup.sh`, while `sudo systemctl start foo` grants what
//! it says. Before the owner reads a request, its command line is parsed for what sudo runs:
//! the program, a script handed to an interpreter (`sh`, `bash`, `python3`, `node`, …), the
//! makefile of `make` and the justfile of `just`, and every command of a shell's `-c` string,
//! which all run as root. A file among them that the asking session can change is recorded
//! with its SHA-256, and a run whose file no longer has that hash is refused. A command line
//! whose root part cannot be resolved without running it (a variable or command expansion, a
//! glob, `eval`, code read from a pipe) is recorded as "can't tell what runs as root", never
//! as clean. It warns; the owner decides. What the code does is not judged.
//!
//! What the session can change: a sandboxed session, the folders its sandbox lets it write;
//! an unsandboxed one, anything the owner's user could write, read as any file or folder on
//! the path not owned by root, or owned by root and writable by its group or others.
//!
//! What it misses: a bare program is looked up on the daemon's `PATH`, which is the run's
//! (sudoers' `secure_path` may differ), and a program created later in an earlier `PATH`
//! folder is not looked for; files a script or makefile runs in turn, and what a non-shell
//! interpreter's code names indirectly, are not followed; nor are commands sudo reaches as an
//! argument of another program (`xargs sudo`, `find -exec sudo`), only reported as "can't
//! tell". A command naming no `sudo` word is never parsed. #1388 moves this check into the
//! rule scan's list.

use std::ffi::OsStr;
use std::io::{ErrorKind, Read as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::hints::program_name;

/// What a command runs as root that its asking session can change, as recorded for one
/// revision of its request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootRuns {
    /// Files sudo runs that the session can change, in the order the command names them.
    pub editable: Vec<RootFile>,
    /// Part of what runs as root cannot be resolved from the command line.
    pub cannot_tell: bool,
}

impl RootRuns {
    /// Neither an editable file nor a part that cannot be told.
    pub fn is_clean(&self) -> bool {
        self.editable.is_empty() && !self.cannot_tell
    }
}

/// One file sudo runs that the session can change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootFile {
    /// Absolute, as the command names it (not following links).
    pub path: PathBuf,
    /// Lower-case hex SHA-256 of its contents when recorded; `None` when it did not exist,
    /// which the session could still create.
    pub sha256: Option<String>,
}

/// Where the asking session can write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionWrites {
    /// The folders its sandbox binds read-write, as host paths.
    Folders(Vec<PathBuf>),
    /// It runs unsandboxed, as the owner's user.
    Unsandboxed,
}

impl SessionWrites {
    /// Whether the session can change what `path` names: the file, or the folder holding it.
    pub fn can_edit(&self, path: &Path) -> bool {
        match self {
            SessionWrites::Folders(folders) => {
                folders.iter().any(|folder| path.starts_with(folder))
            }
            SessionWrites::Unsandboxed => path.ancestors().any(owner_may_write),
        }
    }
}

/// An existing file or folder that is not root's alone: the owner's user may change it, or
/// replace what is inside it. A link's own mode means nothing, only its owner.
fn owner_may_write(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let is_open_to_others =
        !metadata.file_type().is_symlink() && metadata.permissions().mode() & 0o022 != 0;
    metadata.uid() != 0 || is_open_to_others
}

/// The environment a command's words are resolved in: the run's directory and `PATH`, and
/// the home `~` names.
#[derive(Debug, Clone, Copy)]
pub struct Lookup<'a> {
    pub cwd: &'a Path,
    pub search_path: Option<&'a OsStr>,
    pub home: Option<&'a Path>,
}

/// What `command` runs as root that a session writing `writes` can change, its files hashed
/// now. Reads the filesystem; a command naming no `sudo` word reads nothing and is clean.
pub fn root_runs(command: &str, lookup: Lookup<'_>, writes: &SessionWrites) -> RootRuns {
    if !command.contains("sudo") {
        return RootRuns::default();
    }
    let mut found = Found::default();
    Walk {
        lookup,
        found: &mut found,
    }
    .script(command, lookup.cwd, Context::Owner, 0);
    let mut runs = RootRuns {
        editable: Vec::new(),
        cannot_tell: found.cannot_tell,
    };
    for path in found.files {
        let resolved = std::fs::canonicalize(&path).ok();
        let is_editable = writes.can_edit(&path)
            || resolved
                .as_deref()
                .is_some_and(|resolved| writes.can_edit(resolved));
        if !is_editable {
            continue;
        }
        if path.to_str().is_none_or(|text| text.contains('\n')) {
            runs.cannot_tell = true;
            continue;
        }
        match file_sha256(&path) {
            Ok(sha256) => runs.editable.push(RootFile { path, sha256 }),
            Err(_) => runs.cannot_tell = true,
        }
    }
    runs
}

/// The first recorded file whose contents are no longer what was recorded: changed, removed,
/// created, or now unreadable.
pub fn changed_file(files: &[RootFile]) -> Option<&Path> {
    files
        .iter()
        .find(|file| Contents::read(&file.path) != Contents::recorded(file))
        .map(|file| file.path.as_path())
}

/// A recorded file's contents as one read found them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contents {
    /// Lower-case hex SHA-256.
    Hash(String),
    /// No file there.
    Absent,
    /// It could not be read: not a regular file, no permission, or too slow.
    Unreadable,
}

impl Contents {
    /// What `path` holds now.
    pub fn read(path: &Path) -> Contents {
        match file_sha256(path) {
            Ok(Some(sha256)) => Contents::Hash(sha256),
            Ok(None) => Contents::Absent,
            Err(_) => Contents::Unreadable,
        }
    }

    /// What `file` held when it was recorded.
    pub fn recorded(file: &RootFile) -> Contents {
        file.sha256.clone().map_or(Contents::Absent, Contents::Hash)
    }
}

/// The digest an owner approves a request with when its sudo runs files the session can
/// edit: the request's `digest`, bound to each of those files as the owner's view read them,
/// so an approval names the contents it was shown. `digest` itself when there are none.
pub fn digest_as_read(digest: &str, files: &[Contents]) -> String {
    if files.is_empty() {
        return digest.to_owned();
    }
    let mut hasher = Sha256::new();
    hasher.update(b"cockpit-approvals run as read v1\0");
    hasher.update(digest.as_bytes());
    for file in files {
        match file {
            Contents::Hash(sha256) => {
                hasher.update(b"\0hash ");
                hasher.update(sha256.as_bytes());
            }
            Contents::Absent => hasher.update(b"\0absent"),
            Contents::Unreadable => hasher.update(b"\0unreadable"),
        }
    }
    hex(&hasher.finalize())
}

/// Lower-case hex SHA-256 of the file `path` names (links followed); `None` when there is
/// none. Anything but a regular file is an error.
pub fn file_sha256(path: &Path) -> std::io::Result<Option<String>> {
    let Some(mut file) = open_regular(path)? else {
        return Ok(None);
    };
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(Some(hex(&hasher.finalize())))
}

/// `path` opened for reading when it is a regular file; `None` when there is none. Checked
/// before opening, which would wait for a writer on a FIFO, and again on what was opened,
/// which a device or a swapped-in link would otherwise make an endless read. A FIFO swapped
/// in between the two still blocks the open: the daemon bounds how long it waits.
fn open_regular(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let not_regular = || std::io::Error::new(ErrorKind::InvalidInput, "not a regular file");
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Err(not_regular()),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(not_regular());
    }
    Ok(Some(file))
}

/// The lines of a file the owner reads before deciding.
pub const FILE_HEAD_LINES: usize = 200;

/// A file as it is now, for the owner to read: its hash and its first lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNow {
    /// `None` when the file is not there.
    pub sha256: Option<String>,
    /// The first [`FILE_HEAD_LINES`] lines; `None` when absent, unreadable or not UTF-8 text.
    pub head: Option<String>,
    /// Lines after `head`.
    pub more_lines: u64,
}

/// The most of a file kept in memory to show its first lines; the rest is hashed and its
/// lines counted as it streams by.
const HEAD_BYTES_MAX: usize = 256 * 1024;

/// Reads `path` once: its hash, and its first lines when it is text (valid UTF-8 without a
/// NUL byte). Anything but a regular file is an error.
pub fn file_now(path: &Path) -> std::io::Result<FileNow> {
    let Some(mut file) = open_regular(path)? else {
        return Ok(FileNow {
            sha256: None,
            head: None,
            more_lines: 0,
        });
    };
    let mut hasher = Sha256::new();
    let mut kept = Vec::new();
    let mut is_text = true;
    let mut line_breaks: u64 = 0;
    let mut last_byte = None;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let chunk = &buffer[..read];
        hasher.update(chunk);
        is_text &= !chunk.contains(&0);
        line_breaks += chunk.iter().filter(|byte| **byte == b'\n').count() as u64;
        last_byte = chunk.last().copied();
        let room = HEAD_BYTES_MAX.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..room.min(read)]);
    }
    let sha256 = Some(hex(&hasher.finalize()));
    let all_lines = line_breaks + u64::from(last_byte.is_some_and(|byte| byte != b'\n'));
    let text = match std::str::from_utf8(&kept) {
        Ok(text) => Some(text),
        // A character cut at the end of what was kept is still text.
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&kept[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    };
    let Some(text) = text.filter(|_| is_text) else {
        return Ok(FileNow {
            sha256,
            head: None,
            more_lines: 0,
        });
    };
    let head: Vec<&str> = text.lines().take(FILE_HEAD_LINES).collect();
    Ok(FileNow {
        sha256,
        head: Some(head.join("\n")),
        more_lines: all_lines.saturating_sub(head.len() as u64),
    })
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const STORED_VERSION: &str = "v1";
const ABSENT: &str = "absent";

/// The stored form: a first line `v1 known` or `v1 cannot-tell`, then one line per file,
/// its hash (or `absent`) and its path. Paths with a line break are never recorded.
pub(crate) fn encode(runs: &RootRuns) -> String {
    let mut text = format!(
        "{STORED_VERSION} {}",
        if runs.cannot_tell {
            "cannot-tell"
        } else {
            "known"
        }
    );
    for file in &runs.editable {
        text.push('\n');
        text.push_str(file.sha256.as_deref().unwrap_or(ABSENT));
        text.push(' ');
        text.push_str(&file.path.to_string_lossy());
    }
    text
}

/// The stored form read back; `None` when it is not one [`encode`] writes.
pub(crate) fn decode(text: &str) -> Option<RootRuns> {
    let mut lines = text.split('\n');
    let cannot_tell = match lines.next()?.strip_prefix(STORED_VERSION)? {
        " known" => false,
        " cannot-tell" => true,
        _ => return None,
    };
    let editable = lines
        .map(|line| {
            let (sha256, path) = line.split_once(' ')?;
            let sha256 = match sha256 {
                ABSENT => None,
                hash if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
                    Some(hash.to_owned())
                }
                _ => return None,
            };
            let path = PathBuf::from(path);
            path.is_absolute().then_some(RootFile { path, sha256 })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(RootRuns {
        editable,
        cannot_tell,
    })
}

/// How deep `-c` strings and nested sudos are followed before the rest counts as unknown.
const DEPTH_MAX: usize = 8;

/// The files the command runs as root, before telling which the session can change.
#[derive(Debug, Default)]
struct Found {
    files: Vec<PathBuf>,
    cannot_tell: bool,
}

impl Found {
    fn file(&mut self, path: PathBuf) {
        if !self.files.contains(&path) {
            self.files.push(path);
        }
    }
}

/// Who runs the commands of a script being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// The owner's user: only a sudo in it runs something as root.
    Owner,
    /// Root: a shell sudo started, so every command counts.
    Root,
}

struct Walk<'a, 'f> {
    lookup: Lookup<'a>,
    found: &'f mut Found,
}

impl Walk<'_, '_> {
    /// Each command of `script`, read by a shell running as `context` in `cwd`.
    fn script(&mut self, script: &str, cwd: &Path, context: Context, depth: usize) {
        if depth > DEPTH_MAX {
            self.found.cannot_tell = true;
            return;
        }
        let Some(commands) = lex(script, self.lookup.home) else {
            self.found.cannot_tell = true;
            return;
        };
        let mut cwd = cwd.to_path_buf();
        for command in commands {
            if self.changed_directory(&command, &mut cwd) {
                continue;
            }
            match context {
                Context::Owner => self.owner_command(&command, &cwd, depth),
                Context::Root => self.root_shell_command(&command, &cwd, depth),
            }
        }
    }

    /// Whether `command` is a `cd`, which moves the commands after it to `cwd`; `pushd` and
    /// `popd` move it where the command line does not say.
    fn changed_directory(&mut self, command: &Simple, cwd: &mut PathBuf) -> bool {
        let words = skip_assignments(skip_keywords(&command.words));
        let Some((program, rest)) = words.split_first() else {
            return false;
        };
        if !program.is_literal {
            return false;
        }
        match program.text.as_str() {
            "cd" => {}
            "pushd" | "popd" => {
                self.found.cannot_tell = true;
                return true;
            }
            _ => return false,
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
        match (directory, self.lookup.home) {
            (Some(directory), _) if directory.is_literal && directory.text != "-" => {
                *cwd = if is_physical {
                    let path = absolute(cwd, &directory.text);
                    std::fs::canonicalize(&path).unwrap_or(path)
                } else {
                    logical_directory(cwd, &directory.text)
                };
            }
            (None, Some(home)) => *cwd = home.to_path_buf(),
            _ => self.found.cannot_tell = true,
        }
        true
    }

    /// A command the owner's user runs: what follows a `sudo` program runs as root. A `sudo`
    /// word anywhere else, or a program that is an expansion, may run sudo unseen.
    fn owner_command(&mut self, command: &Simple, cwd: &Path, depth: usize) {
        let words = skip_wrappers(skip_keywords(&command.words), cwd);
        let mentions_sudo = command.words.iter().any(|word| word.text.contains("sudo"));
        self.found.cannot_tell |= words.cannot_tell && mentions_sudo;
        let Some((program, rest)) = words.words.split_first() else {
            return;
        };
        if !program.is_literal {
            self.found.cannot_tell = true;
            return;
        }
        match program_name(&program.text) {
            "sudo" => self.sudo(rest, &command.stdin, &words.cwd, depth),
            "eval" => self.found.cannot_tell = true,
            name if is_shell(name) => match shell_arguments(rest).code {
                ShellCode::CommandString(script) => {
                    self.script(&script.text, &words.cwd, Context::Owner, depth + 1);
                }
                ShellCode::CannotTell => self.found.cannot_tell = true,
                ShellCode::Script(_) | ShellCode::Input => {}
            },
            _ => {
                if rest.iter().any(|word| program_name(&word.text) == "sudo") {
                    self.found.cannot_tell = true;
                }
            }
        }
    }

    /// `sudo`'s arguments: its options, then the command it runs as root.
    fn sudo(&mut self, arguments: &[Word], stdin: &Stdin, cwd: &Path, depth: usize) {
        if depth > DEPTH_MAX {
            self.found.cannot_tell = true;
            return;
        }
        let Some(options) = sudo_options(arguments) else {
            self.found.cannot_tell = true;
            return;
        };
        if options.runs_nothing {
            return;
        }
        let cwd = match &options.chdir {
            Some(directory) if !directory.is_literal => {
                self.found.cannot_tell = true;
                return;
            }
            Some(directory) => absolute(cwd, &directory.text),
            None => cwd.to_path_buf(),
        };
        let command = skip_assignments(&arguments[options.command_at..]);
        if command.is_empty() {
            // `sudo -s` or `sudo -i` alone: a root shell reading start-up files and the terminal.
            self.found.cannot_tell |= options.starts_shell;
            return;
        }
        self.root_command(command, stdin, &cwd, depth + 1);
    }

    /// A command of a shell running as root: its built-ins too.
    fn root_shell_command(&mut self, command: &Simple, cwd: &Path, depth: usize) {
        let words = skip_keywords(&command.words);
        let words = skip_assignments(words);
        let Some((program, rest)) = words.split_first() else {
            return;
        };
        if !program.is_literal {
            self.found.cannot_tell = true;
            return;
        }
        match program.text.as_str() {
            "eval" => self.found.cannot_tell = true,
            "source" | "." => match rest.first() {
                Some(file) if file.is_literal => match self.sourced(&file.text, cwd) {
                    Some(path) => self.found.file(path),
                    None => self.found.cannot_tell = true,
                },
                Some(_) => self.found.cannot_tell = true,
                None => {}
            },
            name if SHELL_BUILTINS.contains(&name) => {}
            _ => self.root_command(words, &command.stdin, cwd, depth),
        }
    }

    /// A command running as root: its program, and what that program reads as code.
    fn root_command(&mut self, words: &[Word], stdin: &Stdin, cwd: &Path, depth: usize) {
        let words = skip_wrappers(words, cwd);
        self.found.cannot_tell |= words.cannot_tell;
        let cwd = words.cwd.as_path();
        let Some((program, arguments)) = words.words.split_first() else {
            return;
        };
        if !program.is_literal {
            self.found.cannot_tell = true;
            return;
        }
        let name = program_name(&program.text);
        if name == "sudo" {
            self.sudo(arguments, stdin, cwd, depth);
            return;
        }
        match self.program(&program.text, cwd) {
            Some(path) => self.found.file(path),
            None => {
                self.found.cannot_tell = true;
                return;
            }
        }
        if is_shell(name) {
            self.shell_code(arguments, stdin, cwd, depth);
        } else if let Some(interpreter) = interpreter(name) {
            self.interpreter_code(&interpreter, arguments, stdin, cwd);
        } else if matches!(name, "make" | "gmake") {
            self.makefiles(arguments, cwd);
        } else if name == "just" {
            self.justfile(arguments, cwd);
        }
    }

    /// The file a program word runs: a path from `cwd`, or the first match on `PATH`.
    fn program(&self, word: &str, cwd: &Path) -> Option<PathBuf> {
        if word.contains('/') {
            return Some(absolute(cwd, word));
        }
        let search_path = self.lookup.search_path?;
        std::env::split_paths(search_path)
            .filter(|folder| folder.is_absolute())
            .map(|folder| folder.join(word))
            .find(|candidate| is_executable_file(candidate))
    }

    /// The file `source` or `.` reads: a path, else `PATH` then the directory, as bash does.
    fn sourced(&self, word: &str, cwd: &Path) -> Option<PathBuf> {
        if word.contains('/') {
            return Some(absolute(cwd, word));
        }
        self.lookup
            .search_path
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|folder| folder.is_absolute())
            .map(|folder| folder.join(word))
            .find(|candidate| candidate.is_file())
            .or_else(|| Some(cwd.join(word)))
    }

    /// A shell's code: its `-c` string, its script, its start-up files, or its input.
    fn shell_code(&mut self, arguments: &[Word], stdin: &Stdin, cwd: &Path, depth: usize) {
        let shell = shell_arguments(arguments);
        for file in shell.start_files {
            self.found.file(absolute(cwd, &file.text));
        }
        match shell.code {
            ShellCode::CommandString(script) => {
                self.script(&script.text, cwd, Context::Root, depth + 1);
            }
            ShellCode::Script(script) => self.found.file(absolute(cwd, &script.text)),
            ShellCode::CannotTell => self.found.cannot_tell = true,
            ShellCode::Input => match stdin {
                Stdin::File(file) if file.is_literal => self.found.file(absolute(cwd, &file.text)),
                Stdin::Text(text) if text.is_literal => {
                    self.script(&text.text, cwd, Context::Root, depth + 1);
                }
                _ => self.found.cannot_tell = true,
            },
        }
    }

    /// A non-shell interpreter's code: its script, the files its inline code names, or its
    /// input. A module run from the search path (`python3 -m`) cannot be told.
    fn interpreter_code(
        &mut self,
        interpreter: &Interpreter,
        arguments: &[Word],
        stdin: &Stdin,
        cwd: &Path,
    ) {
        let mut words = arguments.iter();
        while let Some(word) = words.next() {
            if !word.is_literal {
                self.found.cannot_tell = true;
                return;
            }
            let text = word.text.as_str();
            if text == "--" {
                break;
            }
            if text == "-" {
                return self.interpreter_input(stdin, cwd);
            }
            if interpreter.runs_module.contains(&text) {
                self.found.cannot_tell = true;
                return;
            }
            if interpreter.inline_code.contains(&text) {
                match words.next() {
                    Some(code) if code.is_literal => self.files_named(&code.text, cwd),
                    _ => self.found.cannot_tell = true,
                }
                return;
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
                    Grouped::Code(Some(code)) => return self.files_named(code, cwd),
                    Grouped::Code(None) => match words.next() {
                        Some(code) if code.is_literal => return self.files_named(&code.text, cwd),
                        _ => {
                            self.found.cannot_tell = true;
                            return;
                        }
                    },
                    Grouped::Module => {
                        self.found.cannot_tell = true;
                        return;
                    }
                }
            }
            if text.starts_with('-') {
                continue;
            }
            self.found.file(absolute(cwd, text));
            return;
        }
        match words.next() {
            Some(script) if script.is_literal => self.found.file(absolute(cwd, &script.text)),
            Some(_) => self.found.cannot_tell = true,
            None => self.interpreter_input(stdin, cwd),
        }
    }

    fn interpreter_input(&mut self, stdin: &Stdin, cwd: &Path) {
        match stdin {
            Stdin::File(file) if file.is_literal => self.found.file(absolute(cwd, &file.text)),
            Stdin::Text(text) if text.is_literal => self.files_named(&text.text, cwd),
            _ => self.found.cannot_tell = true,
        }
    }

    /// Existing files inline code names by a path with a `/`.
    fn files_named(&mut self, code: &str, cwd: &Path) {
        for token in code.split(|character: char| {
            !(character.is_alphanumeric() || "._/~+-@".contains(character))
        }) {
            if token.contains('/') {
                let path = absolute(cwd, token);
                if path.is_file() {
                    self.found.file(path);
                }
            }
        }
    }

    /// The makefiles `make` reads: those `-f` names, else the first of `GNUmakefile`,
    /// `makefile` and `Makefile` that exists, with each before it, which would take its place
    /// if created; all three when none exists. `-C` moves where they are looked for.
    fn makefiles(&mut self, arguments: &[Word], cwd: &Path) {
        let mut directory = cwd.to_path_buf();
        let mut named: Vec<String> = Vec::new();
        let mut words = arguments.iter();
        while let Some(word) = words.next() {
            let text = word.text.as_str();
            let value = |prefix: &str| text.strip_prefix(prefix).filter(|rest| !rest.is_empty());
            let (target, value) = if matches!(text, "-f" | "--file" | "--makefile") {
                ("file", words.next())
            } else if matches!(text, "-C" | "--directory") {
                ("directory", words.next())
            } else if let Some(file) = value("--file=")
                .or_else(|| value("--makefile="))
                .or_else(|| value("-f").filter(|_| !text.starts_with("--")))
            {
                named.push(file.to_owned());
                self.found.cannot_tell |= !word.is_literal;
                continue;
            } else if let Some(folder) =
                value("--directory=").or_else(|| value("-C").filter(|_| !text.starts_with("--")))
            {
                directory = absolute(&directory, folder);
                self.found.cannot_tell |= !word.is_literal;
                continue;
            } else {
                continue;
            };
            match value {
                Some(value) if value.is_literal => match target {
                    "file" => named.push(value.text.clone()),
                    _ => directory = absolute(&directory, &value.text),
                },
                _ => self.found.cannot_tell = true,
            }
        }
        if !named.is_empty() {
            for file in named {
                self.found.file(absolute(&directory, &file));
            }
            return;
        }
        for name in ["GNUmakefile", "makefile", "Makefile"] {
            let path = directory.join(name);
            let exists = path.exists();
            self.found.file(path);
            if exists {
                return;
            }
        }
    }

    /// The justfile `just` reads: the one `--justfile` names, else the nearest `justfile`
    /// (any case) or `.justfile` from the directory up; `justfile` there when none exists.
    fn justfile(&mut self, arguments: &[Word], cwd: &Path) {
        let mut directory = cwd.to_path_buf();
        let mut words = arguments.iter();
        while let Some(word) = words.next() {
            let text = word.text.as_str();
            let (is_file, value) = match text {
                "-f" | "--justfile" => (true, words.next().cloned()),
                "-d" | "--working-directory" => (false, words.next().cloned()),
                _ => match text.split_once('=') {
                    Some(("--justfile", file)) => (true, Some(word.with_text(file))),
                    Some(("--working-directory", folder)) => (false, Some(word.with_text(folder))),
                    _ => continue,
                },
            };
            match value {
                Some(value) if value.is_literal && is_file => {
                    self.found.file(absolute(cwd, &value.text));
                    return;
                }
                Some(value) if value.is_literal => directory = absolute(cwd, &value.text),
                _ => {
                    self.found.cannot_tell = true;
                    return;
                }
            }
        }
        for folder in directory.ancestors() {
            if let Some(found) = justfile_in(folder) {
                self.found.file(found);
                return;
            }
        }
        self.found.file(directory.join("justfile"));
    }
}

fn justfile_in(folder: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(folder).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| {
            name.to_str().is_some_and(|name| {
                name.eq_ignore_ascii_case("justfile") || name.eq_ignore_ascii_case(".justfile")
            })
        })
        .min()
        .map(|name| folder.join(name))
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// `word` as an absolute path from `cwd`, as the kernel opens it: `..` is left for the
/// kernel to resolve through links, so the file hashed is the file run.
fn absolute(cwd: &Path, word: &str) -> PathBuf {
    cwd.join(word)
        .components()
        .filter(|component| *component != Component::CurDir)
        .collect()
}

/// The directory a shell's `cd` moves to: `..` resolved by name, as `cd` does without `-P`.
fn logical_directory(cwd: &Path, word: &str) -> PathBuf {
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

fn is_shell(name: &str) -> bool {
    matches!(
        name,
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "mksh" | "ash" | "yash" | "fish"
    )
}

/// Where a shell reads its code from.
enum ShellCode<'w> {
    /// `-c STRING`.
    CommandString(&'w Word),
    /// Its first operand.
    Script(&'w Word),
    /// Its input: no operand, `-s` or `-i`.
    Input,
    /// An expansion where its code or options are.
    CannotTell,
}

struct ShellArguments<'w> {
    code: ShellCode<'w>,
    /// `--rcfile` and `--init-file`.
    start_files: Vec<&'w Word>,
}

fn shell_arguments(arguments: &[Word]) -> ShellArguments<'_> {
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
            shell.code = ShellCode::CannotTell;
            return shell;
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
        Some(word) if !word.is_literal => ShellCode::CannotTell,
        Some(word) if is_command_string => ShellCode::CommandString(word),
        None if is_command_string => ShellCode::CannotTell,
        Some(word) if !reads_input => ShellCode::Script(word),
        _ => ShellCode::Input,
    };
    shell
}

const SHELL_BUILTINS: &[&str] = &[
    ":", "true", "false", "echo", "printf", "test", "[", "[[", "exit", "return", "export", "unset",
    "set", "shift", "pwd", "read", "local", "declare", "typeset", "readonly", "alias", "unalias",
    "trap", "wait", "umask", "ulimit", "break", "continue", "let", "shopt", "getopts", "hash",
    "type", "jobs", "fg", "bg", "disown", "kill",
];

/// A non-shell interpreter's options: those that take inline code, those that run a module
/// found on a search path, and those that take another value.
struct Interpreter {
    inline_code: &'static [&'static str],
    runs_module: &'static [&'static str],
    takes_value: &'static [&'static str],
}

fn interpreter(name: &str) -> Option<Interpreter> {
    let base =
        name.trim_end_matches(|character: char| character.is_ascii_digit() || character == '.');
    Some(match base {
        "python" | "pypy" => Interpreter {
            inline_code: &["-c"],
            runs_module: &["-m"],
            takes_value: &["-W", "-X", "--check-hash-based-pycs"],
        },
        "perl" => Interpreter {
            inline_code: &["-e", "-E"],
            runs_module: &[],
            takes_value: &["-I", "-M", "-m"],
        },
        "ruby" => Interpreter {
            inline_code: &["-e"],
            runs_module: &[],
            takes_value: &["-I", "-r", "-C"],
        },
        "node" | "nodejs" => Interpreter {
            inline_code: &["-e", "--eval", "-p", "--print"],
            runs_module: &[],
            takes_value: &["-r", "--require", "--import", "--loader"],
        },
        "php" => Interpreter {
            inline_code: &["-r"],
            runs_module: &[],
            takes_value: &["-c", "-d", "-z"],
        },
        "lua" | "luajit" => Interpreter {
            inline_code: &["-e"],
            runs_module: &["-l"],
            takes_value: &[],
        },
        "Rscript" => Interpreter {
            inline_code: &["-e"],
            runs_module: &[],
            takes_value: &[],
        },
        "tclsh" => Interpreter {
            inline_code: &[],
            runs_module: &[],
            takes_value: &[],
        },
        _ => return None,
    })
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
    for (index, letter) in letters.char_indices() {
        let flag = format!("-{letter}");
        let rest = &letters[index + letter.len_utf8()..];
        let is = |flags: &[&str]| flags.contains(&flag.as_str());
        if is(interpreter.inline_code) {
            return Grouped::Code(Some(rest).filter(|code| !code.is_empty()));
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
struct SudoOptions {
    command_at: usize,
    runs_nothing: bool,
    starts_shell: bool,
    chdir: Option<Word>,
}

fn sudo_options(arguments: &[Word]) -> Option<SudoOptions> {
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
fn skip_keywords(words: &[Word]) -> &[Word] {
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
fn skip_assignments(words: &[Word]) -> &[Word] {
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
struct Unwrapped<'w> {
    words: &'w [Word],
    cwd: PathBuf,
    cannot_tell: bool,
}

fn skip_wrappers<'w>(words: &'w [Word], cwd: &Path) -> Unwrapped<'w> {
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
struct Word {
    text: String,
    /// No expansion or glob: `text` is what the program gets.
    is_literal: bool,
}

impl Word {
    fn with_text(&self, text: &str) -> Word {
        Word {
            text: text.to_owned(),
            is_literal: self.is_literal,
        }
    }
}

/// Where a command's input comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stdin {
    /// The run's terminal.
    Terminal,
    /// The previous command of a pipeline.
    Pipe,
    /// `< FILE`.
    File(Word),
    /// A here-document or here-string.
    Text(Word),
}

/// One simple command: its words and its input.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Simple {
    words: Vec<Word>,
    stdin: Stdin,
}

/// The simple commands of a shell command line, in order; `None` when it does not lex (an
/// unterminated quote or expansion).
fn lex(script: &str, home: Option<&Path>) -> Option<Vec<Simple>> {
    Lexer {
        characters: script.chars().collect(),
        position: 0,
        home,
        commands: Vec::new(),
        current: Simple {
            words: Vec::new(),
            stdin: Stdin::Terminal,
        },
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
    position: usize,
    home: Option<&'h Path>,
    commands: Vec<Simple>,
    current: Simple,
    pending_heredocs: Vec<PendingHeredoc>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Redirect {
    Input,
    Heredoc { strips_tabs: bool },
    HereString,
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
                    self.position += 2;
                    if self.peek(0) == Some('>') {
                        self.position += 1;
                    }
                    self.redirect_target(Redirect::Other)?;
                }
                '&' => {
                    self.position += 1;
                    if self.peek(0) == Some('&') {
                        self.position += 1;
                    }
                    self.end_command(Stdin::Terminal);
                }
                '<' | '>' => self.redirect()?,
                _ => {
                    let word = self.word()?;
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

    fn end_command(&mut self, next_stdin: Stdin) {
        let current = std::mem::replace(
            &mut self.current,
            Simple {
                words: Vec::new(),
                stdin: next_stdin,
            },
        );
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
            ('<', Some('&')) | ('>', Some('&')) | ('>', Some('>')) | ('>', Some('|')) => {
                self.position += 1;
                Redirect::Other
            }
            ('<', Some('>')) => {
                self.position += 1;
                Redirect::Input
            }
            ('<', _) => Redirect::Input,
            _ => Redirect::Other,
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
        let start = self.position;
        while let Some(character) = self.peek(0) {
            match character {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | ')' => break,
                '(' if self.position == start => break,
                '<' | '>' if self.peek(1) != Some('(') => break,
                '<' | '>' => {
                    is_literal = false;
                    self.position += 1;
                    self.balanced('(', ')')?;
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
                                self.expansion(&mut text)?;
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
                    self.expansion(&mut text)?;
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
        Some(Word { text, is_literal })
    }

    /// Skips an expansion at `$` or a backtick, keeping its text for display.
    fn expansion(&mut self, text: &mut String) -> Option<()> {
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
            }
            ('$', Some('(')) => {
                self.position += 1;
                self.balanced('(', ')')?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(text: &str) -> Word {
        Word {
            text: text.to_owned(),
            is_literal: true,
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
                    is_literal: false
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
    fn an_unterminated_quote_does_not_lex() {
        assert_eq!(lex("sudo sh -c 'x", None), None);
    }

    #[test]
    fn the_stored_form_reads_back() {
        let runs = RootRuns {
            editable: vec![
                RootFile {
                    path: PathBuf::from("/clone/x y.sh"),
                    sha256: Some("a".repeat(64)),
                },
                RootFile {
                    path: PathBuf::from("/clone/Makefile"),
                    sha256: None,
                },
            ],
            cannot_tell: true,
        };
        assert_eq!(decode(&encode(&runs)), Some(runs));
        assert_eq!(
            decode(&encode(&RootRuns::default())),
            Some(RootRuns::default())
        );
        assert_eq!(decode("v2 known"), None);
        assert_eq!(decode("v1 known\nabc /x"), None);
        assert_eq!(decode("v1 known\nabsent relative"), None);
    }
}
