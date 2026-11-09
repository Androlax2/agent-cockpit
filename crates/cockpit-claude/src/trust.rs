//! Claude Code's own folder trust, recorded for a parallel clone before its launch (#702,
//! decided in docs/decisions/2026-11-04-702-claude-folder-trust.md).
//!
//! Measured on 2.1.317: Claude Code keeps trust as `projects[<git root>].hasTrustDialogAccepted`
//! in its global config, `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`, and saves that
//! file under a `<file>.lock` directory lock, re-reading it and merging `projects` first. Its
//! lookup walks from the working directory up to the git root only, so a project's trust never
//! covers a clone: each clone is its own root.
//!
//! The write here takes the same lock, re-reads the file, changes only the clone's trust key
//! and its project MCP approvals, keeps every other byte of the file and replaces it by an
//! atomic rename. Anything it cannot do safely is refused, writing nothing: the dialog then
//! shows and the session waits on the owner.
//!
//! A clone's settings are accepted only when they match the trusted checkout's working tree;
//! when they match its fetched default branch instead, the refusal says the checkout is
//! behind and how to fast-forward it (#1279). The owner's own approvals of the project's
//! `.mcp.json` servers in the trusted checkout carry over to the clone.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use cockpit_provider_api::{DefaultBranch, ReadFile};
use serde_json::Value;

/// The workspace files a launch there reads as harness settings; trusting the folder is what
/// lets them apply (hooks, permission rules, MCP servers).
pub const PROJECT_SETTINGS: [&str; 2] = [".claude", ".mcp.json"];

/// The key Claude Code reads.
const TRUST_KEY: &str = "hasTrustDialogAccepted";

/// The project's `.mcp.json` servers the owner approved, rejected, or all approved at once,
/// as Claude Code 2.1.321 keeps them in a project's entry.
const ENABLED_SERVERS_KEY: &str = "enabledMcpjsonServers";
const DISABLED_SERVERS_KEY: &str = "disabledMcpjsonServers";
const ALL_SERVERS_KEY: &str = "enableAllProjectMcpServers";

/// The project MCP server file Claude Code reads in each directory it compares settings in.
const MCP_FILE: &str = ".mcp.json";

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
pub(crate) const CREATED_MODE: u32 = 0o600;

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
    unexpected_against(clone, start, &|relative| {
        working_tree_file(trusted_checkout, relative)
    })
}

/// Whether `clone`'s harness settings, read from `start` up to its root, match the trusted
/// checkout's working tree, the only reference the owner accepted (#702). Why not, naming
/// each file that differs, when they do not; when they match `default_branch` instead, the
/// checkout is behind what the owner merged, and the reason gives the command that accepts
/// it (#1279): the owner's own pull is the acceptance, since agents can merge too.
pub fn vetted_settings(
    clone: &Path,
    trusted_checkout: &Path,
    start: &Path,
    default_branch: Option<&DefaultBranch<'_>>,
) -> Result<(), String> {
    let against_working_tree = unexpected_settings(clone, trusted_checkout, start)?;
    if against_working_tree.is_empty() {
        return Ok(());
    }
    let differing = against_working_tree
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let differ = format!(
        "its harness settings differ from the trusted checkout's working tree ({differing})"
    );
    let Some(default_branch) = default_branch else {
        return Err(differ);
    };
    match unexpected_against(clone, start, default_branch.file) {
        Ok(against_default_branch) if against_default_branch.is_empty() => Err(format!(
            "run `git -C {} pull --ff-only`: the trusted checkout is behind its origin/HEAD \
             at {}, which holds this clone's settings ({differing})",
            shell_word(trusted_checkout),
            short_commit(default_branch.commit)
        )),
        Ok(_) => Err(differ),
        Err(error) => Err(format!(
            "{differ}; its origin/HEAD cannot be compared: {error}"
        )),
    }
}

/// `path` as one POSIX shell word, quoted only when it needs to be.
fn shell_word(path: &Path) -> String {
    let text = path.to_string_lossy();
    let is_plain = !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._-+@%:,".contains(character));
    if is_plain {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The first twelve characters of a commit id, as the owner reads it.
pub fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// The harness settings files in `clone`, from `start` up to its root, that `reference` does
/// not hold with the same bytes; see [`unexpected_settings`].
fn unexpected_against(
    clone: &Path,
    start: &Path,
    reference: &ReadFile<'_>,
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
            compare(clone, reference, &directory.join(name), &mut unexpected)?;
        }
    }
    unexpected.sort();
    Ok(unexpected)
}

fn compare(
    clone: &Path,
    reference: &ReadFile<'_>,
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
                reference,
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
    let Some(accepted) = reference(relative)? else {
        unexpected.push(relative.to_owned());
        return Ok(());
    };
    let brought = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if brought != accepted {
        unexpected.push(relative.to_owned());
    }
    Ok(())
}

