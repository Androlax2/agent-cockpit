//! The project's `[[services]]` (#880): host services a sandboxed session reaches through a
//! forward once the owner allows each, read through `cockpit_config::load` from an isolated
//! home and project.

use std::fs;
use std::path::{Path, PathBuf};

use cockpit_config::{ConfigError, EffectiveConfig, Environment, Service, ServiceEndpoint};
use tempfile::TempDir;

fn load(home: &Path, project_dir: Option<&Path>) -> Result<EffectiveConfig, ConfigError> {
    let environment = Environment {
        home: Some(home.into()),
        ..Environment::default()
    };
    cockpit_config::load(&environment, project_dir, &[])
}

/// A project directory whose `.torlek/config.toml` holds `contents`.
fn project(home: &TempDir, contents: &str) -> PathBuf {
    let project = home.path().join("project");
    fs::create_dir_all(project.join(".torlek")).unwrap();
    fs::write(project.join(".torlek/config.toml"), contents).unwrap();
    project
}

fn refusal(contents: &str) -> String {
    let home = TempDir::new().unwrap();
    let project = project(&home, contents);
    load(home.path(), Some(&project))
        .expect_err("the declaration is refused")
        .to_string()
}

#[test]
fn a_project_declares_a_port_and_a_socket_service_in_its_order() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[services]]\nname = \"figcraft\"\nlisten = \"127.0.0.1:7979\"\n\
         target = \"127.0.0.1:7979\"\n\n\
         [[services]]\nname = \"engine\"\nlisten = \"unix:/run/torlek/engine.sock\"\n\
         target = \"unix:/run/user/1000/docker.sock\"\n",
    );

    let config = load(home.path(), Some(&project)).unwrap();

    let services: Vec<&Service> = config
        .services
        .iter()
        .map(|service| &service.value)
        .collect();
    assert_eq!(
        services,
        [
            &Service {
                name: "figcraft".to_owned(),
                listen: ServiceEndpoint::Port(7979),
                target: ServiceEndpoint::Port(7979),
            },
            &Service {
                name: "engine".to_owned(),
                listen: ServiceEndpoint::Socket(PathBuf::from("/run/torlek/engine.sock")),
                target: ServiceEndpoint::Socket(PathBuf::from("/run/user/1000/docker.sock")),
            },
        ]
    );
}

#[test]
fn config_show_lists_each_service_with_its_line() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[services]]\nname = \"figcraft\"\nlisten = \"127.0.0.1:7979\"\n\
         target = \"unix:/run/figcraft.sock\"\n",
    );

    let shown = cockpit_config::render(&load(home.path(), Some(&project)).unwrap());

    assert!(
        shown.contains(
            "services.\"figcraft\" = { listen = \"127.0.0.1:7979\", target = \"unix:/run/figcraft.sock\" }"
        ),
        "{shown}"
    );
    assert!(shown.contains(".torlek/config.toml:1"), "{shown}");
}

/// One `[[services]]` entry.
fn entry(name: &str, listen: &str, target: &str) -> String {
    format!("[[services]]\nname = \"{name}\"\nlisten = \"{listen}\"\ntarget = \"{target}\"\n")
}

#[test]
fn each_malformed_service_is_refused_naming_why() {
    let port = "127.0.0.1:2001";
    let cases = [
        (entry("", port, port), "has the name \"\""),
        (entry("FigCraft", port, port), "has the name \"FigCraft\""),
        (entry("a b", port, port), "has the name \"a b\""),
        (
            entry("a", port, port) + &entry("a", "127.0.0.1:2002", port),
            "repeats the name \"a\"",
        ),
        (
            entry("a", port, "0.0.0.0:2"),
            "has the target \"0.0.0.0:2\"",
        ),
        (
            entry("a", port, "127.0.0.1:2-9"),
            "has the target \"127.0.0.1:2-9\"",
        ),
        (
            entry("a", "127.0.0.1:+2001", port),
            "has the listen \"127.0.0.1:+2001\"",
        ),
        (
            entry("a", port, "unix:run/a.sock"),
            "has the target \"unix:run/a.sock\"",
        ),
        (
            entry("a", "unix:/run/a b.sock", port),
            "has the listen \"unix:/run/a b.sock\"",
        ),
        (entry("a", "127.0.0.1:80", port), "listens on port 80"),
        (entry("a", "127.0.0.1:3128", port), "listens on port 3128"),
        (
            entry("a", port, port) + &entry("b", port, "127.0.0.1:2002"),
            "\"b\" listens at 127.0.0.1:2001, where \"a\" listens",
        ),
        (
            (0..=32)
                .map(|index| {
                    entry(
                        &format!("s{index}"),
                        &format!("127.0.0.1:{}", 3000 + index),
                        port,
                    )
                })
                .collect(),
            "one more than the 32 services a project declares",
        ),
        (
            "[[services]]\nname = \"a\"\nlisten = \"127.0.0.1:2001\"\n".to_owned(),
            "missing field `target`",
        ),
    ];
    for (contents, expected) in cases {
        let refused = refusal(&contents);
        assert!(refused.contains(expected), "{refused:?} names {expected:?}");
    }
}

#[test]
fn the_owners_file_declares_no_service() {
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config/torlek");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[[services]]\nname = \"a\"\nlisten = \"127.0.0.1:1\"\ntarget = \"127.0.0.1:2\"\n",
    )
    .unwrap();

    let refused = load(home.path(), None).expect_err("refused").to_string();

    assert!(
        refused.contains("`services` describes a project"),
        "{refused}"
    );
}
