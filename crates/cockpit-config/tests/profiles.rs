//! Profiles (#196, #134): what selects one, and what each places under `cockpit-<name>`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cockpit_config::{
    Build, ConfigError, Environment, Layout, Profile, ProfileChoice, Source, resolve_profile,
};

fn resolved(choice: ProfileChoice<'_>, build: Build) -> (String, Source) {
    let profile = resolve_profile(choice, build).expect("a valid choice");
    (profile.value.name().to_owned(), profile.source)
}

#[test]
fn without_a_choice_a_release_runs_the_default_profile_and_any_other_build_runs_dev() {
    for (build, expected) in [
        (Build::Release, ("torlek", Source::ProductDefault)),
        (
            Build::Development,
            ("dev", Source::DevelopmentBuild { ignored: None }),
        ),
    ] {
        let (name, source) = resolved(ProfileChoice::default(), build);
        assert_eq!((name.as_str(), source), expected, "{build:?}");
    }
}

#[test]
fn the_flag_beats_the_command_name_which_beats_the_inherited_variable() {
    let cases: [(ProfileChoice<'_>, &str, Source); 3] = [
        (
            ProfileChoice {
                flag: Some("review"),
                variable: Some(OsStr::new("next")),
                command_name: Some(OsStr::new("/opt/bin/torlek-other")),
            },
            "review",
            Source::Flag("--profile"),
        ),
        (
            ProfileChoice {
                flag: None,
                variable: Some(OsStr::new("torlek")),
                command_name: Some(OsStr::new("/opt/bin/torlek-next")),
            },
            "next",
            Source::CommandName,
        ),
        (
            ProfileChoice {
                flag: None,
                variable: Some(OsStr::new("next")),
                command_name: Some(OsStr::new("/opt/bin/torlek")),
            },
            "next",
            Source::Environment("TORLEK_PROFILE"),
        ),
    ];
    for build in [Build::Release, Build::Development] {
        for (choice, name, source) in &cases {
            assert_eq!(
                resolved(*choice, build),
                ((*name).to_owned(), source.clone()),
                "{choice:?} {build:?}"
            );
        }
    }
}

#[test]
fn a_development_build_reaches_the_default_profile_only_through_the_flag() {
    let inherited = ProfileChoice {
        variable: Some(OsStr::new("torlek")),
        ..ProfileChoice::default()
    };
    assert_eq!(
        resolved(inherited, Build::Development),
        (
            "dev".to_owned(),
            Source::DevelopmentBuild {
                ignored: Some("TORLEK_PROFILE")
            }
        )
    );
    let explicit = ProfileChoice {
        flag: Some("torlek"),
        ..inherited
    };
    assert_eq!(
        resolved(explicit, Build::Development),
        ("torlek".to_owned(), Source::Flag("--profile"))
    );
    assert_eq!(
        resolved(inherited, Build::Release),
        ("torlek".to_owned(), Source::Environment("TORLEK_PROFILE"))
    );
}

#[test]
fn a_command_name_that_names_no_profile_selects_none() {
    for command_name in [
        "torlek",
        "/usr/bin/torlek",
        "torlek-tui",
        "torlek-",
        "cockpit-next",
        "other",
    ] {
        let choice = ProfileChoice {
            command_name: Some(OsStr::new(command_name)),
            ..ProfileChoice::default()
        };
        assert_eq!(
            resolved(choice, Build::Release),
            ("torlek".to_owned(), Source::ProductDefault),
            "{command_name}"
        );
    }
}

#[test]
fn a_profile_name_that_is_not_a_plain_name_or_names_a_program_is_refused() {
    for name in [
        "",
        "Dev",
        "1st",
        "-dev",
        "dev-",
        "a--b",
        "a_b",
        "a/b",
        "daemon",
        "tui",
        "notify",
        "next-notify",
        "claude-hook",
        "abcdefghijklmnopqrstuvwxy",
    ] {
        let flag = ProfileChoice {
            flag: Some(name),
            ..ProfileChoice::default()
        };
        let refused = resolve_profile(flag, Build::Release);
        assert!(
            matches!(&refused, Err(ConfigError::Invalid { key, .. }) if key == "profile"),
            "{name:?}: {refused:?}"
        );
    }
}

#[test]
fn an_empty_variable_counts_as_unset() {
    let choice = ProfileChoice {
        variable: Some(OsStr::new("")),
        ..ProfileChoice::default()
    };
    assert_eq!(
        resolved(choice, Build::Release),
        ("torlek".to_owned(), Source::ProductDefault)
    );
}

#[test]
fn each_profile_names_its_directories_tmux_server_and_units() {
    for (name, qualified, unit, label) in [
        (
            "torlek",
            "torlek",
            "torlek.service",
            "io.github.androlax2.torlek",
        ),
        (
            "dev",
            "torlek-dev",
            "torlek-dev.service",
            "io.github.androlax2.torlek-dev",
        ),
        (
            "next",
            "torlek-next",
            "torlek-next.service",
            "io.github.androlax2.torlek-next",
        ),
    ] {
        let profile = Profile::parse(name).unwrap();
        assert_eq!(
            (
                profile.qualified_name(),
                profile.systemd_unit(),
                profile.launchd_label()
            ),
            (qualified.to_owned(), unit.to_owned(), label.to_owned())
        );
    }
}

fn environment_for(profile: &str, home: &Path, runtime: Option<&Path>) -> Environment {
    Environment {
        home: Some(home.into()),
        xdg_runtime_dir: runtime.map(Into::into),
        profile: cockpit_config::Sourced::new(
            Profile::parse(profile).unwrap(),
            Source::Flag("--profile"),
        ),
        ..Environment::default()
    }
}

#[test]
fn every_directory_and_the_socket_directory_are_the_profiles_own() {
    let home = Path::new("/home/owner");
    let layout = Layout::resolve(
        &environment_for("dev", home, Some(Path::new("/run/user/1000"))),
        home,
    );
    assert_eq!(
        [
            layout.config_dir.value.clone(),
            layout.state_dir.value.clone(),
            layout.cache_dir.value.clone(),
            layout.socket_dir().unwrap(),
        ],
        [
            PathBuf::from("/home/owner/.config/torlek-dev"),
            PathBuf::from("/home/owner/.local/state/torlek-dev"),
            PathBuf::from("/home/owner/.cache/torlek-dev"),
            PathBuf::from("/run/user/1000/torlek-dev"),
        ]
    );
    let without_runtime = Layout::resolve(&environment_for("dev", home, None), home);
    assert_eq!(
        without_runtime.socket_dir().unwrap(),
        Path::new("/home/owner/.local/state/torlek-dev/run")
    );
}

#[test]
fn a_launched_process_gets_the_profile_beside_the_xdg_variables() {
    let home = Path::new("/home/owner");
    let variables =
        environment_for("next", home, Some(Path::new("/run/user/1000"))).layout_variables();
    assert_eq!(
        variables,
        [
            ("XDG_RUNTIME_DIR", "/run/user/1000".into()),
            ("TORLEK_PROFILE", "next".into()),
        ]
    );
}

#[test]
fn config_show_names_the_profile_and_its_source() {
    let home = tempfile::tempdir().unwrap();
    let environment = environment_for("dev", home.path(), None);
    let shown = cockpit_config::render(&cockpit_config::load(&environment, None, &[]).unwrap());
    assert!(
        shown.starts_with("profile = \"dev\"  # command line --profile\n"),
        "{shown}"
    );
}
