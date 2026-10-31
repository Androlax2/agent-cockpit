//! What the owner is told about a command beyond its text (#156): a `timeout` that takes
//! the terminal from it, and the build files or scripts the session changed that it runs.
//! Both read the command's words roughly, the way the owner would: a hint too many is
//! read and dismissed, a hint missing hides what runs.

use std::path::Path;

/// Whether the command runs `timeout` without `--foreground`. Without it `timeout` puts the
/// command in a background process group: a prompt (`sudo`, `read`) stops it instead of
/// reading the terminal, and the run hangs until the timeout. `timeout` counts as run
/// wherever its options and a duration follow it (`env X=1 timeout 5 …`, `then timeout …`).
pub fn timeout_without_foreground(command: &str) -> bool {
    let words = words(command);
    words.iter().enumerate().any(|(index, word)| {
        program_name(word) == "timeout" && matches!(read_options(&words[index + 1..]), Some(false))
    })
}

/// Reads `timeout`'s options up to its duration: whether one was `--foreground`, or `None`
/// when no duration follows (`echo timeout`, so not the program). Option values (`-s KILL`,
/// `-k 5`) are skipped.
fn read_options(after: &[&str]) -> Option<bool> {
    let mut is_foreground = false;
    let mut words = after.iter();
    while let Some(word) = words.next() {
        if !word.starts_with('-') || word.len() < 2 {
            return is_duration(word).then_some(is_foreground);
        }
        is_foreground |= is_foreground_option(word);
        if takes_value(word) {
            words.next();
        }
    }
    None
}

/// `--foreground` or an unambiguous abbreviation of it (`--f`), or `-f` alone or in a
/// cluster of short options (`-fk`).
fn is_foreground_option(option: &str) -> bool {
    match option.strip_prefix("--") {
        Some(long) => !long.is_empty() && "foreground".starts_with(long),
        None => option[1..].contains('f'),
    }
}

/// Whether `option` takes the next word as its value: `-s`/`--signal`, `-k`/`--kill-after`,
/// or a short cluster ending in one of them (`-fk 5`).
fn takes_value(option: &str) -> bool {
    matches!(option, "--signal" | "--kill-after")
        || (!option.starts_with("--") && (option.ends_with('s') || option.ends_with('k')))
}

/// `5`, `1.5`, `30s`, `2m`, `1h`, `1d`: `timeout`'s duration.
fn is_duration(word: &str) -> bool {
    let number = word.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(word);
    !number.is_empty()
        && number.chars().any(|character| character.is_ascii_digit())
        && number
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
}

/// Which of `changed_files` (paths from the checkout's root) `command` runs: build files of
/// the tools it calls (`make` → Makefiles, `cargo` → `build.rs`, a package manager →
/// `package.json`, `gh` → workflows, `git` → hook managers' files) and files it names.
/// Sorted, each once.
pub fn build_files_run(command: &str, changed_files: &[String]) -> Vec<String> {
    let words = words(command);
    let mut run: Vec<String> = changed_files
        .iter()
        .filter(|changed| {
            words
                .iter()
                .any(|word| tool_runs(program_name(word), changed) || names_file(word, changed))
        })
        .cloned()
        .collect();
    run.sort();
    run.dedup();
    run
}

fn tool_runs(program: &str, changed: &str) -> bool {
    let file_name = Path::new(changed)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(changed);
    match program {
        "make" | "gmake" => {
            matches!(file_name, "Makefile" | "makefile" | "GNUmakefile")
                || file_name.ends_with(".mk")
        }
        "cargo" => file_name == "build.rs" || changed.contains(".cargo/config"),
        "npm" | "npx" | "pnpm" | "yarn" | "bun" => file_name == "package.json",
        "gh" | "act" => changed.starts_with(".github/workflows/"),
        "git" | "pre-commit" | "lefthook" => {
            changed.starts_with(".husky/")
                || changed.starts_with(".githooks/")
                || matches!(
                    file_name,
                    ".pre-commit-config.yaml" | "lefthook.yml" | "lefthook.yaml"
                )
        }
        _ => false,
    }
}

/// Whether `word` names `changed`: the same path, or one ending in the other at a `/` (the
/// command may run from a subdirectory, or name the file absolutely). A changed directory
/// (`build/`) is named by any path inside it.
fn names_file(word: &str, changed: &str) -> bool {
    let word = word.trim_start_matches("./");
    if let Some(directory) = changed.strip_suffix('/') {
        return word.starts_with(changed) || word.contains(&format!("/{directory}/"));
    }
    !word.is_empty()
        && (word == changed
            || changed.ends_with(&format!("/{word}"))
            || word.ends_with(&format!("/{changed}")))
}

/// The program a word names: its last path component (`/usr/bin/make` is `make`).
fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The command's words, split at blanks, quotes and shell operators; `;` stands for every
/// operator that starts another command.
fn words(command: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in command.char_indices() {
        let is_separator = character.is_whitespace() || "'\"`$(){}<>".contains(character);
        let is_operator = ";|&\n".contains(character);
        if is_separator || is_operator {
            if let Some(word_start) = start.take() {
                words.push(&command[word_start..index]);
            }
            if is_operator && words.last() != Some(&";") {
                words.push(";");
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(word_start) = start {
        words.push(&command[word_start..]);
    }
    words
}
