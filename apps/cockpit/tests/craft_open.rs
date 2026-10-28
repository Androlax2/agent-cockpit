//! craft-open starts independent, confined FigCraft windows without replacing existing work.

mod common;
use common::sandbox::*;

use std::fs;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Craft {
    sandbox: Sandbox,
    project: PathBuf,
}

impl Craft {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let project = sandbox.home.join("IdeaProjects/opusline");
        fs::create_dir_all(&project).unwrap();
        // The real process exposes NUL-separated arguments and binds its own TCP listener;
        // only the desktop renderer is replaced. pgrep discovers just this fixture's processes.
        sandbox.shim("setsid", "shift; \"$@\" &");
        sandbox.shim("figcraft", r#"printf '%s\n' "${XDG_CONFIG_HOME:-}" >> "$HOME/config-homes"
exec python3 -c '
import json, os, socket, sys
from pathlib import Path
listener = socket.socket()
listener.bind(("127.0.0.1", int(sys.argv[sys.argv.index("--control") + 1])))
listener.listen()
documents = json.loads(os.environ.get("SHIM_FIGCRAFT_DOCUMENTS", "[{\"active\":true,\"dirty\":false,\"index\":0,\"name\":\"Untitled\",\"path\":null}]"))
record = Path(os.environ["XDG_CONFIG_HOME"]) / "documents.json"
record.write_text(json.dumps(documents))
with open(os.environ["HOME"] + "/figcraft-pids", "a") as record:
    record.write(str(os.getpid()) + "\n")
while True:
    connection, _ = listener.accept()
    with connection:
        for line in connection.makefile("rb"):
            request = json.loads(line)
            with open(os.environ["HOME"] + "/figcraft-requests", "a") as log:
                log.write(json.dumps(request) + "\n")
            command = request["params"]["command"]
            reply = {"id": request["id"], "ok": True}
            if request["method"] != "engine.execute" or request["params"]["window"] != 0:
                reply.update(ok=False, error="unexpected control request")
            elif command == "doc.list":
                reply["result"] = documents
            elif command == "file.rename":
                if os.environ.get("SHIM_FIGCRAFT_RENAME_ERROR"):
                    reply.update(ok=False, error="fixture rename failed")
                else:
                    documents[0]["name"] = request["params"]["params"]["name"]
                    (Path(os.environ["XDG_CONFIG_HOME"]) / "documents.json").write_text(json.dumps(documents))
                    reply["result"] = {}
            else:
                reply.update(ok=False, error="unexpected command")
            connection.sendall((json.dumps(reply) + "\n").encode())
' "$@""#);
        sandbox.shim("pgrep", r#"case " $* " in
*" figcraft "*) cat "$HOME/figcraft-pids" 2>/dev/null; printf '%s\n' "${SHIM_FIGCRAFT_PID:-}" ;;
*" greywall "*) printf '%s\n' "${SHIM_GREYWALL_PID:-}" ;;
esac"#);
        sandbox.shim("hyprctl", "exit ${SHIM_HYPRCTL_STATUS:-0}");
        Self { sandbox, project }
    }

    fn command(&self, directory: &Path, arguments: &[&str], extra: &[(&str, &str)]) -> Command {
        // Generous: a start returns as soon as its port answers, and a loaded machine (make test while
        // another session builds) took longer than 2 s to start the fake (todo 0ca1de7a).
        let mut command = self.sandbox.command(bin().join("craft-open"), &[("CRAFT_OPEN_WAIT_S", "15")]);
        command.env_remove("CRAFT_OPEN_PORT").envs(extra.iter().copied()).args(arguments).current_dir(directory);
        command
    }

    fn run(&self, arguments: &[&str], extra: &[(&str, &str)]) -> Output {
        self.command(&self.project, arguments, extra).output().unwrap()
    }

    fn other_project(&self, name: &str) -> PathBuf {
        let project = self.sandbox.home.join("IdeaProjects").join(name);
        fs::create_dir_all(&project).unwrap();
        project
    }

    fn started_port(&self, index: usize) -> u16 {
        let calls = self.sandbox.calls("figcraft");
        let words: Vec<_> = calls[index].split_whitespace().collect();
        words[words.iter().position(|word| *word == "--control").unwrap() + 1].parse().unwrap()
    }

    fn documents(&self, index: usize) -> serde_json::Value {
        let path = self.sandbox.home.join(format!(".config/greywall-agents/figcraft/instances/{}/config/documents.json", self.started_port(index)));
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }
}

