//! The readiness report through `cockpit_readiness::assess` (#50, report 09): one status per
//! requirement, and a next action for every requirement that is not available.

use std::fs;
use std::path::PathBuf;

use cockpit_config::{AgentSandbox, EffectiveConfig, Environment};
use cockpit_readiness::{
    CheckoutFacts, ControlFacts, DockerEngine, HostFacts, Platform, PresetFacts, ProjectFacts,
    ProjectNeeds, ProviderFacts, ProviderFound, ProviderUnusable, Report, Requirement, Status,
    Trust, assess, needs_docker,
};
use tempfile::TempDir;

const NATIVE_BACKEND: &str = "torlek-sandbox with bubblewrap 0.13.1 and Landlock ABI 9";

const SHARED_DESKTOP_REFUSAL: &str = "the sandbox refuses `desktop session shared`: the native sandbox does not show the owner's desktop yet (DISPLAY names /tmp/.X11-unix/X1)";

const NO_BUBBLEWRAP: &str = "bubblewrap (`bwrap`) is not on the daemon's PATH";

/// The owner's file of a daemon that applies `required`: the file it read says so.
const OWNER_REQUIRES_THE_SANDBOX: &str = "[agents]\nsandbox = \"required\"\n";

/// The owner's file `host`'s daemon last read: nothing changed since.
fn owner_file_as_applied(host: &HostFacts) -> String {
    format!(
        "[agents]\nsandbox = \"{}\"\n",
        host.sandbox_setting.as_str()
    )
}

fn ready_host() -> HostFacts {
    HostFacts {
        platform: Platform::Linux,
        tmux: Ok("3.6".to_owned()),
        // The product default; `required` runs agents in the native sandbox (#829).
        sandbox_setting: AgentSandbox::Off,
        sandbox_backend: Ok(NATIVE_BACKEND.to_owned()),
        shared_desktop_refusal: None,
        providers: vec![ProviderFacts {
            name: "claude".to_owned(),
            found: Ok(ProviderFound {
                sign_in: cockpit_readiness::SignInFacts {
                    signed_in: None,
                    detail: "not probed".to_owned(),
                    next: "check sign-in".to_owned(),
                },
                version: Some("2.1.316".to_owned()),
                tested: true,
                capabilities: vec!["resume".to_owned(), "events".to_owned()],
            }),
        }],
        controls: vec![ControlFacts {
            control: "memory-max".to_owned(),
            enforced: Ok("systemd scope".to_owned()),
        }],
    }
}

struct Fixture {
    home: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            home: TempDir::new().unwrap(),
        }
    }

    fn project_dir(&self) -> PathBuf {
        self.home.path().join("project")
    }

    /// The project's `.torlek/config.toml`, and the owner's file when `owner` is set.
    fn configure(&self, project: &str, owner: Option<&str>) {
        fs::create_dir_all(self.project_dir().join(".torlek")).unwrap();
        fs::write(self.project_dir().join(".torlek/config.toml"), project).unwrap();
        if let Some(owner) = owner {
            let dir = self.home.path().join(".config/torlek");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("config.toml"), owner).unwrap();
        }
    }

    fn config(&self) -> Result<EffectiveConfig, String> {
        let environment = Environment {
            home: Some(self.home.path().into()),
            ..Environment::default()
        };
        cockpit_config::load(&environment, Some(&self.project_dir()), &[])
            .map_err(|error| error.to_string())
    }

    /// The owner's configuration alone, as a report without a project reads it.
    fn owner_config(&self) -> Result<EffectiveConfig, String> {
        let environment = Environment {
            home: Some(self.home.path().into()),
            ..Environment::default()
        };
        cockpit_config::load(&environment, None, &[]).map_err(|error| error.to_string())
    }

    fn facts(&self) -> ProjectFacts {
        ProjectFacts {
            dir: self.project_dir(),
            checkout: Some(CheckoutFacts {
                project: "project".to_owned(),
                project_root: self.project_dir(),
                trust: Trust::Untrusted,
            }),
            presets: vec![PresetFacts {
                name: "code-change".to_owned(),
                capabilities: vec!["events".to_owned()],
            }],
            capacity: ProjectNeeds {
                cpu_millis: 8000,
                memory_mib: 16_384,
            },
            docker: None,
        }
    }

    fn report(&self, host: &HostFacts, project: &ProjectFacts) -> Report {
        assess(host, &self.config(), Some(project))
    }
}

