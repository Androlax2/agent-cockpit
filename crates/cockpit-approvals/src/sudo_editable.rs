//! "sudo runs a file this session can edit" (#1383), one of the rule scan's rules (#1388,
//! `rules.rs`), which reads its result instead of repeating the check: an approved `sudo ./setup.sh`
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
//! tell". A command naming no `sudo` word is never parsed.

use std::ffi::OsStr;
use std::io::{ErrorKind, Read as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::hints::program_name;
use crate::shell::{
    Interpreter, InterpreterCode, SHELL_BUILTINS, ShellCode, Simple, Stdin, Word, absolute,
    interpreter, interpreter_code, is_shell, lex, logical_directory, shell_arguments,
    skip_assignments, skip_keywords, skip_wrappers, sudo_options,
};

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
        // As named, by name with `..` taken back (a file not there yet has no other form),
        // and through its links.
        let resolved = std::fs::canonicalize(&path).ok();
        let is_editable = writes.can_edit(&path)
            || writes.can_edit(&logical_directory(Path::new("/"), &path.to_string_lossy()))
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

/// `path` opened for reading when it is a regular file; `None` when there is none. Opened
/// without waiting for a writer on a FIFO (`O_NONBLOCK`), then checked on what was opened,
/// which a device or a swapped-in link would otherwise make an endless read.
pub(crate) fn open_regular(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let opened = match rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    ) {
        Ok(opened) => opened,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(errno) => return Err(errno.into()),
    };
    let file = std::fs::File::from(opened);
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "not a regular file",
        ));
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
    let mut is_cut = false;
    let mut text = TextCheck::default();
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
        text.feed(chunk);
        line_breaks += chunk.iter().filter(|byte| **byte == b'\n').count() as u64;
        last_byte = chunk.last().copied();
        let room = HEAD_BYTES_MAX.saturating_sub(kept.len());
        is_cut |= read > room;
        kept.extend_from_slice(&chunk[..room.min(read)]);
    }
    let sha256 = Some(hex(&hasher.finalize()));
    if !text.is_text() {
        return Ok(FileNow {
            sha256,
            head: None,
            more_lines: 0,
        });
    }
    if is_cut {
        // A line cut where `kept` ends is not shown as if whole: it counts as a line more.
        let whole_lines = kept
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        kept.truncate(whole_lines);
    }
    let all_lines = line_breaks + u64::from(last_byte.is_some_and(|byte| byte != b'\n'));
    let kept = String::from_utf8(kept).expect("whole lines of valid UTF-8 are valid UTF-8");
    let head: Vec<&str> = kept.lines().take(FILE_HEAD_LINES).collect();
    Ok(FileNow {
        sha256,
        head: Some(head.join("\n")),
        more_lines: all_lines.saturating_sub(head.len() as u64),
    })
}

/// Whether a stream read in chunks is text: valid UTF-8 throughout, without a NUL byte.
#[derive(Default)]
struct TextCheck {
    /// The start of a character a chunk ended within.
    carried: Vec<u8>,
    is_invalid: bool,
}

impl TextCheck {
    fn feed(&mut self, chunk: &[u8]) {
        if self.is_invalid {
            return;
        }
        let mut bytes = std::mem::take(&mut self.carried);
        bytes.extend_from_slice(chunk);
        self.is_invalid = bytes.contains(&0);
        match std::str::from_utf8(&bytes) {
            Ok(_) => {}
            Err(error) if error.error_len().is_none() => {
                self.carried = bytes[error.valid_up_to()..].to_vec();
            }
            Err(_) => self.is_invalid = true,
        }
    }

    fn is_text(&self) -> bool {
        !self.is_invalid && self.carried.is_empty()
    }
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
                ShellCode::CommandString(script) if script.is_literal => {
                    self.script(&script.text, &words.cwd, Context::Owner, depth + 1);
                }
                ShellCode::CommandString(_) | ShellCode::CannotTell => {
                    self.found.cannot_tell = true;
                }
                ShellCode::Script(script) if !script.is_literal => self.found.cannot_tell = true,
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
            ShellCode::CommandString(script) if script.is_literal => {
                self.script(&script.text, cwd, Context::Root, depth + 1);
            }
            ShellCode::Script(script) if script.is_literal => {
                self.found.file(absolute(cwd, &script.text));
            }
            ShellCode::CommandString(_) | ShellCode::Script(_) | ShellCode::CannotTell => {
                self.found.cannot_tell = true;
            }
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
        match interpreter_code(interpreter, arguments) {
            InterpreterCode::Inline(code) if code.is_literal => self.files_named(&code.text, cwd),
            InterpreterCode::Script(script) if script.is_literal => {
                self.found.file(absolute(cwd, &script.text));
            }
            InterpreterCode::Input => self.interpreter_input(stdin, cwd),
            InterpreterCode::Inline(_)
            | InterpreterCode::Script(_)
            | InterpreterCode::CannotTell => {
                self.found.cannot_tell = true;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