impl Drop for Craft {
    fn drop(&mut self) {
        for pid in fs::read_to_string(self.sandbox.home.join("figcraft-pids")).unwrap_or_default().lines() {
            let _ = Command::new("kill").arg(pid).status();
        }
    }
}

/// A port that refuses connections yet cannot be bound. Under a parallel test run another process
/// can take a freed port before it is checked (todo c67685bf), so a port that does not qualify is
/// replaced by a fresh one instead of failing the test that needs it.
fn time_wait_port() -> u16 {
    (0..20).find_map(|_| time_wait_candidate()).expect("no port stayed closed and unbindable in 20 tries")
}

fn time_wait_candidate() -> Option<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let port = client.local_addr().unwrap().port();
    let (mut peer, _) = listener.accept().unwrap();
    // Close the client first so its local port stays unbindable after both peers exit.
    drop(client);
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    drop(peer);
    drop(listener);
    let is_closed = TcpStream::connect(("127.0.0.1", port)).is_err();
    let is_unbindable = TcpListener::bind(("127.0.0.1", port))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::AddrInUse);
    (is_closed && is_unbindable).then_some(port)
}

fn wait_until_closed(port: u16) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(std::time::Instant::now() < deadline, "port {port} still answers 15 s after its instance was signalled");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn allocation_skips_ports_that_refuse_connections_but_cannot_be_bound() {
    let craft = Craft::new();
    let blocked_port = time_wait_port();
    craft.sandbox.shim("shuf", &format!(r#"if [[ ! -e $HOME/first-candidate-used ]]; then
    : > "$HOME/first-candidate-used"
    echo {blocked_port}
else
    PATH="${{PATH#*:}}" shuf "$@"
fi"#));
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    assert_ne!(craft.started_port(0), blocked_port);
    assert!(!craft.sandbox.home.join(format!(".config/greywall-agents/figcraft/ports/{blocked_port}")).exists());
}

#[test]
fn new_ports_stay_out_of_the_kernels_ephemeral_range() {
    let craft = Craft::new();
    let range = fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range").unwrap();
    let bounds: Vec<u16> = range.split_whitespace().map(|bound| bound.parse().unwrap()).collect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let ephemeral_port = listener.local_addr().unwrap().port();
    assert!((bounds[0]..=bounds[1]).contains(&ephemeral_port));
    drop(listener);
    craft.sandbox.shim("shuf", &format!(r#"if [[ ! -e $HOME/first-candidate-used ]]; then
    : > "$HOME/first-candidate-used"
    echo {ephemeral_port}
else
    PATH="${{PATH#*:}}" shuf "$@"
fi"#));
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    assert!(!(bounds[0]..=bounds[1]).contains(&craft.started_port(0)));
    assert!(!craft.sandbox.home.join(format!(".config/greywall-agents/figcraft/ports/{ephemeral_port}")).exists());
}

#[test]
fn an_unbindable_fixed_port_is_rejected_without_starting_or_reconnecting() {
    for is_reserved in [false, true] {
        let craft = Craft::new();
        let blocked_port = time_wait_port().to_string();
        let reservation = craft.sandbox.home.join(".config/greywall-agents/figcraft/ports").join(&blocked_port);
        if is_reserved {
            fs::create_dir_all(reservation.parent().unwrap()).unwrap();
            fs::write(&reservation, format!("{}\n", craft.project.display())).unwrap();
        }
        let extra = if is_reserved { vec![] } else { vec![("CRAFT_OPEN_PORT", blocked_port.as_str())] };
        let output = craft.run(&["figcraft", "--workspace", "2", "--session", SESSION_A], &extra);
        assert_eq!(output.status.code(), Some(3), "reserved={is_reserved}: {}{}", stdout(&output), stderr(&output));
        assert!(stderr(&output).contains("cannot be bound"), "{}", stderr(&output));
        assert!(craft.sandbox.calls("figcraft").is_empty());
        assert!(!craft.sandbox.state.join("cockpit/restart-when-idle").exists());
        if is_reserved {
            assert_eq!(fs::read_to_string(reservation).unwrap(), format!("{}\n", craft.project.display()));
        }
    }
}

#[test]
fn a_failed_bind_probe_is_reported_without_starting_a_window() {
    let craft = Craft::new();
    craft.sandbox.shim("python3", "exit 127");
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert_eq!(output.status.code(), Some(1), "{}{}", stdout(&output), stderr(&output));
    assert!(stderr(&output).contains("bind check failed"), "{}", stderr(&output));
    assert!(craft.sandbox.calls("figcraft").is_empty());
}

#[test]
fn a_failed_start_reports_the_private_log_containing_the_child_error() {
    let craft = Craft::new();
    craft.sandbox.shim("figcraft", "echo 'fixture startup failed' >&2; exit 1");
    // It never listens: a short wait, the outcome is the same.
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("CRAFT_OPEN_WAIT_S", "1")]);
    assert_eq!(output.status.code(), Some(1), "{}{}", stdout(&output), stderr(&output));
    let log = craft.sandbox.home.join(format!(".config/greywall-agents/figcraft/instances/{}/config/startup.log", craft.started_port(0)));
    assert!(stderr(&output).contains(log.to_str().unwrap()), "{}", stderr(&output));
    assert!(fs::read_to_string(log).unwrap().contains("fixture startup failed"));
}

#[test]
fn figcraft_starts_with_its_files_limited_to_the_project() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let started = craft.sandbox.calls("figcraft").pop().expect("not started");
    assert!(started.ends_with(&format!("--automation-read-root {0} --automation-write-root {0}", craft.project.display())), "{started}");
}

#[test]
fn new_documents_use_the_project_name_including_parallel_clones() {
    let craft = Craft::new();
    let clone = craft.other_project(".parallel/opusline/parallel-1");
    let custom_clone = craft.other_project("custom-clones/opusline/parallel-2");
    let quoted_name = craft.other_project("a \"project\"");
    for (index, project) in [&craft.project, &clone, &custom_clone, &quoted_name].iter().enumerate() {
        let clones = craft.sandbox.home.join("IdeaProjects/custom-clones");
        let extra = if index == 2 { vec![("COCKPIT_CLONES", clones.to_str().unwrap())] } else { vec![] };
        let output = craft.command(project, &["figcraft", "--workspace", "2"], &extra).output().unwrap();
        assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
        assert_eq!(craft.documents(index)[0]["name"], if index == 3 { "a \"project\"" } else { "opusline" });
    }
}

#[test]
fn saved_recovered_or_renamed_documents_are_never_renamed() {
    use serde_json::json;
    for documents in [
        json!([{"active":true,"dirty":false,"index":0,"name":"Saved design","path":"/saved.figcraft"}]),
        json!([{"active":true,"dirty":false,"index":0,"name":"Untitled","path":"/saved.figcraft"}]),
        json!([{"active":true,"dirty":true,"index":0,"name":"Untitled","path":null}]),
        json!([{"active":true,"dirty":false,"index":0,"name":"My design","path":null}]),
        json!([{"active":true,"dirty":false,"index":0,"name":"Untitled","path":null},
               {"active":false,"dirty":true,"index":1,"name":"Recovered","path":null}]),
    ] {
        let craft = Craft::new();
        let output = craft.run(&["figcraft", "--workspace", "2"], &[("SHIM_FIGCRAFT_DOCUMENTS", &documents.to_string())]);
        assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
        assert_eq!(craft.documents(0), documents);
    }
}

#[test]
fn reopening_a_window_does_not_rename_its_document_again() {
    let craft = Craft::new();
    assert!(craft.run(&["figcraft", "--workspace", "2"], &[]).status.success());
    let before = fs::read(craft.sandbox.home.join("figcraft-requests")).unwrap();
    assert!(craft.run(&["figcraft", "--workspace", "3"], &[]).status.success());
    assert_eq!(fs::read(craft.sandbox.home.join("figcraft-requests")).unwrap(), before);
}

#[test]
fn a_failed_document_rename_is_reported_without_reconnecting_the_session() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--workspace", "2", "--session", SESSION_A], &[("SHIM_FIGCRAFT_RENAME_ERROR", "1")]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("fixture rename failed"), "{}", stderr(&output));
    assert!(!craft.sandbox.state.join("cockpit/restart-when-idle").exists());
}

