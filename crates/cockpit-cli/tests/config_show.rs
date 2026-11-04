//! `cockpit config show` driven as a process with an isolated home and environment, for the
//! default profile's layout (`--profile cockpit`: this test build would run `dev`, #196).

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
        self.write(".config/torlek/config.toml", contents)
    }

    fn show(&self, arguments: &[&str]) -> Output {
        self.show_with_env(arguments, &[])
    }

    fn show_with_env(&self, arguments: &[&str], variables: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_torlek"))
            .args(["--profile", "torlek", "config", "show"])
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
        format!("paths.config_dir = \"{home_path}/.config/torlek\"  # product default")
    );
    assert_eq!(
        line_for(&shown, "paths.state_dir"),
        format!("paths.state_dir = \"{home_path}/.local/state/torlek\"  # product default")
    );
    assert_eq!(
        line_for(&shown, "files.user_config"),
        format!("files.user_config = \"{home_path}/.config/torlek/config.toml\"  # absent")
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
        "paths.config_dir = \"/xdg/config/torlek\"  # environment $XDG_CONFIG_HOME",
        "paths.state_dir = \"/xdg/state/torlek\"  # environment $XDG_STATE_HOME",
        "paths.cache_dir = \"/xdg/cache/torlek\"  # environment $XDG_CACHE_HOME",
        "paths.runtime_dir = \"/run/user/4242/torlek\"  # environment $XDG_RUNTIME_DIR",
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
            "paths.state_dir = \"{}/.local/state/torlek\"  # product default; \
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
        "elsewhere/torlek/config.toml",
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
        "project/.torlek/config.toml",
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
            "files.project_config = \"{}/.torlek/config.toml\"  # absent",
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
        let project_config = home.write("project/.torlek/config.toml", contents);
        let project = home.path().join("project");
        let stderr = stderr_of_failure(&home.show(&["--project", project.to_str().unwrap()]));
        assert_eq!(
            stderr,
            format!(
                "torlek: project config {}:2: `{owner_setting}` is an owner setting; \
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
        (None, "torlek: HOME is not set\n"),
        (
            Some("relative"),
            "torlek: HOME is not an absolute path: relative\n",
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_torlek"));
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
        format!("torlek: project {}: not a directory\n", missing.display())
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
                "torlek: {problem}\n\
                  usage: torlek [--profile NAME] VERB ...\n       \
                  torlek config show [--project DIR] [--set KEY=VALUE]...\n       \
                  torlek config reload\n       \
                  torlek logs level [DIRECTIVES [--for Ns|Nm] | --revert]\n       \
                  torlek agent report [--state working|waiting] [--label TEXT] [--doing TEXT] \
                 [--needs-input QUESTION | --dialog QUESTION] [--prompt] \
                 [--outcome completed|failed|quota-stopped] [--outcome-reason TEXT] \
                 [--conversation ID [--conversation-ends]]\n       \
                  torlek agent reports\n       \
                  torlek agent sessions [--watch]\n       \
                  torlek agent message SESSION TEXT\n       \
                  torlek agent inbox\n       \
                  torlek agent escalate SESSION\n       \
                  torlek agent start DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH] [--model MODEL] [--effort LEVEL]\n       \
                  torlek agent report-bug DESCRIPTION [--kind timeout|unreachable|internal|other] \
                 [--component NAME] [--steps TEXT] [--requested-by-owner]\n       \
                  torlek bug-report new DESCRIPTION [--title TEXT] [--steps TEXT] [--since 2h|RFC3339]\n       \
                  torlek bug-report list [--all] | show ID | discard ID\n       \
                  torlek bug-report edit ID [--title TEXT] [--description TEXT] [--steps TEXT]\n       \
                  torlek bug-report send ID [--gh | --print] [--anyway] | send ID --filed URL\n       \
                  torlek project add DIR [--name NAME]\n       \
                  torlek project list\n       \
                  torlek project remove PROJECT\n       \
                  torlek checkout list [PROJECT]\n       \
                  torlek checkout clone PROJECT BRANCH\n       \
                  torlek checkout review PROJECT PULL_REQUEST\n       \
                  torlek checkout claim CHECKOUT HOLDER\n       \
                  torlek checkout release CHECKOUT HOLDER\n       \
                  torlek checkout sweep [--dry-run]\n       \
                  torlek checkout push CHECKOUT\n       \
                  torlek storage\n       \
                  torlek storage prune CHECKOUT [--dry-run] | --plan ID --revision N --epoch E --confirm | status ID | abandon ID --revision N --epoch E\n       \
                  torlek ask run [--cwd DIR] --reason TEXT COMMAND | --wait ID | --check\n       \
                  torlek ask guide [--cwd DIR] < GUIDE | --wait ID\n       \
                  torlek approval list | show ID | decline ID | input ID < LINE\n       \
                  torlek approval approve ID --revision REVISION --digest DIGEST\n       \
                  torlek approval edit ID --revision REVISION COMMAND\n       \
                  torlek guide show ID | answer ID STEP [ANSWER] | stuck ID STEP NOTE | close ID\n       \
                  torlek session send ID MESSAGE | cancel ID\n       \
                  torlek session new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH] [--model MODEL] [--effort LEVEL]\n       \
                  torlek session list [--ended]\n       \
                  torlek session restart SESSION|--all\n       \
                  torlek session sleep|wake|resume|end|show SESSION\n       \
                  torlek session end --finished\n       \
                  torlek session link SESSION URL\n       \
                  torlek session grant SESSION observe-sessions|message-sessions|start-sessions [--per-hour N [--without-confirmation]]\n       \
                  torlek session revoke SESSION observe-sessions|message-sessions|start-sessions\n       \
                  torlek session grants SESSION\n       \
                  torlek session sandbox SESSION\n       \
                  torlek session sandbox-record SESSION --client CLIENT --observed TEXT...\n       \
                  torlek sandbox install [--engines-only]\n       \
                  torlek sandbox status|restart|stop\n       \
                  torlek sandbox show DIR [--agent NAME]\n       \
                  torlek provider list\n       \
                  torlek desktop-session import\n       \
                  torlek init\n       \
                  torlek doctor [--project DIR]\n       \
                  torlek tui [--version]\n       \
                  torlek version [--all]\n       \
                  torlek daemon install\n       \
                  torlek todo add TITLE [--project P] [--notes TEXT] [--kind KIND] \
                 [--priority 1-6] [--after ID]...\n       \
                 torlek todo propose TITLE --why TEXT [--project P] [--notes TEXT] [--kind KIND]\n       \
                 torlek todo blocked TITLE --why TEXT [--notes TEXT]\n       \
                 torlek todo list [--project P] [--all] | show ID | delete ID\n       \
                 torlek todo start ID | done ID [--summary TEXT] | reopen ID\n       \
                 torlek todo targets\n       \
                 torlek todo handoff ID SESSION --revision R --state-revision R --launch N\n       \
                 torlek todo edit ID [--revision R] [--title T] [--notes TEXT] [--kind KIND]\n       \
                 torlek todo priority ID LEVEL | queue ID | unqueue ID | approve ID | decline ID\n       \
                 torlek todo after ID ON... | unwait ID ON...\n       \
                 torlek todo image add ID FILE | image get ID NUMBER FILE\n       \
                 torlek todo draft save (ID | --new PROJECT) [--title T] [--notes TEXT] [--revision R]\n       \
                 torlek todo drafts | draft discard (ID | --new PROJECT)\n       \
                 torlek workflow presets | start ITEM [--preset NAME] | cancel ID\n       \
                 torlek workflow list [--project P] [--all] | show ID | packet ID\n       \
                 torlek workflow begin ID | checkpoint ID NOTE | unblock ID\n       \
                 torlek workflow complete ID [--step N --attempt N] --changed T --checked T --outcome T \
                 [--needs-human T] [--evidence REF]... [--waive REQUIREMENT]...\n       \
                  torlek workflow release ID [--changed T --checked T --outcome T \
                 [--needs-human T] [--evidence REF]...]\n       \
                  torlek workflow block ID ACTION --detail TEXT\n       \
                  torlek check run CHECK --project P [--checkout PATH] | gate [--project P] [--checkout PATH]\n       \
                  torlek check pending --project P [--after N]\n       \
                  torlek check resolve-pending RUN --project P --revision N --epoch E\n       \
                  torlek check claim CHECK STATEMENT\n       \
                  torlek resources status | profile normal|foreground | pause-queue | resume-queue\n       \
                  torlek resources check WORKLOAD --cpu MILLICORES --memory MIB [--require CONTROL]...\n       \
                  torlek resources checkpoint RESERVATION | stop RESERVATION\n       \
                  torlek queue explain [ITEM] | dispatch\n       \
                  torlek --skill\n       \
                  torlek docs [TOPIC]\n       \
                  torlek brief [--measure]\n       \
                  torlek mcp\n"
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
    let output = Command::new(env!("CARGO_BIN_EXE_torlek"))
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
    for arguments in [&["config"][..], &["configure", "show"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_torlek"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: torlek "));
    }
}

#[test]
fn show_tells_the_owner_a_running_daemon_applies_session_settings_after_config_reload() {
    let home = Home::clean();

    let output = home.show(&[]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("agents.desktop_session")
            && stderr.contains("after `torlek config reload`"),
        "{stderr}"
    );
}
