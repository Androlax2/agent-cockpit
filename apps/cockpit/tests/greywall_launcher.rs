//! The greywall launchers (config/bin/.local/bin/greywall-agents/claude and opencode, sharing
//! launcher-common.sh): what they refuse before any sandbox starts, and what they hand it.

mod common;
use common::sandbox::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every agent runs in its own launcher (cockpit::agents; tests/agents.rs checks each has one).
static AGENTS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(cockpit::agents::names);

#[test]
fn no_session_starts_in_the_live_dotfiles_checkout() {
    // Security audit C1: a session there could write code the host runs, unreviewed.
    for &agent in AGENTS.iter() {
        let sandbox = Sandbox::new();
        let live = sandbox.home.join("dotfiles/apps");
        fs::create_dir_all(&live).unwrap();
        sandbox.shim("greywall", "");
        let output = sandbox.command(bin().join("greywall-agents").join(agent), &[]).current_dir(&live).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{agent}");
        assert!(stderr(&output).contains(&format!("{agent} launcher:")) && stderr(&output).contains("IdeaProjects/dotfiles"), "{}", stderr(&output));
        assert!(sandbox.calls("greywall").is_empty(), "{agent}");
    }
}

/// A home where the launcher runs to its end: greywall is a fake that logs its arguments, DNS
/// answers at once, there is no keyring and no FigCraft unless a test starts one.
struct Launch {
    sandbox: Sandbox,
    project: PathBuf,
}

