//! The readiness report through `cockpit_readiness::assess` (#50, report 09): one status per
//! requirement, and a next action for every requirement that is not available.

use std::fs;
use std::path::PathBuf;

use cockpit_config::{AgentSandbox, EffectiveConfig, Environment};
use cockpit_readiness::{
    CheckoutFacts, ControlFacts, DockerEngine, HostFacts, Platform, PresetFacts, ProjectFacts,
    ProjectNeeds, ProviderFacts, ProviderFound, Report, Requirement, SandboxEngine, Status, Trust,
    assess, needs_docker,
};
use tempfile::TempDir;

fn ready_host() -> HostFacts {
    HostFacts {
        profile: cockpit_config::Profile::parse("next").unwrap(),
        platform: Platform::Linux,
        tmux: Ok("3.5".to_owned()),
        sandbox_engine: Ok(SandboxEngine {
            engine: "greywall 0.3.7 (7c06d51), greyproxy 0.4.5 (cdd5cde)".to_owned(),
            observations: vec!["landlock: yes".to_owned()],
        }),
        sandbox_setting: AgentSandbox::Required,
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

    /// The project's `.cockpit/config.toml`, and the owner's file when `owner` is set.
    fn configure(&self, project: &str, owner: Option<&str>) {
        fs::create_dir_all(self.project_dir().join(".cockpit")).unwrap();
        fs::write(self.project_dir().join(".cockpit/config.toml"), project).unwrap();
        if let Some(owner) = owner {
            let dir = self.home.path().join(".config/cockpit");
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
        None,
    );
    let host = HostFacts {
        platform: Platform::Unsupported("freebsd".to_owned()),
        tmux: Err("no tmux".to_owned()),
        sandbox_engine: Err("no greywall".to_owned()),
        providers: vec![ProviderFacts {
            name: "claude".to_owned(),
            found: Err("no `claude` on PATH".to_owned()),
        }],
        controls: Vec::new(),
        ..ready_host()
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

#[test]
fn a_missing_sandbox_names_the_managed_installer() {
    let fixture = Fixture::new();
    fixture.configure("", None);
    let host = HostFacts {
        sandbox_engine: Err("no managed greywall installation".to_owned()),
        ..ready_host()
    };
    let report = fixture.report(&host, &fixture.facts());
    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::MissingPrerequisite);
    assert!(
        sandbox
            .next
            .as_deref()
            .unwrap()
            .contains("cockpit --profile next sandbox install"),
        "{sandbox:?}"
    );
}

/// Each case: host facts changed from a ready host, the requirement and the status it gets.
#[test]
fn a_host_fact_decides_its_requirement_status() {
    let cases: Vec<(HostFacts, &str, Status)> = vec![
        (
            HostFacts {
                tmux: Err("`tmux` is tmux 3.0; Cockpit needs tmux 3.2 or newer".to_owned()),
                ..ready_host()
            },
            "tmux",
            Status::MissingPrerequisite,
        ),
        (
            HostFacts {
                sandbox_engine: Err("greywall at /bin/greywall is commit abc".to_owned()),
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
        let report = assess(&host, &Ok(owner_only_config()), None);

        assert_eq!(requirement(&report, name).status, status, "{report:#?}");
    }
}

fn missing(name: &str) -> ProviderFacts {
    ProviderFacts {
        name: name.to_owned(),
        found: Err(format!("no `{name}` on PATH")),
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
    assert!(
        config.detail.contains(".cockpit/config.toml"),
        "{config:#?}"
    );
    assert!(
        !report
            .requirements
            .iter()
            .any(|requirement| requirement.name == "project"),
        "{report:#?}"
    );
}

/// Turning the sandbox off is the owner's to do, and the report says so loudly.
#[test]
fn an_unsandboxed_owner_is_reported_as_such() {
    let host = HostFacts {
        sandbox_setting: AgentSandbox::Off,
        ..ready_host()
    };

    let report = assess(&host, &Ok(owner_only_config()), None);

    let sandbox = requirement(&report, "sandbox");
    assert_eq!(sandbox.status, Status::Available);
    assert!(sandbox.detail.contains("UNSANDBOXED"), "{sandbox:#?}");
    assert!(sandbox.next.is_some(), "{sandbox:#?}");
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
        Some(format!("cockpit project add {}", fixture.project_dir().display()).as_str())
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

/// Each case: the project's environment, the owner's sandbox setting, whether the engine is
/// there, and the status of the project's execution environment.
#[test]
fn the_execution_environment_the_project_expects_meets_what_the_owner_granted() {
    let cases = [
        ("native", AgentSandbox::Off, true, Status::Available),
        (
            "native",
            AgentSandbox::Required,
            true,
            Status::NeedsAuthorization,
        ),
        ("sandbox", AgentSandbox::Required, true, Status::Available),
        (
            "sandbox",
            AgentSandbox::Required,
            false,
            Status::MissingPrerequisite,
        ),
        (
            "sandbox",
            AgentSandbox::Off,
            true,
            Status::NeedsAuthorization,
        ),
    ];
    for (environment, setting, has_engine, status) in cases {
        let fixture = Fixture::new();
        fixture.configure(
            &format!("[execution]\nenvironment = \"{environment}\"\n"),
            None,
        );
        let mut host = HostFacts {
            sandbox_setting: setting,
            ..ready_host()
        };
        if !has_engine {
            host.sandbox_engine = Err("no greywall".to_owned());
        }

        let report = fixture.report(&host, &fixture.facts());

        let requirement = requirement(&report, "execution environment");
        assert_eq!(
            requirement.status, status,
            "{environment} {setting:?} {has_engine}: {requirement:#?}"
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
