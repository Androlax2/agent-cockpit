//! No test or tool reaches tmux's default socket directory, where the owner's installed
//! Torlek runs its server (#845, `.agents/rules/ci.md`). `tmux -L <name>` takes
//! `$TMUX_TMPDIR` only when that directory exists and otherwise falls back to `/tmp`; a
//! daemon started without `TMUX_TMPDIR` puts its server there too.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate lives in crates/<name>/")
        .to_path_buf()
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|error| panic!("{dir:?}: {error}")) {
            let path = entry.expect("entry is readable").path();
            let name = path.file_name().unwrap_or_default();
            if name == "target" || name == "__pycache__" {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn rust_sources() -> Vec<PathBuf> {
    files_under(&repo_root().join("crates"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect()
}

/// `path:line: text` for each line of `path` that `offends`.
fn offending_lines(path: &Path, offends: impl Fn(&str) -> bool) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let relative = path.strip_prefix(repo_root()).unwrap_or(path);
    text.lines()
        .enumerate()
        .filter(|(_, line)| offends(line))
        .map(|(index, line)| format!("{}:{}: {}", relative.display(), index + 1, line.trim()))
        .collect()
}

/// Whether `line`, outside a comment, passes tmux a `-L` option: `-L` (or `-Lname`) after
/// whitespace or a quote on a line that runs `tmux`, or a quoted `-L…` argument anywhere in
/// Rust, where a command's words sit on lines of their own. A text check: a `-L` on the line
/// after `tmux` in a shell command passes it.
fn passes_dash_l(line: &str, is_rust: bool) -> bool {
    let code = line.trim_start();
    if code.starts_with('#') || code.starts_with("//") {
        return false;
    }
    let option: String = ['-', 'L'].iter().collect();
    let after_quote = |quote: char| line.contains(&format!("{quote}{option}"));
    if is_rust && after_quote('"') {
        return true;
    }
    line.contains("tmux")
        && line.match_indices(&option).any(|(at, _)| {
            line[..at]
                .chars()
                .next_back()
                .is_none_or(|before| before.is_whitespace() || before == '"' || before == '\'')
        })
}

#[test]
fn no_crate_names_a_tmux_server_with_dash_l() {
    let offenders: Vec<String> = rust_sources()
        .iter()
        .flat_map(|path| offending_lines(path, |line| passes_dash_l(line, true)))
        .collect();
    assert!(
        offenders.is_empty(),
        "name the tmux server by path (`-S`), never by `-L`:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn no_tool_names_a_tmux_server_with_dash_l() {
    let offenders: Vec<String> = files_under(&repo_root().join("tools"))
        .iter()
        .flat_map(|path| offending_lines(path, |line| passes_dash_l(line, false)))
        .collect();
    assert!(
        offenders.is_empty(),
        "name the tmux server by path (`tmux -S`), never by `-L`:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_dash_l_check_catches_the_usual_spellings() {
    let option: String = ['-', 'L'].iter().collect();
    for caught in [
        format!("tmux {option} torlek kill-server"),
        format!("tmux {option}torlek kill-server"),
        format!("    tmux {option}\"$name\" kill-server"),
        format!("[\"tmux\", \"{option}\", name]"),
    ] {
        assert!(passes_dash_l(&caught, false), "{caught}");
    }
    assert!(passes_dash_l(
        &format!("    .args([\"{option}torlek\"])"),
        true
    ));
    for passed in [
        format!("# Never `tmux {option}`: it falls back to /tmp"),
        "tmux -S \"$socket\" kill-server".to_owned(),
        format!("[[ {option} $file ]]"),
        format!("tmux select-pane{option}"),
    ] {
        assert!(!passes_dash_l(&passed, false), "{passed}");
    }
}

#[test]
fn every_test_that_starts_a_daemon_gives_it_a_private_tmux_tmpdir() {
    // How each fixture runs the daemon program: `cockpit-daemon`'s homes, `cockpit-cli`'s
    // and the TUI's end-to-end harness.
    let starts_a_daemon = [
        "new(candidate::program(\"torlek-daemon\"))",
        "base_command(&daemon_binary())",
        "examples/cockpit-e2e-daemon",
    ];
    let offenders: Vec<String> = rust_sources()
        .into_iter()
        .filter(|path| path.components().any(|part| part.as_os_str() == "tests"))
        .filter(|path| !path.ends_with(file!()))
        .filter(|path| {
            let text = fs::read_to_string(path).unwrap_or_default();
            starts_a_daemon.iter().any(|start| text.contains(start))
                && !text.contains("\"TMUX_TMPDIR\"")
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "these start a daemon without setting TMUX_TMPDIR: {offenders:#?}"
    );
}