impl Launch {
    fn new() -> Launch {
        let sandbox = Sandbox::new();
        let project = sandbox.home.join("IdeaProjects/proj-a");
        fs::create_dir_all(&project).unwrap();
        assert!(Command::new("git").args(["init", "-q"]).arg(&project).status().unwrap().success());
        fs::create_dir_all(sandbox.home.join("IdeaProjects/proj-b")).unwrap();
        fs::create_dir_all(sandbox.home.join("dotfiles")).unwrap();
        for (path, content) in [(".local/share/greyproxy/ca-cert.pem", ""), (".local/share/agent-shims/bin/wl-paste", "#!/bin/sh\n"),
                                (".config/greywall/greywall.json", r#"{"filesystem": {"denyWrite": [".git/hooks"], "denyRead": ["~/.ssh/id_*", "~/.local/share/opencode/**", "~/.config/opencode/**"]}}"#)] {
            let file = sandbox.home.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        // Logs its arguments, then runs the command after -- when it exists (the fake opencode).
        write_executable(&sandbox.home.join(".local/bin/greywall"), "#!/bin/sh\necho \"greywall $* (api key: ${ANTHROPIC_API_KEY:-none})\" >> \"$SHIM_LOG\"\n\
                                                                     while [ \"$1\" != -- ]; do shift; done; shift\n\
                                                                     if [ \"$1\" = env ] || [ -x \"$1\" ]; then exec \"$@\"; fi\n");
        // Records its arguments and the environment it got, one line each.
        write_executable(&sandbox.home.join(".opencode/bin/opencode"), "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/opencode-arguments\"\n\
                                                                          env > \"$HOME/opencode-environment\"\n");
        write_executable(&sandbox.home.join(".local/bin/claude"), "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/claude-arguments\"\n\
                                                                         env > \"$HOME/claude-environment\"\n");
        sandbox.shim("dig", "echo 127.0.0.1");
        sandbox.shim("secret-tool", r#"case "$3" in
  agent-anthropic) [ -n "${SHIM_ANTHROPIC_KEY:-}" ] && echo "$SHIM_ANTHROPIC_KEY" || exit 1 ;;
  agent-opencode-anthropic) [ -n "${SHIM_OPENCODE_KEY:-}" ] && echo "$SHIM_OPENCODE_KEY" || exit 1 ;;
  *) exit 1 ;;
esac"#);
        sandbox.shim("pgrep", r#"[ -n "${SHIM_FIGCRAFT_PID:-}" ] && echo "$SHIM_FIGCRAFT_PID""#);
        Launch { sandbox, project }
    }

    fn run(&self, extra: &[(&str, &str)]) -> String {
        self.run_agent("claude", extra, &[])
    }

    /// The greywall call of one launch of `agent` with `arguments`.
    fn run_agent(&self, agent: &str, extra: &[(&str, &str)], arguments: &[&str]) -> String {
        let output = self.sandbox.command(bin().join("greywall-agents").join(agent), extra).args(arguments).current_dir(&self.project).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{agent}: {}", stderr(&output));
        self.sandbox.calls("greywall").pop().expect("greywall did not start")
    }

    /// What the fake opencode recorded: "arguments" or "environment", one line each.
    fn opencode_saw(&self, what: &str) -> Vec<String> {
        fs::read_to_string(self.sandbox.home.join(format!("opencode-{what}"))).unwrap().lines().map(str::to_string).collect()
    }

    fn slug(path: &Path) -> String {
        fs::canonicalize(path).unwrap().display().to_string().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    }

    fn settings(&self) -> serde_json::Value {
        self.settings_of("claude")
    }

    fn settings_of(&self, agent: &str) -> serde_json::Value {
        let path = self.sandbox.runtime.join("greywall-agents").join(format!("{}.{agent}.json", Launch::slug(&self.project)));
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }
}

#[test]
fn other_projects_claude_folders_are_read_only_and_its_own_is_not() {
    // Security audit H2: a session must not plant memory another project loads.
    for &agent in AGENTS.iter() {
        other_projects_folders_are_read_only_for(agent);
    }
}

fn other_projects_folders_are_read_only_for(agent: &str) {
    let launch = Launch::new();
    let started = launch.run_agent(agent, &[], &[]);
    assert!(started.contains("--settings "), "{started}");
    let denied: Vec<String> = launch.settings_of(agent)["filesystem"]["denyWrite"].as_array().unwrap().iter()
        .map(|path| path.as_str().unwrap().to_string()).collect();
    let projects = launch.sandbox.home.join(".claude/projects");
    let other = projects.join(Launch::slug(&launch.sandbox.home.join("IdeaProjects/proj-b"))).display().to_string();
    let own = projects.join(Launch::slug(&launch.project)).display().to_string();
    assert!(denied.contains(&other), "{denied:?}");
    assert!(!denied.contains(&own), "{denied:?}");
    assert!(denied.contains(&".git/hooks".to_string()), "greywall.json's own rules were lost: {denied:?}");
}

#[test]
fn only_the_projects_own_answers_folder_is_mounted() {
    // Security audit M3: no session reads another project's command output.
    for &agent in AGENTS.iter() {
        let launch = Launch::new();
        let started = launch.run_agent(agent, &[], &[]);
        let answers = launch.sandbox.runtime.join("claude-answers").join(Launch::slug(&launch.project));
        assert!(started.contains(&format!("--allow-read-path {} ", answers.display())), "{started}");
        assert!(answers.join("runs").is_dir() && answers.join("guides").is_dir(), "{agent}");
    }
}

fn managed_figcraft(launch: &Launch, port: &str, root: &str, additional_roots: &[&str]) -> WaitingProcess {
    let storage = launch.sandbox.home.join(".config/greywall-agents/figcraft");
    fs::create_dir_all(storage.join("ports")).unwrap();
    fs::write(storage.join("ports").join(port), format!("{root}\n")).unwrap();
    let config = storage.join(format!("instances/{port}/config"));
    fs::create_dir_all(&config).unwrap();
    let mut arguments = vec!["--control", port, "--automation-read-root", root, "--automation-write-root", root];
    arguments.extend_from_slice(additional_roots);
    WaitingProcess::with_environment("figcraft", &arguments, &launch.project, &[("XDG_CONFIG_HOME", config.to_str().unwrap())])
}

#[test]
fn figcrafts_port_reaches_only_the_project_it_serves() {
    let launch = Launch::new();
    let root = fs::canonicalize(&launch.project).unwrap().display().to_string();
    let other = fs::canonicalize(launch.sandbox.home.join("IdeaProjects/proj-b")).unwrap().display().to_string();
    let own = managed_figcraft(&launch, "17979", &root, &[]);
    let foreign = managed_figcraft(&launch, "17980", &other, &[]);
    let process_ids = format!("{}\n{}", foreign.id(), own.id());
    for &agent in AGENTS.iter() {
        let started = launch.run_agent(agent, &[("SHIM_FIGCRAFT_PID", &process_ids)], &[]);
        assert!(started.contains("--forward 17979"), "{agent}: {started}");
        assert!(!started.contains("--forward 17980"), "{agent}: {started}");
        assert!(!started.contains("--forward 7979"), "{agent}: {started}");
    }
    assert!(!launch.run(&[]).contains("--forward 17979"), "forwarded with no FigCraft open");
}

#[test]
fn extra_automation_roots_never_grant_a_figcraft_forward() {
    let launch = Launch::new();
    let root = launch.project.to_str().unwrap();
    for additional in [vec!["--automation-read-root", "/elsewhere"], vec!["--automation-write-root", "/elsewhere"],
                       vec!["--automation-write-root", root]] {
        let open = managed_figcraft(&launch, "17979", root, &additional);
        for &agent in AGENTS.iter() {
            let started = launch.run_agent(agent, &[("SHIM_FIGCRAFT_PID", &open.id())], &[]);
            assert!(!started.contains("--forward 17979"), "{additional:?}: {started}");
        }
    }
}

#[test]
fn a_root_prefix_or_an_unowned_port_never_grants_a_figcraft_forward() {
    let launch = Launch::new();
    let root_prefix = format!("{}-other", launch.project.display());
    let open = managed_figcraft(&launch, "17979", &root_prefix, &[]);
    for &agent in AGENTS.iter() {
        assert!(!launch.run_agent(agent, &[("SHIM_FIGCRAFT_PID", &open.id())], &[]).contains("--forward 17979"));
    }
    // Even correct process arguments cannot claim a port reserved for another checkout.
    let own = managed_figcraft(&launch, "17980", launch.project.to_str().unwrap(), &[]);
    fs::write(launch.sandbox.home.join(".config/greywall-agents/figcraft/ports/17980"), &root_prefix).unwrap();
    for &agent in AGENTS.iter() {
        assert!(!launch.run_agent(agent, &[("SHIM_FIGCRAFT_PID", &own.id())], &[]).contains("--forward 17980"));
    }
}

#[test]
fn disconnected_launches_clear_inherited_control_ports() {
    let launch = Launch::new();
    for &agent in AGENTS.iter() {
        let started = launch.run_agent(agent, &[("FIGCRAFT_CONTROL_PORT", "17980")], &[]);
        assert!(!started.contains("--forward 17980"));
        let environment = fs::read_to_string(launch.sandbox.home.join(format!("{agent}-environment"))).unwrap();
        assert!(!environment.lines().any(|line| line == "FIGCRAFT_CONTROL_PORT=17980"), "{agent} inherited a foreign control port");
    }
}

#[test]
fn figcraft_reservations_are_hidden_even_before_the_first_window_starts() {
    let launch = Launch::new();
    launch.run(&[]);
    let storage = launch.sandbox.home.join(".config/greywall-agents/figcraft");
    assert!(storage.is_dir());
    let settings = launch.settings();
    assert!(settings["filesystem"]["denyRead"].as_array().unwrap().iter()
        .any(|entry| entry == "~/.config/greywall-agents/figcraft" || entry == &storage.display().to_string()));
    assert!(settings["filesystem"]["denyWrite"].as_array().unwrap().iter()
        .any(|entry| entry == "~/.config/greywall-agents/figcraft" || entry == &storage.display().to_string()));
}

#[test]
fn claudes_mcp_override_connects_to_the_port_its_launcher_forwards() {
    let launch = Launch::new();
    let open = managed_figcraft(&launch, "17979", launch.project.to_str().unwrap(), &[]);
    launch.run(&[("SHIM_FIGCRAFT_PID", &open.id())]);
    let arguments = fs::read_to_string(launch.sandbox.home.join("claude-arguments")).unwrap();
    let arguments: Vec<_> = arguments.lines().collect();
    let index = arguments.iter().position(|argument| *argument == "--mcp-config").expect("no MCP override");
    assert!(!arguments.contains(&"--strict-mcp-config"));
    let config: serde_json::Value = serde_json::from_str(arguments[index + 1]).unwrap();
    let server = &config["mcpServers"]["figcraft-app"];
    launch.sandbox.shim("figcraft-cli", "exit 0");
    let mut command = launch.sandbox.command(server["command"].as_str().unwrap(), &[("FIGCRAFT_CONTROL_PORT", "17979")]);
    for argument in server["args"].as_array().unwrap() { command.arg(argument.as_str().unwrap()); }
    assert!(command.output().unwrap().status.success());
    assert_eq!(launch.sandbox.calls("figcraft-cli"), ["figcraft-cli mcp --attach 17979"]);
}

#[test]
fn generic_port_grants_cannot_forward_another_checkouts_reserved_figcraft_port() {
    let launch = Launch::new();
    let foreign = launch.sandbox.home.join("IdeaProjects/proj-b");
    let _open = managed_figcraft(&launch, "17980", foreign.to_str().unwrap(), &[]);
    let grants = launch.sandbox.home.join(".config/greywall-agents/forward");
    fs::create_dir_all(&grants).unwrap();
    fs::write(grants.join("proj-a"), "4102444800 17980\n").unwrap();
    for &agent in AGENTS.iter() {
        let output = launch.sandbox.command(bin().join("greywall-agents").join(agent), &[])
            .current_dir(&launch.project).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{agent}: {}{}", stdout(&output), stderr(&output));
        assert!(stderr(&output).contains("reserved"), "{}", stderr(&output));
    }
    assert!(launch.sandbox.calls("greywall").is_empty());
}

#[test]
fn a_credits_session_gets_the_api_key_as_a_placeholder() {
    let launch = Launch::new();
    let started = launch.run(&[("CLAUDE_SESSIONS_CREDITS", "1"), ("SHIM_ANTHROPIC_KEY", "sk-ant-test")]);
    assert!(started.contains("--secret ANTHROPIC_API_KEY") && started.contains("(api key: sk-ant-test)"), "{started}");
}

#[test]
fn a_credits_session_without_a_key_does_not_start_on_the_subscription() {
    let launch = Launch::new();
    let output = launch.sandbox.command(bin().join("greywall-agents/claude"), &[("CLAUDE_SESSIONS_CREDITS", "1")]).current_dir(&launch.project).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("agent-anthropic-key"), "{}", stderr(&output));
    assert!(launch.sandbox.calls("greywall").is_empty());
}