fn requirement<'report>(report: &'report Report, name: &str) -> &'report Requirement {
    report
        .requirements
        .iter()
        .find(|requirement| requirement.name == name)
        .unwrap_or_else(|| panic!("no {name} in {report:#?}"))
}

#[test]
fn a_ready_host_reports_every_host_requirement_available() {
    let fixture = Fixture::new();
    fixture.configure("", None);

    let report = assess(&ready_host(), &fixture.config(), None);

    let names: Vec<&str> = report
        .requirements
        .iter()
        .map(|requirement| requirement.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "config",
            "platform",
            "tmux",
            "sandbox",
            "provider claude",
            "provider claude sign-in",
            "resource controls"
        ]
    );
    assert!(report.is_ready(), "{report:#?}");
}

#[test]
fn every_requirement_that_is_not_available_names_its_next_action() {
    let fixture = Fixture::new();
    fixture.configure(
        "[workflow]\npreset = \"nope\"\n[needs]\ncpu_millis = 99000\n[[checks]]\nname = \"t\"\ncommand = \"x\"\nworking_directory = \"missing\"\n",
        Some(OWNER_REQUIRES_THE_SANDBOX),
    );
    let host = HostFacts {
        platform: Platform::Unsupported("freebsd".to_owned()),
        tmux: Err("no tmux".to_owned()),
        sandbox_setting: AgentSandbox::Required,
        sandbox_backend: Err(NO_BUBBLEWRAP.to_owned()),
        shared_desktop_refusal: None,
        providers: vec![ProviderFacts {
            name: "claude".to_owned(),
            found: Err(ProviderUnusable::NotOnPath),
        }],
        controls: Vec::new(),
    };
    let mut project = fixture.facts();
    project.checkout = None;

    let report = fixture.report(&host, &project);

    // An available requirement may still advise (an unsandboxed owner, an optional provider).
    for requirement in &report.requirements {
        assert!(
            requirement.status == Status::Available || requirement.next.is_some(),
            "{requirement:#?}"
        );
    }
    assert!(!report.is_ready());
}

/// The security rule of #828: an explicit `required` on a host without the sandbox blocks,
/// since every launch is refused, and the report names why and the owner's one way to run
/// agents anyway.
#[test]
fn a_required_sandbox_without_a_backend_blocks_naming_why() {
    let fixture = Fixture::new();
    fixture.configure("", Some(OWNER_REQUIRES_THE_SANDBOX));
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Required,
        sandbox_backend: Err(NO_BUBBLEWRAP.to_owned()),
        ..ready_host()
    };

    let report = fixture.report(&host, &fixture.facts());

    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::MissingPrerequisite);
    assert!(sandbox.detail.starts_with("no sandbox: "), "{sandbox:?}");
    assert!(sandbox.detail.contains(NO_BUBBLEWRAP), "{sandbox:?}");
    assert!(
        sandbox
            .next
            .as_deref()
            .unwrap()
            .contains("agents.sandbox = \"off\""),
        "{sandbox:?}"
    );
    assert!(!report.is_ready());
}

#[test]
fn a_required_sandbox_with_the_native_backend_is_available_naming_it() {
    let fixture = Fixture::new();
    fixture.configure("", Some(OWNER_REQUIRES_THE_SANDBOX));
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Required,
        ..ready_host()
    };

    let report = fixture.report(&host, &fixture.facts());

    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::Available, "{sandbox:?}");
    assert!(sandbox.detail.contains(NATIVE_BACKEND), "{sandbox:?}");
    assert!(sandbox.detail.contains("torlek network"), "{sandbox:?}");
}

