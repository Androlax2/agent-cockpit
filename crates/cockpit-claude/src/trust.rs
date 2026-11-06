//! Claude Code's own folder trust, recorded for a parallel clone before its launch (#702,
//! decided in docs/decisions/2026-11-04-702-claude-folder-trust.md).
//!
//! Measured on 2.1.317: Claude Code keeps trust as `projects[<git root>].hasTrustDialogAccepted`
//! in its global config, `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`, and saves that
//! file under a `<file>.lock` directory lock, re-reading it and merging `projects` first. Its
//! lookup walks from the working directory up to the git root only, so a project's trust never
//! covers a clone: each clone is its own root.
//!
//! The write here takes the same lock, re-reads the file, changes only the clone's trust key,
//! keeps every other byte of the file and replaces it by an atomic rename. Anything it cannot
//! do safely is refused, writing nothing: the dialog then shows and the session waits on the
//! owner.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The workspace files a launch there reads as harness settings; trusting the folder is what
/// lets them apply (hooks, permission rules, MCP servers).
pub const PROJECT_SETTINGS: [&str; 2] = [".claude", ".mcp.json"];

/// The key Claude Code reads.
const TRUST_KEY: &str = "hasTrustDialogAccepted";

/// A new project entry as Claude Code 2.1.317 writes one when the owner accepts the dialog:
/// its defaults, with the trust key set.
const NEW_PROJECT_ENTRY: &str = r#"{"allowedTools":[],"mcpContextUris":[],"mcpServers":{},"enabledMcpjsonServers":[],"disabledMcpjsonServers":[],"hasTrustDialogAccepted":true,"hasClaudeMdExternalIncludesApproved":false,"hasClaudeMdExternalIncludesWarningShown":false}"#;

/// How long the write waits for a Claude Code instance to release the config lock. Claude
/// Code holds it for one read and write; a stale one is left for Claude Code to reclaim.
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(25);

/// What [`record`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The workspace's trust already was the one asked for; the file is unchanged.
    Unchanged,
    /// The trust key was written.
    Written,
}

/// What [`record`] does when the config file does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenMissing {
    /// Refuse, writing nothing: the owner's own Claude Code keeps that file.
    Refuse,
    /// Create it, owner-only, holding the trust alone: a sandboxed session's own home, where
    /// no Claude Code ran yet (#968).
    Create,
}

/// The mode of a config file [`record`] creates: Claude Code's own.
const CREATED_MODE: u32 = 0o600;

/// Claude Code's global config file for a sandboxed agent launched with `environment`, whose
/// home is backed by `session_home` (#858, #968): the file there that the agent reads as
/// `~/.claude.json`. Refused when `CLAUDE_CONFIG_DIR` names another folder.
pub fn session_config_file(
    environment: &[(OsString, OsString)],
    session_home: &Path,
) -> Result<PathBuf, String> {
    let names_config_dir = environment
        .iter()
        .any(|(name, value)| name == "CLAUDE_CONFIG_DIR" && !value.is_empty());
    if names_config_dir {
        return Err(
            "CLAUDE_CONFIG_DIR is set, so the session's home may not hold Claude Code's config"
                .to_owned(),
        );
    }
    let in_session: Vec<(OsString, OsString)> = environment
        .iter()
        .filter(|(name, _)| name != "HOME")
        .cloned()
        .chain([("HOME".into(), session_home.as_os_str().to_owned())])
        .collect();
    config_file(&in_session)
}

/// Claude Code's global config file for an agent launched with `environment`, or why it
/// cannot be known safely.
pub fn config_file(environment: &[(OsString, OsString)]) -> Result<PathBuf, String> {
    let variable = |name: &str| {
        environment
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .filter(|value| !value.is_empty())
    };
    if variable("CLAUDE_CODE_CUSTOM_OAUTH_URL").is_some() {
        return Err(
            "CLAUDE_CODE_CUSTOM_OAUTH_URL is set, so Claude Code keeps another config file"
                .to_owned(),
        );
    }
    let (config_dir, file) = match (variable("CLAUDE_CONFIG_DIR"), variable("HOME")) {
        (Some(dir), _) => {
            let dir = PathBuf::from(dir);
            (dir.clone(), dir.join(".claude.json"))
        }
        (None, Some(home)) => {
            let home = PathBuf::from(home);
            (home.join(".claude"), home.join(".claude.json"))
        }
        (None, None) => return Err("the launch sets neither HOME nor CLAUDE_CONFIG_DIR".to_owned()),
    };
    // Claude Code still reads this older file when it exists with content.
    let legacy = config_dir.join(".config.json");
    if fs::metadata(&legacy).is_ok_and(|metadata| metadata.len() > 0) {
        return Err(format!(
            "Claude Code reads its older config {}",
            legacy.display()
        ));
    }
    Ok(file)
}