#[test]
fn a_subscription_session_gets_no_api_key_even_from_its_environment() {
    let launch = Launch::new();
    let started = launch.run(&[("ANTHROPIC_API_KEY", "sk-ant-stray"), ("SHIM_ANTHROPIC_KEY", "sk-ant-test")]);
    assert!(!started.contains("ANTHROPIC_API_KEY") && started.contains("(api key: none)"), "{started}");
}

#[test]
fn an_opencode_session_never_gets_the_credits_key_nor_a_stray_one() {
    // The credits key is a Claude session's: opencode would bill it unnoticed.
    let launch = Launch::new();
    let started = launch.run_agent("opencode", &[("ANTHROPIC_API_KEY", "sk-ant-stray"), ("CLAUDE_SESSIONS_CREDITS", "1"), ("SHIM_ANTHROPIC_KEY", "sk-ant-test")], &[]);
    assert!(!started.contains("ANTHROPIC_API_KEY") && started.contains("(api key: none)"), "{started}");
}

#[test]
fn an_opencode_session_gets_its_own_key_as_a_placeholder() {
    // opencode.db is open to every opencode session: the real key stays with greyproxy.
    let launch = Launch::new();
    let started = launch.run_agent("opencode", &[("SHIM_OPENCODE_KEY", "sk-ant-opencode"), ("SHIM_ANTHROPIC_KEY", "sk-ant-credits")], &[]);
    assert!(started.contains("--secret ANTHROPIC_API_KEY") && started.contains("(api key: sk-ant-opencode)"), "{started}");
}