/// #965: `required` with a backend still blocks while the sandbox refuses the shared
/// desktop session, with the launch refusal's own message and fix.
#[test]
fn a_required_sandbox_that_refuses_the_shared_desktop_blocks_naming_the_fix() {
    let fixture = Fixture::new();
    fixture.configure("", Some(OWNER_REQUIRES_THE_SANDBOX));
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Required,
        shared_desktop_refusal: Some(SHARED_DESKTOP_REFUSAL.to_owned()),
        ..ready_host()
    };

    let report = fixture.report(&host, &fixture.facts());

    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::MissingPrerequisite, "{sandbox:?}");
    assert!(
        sandbox.detail.contains(SHARED_DESKTOP_REFUSAL),
        "{sandbox:?}"
    );
    assert!(
        sandbox
            .next
            .as_deref()
            .unwrap()
            .contains("agents.desktop_session = \"withheld\""),
        "{sandbox:?}"
    );
    assert!(!report.is_ready());
}

#[test]
fn a_desktop_refusal_does_not_block_an_unsandboxed_owner() {
    let fixture = Fixture::new();
    fixture.configure("", Some("[agents]\nsandbox = \"off\"\n"));
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Off,
        shared_desktop_refusal: Some(SHARED_DESKTOP_REFUSAL.to_owned()),
        ..ready_host()
    };

    let report = fixture.report(&host, &fixture.facts());

    assert_eq!(requirement(&report, "sandbox").status, Status::Available);
}

/// Each case: host facts changed from a ready host, the requirement and the status it gets.
#[test]
fn a_host_fact_decides_its_requirement_status() {
    let cases: Vec<(HostFacts, &str, Status)> = vec![
        (
            HostFacts {
                tmux: Err("`tmux` is tmux 3.4; Torlek needs tmux 3.6 or newer".to_owned()),
                ..ready_host()
            },
            "tmux",
            Status::MissingPrerequisite,
        ),
        (
            HostFacts {
                sandbox_setting: AgentSandbox::Required,
                sandbox_backend: Err(NO_BUBBLEWRAP.to_owned()),
                ..ready_host()
            },
            "sandbox",
            Status::MissingPrerequisite,
        ),
        (
            HostFacts {
                platform: Platform::Unsupported("windows".to_owned()),
                ..ready_host()
            },
            "platform",
            Status::Unsupported,
        ),
        (
            HostFacts {
                providers: vec![missing("claude")],
                ..ready_host()
            },
            "providers",
            Status::MissingPrerequisite,
        ),
        (
            HostFacts {
                providers: Vec::new(),
                ..ready_host()
            },
            "providers",
            Status::MissingPrerequisite,
        ),
        (ready_host(), "provider claude sign-in", Status::Unknown),
        (
            HostFacts {
                controls: Vec::new(),
                ..ready_host()
            },
            "resource controls",
            Status::Unknown,
        ),
    ];
    for (host, name, status) in cases {
        let fixture = Fixture::new();
        fixture.configure("", Some(&owner_file_as_applied(&host)));

        let report = assess(&host, &fixture.owner_config(), None);

        assert_eq!(requirement(&report, name).status, status, "{report:#?}");
    }
}

fn missing(name: &str) -> ProviderFacts {
    ProviderFacts {
        name: name.to_owned(),
        found: Err(ProviderUnusable::NotOnPath),
    }
}

/// #411: a host needs one usable provider, not every provider Cockpit supports.
#[test]
fn a_provider_that_is_not_installed_is_listed_with_how_to_install_it_without_blocking() {
    let mut host = ready_host();
    host.providers.push(missing("codex"));

    let report = assess(&host, &Ok(owner_only_config()), None);

    let codex = requirement(&report, "provider codex");
    assert!(codex.detail.contains("no `codex` on PATH"), "{codex:#?}");
    assert!(
        codex
            .next
            .as_deref()
            .is_some_and(|next| next.contains("install codex")),
        "{codex:#?}"
    );
    assert!(report.is_ready(), "{report:#?}");
}

fn refused(name: &str) -> ProviderFacts {
    ProviderFacts {
        name: name.to_owned(),
        found: Err(ProviderUnusable::Refused {
            reason: format!("/usr/bin/{name}: `--version` printed \"{name} ?\", not a version"),
        }),
    }
}