#[test]
fn a_new_instance_starts_in_the_dark_theme() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    let preferences = craft.sandbox.home.join(format!(".config/greywall-agents/figcraft/instances/{}/config/FigCraft/state.json", craft.started_port(0)));
    assert_eq!(fs::read_to_string(&preferences).unwrap(), "{\"prefs\":{\"theme\":\"dark\"}}\n");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&preferences).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn an_instances_saved_preferences_are_kept_on_restart() {
    let craft = Craft::new();
    // A port nothing else holds: a fixed one collided with concurrent runs of this suite.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
    let port = port.as_str();
    let preferences = craft.sandbox.home.join(format!(".config/greywall-agents/figcraft/instances/{port}/config/FigCraft/state.json"));
    fs::create_dir_all(preferences.parent().unwrap()).unwrap();
    fs::write(&preferences, "{\"prefs\":{\"theme\":\"light\"}}").unwrap();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("CRAFT_OPEN_PORT", port)]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert_eq!(fs::read_to_string(&preferences).unwrap(), "{\"prefs\":{\"theme\":\"light\"}}");
}

#[test]
fn a_new_window_is_routed_to_the_requested_workspace_before_it_starts() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--workspace", "3"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert_eq!(craft.sandbox.calls("hyprctl"), ["hyprctl eval FIGCRAFT_OPEN_ON_WORKSPACE(3)"]);
    let calls = fs::read_to_string(&craft.sandbox.log).unwrap();
    assert!(calls.find("hyprctl eval").unwrap() < calls.find("figcraft --control").unwrap(), "{calls}");
    assert!(stdout(&output).contains("on workspace 3"), "{}", stdout(&output));
}