#[test]
fn a_claude_session_never_gets_opencodes_key() {
    let launch = Launch::new();
    let started = launch.run(&[("SHIM_OPENCODE_KEY", "sk-ant-opencode")]);
    assert!(!started.contains("ANTHROPIC_API_KEY") && started.contains("(api key: none)"), "{started}");
}

#[test]
fn opencode_runs_in_its_own_profile_and_proxy_identity() {
    let launch = Launch::new();
    let started = launch.run_agent("opencode", &[], &[]);
    assert!(started.contains("--auto-profile --profile opencode --proxy-user opencode "), "{started}");
    assert!(!started.contains("--profile claude"), "{started}");
}

#[test]
fn opencode_keeps_its_arguments_as_given() {
    let launch = Launch::new();
    let arguments = ["--session", "ses_1", "a prompt with spaces", "--prompt"];
    launch.run_agent("opencode", &[], &arguments);
    assert_eq!(launch.opencode_saw("arguments"), arguments);
}

#[test]
fn opencodes_server_registration_stays_in_the_sandboxs_private_tmp() {
    // Each greywall launch has its own network: a service.json shared between them makes
    // unreachable servers replace each other. Data, config and cache stay shared.
    let launch = Launch::new();
    launch.run_agent("opencode", &[], &[]);
    let environment = launch.opencode_saw("environment");
    assert!(environment.contains(&"XDG_STATE_HOME=/tmp/opencode-greywall-state".to_string()), "{environment:?}");
    assert!(environment.contains(&"OPENCODE_DISABLE_AUTOUPDATE=1".to_string()), "{environment:?}");
}