/// #623: installing a program that is already on PATH fixes nothing.
#[test]
fn a_provider_on_path_that_was_refused_is_not_advised_to_be_installed() {
    let mut host = ready_host();
    host.providers.push(refused("opencode"));

    let report = assess(&host, &Ok(owner_only_config()), None);

    let opencode = requirement(&report, "provider opencode");
    assert!(opencode.detail.contains("not a version"), "{opencode:#?}");
    let next = opencode.next.as_deref().unwrap_or_default();
    assert!(next.contains("`opencode --version`"), "{opencode:#?}");
    assert!(!next.contains("install"), "{opencode:#?}");
}

#[test]
fn a_host_whose_only_provider_was_refused_blocks_pointing_at_it() {
    let host = HostFacts {
        providers: vec![missing("claude"), missing("codex"), refused("opencode")],
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let providers = requirement(&report, "providers");
    assert!(
        providers
            .next
            .as_deref()
            .is_some_and(|next| next.starts_with("fix a provider named above")),
        "{providers:#?}"
    );
}

#[test]
fn a_host_whose_only_provider_declaration_was_refused_blocks_pointing_at_it() {
    let host = HostFacts {
        providers: vec![
            missing("claude"),
            missing("codex"),
            ProviderFacts {
                name: "opencode".to_owned(),
                found: Err(ProviderUnusable::DeclarationRefused {
                    reason: "it declares ~/.ssh".to_owned(),
                }),
            },
        ],
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let providers = requirement(&report, "providers");
    assert!(
        providers
            .next
            .as_deref()
            .is_some_and(|next| next.starts_with("fix a provider named above")),
        "{providers:#?}"
    );
}

#[test]
fn a_host_where_no_provider_is_usable_blocks_naming_every_supported_one() {
    let host = HostFacts {
        providers: vec![missing("claude"), missing("codex"), missing("opencode")],
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let providers = requirement(&report, "providers");
    assert_eq!(providers.status, Status::MissingPrerequisite);
    assert!(
        providers
            .next
            .as_deref()
            .is_some_and(|next| next.contains("claude, codex, opencode")),
        "{providers:#?}"
    );
    assert!(!report.is_ready());
}

fn owner_only_config() -> EffectiveConfig {
    let home = TempDir::new().unwrap();
    let environment = Environment {
        home: Some(home.path().into()),
        ..Environment::default()
    };
    cockpit_config::load(&environment, None, &[]).unwrap()
}

#[test]
fn an_unreadable_configuration_misses_a_prerequisite_and_skips_the_project() {
    let fixture = Fixture::new();
    fixture.configure("[execution]\nenvironment = \"vm\"\n", None);

    let report = fixture.report(&ready_host(), &fixture.facts());

    let config = requirement(&report, "config");
    assert_eq!(config.status, Status::MissingPrerequisite);
    assert!(config.detail.contains(".torlek/config.toml"), "{config:#?}");
    assert!(
        !report
            .requirements
            .iter()
            .any(|requirement| requirement.name == "project"),
        "{report:#?}"
    );
}

/// Agents running unsandboxed, by default while no backend exists, is said loudly.
#[test]
fn an_unsandboxed_owner_is_reported_as_such() {
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Off,
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::Available);
    assert!(sandbox.detail.starts_with("no sandbox: "), "{sandbox:#?}");
    assert!(sandbox.detail.contains("UNSANDBOXED"), "{sandbox:#?}");
    assert!(
        sandbox.detail.contains(
            "an unsandboxed agent can reach the owner socket and act as the owner, so Torlek's \
             grants only hold under the sandbox"
        ),
        "{sandbox:#?}"
    );
    assert!(sandbox.next.is_some(), "{sandbox:#?}");
}

/// #963: the daemon applies the setting it last read. One written since is not in force, so
/// the line names both values, what still happens to agents and the command that applies the
/// file, and it blocks.
#[test]
fn a_sandbox_setting_the_daemon_has_not_read_blocks_naming_what_still_applies() {
    let native: Result<String, String> = Ok(NATIVE_BACKEND.to_owned());
    let missing: Result<String, String> = Err(NO_BUBBLEWRAP.to_owned());
    let cases = [
        (
            "required",
            AgentSandbox::Off,
            &native,
            "agents still run UNSANDBOXED".to_owned(),
        ),
        (
            "off",
            AgentSandbox::Required,
            &native,
            format!("every agent still runs in {NATIVE_BACKEND}"),
        ),
        (
            "off",
            AgentSandbox::Required,
            &missing,
            format!("every agent launch is still refused: {NO_BUBBLEWRAP}"),
        ),
    ];
    for (written, applied, backend, effect) in cases {
        let fixture = Fixture::new();
        fixture.configure("", Some(&format!("[agents]\nsandbox = \"{written}\"\n")));
        let host = HostFacts {
            sandbox_setting: applied,
            sandbox_backend: backend.clone(),
            ..ready_host()
        };

        let report = assess(&host, &fixture.owner_config(), None);

        let sandbox = requirement(&report, "sandbox");
        assert_eq!(sandbox.status, Status::MissingPrerequisite, "{sandbox:#?}");
        assert!(
            sandbox
                .detail
                .starts_with(&format!("agents.sandbox = \"{written}\" (user config ")),
            "{sandbox:#?}"
        );
        assert!(
            sandbox.detail.contains(&format!(
                "the running daemon still applies \"{}\"",
                applied.as_str()
            )),
            "{sandbox:#?}"
        );
        assert!(sandbox.detail.contains(&effect), "{sandbox:#?}");
        assert!(
            sandbox
                .next
                .as_deref()
                .unwrap()
                .contains("`torlek config reload`"),
            "{sandbox:#?}"
        );
        assert!(!report.is_ready());
    }
}

/// #963: the reload the line names brings `required` into force, and a host without the
/// sandbox then refuses every launch. The next action says so before the owner reloads.
#[test]
fn a_written_required_on_a_host_without_the_sandbox_warns_that_launches_are_then_refused() {
    let fixture = Fixture::new();
    fixture.configure("", Some(OWNER_REQUIRES_THE_SANDBOX));
    let host = HostFacts {
        sandbox_backend: Err(NO_BUBBLEWRAP.to_owned()),
        ..ready_host()
    };

    let report = assess(&host, &fixture.owner_config(), None);

    let next = requirement(&report, "sandbox").next.as_deref().unwrap();
    assert!(
        next.contains("`torlek config reload`")
            && next.contains(NO_BUBBLEWRAP)
            && next.contains("\"required\" then refuses every launch"),
        "{next}"
    );
}

/// #963: a line the owner deleted leaves the product default, which the daemon has not read
/// either.
#[test]
fn a_sandbox_setting_removed_from_the_owner_file_is_named_as_the_product_default() {
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Required,
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let sandbox = requirement(&report, "sandbox");
    assert!(
        sandbox
            .detail
            .starts_with("agents.sandbox = \"off\" (product default) is not in force"),
        "{sandbox:#?}"
    );
}

/// #963: a file that does not read has no setting to compare, and `config` already blocks on
/// it: the line stays what the daemon applies.
#[test]
fn an_unreadable_configuration_leaves_the_sandbox_line_to_what_the_daemon_applies() {
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Required,
        ..ready_host()
    };

    let report = assess(&host, &Err("config.toml:2: unknown value".to_owned()), None);

    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::Available, "{sandbox:#?}");
    assert!(sandbox.detail.starts_with("sandboxed: "), "{sandbox:#?}");
}