/// The harness settings `clone` brings that `trusted_checkout` lacks or holds otherwise:
/// each file under [`PROJECT_SETTINGS`] in the clone, in every directory from `start` (where
/// the agent starts, inside the clone) up to the clone's root, must exist with the same bytes
/// at the same place in the trusted checkout. Files only the trusted checkout has, such as the
/// owner's own local settings, are fine: the clone applies fewer settings, never other ones.
/// Paths relative to the clone, sorted.
pub fn unexpected_settings(
    clone: &Path,
    trusted_checkout: &Path,
    start: &Path,
) -> Result<Vec<PathBuf>, String> {
    let below = start.strip_prefix(clone).map_err(|_| {
        format!(
            "the agent starts in {}, outside the clone {}",
            start.display(),
            clone.display()
        )
    })?;
    let mut unexpected = Vec::new();
    for directory in below.ancestors() {
        for name in PROJECT_SETTINGS {
            compare(
                clone,
                trusted_checkout,
                &directory.join(name),
                &mut unexpected,
            )?;
        }
    }
    unexpected.sort();
    Ok(unexpected)
}

fn compare(
    clone: &Path,
    trusted_checkout: &Path,
    relative: &Path,
    unexpected: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let path = clone.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    if metadata.is_dir() {
        let entries =
            fs::read_dir(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("read {}: {error}", path.display()))?;
            compare(
                clone,
                trusted_checkout,
                &relative.join(entry.file_name()),
                unexpected,
            )?;
        }
        return Ok(());
    }
    // A link could point anywhere; only a plain file can be compared.
    if !metadata.is_file() {
        unexpected.push(relative.to_owned());
        return Ok(());
    }
    let trusted = trusted_checkout.join(relative);
    let is_same = fs::symlink_metadata(&trusted).is_ok_and(|metadata| metadata.is_file())
        && fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?
            == fs::read(&trusted)
                .map_err(|error| format!("read {}: {error}", trusted.display()))?;
    if !is_same {
        unexpected.push(relative.to_owned());
    }
    Ok(())
}

/// Records Claude Code's trust of `workspace` in `config` as `trusted`: under Claude Code's
/// own lock, from the file as it is then, changing only that project's trust key. Revoking
/// only turns an existing `true` to `false`; it never adds an entry. Refuses, writing nothing,
/// when the file is a link, cannot be parsed, has an unexpected shape or stays locked, and
/// when it is missing unless `when_missing` creates it.
pub fn record(
    config: &Path,
    workspace: &Path,
    trusted: bool,
    when_missing: WhenMissing,
) -> Result<Recorded, String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", workspace.display()))?;
    let key = workspace
        .to_str()
        .ok_or_else(|| format!("{} is not UTF-8", workspace.display()))?;
    // Checked before the lock too, so a file that can never be written takes no lock.
    existing(config, when_missing)?;
    let _lock = ConfigLock::acquire(config)?;
    let (text, mode) = match existing(config, when_missing)? {
        Some(metadata) => (
            fs::read_to_string(config)
                .map_err(|error| format!("read {}: {error}", config.display()))?,
            metadata.mode() & 0o7777,
        ),
        None => ("{}".to_owned(), CREATED_MODE),
    };
    let Some(edited) = with_trust(&text, key, trusted)? else {
        return Ok(Recorded::Unchanged);
    };
    replace(config, mode, &edited)?;
    Ok(Recorded::Written)
}

