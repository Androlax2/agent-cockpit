//! `cockpit config show` driven as a process with an isolated home and environment.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Home {
    dir: TempDir,
}

impl Home {
    fn clean() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temporary home"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, relative_path: &str, contents: &str) -> PathBuf {
        let path = self.path().join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn write_user_config(&self, contents: &str) -> PathBuf {
        self.write(".config/cockpit/config.toml", contents)
    }

    fn show(&self, arguments: &[&str]) -> Output {
        self.show_with_env(arguments, &[])
    }

    fn show_with_env(&self, arguments: &[&str], variables: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cockpit"))
            .args(["config", "show"])
            .args(arguments)
            .env_clear()
            .env("HOME", self.path())
            .envs(variables.iter().copied())
            .output()
            .expect("cockpit runs")
    }
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr_of_failure(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected a configuration error"
    );
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// The line `show` printed for `key`.
fn line_for(shown: &str, key: &str) -> String {
    let prefix = format!("{key} = ");
    shown
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no line for {key} in:\n{shown}"))
        .to_string()
}

#[test]
fn a_clean_home_shows_working_defaults_for_every_value() {
    let home = Home::clean();
    let shown = stdout(&home.show(&[]));
    let home_path = home.path().display();

    assert_eq!(
        line_for(&shown, "paths.config_dir"),
        format!("paths.config_dir = \"{home_path}/.config/cockpit\"  # product default")
    );
    assert_eq!(
        line_for(&shown, "paths.state_dir"),
        format!("paths.state_dir = \"{home_path}/.local/state/cockpit\"  # product default")
    );
    assert_eq!(
        line_for(&shown, "files.user_config"),
        format!("files.user_config = \"{home_path}/.config/cockpit/config.toml\"  # absent")
    );
    assert_eq!(
        line_for(&shown, "projects.trusted"),
        "projects.trusted = []  # product default"
    );
    assert_eq!(
        line_for(&shown, "workflow.preset"),
        "workflow.preset = <unset>  # product default"
    );
    assert_eq!(
        line_for(&shown, "tasks.blocker_project"),
        "tasks.blocker_project = <unset>  # product default"
    );
    assert_eq!(
        line_for(&shown, "tasks.cross_project_open_max"),
        "tasks.cross_project_open_max = 3  # product default"
    );
}

#[test]
fn the_owners_task_settings_name_their_file_and_line() {
    let home = Home::clean();
    let user_config = home
        .write_user_config("[tasks]\nblocker_project = \"dotfiles\"\ncross_project_open_max = 5\n");
    let shown = stdout(&home.show(&[]));
    assert_eq!(
        line_for(&shown, "tasks.blocker_project"),
        format!(
            "tasks.blocker_project = \"dotfiles\"  # user config {}:2",
            user_config.display()
        )
    );
    assert_eq!(
        line_for(&shown, "tasks.cross_project_open_max"),
        format!(
            "tasks.cross_project_open_max = 5  # user config {}:3",
            user_config.display()
        )
    );
}

#[test]
fn every_shown_value_names_its_source() {
    let home = Home::clean();
    let shown = stdout(&home.show(&[]));
    for line in shown.lines() {
        assert!(line.contains(" = ") && line.contains("  # "), "{line}");
    }
}

#[test]
fn a_clean_home_declares_the_provider_owned_start_files() {
    let home = Home::clean();
    let shown = stdout(&home.show(&[]));
    let start_file_labels: Vec<&str> = shown
        .lines()
        .filter_map(|line| line.strip_prefix("start_files.\""))
        .map(|rest| rest.split('"').next().unwrap())
        .collect();
    assert_eq!(
        start_file_labels,
        [
            "CLAUDE.md",
            "settings",
            "plugins",
            "opencode config",
            "codex hooks"
        ]
    );
}

#[test]
fn xdg_variables_choose_the_directories_and_are_named_as_the_source() {
    let home = Home::clean();
    let shown = stdout(&home.show_with_env(
        &[],
        &[
            ("XDG_CONFIG_HOME", "/xdg/config"),
            ("XDG_STATE_HOME", "/xdg/state"),
            ("XDG_CACHE_HOME", "/xdg/cache"),
            ("XDG_RUNTIME_DIR", "/run/user/4242"),
        ],
    ));
    for expected in [
        "paths.config_dir = \"/xdg/config/cockpit\"  # environment $XDG_CONFIG_HOME",
        "paths.state_dir = \"/xdg/state/cockpit\"  # environment $XDG_STATE_HOME",
        "paths.cache_dir = \"/xdg/cache/cockpit\"  # environment $XDG_CACHE_HOME",
        "paths.runtime_dir = \"/run/user/4242/cockpit\"  # environment $XDG_RUNTIME_DIR",
    ] {
        assert!(
            shown.lines().any(|line| line == expected),
            "{expected}\n{shown}"
        );
    }
}

#[test]
fn an_unset_runtime_dir_is_shown_as_unset_not_invented() {
    let home = Home::clean();
    let shown = stdout(&home.show(&[]));
    assert_eq!(
        line_for(&shown, "paths.runtime_dir"),
        "paths.runtime_dir = <unset>  # $XDG_RUNTIME_DIR is not set"
    );
}

#[test]
fn a_relative_runtime_dir_is_ignored_without_claiming_a_default() {
    let home = Home::clean();
    let shown = stdout(&home.show_with_env(&[], &[("XDG_RUNTIME_DIR", "relative/run")]));
    assert_eq!(
        line_for(&shown, "paths.runtime_dir"),
        "paths.runtime_dir = <unset>  # $XDG_RUNTIME_DIR ignored: not an absolute path"
    );
}

#[test]
fn trusted_paths_are_normalized_so_they_compare_by_equality() {
    let home = Home::clean();
    let shown = stdout(&home.show(&["--set", "projects.trusted=[\"/srv/./work/\"]"]));
    assert_eq!(
        line_for(&shown, "projects.trusted"),
        "projects.trusted = [\"/srv/work\"]  # command line --set"
    );
}

#[test]
fn a_relative_xdg_variable_is_ignored_and_the_reason_shown() {
    let home = Home::clean();
    let shown = stdout(&home.show_with_env(&[], &[("XDG_STATE_HOME", "relative/state")]));
    assert_eq!(
        line_for(&shown, "paths.state_dir"),
        format!(
            "paths.state_dir = \"{}/.local/state/cockpit\"  # product default; \
             $XDG_STATE_HOME ignored: not an absolute path",
            home.path().display()
        )
    );
}

#[test]
fn the_user_config_is_read_from_xdg_config_home() {
    let home = Home::clean();
    let config_home = home.path().join("elsewhere");
    let config_path = home.write(
        "elsewhere/cockpit/config.toml",
        "workflow.preset = \"solo\"\n",
    );
    let shown =
        stdout(&home.show_with_env(&[], &[("XDG_CONFIG_HOME", config_home.to_str().unwrap())]));
    assert_eq!(
        line_for(&shown, "workflow.preset"),
        format!(
            "workflow.preset = \"solo\"  # user config {}:1",
            config_path.display()
        )
    );
}

#[test]
fn a_user_setting_names_its_file_and_line() {
    let home = Home::clean();
    let config_path =
        home.write_user_config("# mine\n\n[projects]\ntrusted = [\"~/code\", \"/srv/work\"]\n");
    let shown = stdout(&home.show(&[]));
    assert_eq!(
        line_for(&shown, "projects.trusted"),
        format!(
            "projects.trusted = [\"{}/code\", \"/srv/work\"]  # user config {}:4",
            home.path().display(),
            config_path.display()
        )
    );
}

#[test]
fn a_command_line_override_beats_the_user_config() {
    let home = Home::clean();
    home.write_user_config("[workflow]\npreset = \"solo\"\n");
    let shown = stdout(&home.show(&["--set", "workflow.preset=\"review\""]));
    assert_eq!(
        line_for(&shown, "workflow.preset"),
        "workflow.preset = \"review\"  # command line --set"
    );
}

#[test]
fn the_last_override_of_a_key_wins() {
    let home = Home::clean();
    let shown = stdout(&home.show(&[
        "--set",
        "workflow.preset=\"first\"",
        "--set",
        "workflow.preset=\"second\"",
    ]));
    assert_eq!(
        line_for(&shown, "workflow.preset"),
        "workflow.preset = \"second\"  # command line --set"
    );
}

#[test]
fn a_project_description_beats_the_user_default_and_loses_to_an_override() {
    let home = Home::clean();
    home.write_user_config("workflow.preset = \"solo\"\n");
    let project_config = home.write(
        "project/.cockpit/config.toml",
        "[workflow]\npreset = \"team\"\n",
    );
    let project = home.path().join("project");
    let project_arguments = ["--project", project.to_str().unwrap()];

    let shown = stdout(&home.show(&project_arguments));
    assert_eq!(
        line_for(&shown, "workflow.preset"),
        format!(
            "workflow.preset = \"team\"  # project config {}:2",
            project_config.display()
        )
    );

    let overridden = stdout(&home.show(&[
        project_arguments[0],
        project_arguments[1],
        "--set",
        "workflow.preset=\"review\"",
    ]));
    assert_eq!(
        line_for(&overridden, "workflow.preset"),
        "workflow.preset = \"review\"  # command line --set"
    );
}

#[test]
fn a_project_without_its_own_file_shows_the_file_as_absent() {
    let home = Home::clean();
    let project = home.path().join("project");
    fs::create_dir(&project).unwrap();
    let shown = stdout(&home.show(&["--project", project.to_str().unwrap()]));
    assert_eq!(
        line_for(&shown, "files.project_config"),
        format!(
            "files.project_config = \"{}/.cockpit/config.toml\"  # absent",
            project.display()
        )
    );
}

#[test]
fn a_project_config_cannot_set_owner_settings() {
    for (owner_setting, contents) in [
        ("projects", "\n[projects]\ntrusted = [\"/\"]\n"),
        (
            "start_files",
            "\n[[start_files]]\nlabel = \"x\"\npath = \"/x\"\nproviders = [\"claude\"]\n",
        ),
        ("tasks", "\n[tasks]\nblocker_project = \"elsewhere\"\n"),
    ] {
        let home = Home::clean();
        let project_config = home.write("project/.cockpit/config.toml", contents);
        let project = home.path().join("project");
        let stderr = stderr_of_failure(&home.show(&["--project", project.to_str().unwrap()]));
        assert_eq!(
            stderr,
            format!(
                "cockpit: project config {}:2: `{owner_setting}` is an owner setting; \
                 project configuration describes the project and cannot set it\n",
                project_config.display()
            )
        );
    }
}

#[test]
fn a_user_start_file_with_a_default_label_replaces_that_default() {
    let home = Home::clean();
    let config_path = home.write_user_config(
        "[[start_files]]\nlabel = \"CLAUDE.md\"\npath = \"/etc/agents/CLAUDE.md\"\nproviders = [\"claude\"]\n",
    );
    let shown = stdout(&home.show(&[]));
    assert_eq!(
        line_for(&shown, "start_files.\"CLAUDE.md\""),
        format!(
            "start_files.\"CLAUDE.md\" = {{ path = \"/etc/agents/CLAUDE.md\", providers = [\"claude\"] }}  \
             # user config {}:1",
            config_path.display()
        )
    );
}

#[test]
fn a_user_start_file_with_a_new_label_is_added_after_the_defaults() {
    let home = Home::clean();
    home.write_user_config(
        "[[start_files]]\nlabel = \"launcher\"\npath = \"~/.local/bin/launch\"\nproviders = [\"codex\"]\n",
    );
    let shown = stdout(&home.show(&[]));
    let last_line = shown.lines().last().unwrap();
    assert!(
        last_line.starts_with(&format!(
            "start_files.\"launcher\" = {{ path = \"{}/.local/bin/launch\", providers = [\"codex\"] }}",
            home.path().display()
        )),
        "{last_line}"
    );
}

#[test]
fn invalid_configuration_fails_loud_with_its_location() {
    let start_file = |fields: &str| format!("\n[[start_files]]\n{fields}\n");
    for (contents, expected_message) in [
        ("\nunknown_key = 1\n", "unknown field `unknown_key`"),
        (
            "[projects]\ntrusted = [\"relative/path\"]\n",
            "config.toml:2: `projects.trusted` entry \"relative/path\" is not an absolute path",
        ),
        (
            "[projects]\ntrusted = [\"~/a/../b\"]\n",
            "config.toml:2: `projects.trusted` entry \"~/a/../b\" contains `..`",
        ),
        (
            "workflow.preset = \"  \"\n",
            "config.toml:1: `workflow.preset` is empty",
        ),
        (
            "[tasks]\nblocker_project = \"\"\n",
            "config.toml:2: `tasks.blocker_project` is empty",
        ),
        (
            "[tasks]\ncross_project_open_max = -1\n",
            "config.toml:2: `tasks.cross_project_open_max` must be a whole number from 0",
        ),
        (
            &start_file("label = \"CLAUDE.md \"\npath = \"/x\"\nproviders = [\"claude\"]"),
            "config.toml:2: `start_files` entry label \"CLAUDE.md \" has surrounding whitespace",
        ),
        (
            &start_file("label = \"x\"\npath = \"/x\"\nproviders = [\"claud\"]"),
            "config.toml:2: `start_files` entry names unknown provider \"claud\"; known: claude, codex, opencode",
        ),
        (
            &start_file("label = \"x\"\npath = \"x\"\nproviders = [\"claude\"]"),
            "config.toml:2: `start_files` entry path \"x\" is not an absolute path",
        ),
        (
            &start_file("label = \"x\"\npath = \"/x\"\nproviders = []"),
            "config.toml:2: `start_files` entry names no providers",
        ),
        (
            &format!(
                "{}{}",
                start_file("label = \"x\"\npath = \"/x\"\nproviders = [\"claude\"]"),
                start_file("label = \"x\"\npath = \"/y\"\nproviders = [\"claude\"]")
            ),
            "config.toml:7: `start_files` entry repeats a label in this file",
        ),
    ] {
        let home = Home::clean();
        home.write_user_config(contents);
        let stderr = stderr_of_failure(&home.show(&[]));
        assert!(stderr.contains(expected_message), "{contents:?}:\n{stderr}");
    }
}

#[test]
fn an_override_must_name_an_overridable_key_with_a_one_line_value() {
    for (argument, expected_message) in [
        ("workflow.preset", "expected key=value"),
        (
            "start_files=[]",
            "unknown key `start_files`; --set accepts projects.trusted, workflow.preset",
        ),
        (
            "workflow.preset=\"a\"\n[projects]",
            "the value must be on one line",
        ),
        (
            "workflow.preset=solo",
            "--set workflow.preset=solo: TOML parse error",
        ),
    ] {
        let home = Home::clean();
        let stderr = stderr_of_failure(&home.show(&["--set", argument]));
        assert!(stderr.contains(expected_message), "{argument:?}:\n{stderr}");
    }
}

#[test]
fn a_missing_or_relative_home_fails_instead_of_guessing() {
    for (home_value, expected_message) in [
        (None, "cockpit: HOME is not set\n"),
        (
            Some("relative"),
            "cockpit: HOME is not an absolute path: relative\n",
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command.args(["config", "show"]).env_clear();
        if let Some(home_value) = home_value {
            command.env("HOME", home_value);
        }
        let stderr = stderr_of_failure(&command.output().unwrap());
        assert_eq!(stderr, expected_message);
    }
}

#[test]
fn a_project_that_is_not_a_directory_fails() {
    let home = Home::clean();
    let missing = home.path().join("missing");
    let stderr = stderr_of_failure(&home.show(&["--project", missing.to_str().unwrap()]));
    assert_eq!(
        stderr,
        format!("cockpit: project {}: not a directory\n", missing.display())
    );
}

#[test]
fn a_rejected_argument_is_named_before_the_usage() {
    let home = Home::clean();
    for (arguments, problem) in [
        (&["--frobnicate", "x"][..], "unknown argument --frobnicate"),
        (&["--set"][..], "--set needs a value"),
        (
            &["--project", "--set", "workflow.preset=\"x\""][..],
            "--project needs a value",
        ),
        (
            &["--project", "/", "--project", "/"][..],
            "--project given twice",
        ),
    ] {
        let output = home.show(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr,
            format!(
                "cockpit: {problem}\n\
                 usage: cockpit config show [--project DIR] [--set KEY=VALUE]...\n       \
                 cockpit agent report [--state working|waiting] [--label TEXT] [--doing TEXT] \
                 [--needs-input QUESTION | --dialog QUESTION] [--prompt] \
                 [--outcome completed|failed|quota-stopped] [--outcome-reason TEXT] \
                 [--conversation ID [--conversation-ends]]\n       \
                 cockpit agent reports\n       \
                 cockpit project add DIR [--name NAME]\n       \
                 cockpit project list\n       \
                 cockpit checkout list [PROJECT]\n       \
                 cockpit checkout clone PROJECT BRANCH\n       \
                 cockpit checkout review PROJECT PULL_REQUEST\n       \
                 cockpit checkout claim CHECKOUT HOLDER\n       \
                 cockpit checkout release CHECKOUT HOLDER\n       \
                 cockpit checkout sweep [--dry-run]\n       \
                 cockpit checkout push CHECKOUT\n       \
                 cockpit ask run [--cwd DIR] --reason TEXT COMMAND | --wait ID | --check\n       \
                 cockpit ask guide [--cwd DIR] < GUIDE | --wait ID\n       \
                 cockpit approval list | show ID | decline ID | input ID < LINE\n       \
                 cockpit approval approve ID --revision REVISION --digest DIGEST\n       \
                 cockpit approval edit ID --revision REVISION COMMAND\n       \
                 cockpit guide show ID | answer ID STEP [ANSWER] | stuck ID STEP NOTE | close ID\n       \
                 cockpit session new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]\n       \
                 cockpit session list [--ended]\n       \
                 cockpit session restart SESSION|--all\n       \
                 cockpit session sleep|wake|resume|end|show SESSION\n       \
                 cockpit session end --finished\n       \
                 cockpit session link SESSION URL\n       \
                 cockpit provider list\n       \
                 cockpit tui\n       \
                 cockpit version [--all]\n       \
                 cockpit todo add TITLE [--project P] [--notes TEXT] [--kind KIND] \
                 [--priority 1-6] [--after ID]...\n       \
                 cockpit todo propose TITLE --why TEXT [--project P] [--notes TEXT] [--kind KIND]\n       \
                 cockpit todo blocked TITLE --why TEXT [--notes TEXT]\n       \
                 cockpit todo list [--project P] [--all] | show ID | delete ID\n       \
                 cockpit todo start ID | done ID [--summary TEXT] | reopen ID\n       \
                 cockpit todo edit ID [--revision R] [--title T] [--notes TEXT] [--kind KIND]\n       \
                 cockpit todo priority ID LEVEL | queue ID | unqueue ID | approve ID | decline ID\n       \
                 cockpit todo after ID ON... | unwait ID ON...\n       \
                 cockpit todo image add ID FILE | image get ID NUMBER FILE\n       \
                 cockpit todo draft save (ID | --new PROJECT) [--title T] [--notes TEXT] [--revision R]\n       \
                 cockpit todo drafts | draft discard (ID | --new PROJECT)\n       \
                 cockpit resources status | profile normal|foreground | pause-queue | resume-queue\n       \
                 cockpit resources check WORKLOAD --cpu MILLICORES --memory MIB [--require CONTROL]...\n       \
                 cockpit resources checkpoint RESERVATION | stop RESERVATION\n"
            )
        );
    }
}

#[test]
fn a_non_utf8_project_path_is_reported_not_a_crash() {
    use std::os::unix::ffi::OsStrExt;
    let home = Home::clean();
    let project = home
        .path()
        .join(std::ffi::OsStr::from_bytes(b"project\xff"));
    let output = Command::new(env!("CARGO_BIN_EXE_cockpit"))
        .args(["config", "show", "--project"])
        .arg(&project)
        .env_clear()
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).ends_with(": not a directory\n"));
}

#[test]
fn an_unknown_verb_prints_usage_and_exits_2() {
    for arguments in [&[][..], &["config"][..], &["configure", "show"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_cockpit"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: cockpit "));
    }
}
