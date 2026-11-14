//! The hosts an approved run's card names (#1387): each becomes one entry of the run's egress
//! proxy, which refuses every other destination.

use cockpit_sandbox::{AllowEntry, AllowSource, EgressRefusal, Target, card_allowlist, card_entry};

fn entry(target: Target, port: u16) -> AllowEntry {
    AllowEntry {
        target,
        port,
        source: AllowSource::Card { run: 7 },
    }
}

#[test]
fn a_card_host_is_a_name_or_an_address_on_443_unless_it_names_a_port() {
    let cases = [
        (
            "Example.COM.",
            entry(Target::Name("example.com".to_owned()), 443),
        ),
        (
            "example.com:8443",
            entry(Target::Name("example.com".to_owned()), 8443),
        ),
        (
            "127.0.0.2:8080",
            entry(Target::Address("127.0.0.2".parse().unwrap()), 8080),
        ),
        (
            "[::1]:8080",
            entry(Target::Address("::1".parse().unwrap()), 8080),
        ),
        ("::1", entry(Target::Address("::1".parse().unwrap()), 443)),
    ];
    for (host, expected) in cases {
        assert_eq!(card_entry(host, 7), Ok(expected), "{host}");
    }
}

#[test]
fn a_card_host_that_names_no_single_destination_is_refused() {
    for host in [
        "",
        "*.example.com",
        "example.com:0",
        "example.com:65536",
        "example.com:",
        "exa mple.com",
        "https://example.com",
        "[::1",
    ] {
        assert!(card_entry(host, 7).is_err(), "{host:?}");
    }
}

#[test]
fn a_card_host_entry_names_the_run_whose_card_it_is() {
    assert_eq!(
        AllowSource::Card { run: 7 }.to_string(),
        "approved run 7's card"
    );
}

#[test]
fn a_run_reaches_only_the_hosts_its_card_names() {
    let list = card_allowlist(7, &["example.com".to_owned()]).unwrap();

    assert!(list.decide("example.com", 443).is_ok());
    assert!(matches!(
        list.decide("other.example", 443),
        Err(EgressRefusal::NotAllowed { .. })
    ));
}
