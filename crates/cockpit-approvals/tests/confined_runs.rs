//! Confined runs (#1387): `Runner` on a real state database, with fake `systemd-run` and
//! `systemctl` that record what they are asked and run the command in place. What the real
//! manager enforces is proved in `cockpit-processes` and the daemon's ignored tests.
#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_approvals::{
    Approved, EgressProxy, NoResult, Project, Reach, RootRoute, RunConfinement, RunRequest,
    RunSettings, Runner, SudoAskpass, approve_run,
};
use cockpit_processes::{LaunchContainment, Supervisor, SystemdTools};
use cockpit_turso::Database;
use common::{NOW, session};

const FAKE_SYSTEMD_RUN: &str = r#"#!/bin/sh
state=@STATE@
if [ "$1" = --version ]; then echo "systemd 262 (fake)"; exit 0; fi
printf '%s\n' "$@" > "$state/arguments"
unit=
while [ $# -gt 0 ]; do
  case $1 in
    --unit=*) unit=${1#--unit=} ;;
    --) shift; break ;;
  esac
  shift
done
printf '%s\n' "$$" > "$state/$unit"
exec "$@"
"#;

const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
state=@STATE@
case $2 in
  show-environment) exit 0 ;;
  kill) /bin/kill -s "${4#--signal=SIG}" -- "-$(cat "$state/$5")" 2>/dev/null; exit 0 ;;
  show)
    if [ -f "$state/$5" ] && /bin/kill -s 0 -- "-$(cat "$state/$5")" 2>/dev/null; then
      echo active
    else
      echo inactive
    fi ;;
esac
"#;

/// `torlek-sandbox` as the runner drives it: `run-egress` records its arguments, says it is
/// ready, and once its input closes names one destination it refused; `hand-over` records its
/// socket and runs the rest in its place. `run-root-route` and `root-route-sudo` speak the
/// root route's lines through two FIFOs instead of a socket, and the run's `sudo` prints the
/// root command it was let go instead of running it. The real hand-off and relay are proved
/// in `cockpit-sandbox-launcher`.
const FAKE_SANDBOX: &str = r#"#!/bin/sh
state=@STATE@
hex() { od -An -tx1 | tr -d ' \n'; }
unhex() {
  awk '{ for (i = 1; i <= length($0); i += 2)
    printf "%c", index("0123456789abcdef", substr($0, i, 1)) * 16 - 17 \
      + index("0123456789abcdef", substr($0, i + 1, 1)) }'
}
case $1 in
  run-root-route)
    folder=$2; shift; printf '%s\n' "$@" > "$state/root-route"
    mkfifo "$folder/asks" "$folder/answers" || exit 125
    echo ready
    ask=0
    while read -r line < "$folder/asks"; do
      ask=$((ask + 1))
      echo "ask $ask $line"
      read -r decision || exit 0
      printf '%s\n' "$decision" > "$folder/answers"
      case $decision in
        refuse*) echo "ended $ask refused: $(printf '%s' "${decision#refuse $ask }" | unhex)" ;;
        *) echo "ended $ask exited 0" ;;
      esac
    done ;;
  root-route-sudo)
    folder=$2; shift 2
    line="$$ $(printf '%s' "$PWD" | hex)"
    for word in "$@"; do line="$line $(printf '%s' "$word" | hex)"; done
    printf '%s\n' "$line" > "$folder/asks"
    read -r decision < "$folder/answers"
    set -- $decision
    case $1 in
      go) shift 3; printf 'root ran:'
          for word in "$@"; do printf ' %s' "$(printf '%s' "$word" | unhex)"; done
          echo; exit 0 ;;
      *) echo "sudo: $(printf '%s' "$3" | unhex)" >&2; exit 1 ;;
    esac ;;
  run-egress)
    # As the real one: it opens its socket in FOLDER, which must exist.
    [ -d "$2" ] || { echo "cannot create $2/proxy-handoff" >&2; exit 125; }
    shift; printf '%s\n' "$@" > "$state/egress"
    echo ready
    cat > /dev/null
    echo "refused other.example:443" ;;
  hand-over)
    printf '%s\n' "$2" > "$state/handed-over"
    shift 2; exec "$@" ;;
