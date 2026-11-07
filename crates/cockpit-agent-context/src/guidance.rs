//! The guidance an agent's harness reads besides the product's own packets (#420): the owner's
//! user file and the session's project files. They join the manifest with their path and hash,
//! the trust decision that covers them and a precedence below the product's invariants, so the
//! owner and the agent can see which guidance shadows which. Listing a file is not a promise
//! that a harness loads it: each harness decides that, and an `@`-imported file is part of the
//! file that imports it, so changing only an import does not change the manifest.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::manifest::{ManifestEntry, Precedence, Scope, Trust, UNREADABLE, hex};

/// Under the owner's home.
const USER_FILES: [&str; 1] = [".claude/CLAUDE.md"];

/// Under the project root.
const PROJECT_FILES: [&str; 3] = ["CLAUDE.md", ".claude/CLAUDE.md", "AGENTS.md"];

/// Under the project root: every `.md` file directly inside is a rule.
const PROJECT_RULE_DIRECTORIES: [&str; 2] = [".agents/rules", ".claude/rules"];

/// Where to look for guidance, and whether the owner trusts the project it belongs to.
#[derive(Debug, Clone, Copy)]
pub struct GuidanceSearch<'a> {
    /// The owner's home; none when the daemon cannot tell, which lists no user guidance.
    pub home: Option<&'a Path>,
    /// The checkout the session works in, or its directory when that is no registered project.
    pub project_root: &'a Path,
    /// The owner's decision on the project; a directory that is no project is not trusted.
    pub project_is_trusted: bool,
}

/// The guidance files that exist, user first, then project: the order of their precedence.
/// A file that exists but cannot be read is listed with [`UNREADABLE`] for a hash.
pub fn discover_guidance(search: &GuidanceSearch<'_>) -> Vec<ManifestEntry> {
    let user_files: Vec<(PathBuf, String)> = search
        .home
        .into_iter()
        .flat_map(|home| USER_FILES.map(|relative| home.join(relative)))
        .filter_map(hash_file)
        .collect();
    let project_files: Vec<(PathBuf, String)> = project_candidates(search.project_root)
        .into_iter()
        .filter(|(path, _)| user_files.iter().all(|(user_path, _)| user_path != path))
        .collect();
    let project_paths: Vec<String> = project_files
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect();
    let project_trust = if search.project_is_trusted {
        Trust::Trusted
    } else {
        Trust::Untrusted
    };
    let user_entries = user_files.into_iter().map(|(path, sha256)| ManifestEntry {
        path: path.display().to_string(),
        source: "user".to_owned(),
        scope: Scope::User,
        version: None,
        sha256,
        trust: Trust::Owner,
        precedence: Precedence::User,
        supersedes: project_paths.clone(),
    });
    let project_entries = project_files
        .into_iter()
        .map(|(path, sha256)| ManifestEntry {
            path: path.display().to_string(),
            source: "project".to_owned(),
            scope: Scope::Project,
            version: None,
            sha256,
            trust: project_trust,
            precedence: Precedence::Project,
            supersedes: Vec::new(),
        });
    user_entries.chain(project_entries).collect()
}

fn project_candidates(root: &Path) -> Vec<(PathBuf, String)> {
    let mut files: Vec<(PathBuf, String)> = PROJECT_FILES
        .iter()
        .filter_map(|relative| hash_file(root.join(relative)))
        .collect();
    for relative in PROJECT_RULE_DIRECTORIES {
        files.extend(rule_files(&root.join(relative)));
    }
    files
}

/// The `.md` files directly in `directory`, by name; an unreadable directory is itself listed
/// as unreadable instead of silently holding no rules.
fn rule_files(directory: &Path) -> Vec<(PathBuf, String)> {
    let names = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(_) => return vec![(directory.to_path_buf(), UNREADABLE.to_owned())],
    };
    let mut paths: Vec<PathBuf> = names
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    paths.sort();
    paths.into_iter().filter_map(hash_file).collect()
}

/// The path with the hash of the file it names; none when nothing is there or it is no file.
fn hash_file(path: PathBuf) -> Option<(PathBuf, String)> {
    let sha256 = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => match fs::read(&path) {
            Ok(bytes) => hex(&Sha256::digest(bytes)),
            Err(_) => UNREADABLE.to_owned(),
        },
        Ok(_) => return None,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(_) => UNREADABLE.to_owned(),
    };
    Some((path, sha256))
}
