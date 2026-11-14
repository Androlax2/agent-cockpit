//! What a confined run's output says it tried and its limits refused (#1387): the record and
//! the pane name the path, the host or sudo, from what the programs printed.

use std::path::PathBuf;

use cockpit_approvals::limit_hits;

fn hidden() -> Vec<PathBuf> {
    ["/home/owner/.ssh", "/home/owner/.local/state/torlek"]
        .map(PathBuf::from)
        .into()
}

fn hits(output: &str) -> Vec<String> {
    limit_hits(output, &hidden())
}

#[test]
fn a_hidden_path_read_is_named() {
    let cases = [
        ("cat", "cat: /home/owner/.ssh/id_ed25519: Permission denied"),
        ("bash", "bash: /home/owner/.ssh/config: Permission denied"),
        (
            "node",
            "Error: EACCES: permission denied, open '/home/owner/.ssh/config'",
        ),
        (
            "an emptied folder",
            "cat: /home/owner/.local/state/torlek/state.db: No such file or directory",
        ),
    ];
    for (program, output) in cases {
        let path = output
            .split(['\'', ':', ' ', ','])
            .find(|word| word.starts_with("/home/owner/"))
            .unwrap();
        assert_eq!(
            hits(output),
            [format!(
                "tried to read {path}, which confined runs cannot see"
            )],
            "{program}"
        );
    }
}

#[test]
fn a_permission_denied_outside_the_hidden_paths_is_no_limit_hit() {
    assert_eq!(
        hits("cat: /etc/shadow: Permission denied"),
        Vec::<String>::new()
    );
}

#[test]
fn a_write_outside_the_writable_folders_is_named() {
    let cases = [
        ("bash", "bash: /home/owner/.bashrc: Read-only file system"),
        (
            "dash",
            "/bin/sh: 1: cannot create /home/owner/.bashrc: Read-only file system",
        ),
        (
            "touch",
            "touch: cannot touch '/home/owner/.bashrc': Read-only file system",
        ),
        (
            "mkdir",
            "mkdir: cannot create directory \u{2018}/home/owner/.bashrc\u{2019}: Read-only file system",
        ),
        (
            "python",
            "OSError: [Errno 30] Read-only file system: '/home/owner/.bashrc'",
        ),
    ];
    for (program, output) in cases {
        assert_eq!(
            hits(output),
            ["tried to write /home/owner/.bashrc, outside the folders this run may write"],
            "{program}"
        );
    }
}

#[test]
fn a_host_reached_is_named() {
    let cases = [
        ("curl", "curl: (6) Could not resolve host: example.com"),
        (
            "curl's connect",
            "curl: (7) Failed to connect to example.com port 443 after 1 ms: Couldn't connect",
        ),
        (
            "curl 8's connect",
            "curl: (7) Failed to connect to example.com:443 after 0 ms: Could not connect to server",
        ),
        (
            "git",
            "fatal: unable to access 'https://example.com/owner/repo.git/': Could not resolve \
             host: example.com",
        ),
        (
            "ssh",
            "ssh: Could not resolve hostname example.com: Temporary failure in name resolution",
        ),
        ("node", "Error: getaddrinfo ENOTFOUND example.com"),
    ];
    for (program, output) in cases {
        assert_eq!(
            hits(output),
            ["tried to reach example.com, but this run has no network"],
            "{program}"
        );
    }
}

#[test]
fn a_network_failure_without_a_host_names_the_network() {
    assert_eq!(
        hits("connect: Network is unreachable"),
        ["tried to reach the network, which this run does not have"]
    );
}

#[test]
fn sudo_names_the_root_route_it_needs() {
    assert_eq!(
        hits("sudo: /usr/bin/sudo must be owned by uid 0 and have the setuid bit set"),
        ["tried to use sudo, which needs the root route confined runs do not have yet"]
    );
}

#[test]
fn each_hit_is_named_once_in_the_order_it_happened() {
    let output = "bash: /home/owner/.bashrc: Read-only file system\n\
                  curl: (6) Could not resolve host: example.com\n\
                  bash: /home/owner/.bashrc: Read-only file system\n";

    assert_eq!(
        hits(output),
        [
            "tried to write /home/owner/.bashrc, outside the folders this run may write",
            "tried to reach example.com, but this run has no network",
        ]
    );
}

#[test]
fn output_without_a_limit_hit_has_none() {
    assert_eq!(hits("ok\nall 12 tests passed\n"), Vec::<String>::new());
}