#[test]
fn controls_the_host_cannot_enforce_are_named_as_requested_only() {
    let host = HostFacts {
        controls: vec![
            ControlFacts {
                control: "cpu-weight".to_owned(),
                enforced: Ok("systemd scope".to_owned()),
            },
            ControlFacts {
                control: "freeze".to_owned(),
                enforced: Err("no cgroup freezer".to_owned()),
            },
        ],
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let controls = requirement(&report, "resource controls");
    assert!(
        controls
            .detail
            .contains("requested only, not enforced here: freeze (no cgroup freezer)"),
        "{controls:#?}"
    );
}

#[test]
fn a_project_that_is_not_registered_names_the_command_that_registers_it() {
    let fixture = Fixture::new();
    fixture.configure("", None);
    let mut project = fixture.facts();
    project.checkout = None;

    let report = fixture.report(&ready_host(), &project);

    let requirement = requirement(&report, "project");
    assert_eq!(requirement.status, Status::MissingPrerequisite);
    assert_eq!(
        requirement.next.as_deref(),
        Some(format!("torlek project add {}", fixture.project_dir().display()).as_str())
    );
}

#[test]
fn a_project_on_a_windows_drive_under_wsl_must_move() {
    let fixture = Fixture::new();
    fixture.configure("", None);
    let host = HostFacts {
        platform: Platform::Wsl2 {
            windows_drives: vec![fixture.home.path().to_path_buf()],
        },
        ..ready_host()
    };

    let report = fixture.report(&host, &fixture.facts());

    let requirement = requirement(&report, "project");
    assert_eq!(requirement.status, Status::MissingPrerequisite);
    assert!(
        requirement.detail.contains("Windows drive"),
        "{requirement:#?}"
    );
}

/// Each case: the trust `cockpit-projects` decided and the status of `project trust`.
#[test]
fn an_untrusted_project_needs_the_owner_to_trust_it() {
    let cases = [
        (Trust::Trusted, Status::Available),
        (
            Trust::Inherited {
                from: PathBuf::from("/p/main"),
            },
            Status::Available,
        ),
        (Trust::NeverForReview, Status::Available),
        (Trust::Untrusted, Status::NeedsAuthorization),
    ];
    for (trust, status) in cases {
        let fixture = Fixture::new();
        fixture.configure("", None);
        let mut project = fixture.facts();
        project.checkout.as_mut().unwrap().trust = trust.clone();

        let report = fixture.report(&ready_host(), &project);

        let requirement = requirement(&report, "project trust");
        assert_eq!(requirement.status, status, "{trust:?}: {requirement:#?}");
        assert!(
            report.is_ready(),
            "authorization is the owner's choice, not a defect"
        );
    }
}

/// Each case: the project file, the provider's capabilities, and the preset's status.
#[test]
fn a_preset_needs_a_provider_that_reports_its_steps_capabilities() {
    let cases = [
        ("", vec!["events"], Status::Available),
        (
            "[workflow]\npreset = \"code-change\"\n",
            vec!["events"],
            Status::Available,
        ),
        (
            "[workflow]\npreset = \"code-change\"\n",
            vec!["resume"],
            Status::MissingPrerequisite,
        ),
        (
            "[workflow]\npreset = \"nope\"\n",
            vec!["events"],
            Status::MissingPrerequisite,
        ),
    ];
    for (contents, capabilities, status) in cases {
        let fixture = Fixture::new();
        fixture.configure(contents, None);
        let host = HostFacts {
            providers: vec![ProviderFacts {
                name: "claude".to_owned(),
                found: Ok(ProviderFound {
                    sign_in: cockpit_readiness::SignInFacts {
                        signed_in: None,
                        detail: "not probed".to_owned(),
                        next: "check sign-in".to_owned(),
                    },
                    version: None,
                    tested: false,
                    capabilities: capabilities.iter().map(|name| name.to_string()).collect(),
                }),
            }],
            ..ready_host()
        };

        let report = fixture.report(&host, &fixture.facts());

        let preset = requirement(&report, "workflow preset");
        assert_eq!(preset.status, status, "{contents:?}: {preset:#?}");
    }
}

/// Each case: the project's environment, the owner's sandbox setting and the status of the
/// project's execution environment. No backend exists (#828), so a project expecting the
/// sandbox is never available, and `required` blocks every project.
#[test]
fn the_execution_environment_the_project_expects_meets_what_the_owner_granted() {
    let native: Result<String, String> = Ok(NATIVE_BACKEND.to_owned());
    let missing: Result<String, String> = Err(NO_BUBBLEWRAP.to_owned());
    let cases = [
        ("native", AgentSandbox::Off, &native, Status::Available),
        ("native", AgentSandbox::Required, &native, Status::Available),
        (
            "native",
            AgentSandbox::Required,
            &missing,
            Status::MissingPrerequisite,
        ),
        (
            "sandbox",
            AgentSandbox::Required,
            &native,
            Status::Available,
        ),
        (
            "sandbox",
            AgentSandbox::Required,
            &missing,
            Status::MissingPrerequisite,
        ),
        (
            "sandbox",
            AgentSandbox::Off,
            &native,
            Status::NeedsAuthorization,
        ),
    ];
    for (environment, setting, backend, status) in cases {
        let fixture = Fixture::new();
        let host = HostFacts {
            sandbox_setting: setting,
            sandbox_backend: backend.clone(),
            ..ready_host()
        };
        fixture.configure(
            &format!("[execution]\nenvironment = \"{environment}\"\n"),
            Some(&owner_file_as_applied(&host)),
        );

        let report = fixture.report(&host, &fixture.facts());

        let requirement = requirement(&report, "execution environment");
        assert_eq!(
            requirement.status, status,
            "{environment} {setting:?} {backend:?}: {requirement:#?}"
        );
    }
}

/// #965: a project expecting the sandbox is not told its agents run there while every launch
/// is refused over the shared desktop.
#[test]
fn the_execution_environment_blocks_while_the_sandbox_refuses_the_shared_desktop() {
    for environment in ["native", "sandbox"] {
        let fixture = Fixture::new();
        fixture.configure(
            &format!("[execution]\nenvironment = \"{environment}\"\n"),
            Some(OWNER_REQUIRES_THE_SANDBOX),
        );
        let host = HostFacts {
            sandbox_setting: AgentSandbox::Required,
            shared_desktop_refusal: Some(SHARED_DESKTOP_REFUSAL.to_owned()),
            ..ready_host()
        };

        let report = fixture.report(&host, &fixture.facts());

        let requirement = requirement(&report, "execution environment");
        assert_eq!(
            requirement.status,
            Status::MissingPrerequisite,
            "{environment}: {requirement:#?}"
        );
        assert!(
            requirement.detail.contains(SHARED_DESKTOP_REFUSAL),
            "{requirement:#?}"
        );
    }
}

/// #963: with a written `agents.sandbox` the daemon has not read, the line would advise
/// writing what the owner's file already says. It defers to the `sandbox` line instead.
#[test]
fn the_execution_environment_is_unknown_while_the_written_sandbox_setting_is_not_in_force() {
    for environment in ["native", "sandbox"] {
        let fixture = Fixture::new();
        fixture.configure(
            &format!("[execution]\nenvironment = \"{environment}\"\n"),
            Some(OWNER_REQUIRES_THE_SANDBOX),
        );

        let report = fixture.report(&ready_host(), &fixture.facts());

        let requirement = requirement(&report, "execution environment");
        assert_eq!(requirement.status, Status::Unknown, "{requirement:#?}");
        assert!(
            requirement.detail.contains(&format!(
                "the project expects {environment} (project config "
            )) && requirement
                .detail
                .contains("agents.sandbox = \"required\", which the running daemon has not read"),
            "{requirement:#?}"
        );
        let next = requirement.next.as_deref().unwrap();
        assert!(
            next.contains("`sandbox` line") && !next.contains("set agents.sandbox"),
            "{requirement:#?}"
        );
    }
}

#[test]
fn needs_beyond_the_host_budget_are_never_admitted() {
    let fixture = Fixture::new();
    fixture.configure("[needs]\nmemory_mib = 32768\n", None);

    let report = fixture.report(&ready_host(), &fixture.facts());

    let needs = requirement(&report, "needs");
    assert_eq!(needs.status, Status::Unsupported);
    assert!(!report.is_ready());
}

#[test]
fn needs_within_the_host_budget_are_available() {
    let fixture = Fixture::new();
    fixture.configure("[needs]\ncpu_millis = 2000\nmemory_mib = 4096\n", None);

    let report = fixture.report(&ready_host(), &fixture.facts());

    assert_eq!(requirement(&report, "needs").status, Status::Available);
}

#[test]
fn a_check_whose_working_directory_is_missing_names_its_line() {
    let fixture = Fixture::new();
    fixture.configure(
        "[[checks]]\nname = \"test\"\ncommand = \"cargo test\"\n\n[[checks]]\nname = \"lint\"\ncommand = \"make lint\"\nworking_directory = \"tools\"\n",
        None,
    );

    let report = fixture.report(&ready_host(), &fixture.facts());

    assert_eq!(requirement(&report, "check test").status, Status::Available);
    let lint = requirement(&report, "check lint");
    assert_eq!(lint.status, Status::MissingPrerequisite);
    assert!(
        lint.next.as_deref().unwrap().contains("config.toml:5"),
        "{lint:#?}"
    );
}

#[test]
fn docker_is_needed_by_the_project_environment_or_by_one_check() {
    let cases = [
        ("", false),
        ("[execution]\nenvironment = \"docker\"\n", true),
        (
            "[[checks]]\nname = \"t\"\ncommand = \"x\"\nenvironment = \"docker\"\n",
            true,
        ),
        ("[[checks]]\nname = \"t\"\ncommand = \"x\"\n", false),
    ];
    for (contents, expected) in cases {
        let fixture = Fixture::new();
        fixture.configure(contents, None);

        assert_eq!(
            needs_docker(&fixture.config().unwrap()),
            expected,
            "{contents:?}"
        );
    }
}

fn engine(rootless: bool) -> DockerEngine {
    DockerEngine {
        id: "ENGINE-ID".to_owned(),
        name: "host".to_owned(),
        server_version: "28.1.1".to_owned(),
        rootless,
        in_desktop_vm: false,
    }
}

fn docker_desktop() -> DockerEngine {
    DockerEngine {
        in_desktop_vm: true,
        ..engine(false)
    }
}

/// Each case: the probe's answer, the platform, and the status of `docker`.
#[test]
fn a_docker_engine_is_identified_and_its_isolation_reported() {
    let cases = [
        (Ok(engine(true)), Platform::Linux, Status::Available),
        (
            Ok(engine(false)),
            Platform::Linux,
            Status::MissingPrerequisite,
        ),
        (Ok(engine(false)), Platform::MacOs, Status::Available),
        (
            Ok(docker_desktop()),
            Platform::Wsl2 {
                windows_drives: Vec::new(),
            },
            Status::Available,
        ),
        (
            Err("docker info: no engine answered".to_owned()),
            Platform::Linux,
            Status::MissingPrerequisite,
        ),
    ];
    for (probe, platform, status) in cases {
        let fixture = Fixture::new();
        fixture.configure("[execution]\nenvironment = \"docker\"\n", None);
        let mut project = fixture.facts();
        project.docker = Some(probe.clone());
        let host = HostFacts {
            platform: platform.clone(),
            ..ready_host()
        };

        let report = fixture.report(&host, &project);

        let docker = requirement(&report, "docker");
        assert_eq!(docker.status, status, "{probe:?} {platform:?}: {docker:#?}");
        if let Ok(engine) = &probe {
            assert!(docker.detail.contains(&engine.id), "{docker:#?}");
        }
    }
}

#[test]
#[should_panic(expected = "an available requirement has no next action to need")]
fn an_available_requirement_cannot_be_built_as_needing_something() {
    Requirement::needs("x", Status::Available, "fine", "nothing");
}

#[test]
fn sign_in_facts_choose_the_status_and_keep_the_providers_next_step() {
    let fixture = Fixture::new();
    fixture.configure("", None);
    for (signed_in, status) in [
        (Some(true), Status::Available),
        (Some(false), Status::NeedsAuthorization),
        (None, Status::Unknown),
    ] {
        let mut host = ready_host();
        let found = host.providers[0].found.as_mut().unwrap();
        found.sign_in = cockpit_readiness::SignInFacts {
            signed_in,
            detail: "provider evidence (unverified)".to_owned(),
            next: "provider-specific next step".to_owned(),
        };
        let report = assess(&host, &fixture.config(), None);
        let fact = requirement(&report, "provider claude sign-in");
        assert_eq!(fact.status, status);
        assert_eq!(fact.detail, "provider evidence (unverified)");
        assert_eq!(
            fact.next.as_deref(),
            if signed_in == Some(true) {
                None
            } else {
                Some("provider-specific next step")
            }
        );
    }
}