/// The bytes of the plain file at `relative` in `checkout`; `None` when there is none there.
fn working_tree_file(checkout: &Path, relative: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = checkout.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => fs::read(&path)
            .map(Some)
            .map_err(|error| format!("read {}: {error}", path.display())),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

/// The project MCP approvals a clone's entry gets: the owner's in the trusted checkout, for
/// the servers the clone defines exactly as the checkout does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpApprovals {
    /// The servers Claude Code starts without asking.
    pub enabled: Vec<String>,
    /// The servers the owner rejected, which Claude Code neither starts nor asks about.
    pub disabled: Vec<String>,
}

/// [`McpApprovals`] carried from the trusted checkout, and the clone's servers left to
/// Claude Code's prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarriedApprovals {
    pub approvals: McpApprovals,
    /// Servers the owner never approved in the trusted checkout, or approved as the
    /// checkout defines them while the clone defines them otherwise. Sorted.
    pub waiting: Vec<String>,
}

/// The owner's project MCP approvals for `trusted_checkout`, read from `owner_config`, carried
/// to `clone` (#1279): a server is approved in the clone only when the owner approved it in
/// the checkout (one by one or all at once) and each `.mcp.json` from `start` up to the root
/// defines it with the same value in both. Rejections carry as they are.
pub fn carried_approvals(
    owner_config: &Path,
    clone: &Path,
    trusted_checkout: &Path,
    start: &Path,
) -> Result<CarriedApprovals, String> {
    let below = start.strip_prefix(clone).map_err(|_| {
        format!(
            "the agent starts in {}, outside the clone {}",
            start.display(),
            clone.display()
        )
    })?;
    let brought = mcp_servers(clone, below)?;
    let accepted = mcp_servers(trusted_checkout, below)?;
    let owner = owner_approvals(owner_config, trusted_checkout)?;
    let mut carried = CarriedApprovals {
        approvals: McpApprovals {
            enabled: Vec::new(),
            disabled: owner.disabled.clone(),
        },
        waiting: Vec::new(),
    };
    for (name, definitions) in &brought {
        if owner.disabled.contains(name) {
            continue;
        }
        let is_approved = owner.all || owner.enabled.contains(name);
        if is_approved && accepted.get(name) == Some(definitions) {
            carried.approvals.enabled.push(name.clone());
        } else {
            carried.waiting.push(name.clone());
        }
    }
    Ok(carried)
}

/// Each server the `.mcp.json` files in `root`, from `below` up to `root` itself, define:
/// its name, then the file and value of each definition.
fn mcp_servers(
    root: &Path,
    below: &Path,
) -> Result<BTreeMap<String, Vec<(PathBuf, Value)>>, String> {
    let mut servers: BTreeMap<String, Vec<(PathBuf, Value)>> = BTreeMap::new();
    for directory in below.ancestors() {
        let relative = directory.join(MCP_FILE);
        let Some(bytes) = working_tree_file(root, &relative)? else {
            continue;
        };
        let path = root.join(&relative);
        let file: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{} is not JSON: {error}", path.display()))?;
        let Some(defined) = file.get("mcpServers") else {
            continue;
        };
        let defined = defined
            .as_object()
            .ok_or_else(|| format!("{}'s `mcpServers` is not an object", path.display()))?;
        for (name, definition) in defined {
            servers
                .entry(name.clone())
                .or_default()
                .push((relative.clone(), definition.clone()));
        }
    }
    Ok(servers)
}

/// The owner's approvals in `checkout`'s entry of `config`; none when it has no entry or
/// `config` does not exist.
#[derive(Default)]
struct OwnerApprovals {
    enabled: Vec<String>,
    disabled: Vec<String>,
    all: bool,
}

fn owner_approvals(config: &Path, checkout: &Path) -> Result<OwnerApprovals, String> {
    let checkout = checkout
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", checkout.display()))?;
    let key = checkout
        .to_str()
        .ok_or_else(|| format!("{} is not UTF-8", checkout.display()))?;
    // Read without Claude Code's lock: it replaces the file whole, and a file read while
    // it is written fails to parse, which carries nothing.
    let text = match fs::read_to_string(config) {
        Ok(text) => text,
        // An owner who never ran Claude Code approved nothing.
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(OwnerApprovals::default());
        }
        Err(error) => return Err(format!("read {}: {error}", config.display())),
    };
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not JSON: {error}", config.display()))?;
    let entry = &parsed["projects"][key];
    let names = |member: &str| -> Result<Vec<String>, String> {
        match entry.get(member) {
            None => Ok(Vec::new()),
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("the checkout's `{member}` holds a non-string"))
                })
                .collect(),
            Some(_) => Err(format!("the checkout's `{member}` is not a list")),
        }
    };
    Ok(OwnerApprovals {
        enabled: names(ENABLED_SERVERS_KEY)?,
        disabled: names(DISABLED_SERVERS_KEY)?,
        all: entry.get(ALL_SERVERS_KEY) == Some(&Value::Bool(true)),
    })
}

