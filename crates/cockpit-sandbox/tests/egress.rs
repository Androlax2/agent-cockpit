//! The egress decision core (#831): an allowlist snapshot decides each destination, and
//! every address a name resolves to must be public unless granted as such.

use std::net::IpAddr;

use cockpit_sandbox::{
    AllowEntry, AllowSource, Allowlist, EgressRefusal, EgressReport, MAX_REPORTED_DESTINATIONS,
    Subject, Target, non_public_class,
};

fn adapter() -> AllowSource {
    AllowSource::Adapter {
        provider: "claude".to_owned(),
    }
}

fn allowlist(entries: Vec<AllowEntry>) -> Allowlist {
    Allowlist {
        program: "claude".to_owned(),
        entries,
    }
}

fn name(name: &str, port: u16) -> AllowEntry {
    AllowEntry {
        target: Target::Name(name.to_owned()),
        port,
        source: adapter(),
    }
}

fn address(address: &str, port: u16) -> AllowEntry {
    AllowEntry {
        target: Target::Address(address.parse().unwrap()),
        port,
        source: AllowSource::Grant {
            grant: "g1".to_owned(),
        },
    }
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn a_named_host_is_allowed_on_its_port_only() {
    let list = allowlist(vec![name("api.anthropic.com", 443)]);

    assert!(list.decide("api.anthropic.com", 443).is_ok());
    assert!(matches!(
        list.decide("api.anthropic.com", 80),
        Err(EgressRefusal::NotAllowed { .. })
    ));
}

#[test]
fn a_host_matches_whatever_its_case_and_trailing_dot() {
    let list = allowlist(vec![name("api.anthropic.com", 443)]);

    assert!(list.decide("API.Anthropic.COM.", 443).is_ok());
}

#[test]
fn a_name_on_443_expects_its_tls_server_name_and_another_port_does_not() {
    let list = allowlist(vec![name("a.example.com", 443), name("a.example.com", 22)]);

    assert!(
        list.decide("a.example.com", 443)
            .unwrap()
            .expects_server_name
    );
    assert!(
        !list
            .decide("a.example.com", 22)
            .unwrap()
            .expects_server_name
    );
}

#[test]
fn a_wildcard_covers_names_under_its_domain_but_not_the_domain() {
    let list = allowlist(vec![name("*.example.com", 443)]);

    assert!(list.decide("a.b.example.com", 443).is_ok());
    assert!(list.decide("example.com", 443).is_err());
    assert!(list.decide("badexample.com", 443).is_err());
}

#[test]
fn an_empty_allowlist_says_the_program_declares_no_hosts() {
    let refused = allowlist(Vec::new())
        .decide("api.anthropic.com", 443)
        .unwrap_err();

    assert_eq!(
        refused.to_string(),
        "api.anthropic.com:443 is not allowed: `claude` declares no hosts"
    );
}

#[test]
fn an_ip_literal_is_allowed_only_when_granted_as_that_address_and_port() {
    let list = allowlist(vec![address("127.0.0.1", 8080)]);

    assert!(list.decide("127.0.0.1", 8080).is_ok());
    for (host, port) in [("127.0.0.1", 8081), ("127.0.0.2", 8080), ("::1", 8080)] {
        assert!(
            matches!(
                list.decide(host, port),
                Err(EgressRefusal::AddressLiteral { .. })
            ),
            "{host}:{port}"
        );
    }
}

#[test]
fn a_mapped_ipv6_literal_is_the_ipv4_address_it_maps() {
    let list = allowlist(vec![address("127.0.0.1", 8080)]);

    assert!(list.decide("::ffff:127.0.0.1", 8080).is_ok());
}

#[test]
fn a_host_that_is_no_name_is_refused_as_malformed() {
    let list = allowlist(vec![name("api.anthropic.com", 443)]);

    for host in ["", "a..b", "api anthropic.com", "a/b.com"] {
        assert!(
            matches!(list.decide(host, 443), Err(EgressRefusal::Malformed { .. })),
            "{host:?}"
        );
    }
}

#[test]
fn every_resolved_address_must_be_public() {
    let list = allowlist(vec![name("api.anthropic.com", 443)]);

    assert!(
        list.check_addresses("api.anthropic.com", 443, &[ip("160.79.104.10")])
            .is_ok()
    );
    let refused = list
        .check_addresses(
            "api.anthropic.com",
            443,
            &[ip("160.79.104.10"), ip("169.254.169.254")],
        )
        .unwrap_err();
    assert_eq!(
        refused.to_string(),
        "api.anthropic.com:443 resolves to 169.254.169.254, a link-local (cloud metadata) \
         address, which no entry grants"
    );
}

#[test]
fn a_non_public_address_granted_as_such_passes_the_address_check() {
    let list = allowlist(vec![
        name("db.example.com", 5432),
        address("10.0.0.5", 5432),
    ]);

    assert!(
        list.check_addresses("db.example.com", 5432, &[ip("10.0.0.5")])
            .is_ok()
    );
    assert!(
        list.check_addresses("db.example.com", 5432, &[ip("10.0.0.6")])
            .is_err()
    );
}

#[test]
fn addresses_outside_the_public_internet_are_classed() {
    for refused in [
        "0.0.0.0",
        "10.0.0.1",
        "100.64.0.1",
        "100.100.100.200",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.0.0.1",
        "192.0.2.1",
        "192.168.0.1",
        "198.18.0.1",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::127.0.0.1",
        "::ffff:10.0.0.1",
        "64:ff9b::a00:1",
        "64:ff9b:1::1",
        "100::1",
        "2001::1",
        "2001:db8::1",
        "2002:a00:1::1",
        "fc00::1",
        "fd00:ec2::254",
        "fe80::1",
        "fec0::1",
        "ff02::1",
    ] {
        assert!(non_public_class(ip(refused)).is_some(), "{refused}");
    }
    for public in [
        "1.1.1.1",
        "160.79.104.10",
        "172.32.0.1",
        "2606:4700::1111",
        "64:ff9b::101:101",
        "2002:101:101::1",
    ] {
        assert_eq!(non_public_class(ip(public)), None, "{public}");
    }
}

#[test]
fn a_report_counts_destinations_past_its_bound_together() {
    let mut report = EgressReport::new(Subject::Session {
        session: "s1".to_owned(),
    });

    for index in 0..=MAX_REPORTED_DESTINATIONS {
        report
            .counters(&format!("h{index}.example.com:443"))
            .refused += 1;
    }

    assert_eq!(report.destinations.len(), MAX_REPORTED_DESTINATIONS);
    assert_eq!(report.other.refused, 1);
}
