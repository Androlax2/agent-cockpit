//! The launch spec's host-service forwards (#880): what the launcher accepts to serve, and
//! what it refuses before anything starts.

use std::path::PathBuf;

use cockpit_sandbox::{
    Allowlist, AskedService, EgressDecisions, Endpoint, Forward, LaunchSpec, MAX_FORWARDS,
    NeedsSource, SPEC_VERSION, Subject,
};

fn spec(forwards: Vec<Forward>) -> LaunchSpec {
    LaunchSpec {
        version: SPEC_VERSION,
        subject: Subject::Session {
            session: "s1".to_owned(),
        },
        bubblewrap: PathBuf::from("/usr/bin/bwrap"),
        landlock_abi: 4,
        program: PathBuf::from("/usr/bin/claude"),
        arguments: Vec::new(),
        working_dir: PathBuf::from("/work/checkout"),
        home: PathBuf::from("/home/owner"),
        home_source: PathBuf::from("/state/homes/s1"),
        read_only: vec![PathBuf::from("/state/bin")],
        read_only_declared: Vec::new(),
        sockets: vec![PathBuf::from("/state/sockets")],
        read_write: vec![PathBuf::from("/work/checkout")],
        folders: Vec::new(),
        protected: Vec::new(),
        hidden: Vec::new(),
        needs_source: NeedsSource::Floor,
        forwards,
        login: None,
        egress: Allowlist::default(),
        interception: None,
    }
}

fn forward(service: &str, listen: Endpoint, target: Endpoint) -> Forward {
    Forward {
        service: service.to_owned(),
        listen,
        target,
    }
}

fn port(port: u16) -> Endpoint {
    Endpoint::Port { port }
}

fn socket(path: &str) -> Endpoint {
    Endpoint::Socket {
        path: PathBuf::from(path),
    }
}

fn refusal(spec: &LaunchSpec) -> String {
    spec.validate(&EgressDecisions::default())
        .expect_err("the spec is refused")
        .to_string()
}

#[test]
fn a_port_and_a_socket_forward_to_the_hosts_services_are_accepted() {
    let accepted = spec(vec![
        forward("figcraft", port(7979), port(7979)),
        forward(
            "engine",
            socket("/run/torlek/engine.sock"),
            socket("/run/user/1000/docker.sock"),
        ),
    ]);

    accepted.validate(&EgressDecisions::default()).unwrap();
}

#[test]
fn each_malformed_forward_is_refused_naming_why() {
    let cases = [
        (
            vec![forward("Fig", port(2001), port(2))],
            "service name \"Fig\" is not a lower-case letter, then letters, digits and `-`",
        ),
        (
            vec![
                forward("one", port(2001), port(2)),
                forward("one", port(2003), port(4)),
            ],
            "service one is forwarded twice",
        ),
        (
            vec![
                forward("one", port(2001), port(2)),
                forward("two", port(2001), port(4)),
            ],
            "service two listens on 127.0.0.1:2001, which service one listens on already",
        ),
        (
            vec![forward("one", port(3128), port(2))],
            "service one listens on 3128, the session proxy's port",
        ),
        (
            vec![forward("one", port(0), port(2))],
            "service one listens on port 0; the sandbox binds ports from 1024 up",
        ),
        (
            vec![forward("one", port(80), port(2))],
            "service one listens on port 80; the sandbox binds ports from 1024 up",
        ),
        (
            vec![forward("one", port(2001), port(0))],
            "service one targets port 0",
        ),
        (
            vec![forward("one", socket("/one.sock"), port(2))],
            "service one listens on /one.sock, which is in no folder of its own",
        ),
        (
            vec![forward("one", socket("/work/checkout/one.sock"), port(2))],
            "service one listens in /work/checkout, which overlaps /work/checkout",
        ),
        (
            vec![forward("one", socket("/home/one.sock"), port(2))],
            "service one listens in /home, which overlaps /home/owner",
        ),
        (
            vec![forward("one", socket("/usr/lib/one.sock"), port(2))],
            "service one listens in /usr/lib, which overlaps /usr",
        ),
        (
            vec![
                forward("one", socket("/run/a/one.sock"), port(2)),
                forward("two", socket("/run/a/b/two.sock"), port(2)),
            ],
            "service two listens in /run/a/b, which overlaps /run/a, where service one listens",
        ),
        (
            vec![forward("one", port(2001), socket("run/one.sock"))],
            "run/one.sock is not an absolute path",
        ),
        (
            vec![forward(
                "one",
                port(2001),
                socket("/run/one.sock target unix:/run/two.sock"),
            )],
            "which is not UTF-8 without spaces or control characters",
        ),
        (
            vec![forward("one", socket("/run/a/one\n.sock"), port(2))],
            "which is not UTF-8 without spaces or control characters",
        ),
    ];
    for (forwards, expected) in cases {
        let refused = refusal(&spec(forwards));
        assert!(refused.contains(expected), "{refused:?} names {expected:?}");
    }
}

#[test]
fn a_check_runs_launch_serves_no_forward() {
    let mut check = spec(vec![forward("one", port(2001), port(2))]);
    check.subject = Subject::Check {
        run: "r1".to_owned(),
        check: "test".to_owned(),
    };

    assert_eq!(refusal(&check), "only a session's launch serves forwards");
}

#[test]
fn more_forwards_than_a_launch_serves_are_refused() {
    let forwards = (0..=MAX_FORWARDS)
        .map(|index| {
            let number = u16::try_from(index).unwrap() + 2001;
            forward(&format!("service-{index}"), port(number), port(number))
        })
        .collect();

    assert!(refusal(&spec(forwards)).contains("a launch serves at most"));
}

/// The grant is bound to the whole record: each field that decides what is reached changes
/// it.
#[test]
fn a_changed_target_or_listen_is_another_record() {
    let granted = forward("figcraft", port(7979), port(7979));
    let records = [
        forward("figcraft", port(7979), port(8080)),
        forward("figcraft", port(8080), port(7979)),
        forward("figcraft", port(7979), socket("/run/figcraft.sock")),
        forward("other", port(7979), port(7979)),
    ]
    .map(|edited| edited.record());

    assert!(!records.contains(&granted.record()));
    assert_eq!(granted.record(), granted.clone().record());
}

#[test]
fn the_daemon_files_a_record_its_launcher_wrote() {
    let asked = AskedService::of(&forward("figcraft", port(7979), socket("/run/f.sock")));

    assert!(asked.is_askable());
}

/// A requests file the launcher did not write as it does is refused at the daemon rather
/// than shown to the owner.
#[test]
fn a_record_of_another_service_or_on_two_lines_is_not_filed() {
    let written = forward("figcraft", port(7979), port(7979)).record();
    let malformed = [
        ("Figcraft", written.clone()),
        ("other", written.clone()),
        ("figcraft", format!("{written}\nservice other")),
        (
            "figcraft",
            "service figcraft listen 127.0.0.1:7979".to_owned(),
        ),
        ("figcraft", format!("{written}{}", " ".repeat(8192))),
    ];

    for (service, record) in malformed {
        let asked = AskedService {
            service: service.to_owned(),
            record: record.clone(),
        };
        assert!(!asked.is_askable(), "{service} {record:?} was filed");
    }
}