#[test]
fn hyprctl_reaches_the_running_hyprland_from_a_terminal_without_its_signature() {
    let craft = Craft::new();
    let hypr = craft.sandbox.runtime.join("hypr");
    for (instance, has_socket) in [("stale", false), ("live", true)] {
        fs::create_dir_all(hypr.join(instance)).unwrap();
        if has_socket { fs::write(hypr.join(instance).join(".socket.sock"), "").unwrap(); }
    }
    craft.sandbox.shim("hyprctl", r#"echo "$HYPRLAND_INSTANCE_SIGNATURE" > "$HOME/hyprctl-instance""#);
    let mut command = craft.command(&craft.project, &["figcraft", "--workspace", "2"], &[]);
    let output = command.env_remove("HYPRLAND_INSTANCE_SIGNATURE").output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert_eq!(fs::read_to_string(craft.sandbox.home.join("hyprctl-instance")).unwrap(), "live\n");
}

#[test]
fn an_already_open_window_is_not_moved() {
    let craft = Craft::new();
    assert!(craft.run(&["figcraft", "--workspace", "2"], &[]).status.success());
    let output = craft.run(&["figcraft", "--workspace", "4"], &[]);
    assert!(stdout(&output).contains("already serves"), "{}{}", stdout(&output), stderr(&output));
    assert_eq!(craft.sandbox.calls("hyprctl").len(), 1);
}

#[test]
fn a_failed_workspace_routing_starts_no_window() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--workspace", "2", "--session", SESSION_A], &[("SHIM_HYPRCTL_STATUS", "1")]);
    assert_eq!(output.status.code(), Some(1), "{}{}", stdout(&output), stderr(&output));
    assert!(stderr(&output).contains("could not route FigCraft to workspace 2"), "{}", stderr(&output));
    assert!(craft.sandbox.calls("figcraft").is_empty());
    assert!(!craft.sandbox.state.join("cockpit/restart-when-idle").exists());
}

