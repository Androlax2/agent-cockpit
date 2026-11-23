//! A fake `gh`: a script that records each call (arguments, environment, standard input,
//! folder) and replays the answer given for exactly those arguments. No test reaches GitHub.

#![allow(dead_code, reason = "each test file uses some of these")]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_forge_api::{Access, ForgeHost, Repository, RepositoryPath, Token};
use cockpit_github::GitHub;
use tempfile::TempDir;

pub const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gh-2.104.0");
/// A fine-grained token's shape; never a real one.
pub const TOKEN: &str = "github_pat_11FAKEFAKE0fakefakefake";
pub const VERSION_OUTPUT: &str =
    "gh version 2.104.0 (2026-11-12)\nhttps://github.com/cli/cli/releases/tag/v2.104.0\n";

const SCRIPT: &str = r#"#!/bin/sh
root=__ROOT__
call="$root/calls/$(ls "$root/calls" | wc -l | tr -d ' ')"
mkdir "$call"
for argument in "$@"; do printf '%s\0' "$argument"; done > "$call/arguments"
env -0 > "$call/environment"
pwd > "$call/folder"
cat > "$call/stdin"
for answer in "$root"/answers/*; do
    [ -d "$answer" ] || continue
    if cmp -s "$answer/arguments" "$call/arguments"; then
        cat "$answer/stdout"
        cat "$answer/stderr" >&2
        exit "$(cat "$answer/status")"
    fi
done
echo "fake gh: no answer for: $*" >&2
exit 97
"#;

pub struct FakeGh {
    root: TempDir,
    answers: usize,
}

/// One call the fake received.
#[derive(Debug)]
pub struct Call {
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub stdin: String,
    pub folder: PathBuf,
}

impl FakeGh {
    /// A fake that answers `--version` as `gh` 2.104.0 does.
    pub fn new() -> FakeGh {
        let root = tempfile::Builder::new()
            .prefix("fake-gh-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        for folder in ["bin", "calls", "answers", "config"] {
            fs::create_dir(root.path().join(folder)).unwrap();
        }
        let script = SCRIPT.replace("__ROOT__", &shell_quoted(root.path()));
        let program = root.path().join("bin/gh");
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let mut fake = FakeGh { root, answers: 0 };
        fake.answer(&["--version"], 0, VERSION_OUTPUT, "");
        fake
    }

    /// Answers a call with exactly `arguments`.
    pub fn answer(
        &mut self,
        arguments: &[&str],
        status: i32,
        stdout: &str,
        stderr: &str,
    ) -> &mut FakeGh {
        let folder = self
            .root
            .path()
            .join("answers")
            .join(format!("{:03}", self.answers));
        self.answers += 1;
        fs::create_dir(&folder).unwrap();
        let joined: String = arguments
            .iter()
            .map(|argument| format!("{argument}\0"))
            .collect();
        fs::write(folder.join("arguments"), joined).unwrap();
        fs::write(folder.join("stdout"), stdout).unwrap();
        fs::write(folder.join("stderr"), stderr).unwrap();
        fs::write(folder.join("status"), status.to_string()).unwrap();
        self
    }

    /// Answers with a fixture's text on standard output.
    pub fn answer_with(&mut self, arguments: &[&str], fixture: &str) -> &mut FakeGh {
        self.answer(arguments, 0, &read_fixture(fixture), "")
    }

    /// Answers the three calls of a protection read of `owner/project`, whose default branch
    /// is `main`: its rules with `rules_fixture`, and its `protected` mark with
    /// `protected_fixture`.
    pub fn answer_protection_read(
        &mut self,
        rules_fixture: &str,
        protected_fixture: &str,
    ) -> &mut FakeGh {
        self.answer_with(
            &["api", "repos/owner/project", "--jq=.default_branch"],
            "api-default-branch.stdout",
        )
        .answer_with(
            &[
                "api",
                "repos/owner/project/rules/branches/main?per_page=100",
            ],
            rules_fixture,
        )
        .answer_with(
            &[
                "api",
                "repos/owner/project/branches/main",
                "--jq=.protected",
            ],
            protected_fixture,
        )
    }

    /// Fails with a fixture's text on standard error, as `gh` does.
    pub fn fail_with(&mut self, arguments: &[&str], fixture: &str) -> &mut FakeGh {
        self.answer(arguments, 1, "", &read_fixture(fixture))
    }

    pub fn program(&self) -> PathBuf {
        self.root.path().join("bin/gh")
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.path().join("config")
    }

    pub fn adapter(&self) -> GitHub {
        GitHub::new(self.program(), self.config_dir())
    }

    /// Every call after `--version`, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.every_call()
            .into_iter()
            .filter(|call| call.arguments != ["--version"])
            .collect()
    }

    /// Every call, `--version` included, in order.
    pub fn every_call(&self) -> Vec<Call> {
        let calls = self.root.path().join("calls");
        let mut numbered: Vec<(usize, PathBuf)> = fs::read_dir(&calls)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .map(|path| {
                let number = path.file_name().unwrap().to_str().unwrap().parse().unwrap();
                (number, path)
            })
            .collect();
        numbered.sort();
        numbered
            .into_iter()
            .map(|(_, path)| read_call(&path))
            .collect()
    }

    /// The one call after `--version`.
    pub fn only_call(&self) -> Call {
        let mut calls = self.calls();
        assert_eq!(calls.len(), 1, "{calls:#?}");
        calls.remove(0)
    }
}

fn read_call(path: &Path) -> Call {
    let split = |bytes: Vec<u8>| -> Vec<String> {
        bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8(part.to_vec()).unwrap())
            .collect()
    };
    let environment = split(fs::read(path.join("environment")).unwrap())
        .into_iter()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
        })
        .collect();
    Call {
        arguments: split(fs::read(path.join("arguments")).unwrap()),
        environment,
        stdin: fs::read_to_string(path.join("stdin")).unwrap(),
        folder: PathBuf::from(fs::read_to_string(path.join("folder")).unwrap().trim_end()),
    }
}

pub fn read_fixture(name: &str) -> String {
    fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()
}

fn shell_quoted(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

pub fn repository() -> Repository {
    Repository {
        host: ForgeHost::new("github.com").unwrap(),
        path: RepositoryPath::new("owner/project").unwrap(),
    }
}

pub fn token() -> Token {
    Token::new(TOKEN).unwrap()
}

/// Runs `call` with an [`Access`] to `owner/project` on GitHub made with [`TOKEN`].
pub fn with_access<T>(call: impl FnOnce(&Access<'_>) -> T) -> T {
    let repository = repository();
    let token = token();
    call(&Access {
        repository: &repository,
        token: &token,
    })
}