/// `config`'s metadata, `None` when it is missing and `when_missing` creates it; refused when
/// it is missing otherwise or is not a plain file.
fn existing(config: &Path, when_missing: WhenMissing) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(config) {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata)),
        Ok(_) => Err(format!("{} is not a plain file", config.display())),
        Err(error)
            if error.kind() == ErrorKind::NotFound && when_missing == WhenMissing::Create =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("read {}: {error}", config.display())),
    }
}

/// `text` with `projects[key].hasTrustDialogAccepted` set to `trusted`, every other byte
/// kept; `None` when nothing changes: already trusted, or, to revoke, not trusted (a missing
/// entry or key reads as untrusted, so none is added). The edit is checked by parsing the
/// result.
pub fn with_trust(text: &str, key: &str, trusted: bool) -> Result<Option<String>, String> {
    let unparsable = |error: serde_json::Error| format!("the config is not JSON: {error}");
    let mut expected: Value = serde_json::from_str(text).map_err(unparsable)?;
    let top = expected
        .as_object_mut()
        .ok_or("the config is not a JSON object")?;
    let projects = top
        .entry("projects")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or("the config's `projects` is not an object")?;
    let is_trusted = match projects.get(key) {
        Some(entry) => {
            entry
                .as_object()
                .ok_or("the workspace's project entry is not an object")?
                .get(TRUST_KEY)
                == Some(&Value::Bool(true))
        }
        None => false,
    };
    if is_trusted == trusted {
        return Ok(None);
    }
    let edited = splice(text, key, trusted)?;
    match projects.get_mut(key) {
        Some(entry) => {
            entry
                .as_object_mut()
                .ok_or("the workspace's project entry is not an object")?
                .insert(TRUST_KEY.to_owned(), Value::Bool(trusted));
        }
        None => {
            let entry = serde_json::from_str(NEW_PROJECT_ENTRY).map_err(unparsable)?;
            projects.insert(key.to_owned(), entry);
        }
    }
    let written: Value = serde_json::from_str(&edited).map_err(unparsable)?;
    if written != expected {
        return Err("the edited config does not read back as the intended change".to_owned());
    }
    Ok(Some(edited))
}

/// The textual edit: the trust key's value replaced, or, to trust, a member inserted first in
/// the innermost object that exists on the path `projects` → `key` → trust key. Revoking
/// reaches here only for a key that is `true`, so it only replaces.
fn splice(text: &str, key: &str, trusted: bool) -> Result<String, String> {
    let bytes = text.as_bytes();
    let top = skip_whitespace(bytes, 0);
    let top_members = members(bytes, top)?;
    let insert_first = |object: usize, member: String, is_empty: bool| {
        let at = object + 1;
        let separator = if is_empty { "" } else { "," };
        format!("{}{member}{separator}{}", &text[..at], &text[at..])
    };
    let Some(projects) = find(&top_members, "projects")? else {
        let member = format!("\"projects\":{{{}:{NEW_PROJECT_ENTRY}}}", quoted(key)?);
        return Ok(insert_first(top, member, top_members.is_empty()));
    };
    let project_members = members(bytes, projects.value)?;
    let Some(entry) = find(&project_members, key)? else {
        let member = format!("{}:{NEW_PROJECT_ENTRY}", quoted(key)?);
        return Ok(insert_first(
            projects.value,
            member,
            project_members.is_empty(),
        ));
    };
    let entry_members = members(bytes, entry.value)?;
    match find(&entry_members, TRUST_KEY)? {
        Some(flag) => Ok(format!(
            "{}{trusted}{}",
            &text[..flag.value],
            &text[flag.value_end..]
        )),
        None => Ok(insert_first(
            entry.value,
            format!("\"{TRUST_KEY}\":true"),
            entry_members.is_empty(),
        )),
    }
}

fn quoted(key: &str) -> Result<String, String> {
    serde_json::to_string(key).map_err(|error| error.to_string())
}

/// One member of a JSON object: its decoded name and its value's byte range.
struct Member {
    name: String,
    value: usize,
    value_end: usize,
}

/// The member named `name`; a name given twice is refused, since readers disagree on which
/// one counts.
fn find<'a>(members: &'a [Member], name: &str) -> Result<Option<&'a Member>, String> {
    let mut found = members.iter().filter(|member| member.name == name);
    let first = found.next();
    if found.next().is_some() {
        return Err(format!("the config names `{name}` twice"));
    }
    Ok(first)
}

