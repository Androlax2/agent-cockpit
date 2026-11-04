//! The owner's memory bounds (#406): defaults, provenance and what each refuses.
use cockpit_config::{ConfigError, Environment};

fn environment(home: &std::path::Path) -> Environment {
    Environment {
        home: Some(home.into()),
        ..Environment::default()
    }
}

fn write_owner_config(home: &std::path::Path, body: &str) {
    std::fs::create_dir_all(home.join(".config/torlek")).unwrap();
    std::fs::write(home.join(".config/torlek/config.toml"), body).unwrap();
}

#[test]
fn every_memory_bound_has_a_default_shown_with_its_source() {
    let home = tempfile::tempdir().unwrap();

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    let shown = cockpit_config::render(&config);
    for line in [
        "memory.tmux_history_lines = 10000  # product default",
        "memory.feed_buffer_kib = 1024  # product default",
        "memory.agentless_session_minutes = 10  # product default",
    ] {
        assert!(shown.contains(line), "missing {line}: {shown}");
    }
}

#[test]
fn the_owner_sets_each_memory_bound_and_show_names_the_line() {
    let home = tempfile::tempdir().unwrap();
    write_owner_config(
        home.path(),
        "[memory]\ntmux_history_lines = 2000\nfeed_buffer_kib = 256\nagentless_session_minutes = 3\n",
    );

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    assert_eq!(config.memory.tmux_history_lines.value, 2000);
    assert_eq!(config.memory.feed_buffer_kib.value, 256);
    assert_eq!(config.memory.agentless_session_minutes.value, 3);
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("memory.feed_buffer_kib = 256  # user config"),
        "{shown}"
    );
    assert!(shown.contains("config.toml:3"), "{shown}");
}

#[test]
fn a_project_cannot_set_a_memory_bound() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".torlek")).unwrap();
    std::fs::write(
        project.path().join(".torlek/config.toml"),
        "[memory]\ntmux_history_lines = 1000000\n",
    )
    .unwrap();

    let loaded = cockpit_config::load(&environment(home.path()), Some(project.path()), &[]);

    assert!(
        matches!(loaded, Err(ConfigError::OwnerOnly { key: "memory", .. })),
        "{loaded:?}"
    );
}

#[test]
fn a_memory_bound_outside_its_range_is_refused_with_its_key() {
    let home = tempfile::tempdir().unwrap();
    for (setting, key) in [
        ("tmux_history_lines = 199", "memory.tmux_history_lines"),
        ("tmux_history_lines = 1000001", "memory.tmux_history_lines"),
        ("feed_buffer_kib = 63", "memory.feed_buffer_kib"),
        ("feed_buffer_kib = 1048577", "memory.feed_buffer_kib"),
        (
            "agentless_session_minutes = 0",
            "memory.agentless_session_minutes",
        ),
    ] {
        write_owner_config(home.path(), &format!("[memory]\n{setting}\n"));

        let loaded = cockpit_config::load(&environment(home.path()), None, &[]);

        assert!(
            matches!(&loaded, Err(ConfigError::Invalid { key: refused, .. }) if refused == key),
            "{setting}: {loaded:?}"
        );
    }
}

#[test]
fn a_memory_bound_at_the_edge_of_its_range_is_accepted() {
    let home = tempfile::tempdir().unwrap();
    for setting in [
        "tmux_history_lines = 200",
        "tmux_history_lines = 1000000",
        "feed_buffer_kib = 64",
        "feed_buffer_kib = 1048576",
        "agentless_session_minutes = 1",
    ] {
        write_owner_config(home.path(), &format!("[memory]\n{setting}\n"));

        let loaded = cockpit_config::load(&environment(home.path()), None, &[]);

        assert!(loaded.is_ok(), "{setting}: {loaded:?}");
    }
}