#[test]
fn without_a_workspace_a_new_window_opens_on_the_shared_workspace_7() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--session", SESSION_A], &[]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert_eq!(craft.sandbox.calls("hyprctl"), ["hyprctl eval FIGCRAFT_OPEN_ON_WORKSPACE(7)"]);
    assert!(stdout(&output).contains("on workspace 7"), "{}", stdout(&output));
}

#[test]
fn the_cockpit_workspace_or_an_invalid_workspace_is_refused() {
    let craft = Craft::new();
    for arguments in [
        vec!["figcraft", "--workspace"],
        vec!["figcraft", "--workspace", "6"],
        vec!["figcraft", "--workspace", "0"],
        vec!["figcraft", "--workspace", "2)"],
        vec!["figcraft", "--workspace", ""],
        vec!["figcraft", "--workspace", "2", "--workspace", "3"],
        vec!["figcraft", "--workspace", "2", "--session", ""],
    ] {
        assert_eq!(craft.run(&arguments, &[]).status.code(), Some(2), "{arguments:?}");
    }
    assert!(craft.sandbox.calls("figcraft").is_empty());
    assert!(craft.sandbox.calls("hyprctl").is_empty());
}

#[test]
fn a_foreign_window_is_left_alive_while_this_checkout_starts() {
    let craft = Craft::new();
    let mut open = WaitingProcess::new("figcraft", &["--control", "7979", "--automation-write-root", "/home/elsewhere"], &craft.project);
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("SHIM_FIGCRAFT_PID", &open.id())]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert!(open.is_alive());
    assert_eq!(craft.sandbox.calls("figcraft").len(), 1);
    assert_ne!(craft.started_port(0), 7979);
}

#[test]
fn each_checkout_including_parallel_clones_has_its_own_port_and_recovery_directory() {
    let craft = Craft::new();
    let clone = craft.other_project(".parallel/opusline/parallel-1");
    for project in [&craft.project, &clone] {
        let output = craft.command(project, &["figcraft", "--workspace", "2"], &[]).output().unwrap();
        assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    }
    assert_ne!(craft.started_port(0), craft.started_port(1));
    let homes = fs::read_to_string(craft.sandbox.home.join("config-homes")).unwrap();
    let homes: Vec<_> = homes.lines().collect();
    assert_eq!(homes.len(), 2);
    assert_ne!(homes[0], homes[1]);
    for home in homes { assert!(Path::new(home).is_dir(), "{home}"); }
}

#[test]
fn reuse_finds_the_matching_window_after_a_foreign_one() {
    let craft = Craft::new();
    assert!(craft.run(&["figcraft", "--workspace", "2"], &[]).status.success());
    let foreign = WaitingProcess::new("figcraft", &["--control", "7979", "--automation-write-root", "/elsewhere"], &craft.project);
    let own = fs::read_to_string(craft.sandbox.home.join("figcraft-pids")).unwrap();
    fs::write(craft.sandbox.home.join("figcraft-pids"), format!("{}\n{own}", foreign.id())).unwrap();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    // Remove the unrelated fixture process from the startup cleanup list.
    fs::write(craft.sandbox.home.join("figcraft-pids"), own).unwrap();
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("already serves"));
    assert_eq!(craft.sandbox.calls("figcraft").len(), 1);
}

#[test]
fn simultaneous_opens_of_one_checkout_start_only_one_window() {
    let craft = Craft::new();
    let first = craft.command(&craft.project, &["figcraft", "--workspace", "2"], &[]).spawn().unwrap();
    let second = craft.command(&craft.project, &["figcraft", "--workspace", "2"], &[]).spawn().unwrap();
    assert!(finish(first, std::time::Duration::from_secs(30)).status.success());
    assert!(finish(second, std::time::Duration::from_secs(30)).status.success());
    assert_eq!(craft.sandbox.calls("figcraft").len(), 1);
}