#[test]
fn the_tools_opencode_runs_keep_the_cockpits_real_state_folder() {
    // claude-todo list and show read todo.json there; under the private XDG_STATE_HOME they found
    // an empty list (todo bc277a7c's report could not be read from an opencode session).
    let launch = Launch::new();
    launch.run_agent("opencode", &[], &[]);
    let environment = launch.opencode_saw("environment");
    let real = format!("CLAUDE_SESSIONS_STATE_HOME={}", launch.sandbox.state.display());
    assert!(environment.contains(&real), "{environment:?}");
}

#[test]
fn a_parallel_dotfiles_clone_reads_the_live_checkout_and_another_project_does_not() {
    let launch = Launch::new();
    let live = fs::canonicalize(launch.sandbox.home.join("dotfiles")).unwrap();
    let clone = launch.sandbox.home.join("IdeaProjects/.parallel/dotfiles/p1");
    fs::create_dir_all(&clone).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).arg(&clone).status().unwrap().success());
    let read_live = format!("--allow-read-path {} ", live.display());
    let output = launch.sandbox.command(bin().join("greywall-agents/claude"), &[]).current_dir(&clone).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let started = launch.sandbox.calls("greywall").pop().unwrap();
    assert!(started.contains(&read_live), "{started}");
    assert!(!launch.run(&[]).contains(&read_live), "proj-a reads the live checkout");
}

#[test]
fn sandboxes_mount_the_report_sockets_folder_never_the_socket_alone() {
    // A socket file mounted alone stays the old one in every running sandbox once the socket is made
    // again: on 2026-10-19 a socket restart cut every session off the daemon.
    let profile: serde_json::Value = serde_json::from_str(&fs::read_to_string(repo().join("config/greywall/.config/greywall/greywall.json")).unwrap()).unwrap();
    let writable: Vec<&str> = profile["filesystem"]["allowWrite"].as_array().unwrap().iter().filter_map(|path| path.as_str()).collect();
    assert!(writable.contains(&"/run/user/1000/claude-report"), "{writable:?}");
    assert!(writable.iter().all(|path| !path.ends_with(".sock")), "{writable:?}");
    let unit = fs::read_to_string(repo().join("config/systemd/.config/systemd/user/osd-report.socket")).unwrap();
    assert!(unit.contains("ListenStream=%t/claude-report/osd-report.sock"), "{unit}");
}

#[test]
fn the_launcher_makes_the_report_sockets_folder_before_the_sandbox() {
    // Greywall mounts only what exists when it starts.
    let launch = Launch::new();
    launch.run(&[]);
    assert!(launch.sandbox.runtime.join("claude-report").is_dir());
}

#[test]
fn an_opencode_session_cannot_write_what_every_other_session_runs_or_reads() {
    // The binary, plugins and config, shared instructions and gh's credentials serve every project:
    // a session writing them would reach the others (the same rule keeps Claude's versions and hooks).
    let text = fs::read_to_string(repo().join("config/greywall/.config/greywall/learned/opencode.json")).unwrap();
    let json: String = text.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let profile: serde_json::Value = serde_json::from_str(&json).unwrap();
    let paths = |key: &str| -> Vec<String> {
        profile["filesystem"][key].as_array().unwrap().iter().map(|path| path.as_str().unwrap().to_string()).collect()
    };
    let (writable, read_only) = (paths("allowWrite"), paths("denyWrite"));
    for shared in ["~/.opencode", "~/.config/opencode", "~/.opencode.json", "~/AGENTS.md", "~/.skills", "~/.agents", "~/.config/gh", "~/.local/bin", ".git/hooks", "~/.claude/hooks"] {
        assert!(!writable.contains(&shared.to_string()), "{shared} is writable: {writable:?}");
        assert!(read_only.contains(&shared.to_string()), "{shared} is not in denyWrite: {read_only:?}");
    }
}