esac
"#;

struct Confined {
    fixture: common::Fixture,
    fakes: tempfile::TempDir,
    /// The asking session's checkout, the one folder the run writes.
    checkout: PathBuf,
    /// A folder the run must not see.
    secrets: PathBuf,
}

impl Confined {
    async fn new() -> Confined {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[
                cockpit_authorization::MIGRATIONS,
                cockpit_processes::MIGRATIONS,
                cockpit_approvals::MIGRATIONS,
            ],
        )
        .await
        .unwrap();
        let fixture = common::Fixture { dir, database };
        let checkout = fixture.cwd().join("checkout");
        let secrets = fixture.cwd().join("secrets");
        for folder in [&checkout, &secrets] {
            std::fs::create_dir(folder).unwrap();
        }
        let fakes = tempfile::tempdir().unwrap();
        for (name, script) in [
            ("systemd-run", FAKE_SYSTEMD_RUN),
            ("systemctl", FAKE_SYSTEMCTL),
            ("torlek-sandbox", FAKE_SANDBOX),
        ] {
            let path = fakes.path().join(name);
            std::fs::write(
                &path,
                script.replace("@STATE@", &fakes.path().display().to_string()),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Confined {
            fixture,
            fakes,
            checkout,
            secrets,
        }
    }

    fn settings(&self) -> RunSettings {
        // `env` stands in for the session leader: the fake runs the command in place, so
        // the terminal is already the launcher's.
        let mut settings = RunSettings::new(
            PathBuf::from("/bin/sh"),
            PathBuf::from("/usr/bin/env"),
            Vec::new(),
        );
        settings.environment = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("TMPDIR".into(), "/somewhere/else".into()),
        ];
        settings.confinement = RunConfinement::Required {
            hidden: vec![self.secrets.clone()],
            emptied: Vec::new(),
            reachable: Vec::new(),
            egress: None,
            root: None,
        };
        settings
    }

