//! The compose project name and port block a sandboxed session launches with (#1371).

use cockpit_config::{
    PORT_BLOCK_SIZE, PORT_BLOCKS, PortBlock, compose_project_name, is_container_port_variable,
};

#[test]
fn the_first_block_starts_at_20000_and_each_holds_ten_ports() {
    let first = PortBlock::new(0).unwrap();
    let second = PortBlock::new(1).unwrap();

    assert_eq!(
        (first.first_port(), first.to_string()),
        (20_000, "20000-20009".to_owned())
    );
    assert_eq!(second.first_port(), 20_000 + PORT_BLOCK_SIZE);
}

#[test]
fn the_last_block_ends_below_the_kernels_ephemeral_ports() {
    let last = PortBlock::new(PORT_BLOCKS - 1).unwrap();

    assert_eq!(*last.ports().end(), 32_759);
    assert_eq!(PortBlock::new(PORT_BLOCKS), None);
}

#[test]
fn a_port_names_the_block_holding_it() {
    for (port, expected) in [
        (19_999, None),
        (20_000, Some(0)),
        (20_009, Some(0)),
        (20_010, Some(1)),
        (32_759, Some(PORT_BLOCKS - 1)),
        (32_760, None),
        (8_080, None),
    ] {
        assert_eq!(
            PortBlock::containing(port).map(PortBlock::index),
            expected,
            "port {port}"
        );
    }
}

#[test]
fn a_block_sets_its_base_its_count_and_each_port() {
    let variables = PortBlock::new(2).unwrap().variables();

    assert_eq!(
        &variables[..4],
        &[
            ("TORLEK_PORT_BASE".to_owned(), "20020".to_owned()),
            ("TORLEK_PORT_COUNT".to_owned(), "10".to_owned()),
            ("TORLEK_PORT_0".to_owned(), "20020".to_owned()),
            ("TORLEK_PORT_1".to_owned(), "20021".to_owned()),
        ]
    );
    assert_eq!(
        variables.last(),
        Some(&("TORLEK_PORT_9".to_owned(), "20029".to_owned()))
    );
    assert_eq!(variables.len(), 2 + usize::from(PORT_BLOCK_SIZE));
}

#[test]
fn every_variable_a_block_sets_is_one_a_launch_replaces() {
    let variables = PortBlock::new(0).unwrap().variables();

    assert!(
        variables
            .iter()
            .all(|(name, _)| is_container_port_variable(name))
    );
    assert!(is_container_port_variable("COMPOSE_PROJECT_NAME"));
    assert!(!is_container_port_variable("TORLEK_SESSION"));
}

#[test]
fn the_compose_project_name_is_the_project_then_the_session_as_compose_accepts_it() {
    for (project, expected) in [
        ("app", "app-s0a1b2"),
        ("My.App", "my-app-s0a1b2"),
        ("_internal", "internal-s0a1b2"),
        ("___", "s0a1b2"),
    ] {
        assert_eq!(
            compose_project_name(project, "s0a1b2"),
            expected,
            "{project}"
        );
    }
}