fn opencode_profile() -> serde_json::Value {
    let text = fs::read_to_string(repo().join("config/greywall/.config/greywall/learned/opencode.json")).unwrap();
    let json: String = text.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    serde_json::from_str(&json).unwrap()
}

#[test]
fn an_opencode_session_reaches_the_hook_its_plugin_reports_through() {
    // cockpit.ts runs session-state.sh where ~/.claude/hooks points (greywall makes no link in the
    // hidden ~/.claude: tried on 2026-10-19, "No such file or directory"); the hook keeps its turn
    // files and log in ~/.cache/claude.
    let profile = opencode_profile();
    let has = |key: &str, path: &str| profile["filesystem"][key].as_array().unwrap().iter().any(|entry| entry == path);
    assert!(has("allowRead", "~/dotfiles/config/claude/.claude/hooks") && has("allowWrite", "~/.cache/claude"));
    let plugin = fs::read_to_string(repo().join("config/opencode/.config/opencode/plugins/cockpit.ts")).unwrap();
    assert!(plugin.contains("`${homedir()}/dotfiles/config/claude/.claude/hooks/session-state.sh`"), "the plugin runs another hook path");
}

#[test]
fn opencode_reads_theos_rules_from_its_own_agents_md() {
    // opencode 2 reads ~/.config/opencode/AGENTS.md, never ~/.claude/CLAUDE.md: the stowed AGENTS.md is
    // the same file, which its profile can read where it lives.
    let agents = repo().join("config/opencode/.config/opencode/AGENTS.md");
    assert_eq!(fs::read_link(&agents).unwrap(), Path::new("../../../claude/.claude/CLAUDE.md"));
    assert_eq!(fs::read_to_string(&agents).unwrap(), fs::read_to_string(repo().join("config/claude/.claude/CLAUDE.md")).unwrap());
    let readable = opencode_profile()["filesystem"]["allowRead"].clone();
    assert!(readable.as_array().unwrap().iter().any(|entry| entry == "~/dotfiles/config/claude/.claude/CLAUDE.md"), "{readable}");
}

#[test]
fn the_opencode_profile_names_no_link_in_the_hidden_claude_folder() {
    // ~/.claude/CLAUDE.md and ~/.claude/hooks are stow links: in the sandbox they do not exist, and
    // an entry for them looks allowed while it reads nothing (~/.claude/skills is a real folder).
    let readable = opencode_profile()["filesystem"]["allowRead"].clone();
    for link in ["~/.claude/CLAUDE.md", "~/.claude/hooks"] {
        assert!(!readable.as_array().unwrap().iter().any(|entry| entry == link), "{link}: {readable}");
    }
}

fn denied_reads(settings: &serde_json::Value) -> Vec<String> {
    settings["filesystem"]["denyRead"].as_array().unwrap().iter().map(|path| path.as_str().unwrap().to_string()).collect()
}

#[test]
fn opencode_sees_its_own_data_and_claude_does_not() {
    // Hidden, opencode's folder was an empty one mounted over its database: no session it ran was
    // kept (/resume showed nothing on 2026-10-19).
    let launch = Launch::new();
    launch.run_agent("opencode", &[], &[]);
    launch.run_agent("claude", &[], &[]);
    let opencode = denied_reads(&launch.settings_of("opencode"));
    let claude = denied_reads(&launch.settings_of("claude"));
    assert_eq!(opencode, ["~/.config/greywall-agents/figcraft", "~/.ssh/id_*"], "host-only FigCraft storage and every other rule stay hidden");
    assert!(claude.contains(&"~/.local/share/opencode/**".to_string()) && claude.contains(&"~/.config/opencode/**".to_string()), "{claude:?}");
}

#[test]
fn each_agent_starts_on_a_settings_file_of_its_own() {
    // One file per folder was rewritten by whichever agent started there last.
    let launch = Launch::new();
    for &agent in AGENTS.iter() {
        let started = launch.run_agent(agent, &[], &[]);
        assert!(started.contains(&format!(".{agent}.json ")), "{started}");
    }
}