/// The members of the object whose `{` is at `start`. The text is already known to be JSON,
/// so the scan only needs to find boundaries.
fn members(bytes: &[u8], start: usize) -> Result<Vec<Member>, String> {
    if bytes.get(start) != Some(&b'{') {
        return Err("expected an object".to_owned());
    }
    let mut members = Vec::new();
    let mut at = skip_whitespace(bytes, start + 1);
    if bytes.get(at) == Some(&b'}') {
        return Ok(members);
    }
    loop {
        let name_end = string_end(bytes, at)?;
        let name: String = serde_json::from_slice(&bytes[at..name_end])
            .map_err(|error| format!("a member name: {error}"))?;
        at = skip_whitespace(bytes, name_end);
        if bytes.get(at) != Some(&b':') {
            return Err("expected `:`".to_owned());
        }
        let value = skip_whitespace(bytes, at + 1);
        let value_end = value_end(bytes, value)?;
        members.push(Member {
            name,
            value,
            value_end,
        });
        at = skip_whitespace(bytes, value_end);
        match bytes.get(at) {
            Some(b',') => at = skip_whitespace(bytes, at + 1),
            Some(b'}') => return Ok(members),
            _ => return Err("expected `,` or `}`".to_owned()),
        }
    }
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(|byte| b" \t\r\n".contains(byte)) {
        at += 1;
    }
    at
}

/// The byte after the string whose `"` is at `start`.
fn string_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    if bytes.get(start) != Some(&b'"') {
        return Err("expected a string".to_owned());
    }
    let mut at = start + 1;
    loop {
        match bytes.get(at) {
            Some(b'"') => return Ok(at + 1),
            Some(b'\\') => at += 2,
            Some(_) => at += 1,
            None => return Err("an unterminated string".to_owned()),
        }
    }
}

/// The byte after the value that starts at `start`.
fn value_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    match bytes.get(start) {
        Some(b'"') => string_end(bytes, start),
        Some(b'{' | b'[') => {
            let mut depth = 0_usize;
            let mut at = start;
            loop {
                match bytes.get(at) {
                    Some(b'"') => {
                        at = string_end(bytes, at)?;
                        continue;
                    }
                    Some(b'{' | b'[') => depth += 1,
                    Some(b'}' | b']') => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(at + 1);
                        }
                    }
                    Some(_) => {}
                    None => return Err("an unterminated value".to_owned()),
                }
                at += 1;
            }
        }
        Some(_) => {
            let length = bytes[start..]
                .iter()
                .position(|byte| b",}] \t\r\n".contains(byte))
                .unwrap_or(bytes.len() - start);
            Ok(start + length)
        }
        None => Err("expected a value".to_owned()),
    }
}

/// Claude Code's config lock: the `<file>.lock` directory its lock library creates. Removed
/// on drop.
struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    fn acquire(config: &Path) -> Result<ConfigLock, String> {
        let mut name = OsString::from(config.as_os_str());
        name.push(OsStr::new(".lock"));
        let path = PathBuf::from(name);
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(ConfigLock { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "{} stayed locked for {} s",
                            config.display(),
                            LOCK_WAIT.as_secs()
                        ));
                    }
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => return Err(format!("lock {}: {error}", config.display())),
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir(&self.path) {
            eprintln!("torlek: could not release {}: {error}", self.path.display());
        }
    }
}

/// Replaces `config` with `text` by a rename from a file beside it with `mode`, so a reader
/// sees the old file or the new one, never a part.
fn replace(config: &Path, mode: u32, text: &str) -> Result<(), String> {
    let dir = config
        .parent()
        .ok_or_else(|| format!("{} has no directory", config.display()))?;
    let file_name = config
        .file_name()
        .ok_or_else(|| format!("{} has no file name", config.display()))?;
    let mut temporary_name = OsString::from(".");
    temporary_name.push(file_name);
    // The time too: a file a killed daemon left must not block a later one with its pid.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    temporary_name.push(format!(".torlek-{}-{nanos}", std::process::id()));
    let temporary = dir.join(temporary_name);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, config)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write {}: {error}", config.display()));
    }
    Ok(())
}