#[test]
fn a_closed_instances_port_is_not_reassigned_to_another_checkout() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert!(output.status.success(), "initial open: {}{}", stdout(&output), stderr(&output));
    let original_port = craft.started_port(0);
    let original_pid = fs::read_to_string(craft.sandbox.home.join("figcraft-pids")).unwrap();
    assert!(Command::new("kill").arg(original_pid.trim()).status().unwrap().success());
    // A signalled instance can still answer for a moment; reopening then rightly meets an
    // unverified listener (todo eefc1126). The instance is closed once its port is.
    wait_until_closed(original_port);
    fs::write(craft.sandbox.home.join("figcraft-pids"), "").unwrap();
    let other = craft.other_project("other");
    let output = craft.command(&other, &["figcraft", "--workspace", "2"], &[]).output().unwrap();
    assert!(output.status.success(), "other checkout: {}{}", stdout(&output), stderr(&output));
    assert_ne!(original_port, craft.started_port(1));
    let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
    assert!(output.status.success(), "reopen: {}{}", stdout(&output), stderr(&output));
    assert_eq!(original_port, craft.started_port(2));
}

#[test]
fn an_unrelated_listener_on_a_reserved_port_is_not_reported_as_figcraft() {
    let craft = Craft::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("CRAFT_OPEN_PORT", &port)]);
    assert_eq!(output.status.code(), Some(3), "{}{}", stdout(&output), stderr(&output));
    assert!(craft.sandbox.calls("figcraft").is_empty());
}

#[test]
fn a_window_with_shared_recovery_storage_is_not_adopted_even_with_matching_roots() {
    let craft = Craft::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let root = craft.project.to_str().unwrap();
    let open = WaitingProcess::new("figcraft", &["--control", &port, "--automation-read-root", root, "--automation-write-root", root], &craft.project);
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("SHIM_FIGCRAFT_PID", &open.id()), ("CRAFT_OPEN_PORT", &port)]);
    assert_eq!(output.status.code(), Some(3), "{}{}", stdout(&output), stderr(&output));
    assert!(craft.sandbox.calls("figcraft").is_empty());
}

#[test]
fn simultaneous_opens_of_different_checkouts_get_distinct_ports() {
    let craft = Craft::new();
    let other = craft.other_project("other");
    let first = craft.command(&craft.project, &["figcraft", "--workspace", "2"], &[]).spawn().unwrap();
    let second = craft.command(&other, &["figcraft", "--workspace", "2"], &[]).spawn().unwrap();
    assert!(finish(first, std::time::Duration::from_secs(30)).status.success());
    assert!(finish(second, std::time::Duration::from_secs(30)).status.success());
    assert_ne!(craft.started_port(0), craft.started_port(1));
}

#[test]
fn project_paths_with_spaces_are_kept_as_one_automation_root() {
    let craft = Craft::new();
    let project = craft.other_project("a project");
    for _ in 0..2 {
        let output = craft.command(&project, &["figcraft", "--workspace", "2"], &[]).output().unwrap();
        assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    }
    assert_eq!(craft.sandbox.calls("figcraft").len(), 1);
}

#[test]
fn a_foreign_sandbox_retaining_the_requested_port_stops_startup() {
    let craft = Craft::new();
    let other = craft.other_project("other");
    let holder = WaitingProcess::new("greywall", &["--forward", "17979", "--"], &other);
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("SHIM_GREYWALL_PID", &holder.id()), ("CRAFT_OPEN_PORT", "17979")]);
    assert_eq!(output.status.code(), Some(3), "{}{}", stdout(&output), stderr(&output));
    assert!(craft.sandbox.calls("figcraft").is_empty());
}

#[test]
fn alternate_greywall_flag_spellings_keep_stale_forward_protection() {
    for arguments in [vec!["--forward=17979", "--"], vec!["-f17979", "--"], vec!["-f=17979", "--"], vec!["-f", "17979", "--"]] {
        let craft = Craft::new();
        let other = craft.other_project("other");
        let holder = WaitingProcess::new("greywall", &arguments, &other);
        let output = craft.run(&["figcraft", "--workspace", "2"], &[("SHIM_GREYWALL_PID", &holder.id()), ("CRAFT_OPEN_PORT", "17979")]);
        assert_eq!(output.status.code(), Some(3), "{arguments:?}: {}{}", stdout(&output), stderr(&output));
        assert!(craft.sandbox.calls("figcraft").is_empty());
    }
}