    /// The settings of a daemon whose confined runs reach root through the fake root route,
    /// its helper found by `locate`. `setsid` stands in for the session leader: the run's
    /// shell leads its own session, which the root route checks.
    fn settings_with_root_route(&self, locate: cockpit_approvals::LocateHelper) -> RunSettings {
        let mut settings = self.settings();
        settings.leader = PathBuf::from("/usr/bin/setsid");
        settings.sudo = Some(SudoAskpass::with_stand_in(
            self.fakes.path().join("sudo"),
            std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(self.fakes.path()).unwrap()),
            self.fakes.path().join("launchers"),
            vec!["/bin/true".into()],
        ));
        if let RunConfinement::Required { root, .. } = &mut settings.confinement {
            *root = Some(Box::new(RootRoute {
                launcher: self.fakes.path().join("torlek-sandbox"),
                folders: self.fixture.cwd().join("run-root-route"),
                hidden: vec![self.secrets.clone()],
                locate,
            }));
        }
        settings
    }

    fn root_runner(&self) -> Runner {
        let locate: cockpit_approvals::LocateHelper =
            Arc::new(|| Ok(PathBuf::from("/usr/local/lib/torlek/torlek-sandbox")));
        self.runner(
            self.settings_with_root_route(locate),
            LaunchContainment::SystemdScope,
        )
    }

    fn root_route_folder(&self, approved: &Approved) -> PathBuf {
        self.fixture
            .cwd()
            .join("run-root-route")
            .join(format!("run-{}", approved.id.0))
    }

    fn runner(&self, settings: RunSettings, containment: LaunchContainment) -> Runner {
        let tools = SystemdTools {
            systemd_run: self.fakes.path().join("systemd-run"),
            systemctl: self.fakes.path().join("systemctl"),
        };
        let supervisor = Supervisor::new(
            self.fixture.dir.path().join("executions"),
            containment,
            tools,
        );
        Runner::new(settings, supervisor)
    }

    /// `command`, asked from the checkout's project in `cwd` and approved.
    async fn approved(&self, command: &str, cwd: &Path) -> Approved {
        self.approved_reaching(command, cwd, Reach::default()).await
    }

    /// The settings of a daemon whose runs reach their card's hosts through the fake proxy.
    fn settings_with_proxy(&self) -> RunSettings {
        let mut settings = self.settings();
        if let RunConfinement::Required { egress, .. } = &mut settings.confinement {
            *egress = Some(EgressProxy {
                launcher: self.fakes.path().join("torlek-sandbox"),
                folders: self.fixture.cwd().join("run-egress"),
            });
        }
        settings
    }

    fn recorded(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.fakes.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `command`, asked from the checkout's project in `cwd` with `reach` on its card.
    async fn asked_reaching(&self, command: &str, cwd: &Path, reach: Reach) -> RunRequest {
        let asker = session("s1");
        self.fixture.grant(&asker).await;
        let mut ask = self.fixture.ask(command);
        ask.cwd = cwd.to_owned();
        ask.project = Some(Project {
            name: "demo".to_owned(),
            root: self.checkout.clone(),
        });
        ask.reach = reach;
        let write = self.fixture.database.write().await.unwrap();
        let asked = cockpit_approvals::ask_run(&write, &asker, ask, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        asked
    }

    async fn approved_reaching(&self, command: &str, cwd: &Path, reach: Reach) -> Approved {
        let asker = session("s1");
        self.fixture.grant(&asker).await;
        let mut ask = self.fixture.ask(command);
        ask.cwd = cwd.to_owned();
        ask.project = Some(Project {
            name: "demo".to_owned(),
            root: self.checkout.clone(),
        });
        ask.reach = reach;
        let write = self.fixture.database.write().await.unwrap();
        let asked = cockpit_approvals::ask_run(&write, &asker, ask, NOW)
            .await
            .unwrap();
        let (approved, _) = approve_run(
            &write,
            None,
            asked.id,
            asked.revision,
            &asked.digest(),
            NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        approved
    }

    /// `command`, asked from the checkout, with the files it runs pinned as the daemon pins
    /// them, and approved.
    async fn approved_pinned(&self, command: &str) -> Approved {
        let asked = self
            .asked_reaching(command, &self.checkout, Reach::default())
            .await;
        let lookup = cockpit_approvals::Lookup {
            cwd: &self.checkout,
            search_path: Some("/usr/bin:/bin".as_ref()),
            home: None,
        };
        let writes = cockpit_approvals::SessionWrites::Folders(vec![self.checkout.clone()]);
        let pinned = cockpit_approvals::pinned_files(&asked.command, lookup, &writes);
        let runs = cockpit_approvals::root_runs(&asked.command, lookup, &writes);
        let write = self.fixture.database.write().await.unwrap();
        let checked = cockpit_approvals::record_checks(
            &write,
            asked.id,
            asked.revision,
            &runs,
            &[],
            &pinned.files,
            NOW,
        )
        .await
        .unwrap();
        let (approved, _) = approve_run(
            &write,
            None,
            checked.id,
            checked.revision,
            &checked.digest(),
            NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        approved
    }

    fn arguments(&self) -> Vec<String> {
        std::fs::read_to_string(self.fakes.path().join("arguments"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn has_property(arguments: &[String], value: &str) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == "-p" && pair[1] == value)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_writes_only_its_checkout() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined.approved("echo ran", &confined.checkout).await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert_eq!(ran.result.exit_code, 0);
    assert!(ran.result.output.shown.contains("ran"), "{ran:?}");
    let arguments = confined.arguments();
    assert!(
        has_property(
            &arguments,
            &format!("BindPaths=\"{}\"", confined.checkout.display())
        ),
        "{arguments:?}"
    );
    assert!(
        has_property(
            &arguments,
            &format!("InaccessiblePaths=-\"{}\"", confined.secrets.display())
        ),
        "{arguments:?}"
    );
}

/// `/tmp` is the run's own (`PrivateTmp=`): the owner's `TMPDIR` would point outside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_keeps_its_temporary_files_in_its_own_tmp() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined
        .approved("echo \"tmp=$TMPDIR\"", &confined.checkout)
        .await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert!(ran.result.output.shown.contains("tmp=/tmp"), "{ran:?}");
}

/// Readiness names a host without a systemd user manager; a run there is refused, never run
/// unconfined.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_is_refused_without_a_systemd_manager() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::ProcessGroup);
    let marker = confined.checkout.join("ran");
    let approved = confined
        .approved(&format!("touch '{}'", marker.display()), &confined.checkout)
        .await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(reason.contains("confined"), "{reason}");
    assert!(!marker.exists(), "the command ran unconfined");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_in_a_hidden_folder_is_not_started() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined.approved("true", &confined.secrets).await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(
        reason.contains(&confined.secrets.display().to_string()),
        "{reason}"
    );
    assert!(confined.arguments().is_empty(), "systemd-run was called");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_the_card_names_is_written_besides_the_checkout() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let named = confined.fixture.cwd().join("build-cache");
    std::fs::create_dir(&named).unwrap();
    let approved = confined
        .approved_reaching(
            "true",
            &confined.checkout,
            Reach {
                writes: vec![named.clone()],
                hosts: Vec::new(),
            },
        )
        .await;

    runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    let arguments = confined.arguments();
    for writable in [&confined.checkout, &named] {
        assert!(
            has_property(&arguments, &format!("BindPaths=\"{}\"", writable.display())),
            "{arguments:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_the_card_names_that_now_resolves_elsewhere_is_not_started() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let named = confined.fixture.cwd().join("build-cache");
    let elsewhere = confined.fixture.cwd().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &named).unwrap();
    let approved = confined
        .approved_reaching(
            "true",
            &confined.checkout,
            Reach {
                writes: vec![named.clone()],
                hosts: Vec::new(),
            },
        )
        .await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(reason.contains("now resolves to"), "{reason}");
    assert!(confined.arguments().is_empty(), "systemd-run was called");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_the_card_names_in_a_hidden_one_is_not_started() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined
        .approved_reaching(
            "true",
            &confined.checkout,
            Reach {
                writes: vec![confined.secrets.clone()],
                hosts: Vec::new(),
            },
        )
        .await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(reason.contains("cannot see"), "{reason}");
    assert!(confined.arguments().is_empty(), "systemd-run was called");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_card_naming_hosts_without_a_proxy_is_not_started() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined
        .approved_reaching(
            "true",
            &confined.checkout,
            Reach {
                writes: Vec::new(),
                hosts: vec!["example.com".to_owned()],
            },
        )
        .await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(reason.contains("proxy"), "{reason}");
    assert!(confined.arguments().is_empty(), "systemd-run was called");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_that_cannot_start_leaves_no_folder_behind() {
    let confined = Confined::new().await;
    let mut settings = confined.settings_with_proxy();
    if let RunConfinement::Required {
        egress: Some(proxy),
        ..
    } = &mut settings.confinement
    {
        proxy.launcher = confined.fakes.path().join("no-such-sandbox");
    }
    let runner = confined.runner(settings, LaunchContainment::SystemdScope);
    let approved = confined
        .approved_reaching(
            "true",
            &confined.checkout,
            Reach {
                writes: Vec::new(),
                hosts: vec!["example.com".to_owned()],
            },
        )
        .await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    assert!(matches!(ended, Err(NoResult::NotStarted(_))), "{ended:?}");
    let folder = confined
        .fixture
        .cwd()
        .join("run-egress")
        .join(format!("run-{}", approved.id.0));
    assert!(!folder.exists(), "{} was left behind", folder.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_card_host_is_reached_through_the_runs_own_proxy() {
    let confined = Confined::new().await;
    let runner = confined.runner(
        confined.settings_with_proxy(),
        LaunchContainment::SystemdScope,
    );
    let approved = confined
        .approved_reaching(
            "echo \"proxy=$HTTPS_PROXY\"",
            &confined.checkout,
            Reach {
                writes: Vec::new(),
                hosts: vec!["example.com".to_owned()],
            },
        )
        .await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert!(
        ran.result
            .output
            .shown
            .contains("proxy=http://127.0.0.1:3128"),
        "{ran:?}"
    );
    let folder = confined
        .fixture
        .cwd()
        .join("run-egress")
        .join(format!("run-{}", approved.id.0));
    assert_eq!(
        confined.recorded("egress"),
        [
            folder.display().to_string(),
            "s1".to_owned(),
            approved.id.0.to_string(),
            "example.com".to_owned()
        ]
    );
    let handoff = folder.join("proxy-handoff");
    assert_eq!(
        confined.recorded("handed-over"),
        [handoff.join("handoff.sock").display().to_string()]
    );
    let arguments = confined.arguments();
    assert!(
        has_property(
            &arguments,
            &format!("BindReadOnlyPaths=\"{}\"", handoff.display())
        ),
        "{arguments:?}"
    );
    assert_eq!(
        ran.result.limit_hits,
        ["tried to reach other.example:443, which the run's card does not name"]
    );
    assert!(!folder.exists(), "the run's proxy folder was left behind");
}

/// The card says what a confined run may do, in the same words in the pane and `approval
/// show`.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_card_names_the_runs_limits() {
    let confined = Confined::new().await;
    let runner = confined.runner(
        confined.settings_with_proxy(),
        LaunchContainment::SystemdScope,
    );
    let named = confined.fixture.cwd().join("build-cache");
    let asked = confined
        .asked_reaching(
            "make",
            &confined.checkout,
            Reach {
                writes: vec![named.clone()],
                hosts: vec!["example.com".to_owned(), "127.0.0.2:8080".to_owned()],
            },
        )
        .await;

    assert_eq!(
        runner.card_limits(&asked),
        [
            format!(
                "writes only {}, {} and a /tmp of its own",
                confined.checkout.display(),
                named.display()
            ),
            format!("cannot see {}", confined.secrets.display()),
            "reaches only example.com, 127.0.0.2:8080".to_owned(),
            "no sudo: only a `sudo COMMAND` written in the command line runs as root".to_owned(),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_card_names_each_root_command_and_what_root_writes() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    let site = confined.fixture.cwd().join("site");
    let asked = confined
        .asked_reaching(
            "make && sudo make install",
            &confined.checkout,
            Reach {
                writes: vec![site.clone()],
                hosts: Vec::new(),
            },
        )
        .await;

    assert_eq!(
        runner.card_limits(&asked).last().unwrap(),
        &format!(
            "runs as root, asking your password each time: `make install`; as root it writes \
             only {}, and a script or program it starts runs nothing else as root",
            site.display()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_card_names_a_sudo_that_never_runs_as_root() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    let asked = confined
        .asked_reaching("sudo -u web true", &confined.checkout, Reach::default())
        .await;

    let limits = runner.card_limits(&asked);

    let refused = limits.last().unwrap();
    assert!(
        refused.starts_with("never runs as root: `sudo -A -k -u web true`: ")
            && refused.contains("-u"),
        "{limits:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_card_says_why_a_route_without_its_helper_runs_nothing_as_root() {
    let confined = Confined::new().await;
    let locate: cockpit_approvals::LocateHelper =
        Arc::new(|| Err("the root-run helper is not installed".to_owned()));
    let runner = confined.runner(
        confined.settings_with_root_route(locate),
        LaunchContainment::SystemdScope,
    );
    let asked = confined
        .asked_reaching("sudo make install", &confined.checkout, Reach::default())
        .await;

    assert_eq!(
        runner.card_limits(&asked).last().unwrap(),
        "no sudo: the root-run helper is not installed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_card_says_a_line_ending_with_a_backslash_runs_nothing_as_root() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    let asked = confined
        .asked_reaching("sudo make install \\", &confined.checkout, Reach::default())
        .await;

    let root = runner.card_limits(&asked).last().unwrap().clone();

    assert!(
        root.starts_with("no sudo: its command line ends with a backslash"),
        "{root}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_the_line_writes_runs_its_command_through_the_root_route() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    let site = confined.fixture.cwd().join("site");
    std::fs::create_dir(&site).unwrap();
    let approved = confined
        .approved_reaching(
            "sudo cp a.conf /srv/a.conf",
            &confined.checkout,
            Reach {
                writes: vec![site.clone()],
                hosts: Vec::new(),
            },
        )
        .await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert_eq!(ran.result.exit_code, 0, "{ran:?}");
    assert!(
        ran.result
            .output
            .shown
            .contains("root ran: cp a.conf /srv/a.conf"),
        "{ran:?}"
    );
    assert!(ran.result.limit_hits.is_empty(), "{ran:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_root_route_gets_the_cards_folders_and_the_askpass() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    let site = confined.fixture.cwd().join("site");
    std::fs::create_dir(&site).unwrap();
    let approved = confined
        .approved_reaching(
            "sudo true",
            &confined.checkout,
            Reach {
                writes: vec![site.clone()],
                hosts: Vec::new(),
            },
        )
        .await;

    runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    let folder = confined.root_route_folder(&approved);
    assert_eq!(
        confined.recorded("root-route"),
        [
            folder.display().to_string(),
            confined.fakes.path().join("sudo").display().to_string(),
            confined
                .fakes
                .path()
                .join("launchers/askpass")
                .display()
                .to_string(),
            approved.id.0.to_string(),
            String::new(),
            "--write".to_owned(),
            site.display().to_string(),
            "--hide".to_owned(),
            confined.secrets.display().to_string(),
        ]
    );
    let arguments = confined.arguments();
    assert!(
        has_property(
            &arguments,
            &format!("BindReadOnlyPaths=\"{}\"", folder.display())
        ),
        "{arguments:?}"
    );
    assert!(
        !folder.exists(),
        "the run's root route folder was left behind"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_a_script_starts_is_refused_and_named() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    std::fs::write(confined.checkout.join("setup.sh"), "sudo true\n").unwrap();
    let approved = confined
        .approved("sudo true; sh ./setup.sh", &confined.checkout)
        .await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert_eq!(ran.result.exit_code, 1, "{ran:?}");
    let shown = &ran.result.output.shown;
    assert!(shown.contains("root ran: true"), "{shown}");
    assert!(
        shown.contains("sudo: this sudo was started by process"),
        "{shown}"
    );
    assert!(
        ran.result
            .limit_hits
            .iter()
            .any(|hit| hit
                .starts_with("its sudo ran nothing as root: this sudo was started by process")),
        "{ran:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_with_words_the_line_does_not_write_is_refused() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    let approved = confined
        .approved(
            "program=false; sudo true && sudo $program",
            &confined.checkout,
        )
        .await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert_eq!(ran.result.exit_code, 1, "{ran:?}");
    assert!(
        ran.result
            .output
            .shown
            .contains("sudo: `sudo false` is not a sudo the approved command line writes out"),
        "{ran:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_part_whose_script_changed_after_approval_is_refused() {
    let confined = Confined::new().await;
    let runner = confined.root_runner();
    std::fs::write(confined.checkout.join("install.sh"), "echo installing\n").unwrap();
    let approved = confined
        .approved_pinned("echo 'echo more' >> install.sh; sudo sh ./install.sh")
        .await;
    assert_eq!(approved.bound.len(), 1, "{approved:?}");

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert_eq!(ran.result.exit_code, 1, "{ran:?}");
    assert!(
        ran.result
            .output
            .shown
            .contains("sudo: ./install.sh changed after you approved"),
        "{ran:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_card_without_hosts_says_it_has_no_network() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let asked = confined
        .asked_reaching("make", &confined.checkout, Reach::default())
        .await;

    let limits = runner.card_limits(&asked);

    assert_eq!(
        limits[0],
        format!(
            "writes only {} and a /tmp of its own",
            confined.checkout.display()
        )
    );
    assert_eq!(limits[2], "no network");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unconfined_card_says_the_run_has_the_owners_rights() {
    let confined = Confined::new().await;
    let settings = RunSettings {
        confinement: RunConfinement::Off,
        ..confined.settings()
    };
    let runner = confined.runner(settings, LaunchContainment::SystemdScope);
    let asked = confined
        .asked_reaching("make", &confined.checkout, Reach::default())
        .await;

    assert_eq!(
        runner.card_limits(&asked),
        ["UNCONFINED: runs with your rights (`agents.confined_runs = \"off\"`)"]
    );
}
