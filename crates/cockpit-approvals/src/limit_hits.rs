//! What a confined run tried that its limits refused (#1387), read from what its programs
//! printed: the record and the pane name the hidden path read, the path written outside
//! the writable folders, the host reached, or sudo. A program that swallows its error leaves
//! nothing to read, so an empty list does not prove the run hit no limit.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// The hits named at most, so a run that loops on a refused write stays readable.
const MAX_HITS: usize = 8;

const READ_ONLY: &str = "Read-only file system";
const NO_NETWORK: &str = "tried to reach the network, which this run does not have";
const NO_ROOT_ROUTE: &str =
    "tried to use sudo, which needs the root route confined runs do not have yet";

/// Hosts as curl, Git, ssh and Node name the one they could not reach.
static HOST_REACHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"Could not resolve host: ([^\s'/:]+)",
        r"|Failed to connect to ([^\s'/]+) port",
        r"|Failed to connect to ([^\s'/:]+):\d+ after",
        r"|Could not resolve hostname ([^\s'/:]+)",
        r"|unable to access '[a-z]+://([^/'@:]+)",
        r"|getaddrinfo (?:ENOTFOUND|EAI_AGAIN) ([^\s'/:]+)",
    ))
    .expect("the pattern compiles")
});

/// A failure that names no host.
static NETWORK_GONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Network is unreachable|Temporary failure in name resolution")
        .expect("the pattern compiles")
});

/// What sudo says when it is not the setuid root program, as in a run's user namespace.
static SUDO_WITHOUT_ROOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"sudo: .*(?:must be owned by uid 0|effective uid is not 0|no new privileges)")
        .expect("the pattern compiles")
});

/// Each limit `output` shows the run hit, once, in the order it hit them. `hidden` are the
/// folders the run cannot see: a read refused there, or found missing in an emptied one,
/// names its path.
pub fn limit_hits(output: &str, hidden: &[PathBuf]) -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();
    for line in output.lines() {
        for hit in line_hits(line, hidden) {
            if hits.len() < MAX_HITS && !hits.contains(&hit) {
                hits.push(hit);
            }
        }
    }
    hits
}

/// The hit naming a destination a run's proxy refused: its card does not name it.
pub(crate) fn refused_by_proxy(destination: &str) -> String {
    format!("tried to reach {destination}, which the run's card does not name")
}

/// `hits`, then each of `more` not among them, at most [`MAX_HITS`] in all.
pub(crate) fn merged(mut hits: Vec<String>, more: Vec<String>) -> Vec<String> {
    for hit in more {
        if hits.len() < MAX_HITS && !hits.contains(&hit) {
            hits.push(hit);
        }
    }
    hits
}

fn line_hits(line: &str, hidden: &[PathBuf]) -> Vec<String> {
    let mut hits = Vec::new();
    let is_refused_read = line.contains("Permission denied")
        || line.contains("permission denied")
        || line.contains("No such file or directory");
    if is_refused_read {
        hits.extend(
            hidden_paths_in(line, hidden)
                .map(|path| format!("tried to read {path}, which confined runs cannot see")),
        );
    }
    if let Some(path) = written_path(line) {
        hits.push(format!(
            "tried to write {path}, outside the folders this run may write"
        ));
    }
    let hosts: Vec<&str> = HOST_REACHED
        .captures_iter(line)
        .filter_map(|found| found.iter().skip(1).flatten().next())
        .map(|host| host.as_str())
        .collect();
    hits.extend(
        hosts
            .iter()
            .map(|host| format!("tried to reach {host}, but this run has no network")),
    );
    if hosts.is_empty() && NETWORK_GONE.is_match(line) {
        hits.push(NO_NETWORK.to_owned());
    }
    if SUDO_WITHOUT_ROOT.is_match(line) {
        hits.push(NO_ROOT_ROUTE.to_owned());
    }
    hits
}

/// The paths in `line` under one of `hidden`.
fn hidden_paths_in<'line>(
    line: &'line str,
    hidden: &[PathBuf],
) -> impl Iterator<Item = &'line str> {
    let hidden: Vec<&Path> = hidden.iter().map(PathBuf::as_path).collect();
    path_tokens(line).map(|(_, path)| path).filter(move |path| {
        hidden
            .iter()
            .any(|folder| Path::new(path).starts_with(folder))
    })
}

/// The path a write refused as read-only names: programs print it before the error (`bash:
/// PATH: Read-only file system`) or, as Python does, after it.
fn written_path(line: &str) -> Option<&str> {
    let at = line.find(READ_ONLY)?;
    let paths: Vec<(usize, &str)> = path_tokens(line).collect();
    paths
        .iter()
        .find(|(start, _)| *start > at)
        .or_else(|| paths.iter().rev().find(|(start, _)| *start < at))
        .map(|(_, path)| *path)
}

/// Each absolute path in `line`, with where it starts: a `/` that begins a word, up to a
/// quote, a colon, a comma or a space.
fn path_tokens(line: &str) -> impl Iterator<Item = (usize, &str)> {
    let is_boundary = |character: char| {
        character.is_whitespace()
            || matches!(character, '\'' | '"' | ':' | ',' | '(' | ')' | '`')
            || matches!(character, '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}')
    };
    line.match_indices('/').filter_map(move |(start, _)| {
        let begins_word = line[..start].chars().next_back().is_none_or(is_boundary);
        if !begins_word {
            return None;
        }
        let rest = &line[start..];
        let end = rest.find(is_boundary).unwrap_or(rest.len());
        (end > 1).then(|| (start, &rest[..end]))
    })
}