#[test]
fn fixture_processes_expose_their_arguments_as_soon_as_they_are_created() {
    let craft = Craft::new();
    for _ in 0..50 {
        let holder = WaitingProcess::new("greywall", &["-f=17979", "--"], &craft.project);
        let arguments = fs::read(format!("/proc/{}/cmdline", holder.id())).unwrap();
        assert!(arguments.ends_with(b"greywall\0-f=17979\0--\0"), "{:?}", String::from_utf8_lossy(&arguments));
    }
}

#[test]
fn a_reserved_port_cannot_be_claimed_by_a_different_checkout() {
    let craft = Craft::new();
    let storage = craft.sandbox.home.join(".config/greywall-agents/figcraft/ports");
    fs::create_dir_all(&storage).unwrap();
    fs::write(storage.join("17979"), "/some/other/checkout\n").unwrap();
    let output = craft.run(&["figcraft", "--workspace", "2"], &[("CRAFT_OPEN_PORT", "17979")]);
    assert_eq!(output.status.code(), Some(3), "{}{}", stdout(&output), stderr(&output));
    assert!(craft.sandbox.calls("figcraft").is_empty());
}

#[test]
fn invalid_control_ports_are_rejected_before_starting_a_window() {
    let craft = Craft::new();
    for port in ["0", "65536", "-1", "17979 other", "17979\n", "abc"] {
        assert_eq!(craft.run(&["figcraft", "--workspace", "2"], &[("CRAFT_OPEN_PORT", port)]).status.code(), Some(2), "{port:?}");
    }
    assert!(craft.sandbox.calls("figcraft").is_empty());
}

#[test]
fn a_window_that_does_not_become_ready_does_not_restart_the_session() {
    let craft = Craft::new();
    craft.sandbox.shim("setsid", "exit 0");
    let output = craft.run(&["figcraft", "--workspace", "2", "--session", SESSION_A], &[("CRAFT_OPEN_WAIT_S", "0")]);
    assert_eq!(output.status.code(), Some(1), "{}{}", stdout(&output), stderr(&output));
    assert!(!craft.sandbox.state.join("cockpit/restart-when-idle").exists());
}

#[test]
fn opening_without_a_session_explains_that_existing_connections_are_unchanged() {
    let craft = Craft::new();
    for _ in 0..2 {
        let output = craft.run(&["figcraft", "--workspace", "2"], &[]);
        assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
        let message = stdout(&output);
        assert!(message.contains("Existing sessions were not reconnected"), "{message}");
        assert!(message.contains("craft-open figcraft --session <session-id>"), "{message}");
    }
    assert!(!craft.sandbox.state.join("cockpit/restart-when-idle").exists());
}

#[test]
fn with_a_session_it_schedules_a_restart_and_a_message_to_carry_on() {
    for session in [SESSION_A, OPENCODE_SESSION] {
        let craft = Craft::new();
        let output = craft.run(&["figcraft", "--workspace", "2", "--session", session], &[]);
        assert_eq!(output.status.code(), Some(0), "{session}: {}{}", stdout(&output), stderr(&output));
        let state = craft.sandbox.state.join("cockpit");
        assert!(state.join("restart-when-idle").join(session).exists(), "{session}");
        let queued: Vec<String> = fs::read_dir(state.join("prompt-queue").join(session)).unwrap().flatten()
            .map(|entry| fs::read_to_string(entry.path()).unwrap()).collect();
        assert!(queued[0].contains("FigCraft is connected"), "{queued:?}");
    }
}

#[test]
fn an_unknown_app_or_session_is_refused() {
    let craft = Craft::new();
    assert_eq!(craft.run(&["photoshop"], &[]).status.code(), Some(2));
    for session in ["../x", "ses_../../../../tmp/x", "ses_short", "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L\n../x"] {
        assert_eq!(craft.run(&["figcraft", "--workspace", "2", "--session", session], &[]).status.code(), Some(2), "{session}");
    }
    assert!(!craft.sandbox.state.join("cockpit/restart-when-idle").exists());
}