/// What [`record`] changes in the workspace's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Trust it with these project MCP approvals: a clone's own earlier approvals and an
    /// "approve all" are replaced, its own rejections kept beside the carried ones.
    Trust(McpApprovals),
    /// Turn an existing trust and project MCP approvals off, adding nothing, so a reused
    /// clone keeps none a previous branch earned.
    Revoke,
}

/// Applies `change` to `workspace`'s entry in `config`: under Claude Code's own lock, from the
/// file as it is then, changing only that project's trust and MCP approval keys, and only
/// those whose value changes. Revoking never adds an entry or a key. Refuses, writing nothing,
/// when the file is a link, cannot be parsed, has an unexpected shape or stays locked, and
/// when it is missing unless `when_missing` creates it.
pub fn record(
    config: &Path,
    workspace: &Path,
    change: &Change,
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
    let Some(edited) = with_change(&text, key, change)? else {
        return Ok(Recorded::Unchanged);
    };
    replace(config, mode, &edited)?;
    Ok(Recorded::Written)
}

/// `config`'s metadata, `None` when it is missing and `when_missing` creates it; refused when
/// it is missing otherwise or is not a plain file.
pub(crate) fn existing(
    config: &Path,
    when_missing: WhenMissing,
) -> Result<Option<fs::Metadata>, String> {
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

/// `text` with `change` applied to `projects[key]`, every other byte kept; `None` when nothing
/// changes. A missing entry, key or list reads as untrusted, with nothing approved, so
/// revoking adds none. The edit is checked by parsing the result.
pub fn with_change(text: &str, key: &str, change: &Change) -> Result<Option<String>, String> {
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
    let entry = match projects.get(key) {
        Some(entry) => Some(
            entry
                .as_object()
                .ok_or("the workspace's project entry is not an object")?,
        ),
        None => None,
    };
    let edits = entry_edits(entry, change);
    if edits.is_empty() {
        return Ok(None);
    }
    let edited = splice(text, key, &edits)?;
    let entry = match projects.get_mut(key) {
        Some(entry) => entry,
        None => projects
            .entry(key.to_owned())
            .or_insert(serde_json::from_str(NEW_PROJECT_ENTRY).map_err(unparsable)?),
    };
    let entry = entry
        .as_object_mut()
        .ok_or("the workspace's project entry is not an object")?;
    for (member, value) in &edits {
        entry.insert((*member).to_owned(), value.clone());
    }
    let written: Value = serde_json::from_str(&edited).map_err(unparsable)?;
    if written != expected {
        return Err("the edited config does not read back as the intended change".to_owned());
    }
    Ok(Some(edited))
}

/// The members of the workspace's `entry` that `change` sets, with their new values: only
/// those whose value differs. Revoking a missing entry sets nothing.
fn entry_edits(
    entry: Option<&serde_json::Map<String, Value>>,
    change: &Change,
) -> Vec<(&'static str, Value)> {
    let current = |member: &str| entry.and_then(|entry| entry.get(member));
    let no_servers = Value::Array(Vec::new());
    let listed = |member: &str| current(member).unwrap_or(&no_servers).clone();
    let mut edits = Vec::new();
    match change {
        Change::Trust(approvals) => {
            if current(TRUST_KEY) != Some(&Value::Bool(true)) {
                edits.push((TRUST_KEY, Value::Bool(true)));
            }
            let enabled = approvals.enabled.clone();
            // Rejections the owner made in the clone itself stay: rejecting more is safe.
            let mut disabled: Vec<String> = match listed(DISABLED_SERVERS_KEY) {
                Value::Array(names) => names
                    .into_iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect(),
                _ => Vec::new(),
            };
            for name in &approvals.disabled {
                if !disabled.contains(name) {
                    disabled.push(name.clone());
                }
            }
            for (member, names) in [
                (ENABLED_SERVERS_KEY, enabled),
                (DISABLED_SERVERS_KEY, disabled),
            ] {
                let wanted = Value::Array(names.into_iter().map(Value::String).collect());
                if listed(member) != wanted {
                    edits.push((member, wanted));
                }
            }
        }
        Change::Revoke => {
            if entry.is_none() {
                return edits;
            }
            if current(TRUST_KEY) == Some(&Value::Bool(true)) {
                edits.push((TRUST_KEY, Value::Bool(false)));
            }
            if listed(ENABLED_SERVERS_KEY) != no_servers {
                edits.push((ENABLED_SERVERS_KEY, no_servers.clone()));
            }
        }
    }
    if current(ALL_SERVERS_KEY) == Some(&Value::Bool(true)) {
        edits.push((ALL_SERVERS_KEY, Value::Bool(false)));
    }
    edits
}

/// The textual edit: each member of `edits` set in the workspace's entry, its value replaced
/// or, when the entry lacks it, the member inserted first. A missing entry is Claude Code's
/// own new one with `edits` applied, inserted first in `projects`, itself inserted first in
/// the file when missing. Revoking reaches here only for an existing entry.
fn splice(text: &str, key: &str, edits: &[(&str, Value)]) -> Result<String, String> {
    let bytes = text.as_bytes();
    let top = skip_whitespace(bytes, 0);
    let top_members = members(bytes, top)?;
    let Some(projects) = find(&top_members, "projects")? else {
        let member = format!("\"projects\":{{{}:{}}}", quoted(key)?, new_entry(edits)?);
        return Ok(insert_first(text, top, &member, top_members.is_empty()));
    };
    let project_members = members(bytes, projects.value)?;
    if find(&project_members, key)?.is_none() {
        let member = format!("{}:{}", quoted(key)?, new_entry(edits)?);
        return Ok(insert_first(
            text,
            projects.value,
            &member,
            project_members.is_empty(),
        ));
    }
    let mut edited = text.to_owned();
    for (member, value) in edits {
        let entry = entry_start(&edited, key)?;
        edited = set_member(&edited, entry, member, value)?;
    }
    Ok(edited)
}

/// Claude Code's new project entry with `edits` applied.
fn new_entry(edits: &[(&str, Value)]) -> Result<String, String> {
    let mut entry = NEW_PROJECT_ENTRY.to_owned();
    for (member, value) in edits {
        entry = set_member(&entry, 0, member, value)?;
    }
    Ok(entry)
}

/// Where the value of `projects[key]` starts in `text`, which has it.
fn entry_start(text: &str, key: &str) -> Result<usize, String> {
    let bytes = text.as_bytes();
    let top_members = members(bytes, skip_whitespace(bytes, 0))?;
    let projects = find(&top_members, "projects")?.ok_or("the config lost `projects`")?;
    let project_members = members(bytes, projects.value)?;
    Ok(find(&project_members, key)?
        .ok_or("the config lost the workspace's entry")?
        .value)
}

/// `text` with the member `name` of the object whose `{` is at `object` set to `value`: its
/// value replaced, or the member inserted first.
fn set_member(text: &str, object: usize, name: &str, value: &Value) -> Result<String, String> {
    let value = serde_json::to_string(value).map_err(|error| error.to_string())?;
    let object_members = members(text.as_bytes(), object)?;
    match find(&object_members, name)? {
        Some(member) => Ok(format!(
            "{}{value}{}",
            &text[..member.value],
            &text[member.value_end..]
        )),
        None => Ok(insert_first(
            text,
            object,
            &format!("{}:{value}", quoted(name)?),
            object_members.is_empty(),
        )),
    }
}

/// `text` with `member` inserted first in the object whose `{` is at `object`.
fn insert_first(text: &str, object: usize, member: &str, is_empty: bool) -> String {
    let at = object + 1;
    let separator = if is_empty { "" } else { "," };
    format!("{}{member}{separator}{}", &text[..at], &text[at..])
}

pub(crate) fn quoted(key: &str) -> Result<String, String> {
    serde_json::to_string(key).map_err(|error| error.to_string())
}

/// One member of a JSON object: its decoded name and its value's byte range.
pub(crate) struct Member {
    pub(crate) name: String,
    pub(crate) value: usize,
    pub(crate) value_end: usize,
}

/// The member named `name`; a name given twice is refused, since readers disagree on which
/// one counts.
pub(crate) fn find<'a>(members: &'a [Member], name: &str) -> Result<Option<&'a Member>, String> {
    let mut found = members.iter().filter(|member| member.name == name);
    let first = found.next();
    if found.next().is_some() {
        return Err(format!("the config names `{name}` twice"));
    }
    Ok(first)
}

/// The members of the object whose `{` is at `start`. The text is already known to be JSON,
/// so the scan only needs to find boundaries.
pub(crate) fn members(bytes: &[u8], start: usize) -> Result<Vec<Member>, String> {
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

pub(crate) fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
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
pub(crate) struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    pub(crate) fn acquire(config: &Path) -> Result<ConfigLock, String> {
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
pub(crate) fn replace(config: &Path, mode: u32, text: &str) -> Result<(), String> {
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
