//! Which container ports a session may open (#1371): none of another session's block.

use cockpit_config::PortBlock;
use cockpit_sandbox::{AllowSource, AllowedService, Forward, ServiceDecisions};
use cockpit_sessions::{HeldPortBlock, SessionId, keep_own_container_ports, other_sessions_block};

const FIRST: &str = "s000000000001";
const SECOND: &str = "s000000000002";

fn held(session: &str, project: &str, block: u16) -> HeldPortBlock {
    HeldPortBlock {
        session: SessionId::parse(session).unwrap(),
        project: project.to_owned(),
        block: PortBlock::new(block).unwrap(),
    }
}

#[test]
fn a_port_in_another_sessions_block_names_that_session() {
    let blocks = [held(FIRST, "app", 0), held(SECOND, "app", 1)];

    let holder = other_sessions_block(&blocks, "app", SECOND, 20_005);

    assert_eq!(holder, Some(&blocks[0]));
}

#[test]
fn a_port_of_the_sessions_own_block_another_project_or_no_block_is_not_anothers() {
    let blocks = [held(FIRST, "app", 0), held(SECOND, "web", 1)];

    for (case, session, port) in [
        ("own block", FIRST, 20_005),
        ("another project's session", FIRST, 20_015),
        ("no block holds it", SECOND, 8_080),
        ("a block no session holds", SECOND, 20_025),
    ] {
        assert_eq!(
            other_sessions_block(&blocks, "app", session, port),
            None,
            "{case}"
        );
    }
}

fn allowed_always(forward: Forward, live: bool) -> AllowedService {
    AllowedService {
        service: forward.service.clone(),
        record: forward.record(),
        source: AllowSource::Grant {
            grant: "1".to_owned(),
        },
        live: live.then_some(forward),
    }
}

#[test]
fn a_port_allowed_always_opens_only_in_the_session_whose_block_holds_it() {
    let blocks = [held(FIRST, "app", 0), held(SECOND, "app", 1)];
    let ports = [20_005, 20_015, 8_080];
    let decisions = ServiceDecisions {
        allowed: ports
            .map(|port| allowed_always(Forward::container_port("torlek-e3", port), true))
            .to_vec(),
        ..ServiceDecisions::default()
    };

    let mut seconds = decisions.clone();
    keep_own_container_ports(&mut seconds, &blocks, "app", SECOND);

    let opened: Vec<u16> = seconds
        .live_forwards()
        .filter_map(Forward::listen_port)
        .collect();
    assert_eq!(opened, [20_015, 8_080]);
}

#[test]
fn a_declared_service_is_kept_whatever_port_it_listens_on() {
    let blocks = [held(FIRST, "app", 0)];
    let declared = Forward {
        service: "db".to_owned(),
        listen: cockpit_sandbox::Endpoint::Port { port: 20_005 },
        target: cockpit_sandbox::Endpoint::Port { port: 5_432 },
    };
    let mut decisions = ServiceDecisions {
        allowed: vec![allowed_always(declared, false)],
        ..ServiceDecisions::default()
    };

    keep_own_container_ports(&mut decisions, &blocks, "app", SECOND);

    assert_eq!(decisions.allowed.len(), 1);
}
