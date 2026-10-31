//! The dependency rules of docs/design/greenfield/02-crate-architecture.md: forbidden edges,
//! and allow-lists for the crates report 02 says may import only named crates.
//!
//! The graph comes from `cargo tree`, so renamed, workspace-inherited and target-specific
//! dependencies resolve exactly as Cargo builds them. An edge is forbidden whether it is
//! direct or goes through other crates; dev-dependencies count for the crate that declares
//! them only, because Cargo never builds an intermediate crate's dev-dependencies.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct ForbiddenEdge {
    rule: &'static str,
    from: Sources,
    to: &'static [&'static str],
}

enum Sources {
    Named(&'static [&'static str]),
    /// Every workspace member outside these groups, so a crate added later is checked
    /// without editing this table.
    MembersExcept(&'static [&'static [&'static str]]),
}

impl Sources {
    fn names(&self) -> Vec<&'static str> {
        match self {
            Sources::Named(names) => names.to_vec(),
            Sources::MembersExcept(groups) => groups.concat(),
        }
    }

    fn select<'a>(&self, members: &'a [String]) -> Vec<&'a str> {
        members
            .iter()
            .map(String::as_str)
            .filter(|member| match self {
                Sources::Named(names) => names.contains(member),
                Sources::MembersExcept(groups) => {
                    !groups.iter().any(|group| group.contains(member))
                }
            })
            .collect()
    }
}

/// `from` may build against no `cockpit-*` package outside `allowed`, so a crate added later
/// is forbidden until a decision names it here.
struct AllowList {
    rule: &'static str,
    from: &'static [&'static str],
    allowed: &'static [&'static str],
}

const DAEMON: &str = "cockpit-daemon";
/// Clients that are not frontends; the frontends' allow-lists are stricter.
const CLIENTS: &[&str] = &["cockpit-client", "cockpit-notify"];
const PROVIDER_ADAPTERS: &[&str] = &["cockpit-claude", "cockpit-codex", "cockpit-opencode"];
const PROVIDERS: &[&str] = &[
    "cockpit-provider-api",
    "cockpit-claude",
    "cockpit-codex",
    "cockpit-opencode",
];
const USER_INTERFACES: &[&str] = &["cockpit-cli", "cockpit-tui", "cockpit-web"];
/// Crates named by report 02's current table; `every_rule_names_a_crate_from_report_02`
/// fails when a rename there leaves this table behind.
const FORBIDDEN_EDGES: &[ForbiddenEdge] = &[
    ForbiddenEdge {
        rule: "clients -> persistence",
        from: Sources::Named(CLIENTS),
        to: &["cockpit-turso"],
    },
    ForbiddenEdge {
        rule: "clients -> scheduling",
        from: Sources::Named(CLIENTS),
        to: &["cockpit-scheduling"],
    },
    // Report 05: clients render the capture stream the daemon serves; only the daemon's
    // side runs tmux.
    ForbiddenEdge {
        rule: "clients -> tmux",
        from: Sources::Named(CLIENTS),
        to: &["cockpit-tmux"],
    },
    ForbiddenEdge {
        rule: "providers -> UI",
        from: Sources::Named(PROVIDERS),
        to: USER_INTERFACES,
    },
    ForbiddenEdge {
        rule: "capabilities -> scheduling",
        // Report 02: "lower capabilities never call scheduling". Every member is lower but
        // the daemon above it and the clients, whose own rules cover it; the chief too,
        // because it proposes and never places.
        from: Sources::MembersExcept(&[
            &[DAEMON, "cockpit-scheduling"],
            CLIENTS,
            USER_INTERFACES,
            &["cockpit-authorization"],
        ]),
        to: &["cockpit-scheduling"],
    },
    ForbiddenEdge {
        rule: "agent-channel -> wire types",
        from: Sources::Named(&["cockpit-agent-channel"]),
        to: &["cockpit-protocol", "cockpit-client"],
    },
    ForbiddenEdge {
        rule: "verification -> processes",
        from: Sources::Named(&["cockpit-verification"]),
        to: &["cockpit-processes"],
    },
    // Sessions and tasks occupy checkouts; projects answering about checkouts must not call
    // back up into them (#28).
    ForbiddenEdge {
        rule: "projects -> sessions/tasks",
        from: Sources::Named(&["cockpit-projects"]),
        to: &["cockpit-sessions", "cockpit-tasks"],
    },
    // Approvals keep the owner's decisions; the daemon identifies the sender, finds the
    // project and converts to wire types (#34).
    ForbiddenEdge {
        rule: "approvals -> wire types/channel/projects",
        from: Sources::Named(&["cockpit-approvals"]),
        to: &[
            "cockpit-protocol",
            "cockpit-client",
            "cockpit-agent-channel",
            "cockpit-projects",
        ],
    },
    // Scheduling, sessions and tasks ask admission whether work fits; admission answers from
    // its reservations and the executions they charge, and the daemon converts to wire
    // types (#42).
    ForbiddenEdge {
        rule: "resource-admission -> sessions/tasks/wire types",
        from: Sources::Named(&["cockpit-resource-admission"]),
        to: &[
            "cockpit-sessions",
            "cockpit-tasks",
            "cockpit-protocol",
            "cockpit-client",
        ],
    },
    // Sessions drive every provider through the `cockpit-provider-api` contract alone; no
    // provider's type or path reaches them (#31).
    ForbiddenEdge {
        rule: "sessions -> provider adapters",
        from: Sources::Named(&["cockpit-sessions"]),
        to: PROVIDER_ADAPTERS,
    },
    // #33: no adapter type reaches a capability; the daemon wires each adapter in as a
    // `cockpit-provider-api` provider, and everything above keys on its capabilities.
    ForbiddenEdge {
        rule: "capabilities -> provider adapters",
        // Sessions' own rule above names this edge for them.
        from: Sources::MembersExcept(&[&[DAEMON, "cockpit-sessions"], PROVIDER_ADAPTERS]),
        to: PROVIDER_ADAPTERS,
    },
    // Tasks own the outcome, never the steps (workflows) or the sessions doing them; the
    // daemon identifies the sender, finds its project and converts to wire types (#38).
    ForbiddenEdge {
        rule: "tasks -> workflows/sessions/wire types",
        from: Sources::Named(&["cockpit-tasks"]),
        to: &[
            "cockpit-workflows",
            "cockpit-sessions",
            "cockpit-protocol",
            "cockpit-client",
            "cockpit-agent-channel",
        ],
    },
];

/// Report 02 states these rules as what a crate may import, not what it may not.
const ALLOW_LISTS: &[AllowList] = &[
    AllowList {
        rule: "frontends -> client/protocol only",
        from: &["cockpit-web"],
        allowed: &["cockpit-client", "cockpit-protocol"],
    },
    // #176: the TUI is a long-lived process with its own log (report 12); diagnostics
    // depends on no other Cockpit crate, so the edge reaches no capability.
    AllowList {
        rule: "tui -> client/protocol/diagnostics only",
        from: &["cockpit-tui"],
        allowed: &["cockpit-client", "cockpit-protocol", "cockpit-diagnostics"],
    },
    // #77: the CLI's offline verbs import their capability crate. Adding one is a decision.
    AllowList {
        rule: "cli -> client/protocol/offline capabilities only",
        from: &["cockpit-cli"],
        allowed: &["cockpit-client", "cockpit-protocol", "cockpit-config"],
    },
    // "Authorization depends on nothing product-level": the state database's infrastructure
    // is the only Cockpit crate it builds against.
    AllowList {
        rule: "authorization -> nothing product-level",
        from: &["cockpit-authorization"],
        allowed: &["cockpit-turso"],
    },
    // Report 09: configuration reads files and depends on no product crate; sessions read
    // its start files at launch (#146), so the edge only ever points into it.
    AllowList {
        rule: "config -> nothing product-level",
        from: &["cockpit-config"],
        allowed: &[],
    },
    // #33: an adapter sees Cockpit through the provider contract alone; its in-sandbox
    // bridge reaches the daemon through the `cockpit` CLI as a program, not as a crate.
    AllowList {
        rule: "provider adapters -> provider-api only",
        from: PROVIDER_ADAPTERS,
        allowed: &["cockpit-provider-api"],
    },
    // Producers own what a message or an entity means and depend on messaging, never the
    // reverse; the daemon converts the feed to wire types (#41).
    AllowList {
        rule: "messaging -> the database only",
        from: &["cockpit-messaging"],
        allowed: &["cockpit-turso"],
    },
];

/// Package names of the first column of `cargo tree --prefix none -f {p}` output.
fn cargo_tree_packages(manifest_path: &Path, selection: &[&str]) -> Vec<String> {
    let output = Command::new(env!("CARGO"))
        .arg("tree")
        .arg("--manifest-path")
        .arg(manifest_path)
        .args(["--locked", "--target", "all"])
        .args(["--edges", "normal,build,dev", "--prefix", "none"])
        .args(["--format", "{p}"])
        .args(selection)
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo tree {selection:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn workspace_members(manifest_path: &Path) -> Vec<String> {
    cargo_tree_packages(manifest_path, &["--workspace", "--depth", "0"])
}

/// Every package `member` builds against, itself included.
fn packages_reachable_from(manifest_path: &Path, member: &str) -> Vec<String> {
    cargo_tree_packages(manifest_path, &["--package", member])
}

fn violation(rule: &str, source: &str, target: &str) -> String {
    format!(
        "rule `{rule}`: {source} depends on {target} \
         (`cargo tree --target all -e normal,build,dev -p {source} -i {target}` shows the path)"
    )
}

/// Every member with the sorted packages it builds against, itself included.
fn reachable_by_member(manifest_path: &Path) -> BTreeMap<String, Vec<String>> {
    workspace_members(manifest_path)
        .into_iter()
        .map(|member| {
            let mut reachable = packages_reachable_from(manifest_path, &member);
            reachable.sort();
            reachable.dedup();
            (member, reachable)
        })
        .collect()
}

/// One line per forbidden edge in the workspace, naming its rule.
fn forbidden_edges_in(manifest_path: &Path) -> Vec<String> {
    let reachable_by_member = reachable_by_member(manifest_path);
    let members: Vec<String> = reachable_by_member.keys().cloned().collect();
    let mut violations = Vec::new();
    for edge in FORBIDDEN_EDGES {
        for source in edge.from.select(&members) {
            for target in edge.to {
                if reachable_by_member[source]
                    .iter()
                    .any(|package| package == target)
                {
                    violations.push(violation(edge.rule, source, target));
                }
            }
        }
    }
    for allow_list in ALLOW_LISTS {
        for source in allow_list.from {
            let Some(reachable) = reachable_by_member.get(*source) else {
                continue;
            };
            for target in reachable.iter().filter(|package| {
                package.starts_with("cockpit-")
                    && package != source
                    && !allow_list.allowed.contains(&package.as_str())
            }) {
                violations.push(violation(allow_list.rule, source, target));
            }
        }
    }
    violations
}

/// The daemon wires capabilities together, so it sits above all of them (report 02): a
/// member that depends on it, directly or through another crate, inverts that direction.
fn members_depending_on_the_daemon(manifest_path: &Path) -> Vec<String> {
    reachable_by_member(manifest_path)
        .into_iter()
        .filter(|(member, reachable)| {
            member != DAEMON && reachable.iter().any(|package| package == DAEMON)
        })
        .map(|(member, _)| violation("nothing -> daemon", &member, DAEMON))
        .collect()
}

#[test]
fn the_workspace_has_no_forbidden_dependency_edge() {
    let root_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    assert_eq!(forbidden_edges_in(&root_manifest), Vec::<String>::new());
}

#[test]
fn no_workspace_member_depends_on_the_daemon() {
    let root_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    assert_eq!(
        members_depending_on_the_daemon(&root_manifest),
        Vec::<String>::new()
    );
}

#[test]
fn a_capability_depending_on_the_daemon_fails_naming_the_rule() {
    let manifest_path = fixture_workspace(
        "capability_to_daemon",
        &[
            ("cockpit-tasks", &depends_on("cockpit-middle")),
            ("cockpit-middle", &depends_on(DAEMON)),
            (DAEMON, ""),
        ],
    );
    let violations = members_depending_on_the_daemon(&manifest_path);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .all(|line| line.starts_with("rule `nothing -> daemon`")),
        "{violations:?}"
    );
}

/// A rule naming a crate that no longer exists would pass forever without checking anything.
#[test]
fn every_rule_names_a_crate_from_report_02() {
    let report = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/design/greenfield/02-crate-architecture.md"),
    )
    .expect("report 02 is readable");
    let forbidden_edge_crates = FORBIDDEN_EDGES
        .iter()
        .flat_map(|edge| edge.from.names().into_iter().chain(edge.to.iter().copied()));
    let allow_list_crates = ALLOW_LISTS
        .iter()
        .flat_map(|allow_list| allow_list.from.iter().chain(allow_list.allowed).copied());
    let unknown: Vec<&str> = forbidden_edge_crates
        .chain(allow_list_crates)
        .filter(|crate_name| !report.contains(&format!("`{crate_name}`")))
        .collect();
    assert_eq!(unknown, Vec::<&str>::new());
}

/// A `[dependencies]` table with one path dependency on the fixture crate `target`.
fn depends_on(target: &str) -> String {
    format!("[dependencies]\n{target} = {{ path = \"../{target}\" }}")
}

/// A throwaway workspace of empty library crates; `crates` pairs each name with the lines of
/// its `[dependencies]`-style tables (header lines included).
fn fixture_workspace(test_name: &str, crates: &[(&str, &str)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("crate-graph")
        .join(test_name);
    if root.exists() {
        fs::remove_dir_all(&root).expect("old fixture is removable");
    }
    for (crate_name, dependency_tables) in crates {
        let crate_dir = root.join(crate_name);
        fs::create_dir_all(crate_dir.join("src")).expect("fixture crate is writable");
        fs::write(crate_dir.join("src/lib.rs"), "").expect("fixture lib.rs is writable");
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                 {dependency_tables}\n"
            ),
        )
        .expect("fixture manifest is writable");
    }
    let members: Vec<String> = crates
        .iter()
        .map(|(crate_name, _)| format!("\"{crate_name}\""))
        .collect();
    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        format!(
            "[workspace]\nmembers = [{}]\nresolver = \"3\"\n",
            members.join(", ")
        ),
    )
    .expect("fixture workspace manifest is writable");
    let lockfile = Command::new(env!("CARGO"))
        .arg("generate-lockfile")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output()
        .expect("cargo runs");
    assert!(
        lockfile.status.success(),
        "{}",
        String::from_utf8_lossy(&lockfile.stderr)
    );
    manifest_path
}

#[test]
fn a_client_depending_on_persistence_fails_naming_the_rule() {
    let manifest_path = fixture_workspace(
        "client_to_persistence",
        &[
            ("cockpit-client", &depends_on("cockpit-turso")),
            ("cockpit-turso", ""),
        ],
    );
    assert_eq!(
        forbidden_edges_in(&manifest_path),
        [
            "rule `clients -> persistence`: cockpit-client depends on cockpit-turso \
          (`cargo tree --target all -e normal,build,dev -p cockpit-client -i cockpit-turso` shows the path)"
        ]
    );
}

#[test]
fn each_rule_catches_its_forbidden_edge() {
    for (rule, source, target) in [
        (
            "clients -> scheduling",
            "cockpit-client",
            "cockpit-scheduling",
        ),
        ("clients -> tmux", "cockpit-notify", "cockpit-tmux"),
        ("providers -> UI", "cockpit-provider-api", "cockpit-web"),
        (
            "capabilities -> scheduling",
            "cockpit-workflows",
            "cockpit-scheduling",
        ),
        (
            "capabilities -> scheduling",
            "cockpit-tasks",
            "cockpit-scheduling",
        ),
        (
            "capabilities -> scheduling",
            "cockpit-chief",
            "cockpit-scheduling",
        ),
        (
            "verification -> processes",
            "cockpit-verification",
            "cockpit-processes",
        ),
        (
            "projects -> sessions/tasks",
            "cockpit-projects",
            "cockpit-sessions",
        ),
        (
            "projects -> sessions/tasks",
            "cockpit-projects",
            "cockpit-tasks",
        ),
        (
            "tui -> client/protocol/diagnostics only",
            "cockpit-tui",
            "cockpit-config",
        ),
        (
            "frontends -> client/protocol only",
            "cockpit-web",
            "cockpit-diagnostics",
        ),
        (
            "frontends -> client/protocol only",
            "cockpit-web",
            "cockpit-sessions",
        ),
        (
            "cli -> client/protocol/offline capabilities only",
            "cockpit-cli",
            "cockpit-turso",
        ),
        (
            "authorization -> nothing product-level",
            "cockpit-authorization",
            "cockpit-chief",
        ),
        (
            "authorization -> nothing product-level",
            "cockpit-authorization",
            "cockpit-workflows",
        ),
        (
            "config -> nothing product-level",
            "cockpit-config",
            "cockpit-sessions",
        ),
        (
            "approvals -> wire types/channel/projects",
            "cockpit-approvals",
            "cockpit-protocol",
        ),
        (
            "approvals -> wire types/channel/projects",
            "cockpit-approvals",
            "cockpit-agent-channel",
        ),
        (
            "approvals -> wire types/channel/projects",
            "cockpit-approvals",
            "cockpit-projects",
        ),
        (
            "sessions -> provider adapters",
            "cockpit-sessions",
            "cockpit-claude",
        ),
        (
            "sessions -> provider adapters",
            "cockpit-sessions",
            "cockpit-codex",
        ),
        (
            "sessions -> provider adapters",
            "cockpit-sessions",
            "cockpit-opencode",
        ),
        (
            "capabilities -> provider adapters",
            "cockpit-tasks",
            "cockpit-claude",
        ),
        (
            "capabilities -> provider adapters",
            "cockpit-agent-channel",
            "cockpit-claude",
        ),
        (
            "provider adapters -> provider-api only",
            "cockpit-claude",
            "cockpit-sessions",
        ),
        (
            "messaging -> the database only",
            "cockpit-messaging",
            "cockpit-protocol",
        ),
        (
            "messaging -> the database only",
            "cockpit-messaging",
            "cockpit-teams",
        ),
        (
            "tasks -> workflows/sessions/wire types",
            "cockpit-tasks",
            "cockpit-workflows",
        ),
        (
            "tasks -> workflows/sessions/wire types",
            "cockpit-tasks",
            "cockpit-sessions",
        ),
        (
            "tasks -> workflows/sessions/wire types",
            "cockpit-tasks",
            "cockpit-protocol",
        ),
        (
            "tasks -> workflows/sessions/wire types",
            "cockpit-tasks",
            "cockpit-agent-channel",
        ),
    ] {
        let manifest_path = fixture_workspace(
            &format!("{source}_to_{target}"),
            &[(source, &depends_on(target)), (target, "")],
        );
        let violations = forbidden_edges_in(&manifest_path);
        assert_eq!(violations.len(), 1, "{rule}: {violations:?}");
        assert!(
            violations[0].starts_with(&format!("rule `{rule}`: {source} depends on {target}")),
            "{violations:?}"
        );
    }
}

#[test]
fn every_allowed_frontend_and_authorization_edge_passes() {
    for (source, target) in [
        ("cockpit-cli", "cockpit-client"),
        ("cockpit-tui", "cockpit-client"),
        ("cockpit-tui", "cockpit-diagnostics"),
        ("cockpit-web", "cockpit-protocol"),
        ("cockpit-cli", "cockpit-config"),
        ("cockpit-authorization", "cockpit-turso"),
        ("cockpit-daemon", "cockpit-claude"),
        ("cockpit-claude", "cockpit-provider-api"),
        ("cockpit-messaging", "cockpit-turso"),
    ] {
        let manifest_path = fixture_workspace(
            &format!("allowed_{source}_to_{target}"),
            &[(source, &depends_on(target)), (target, "")],
        );
        assert_eq!(
            forbidden_edges_in(&manifest_path),
            Vec::<String>::new(),
            "{source} -> {target}"
        );
    }
}

/// The #77 list does not launder: an allowed crate's own dependencies are checked too.
#[test]
fn a_crate_reached_through_an_allowed_one_is_caught() {
    let manifest_path = fixture_workspace(
        "through_allowed",
        &[
            ("cockpit-cli", &depends_on("cockpit-config")),
            ("cockpit-config", &depends_on("cockpit-sessions")),
            ("cockpit-sessions", ""),
        ],
    );
    let violations: Vec<String> = forbidden_edges_in(&manifest_path)
        .into_iter()
        .filter(|line| line.contains("cockpit-cli"))
        .collect();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with(
            "rule `cli -> client/protocol/offline capabilities only`: \
             cockpit-cli depends on cockpit-sessions"
        ),
        "{violations:?}"
    );
}

#[test]
fn an_edge_through_an_intermediate_crate_is_forbidden() {
    let manifest_path = fixture_workspace(
        "through_intermediate",
        &[
            ("cockpit-client", &depends_on("cockpit-middle")),
            ("cockpit-middle", &depends_on("cockpit-scheduling")),
            ("cockpit-scheduling", ""),
        ],
    );
    let violations = forbidden_edges_in(&manifest_path);
    assert!(
        violations.iter().any(|line| line.starts_with(
            "rule `clients -> scheduling`: cockpit-client depends on cockpit-scheduling"
        )),
        "{violations:?}"
    );
}

#[test]
fn renamed_dev_and_target_specific_dependencies_count() {
    for (case, dependency_tables) in [
        (
            "renamed",
            "[dependencies]\nstore = { package = \"cockpit-turso\", path = \"../cockpit-turso\" }",
        ),
        (
            "dev",
            "[dev-dependencies]\ncockpit-turso = { path = \"../cockpit-turso\" }",
        ),
        (
            "build",
            "[build-dependencies]\ncockpit-turso = { path = \"../cockpit-turso\" }",
        ),
        (
            "target_specific",
            "[target.'cfg(target_os = \"macos\")'.dependencies]\n\
             cockpit-turso = { path = \"../cockpit-turso\" }",
        ),
    ] {
        let manifest_path = fixture_workspace(
            &format!("dependency_kind_{case}"),
            &[("cockpit-cli", dependency_tables), ("cockpit-turso", "")],
        );
        assert_eq!(forbidden_edges_in(&manifest_path).len(), 1, "{case}");
    }
}

#[test]
fn an_intermediate_crates_dev_dependency_is_not_an_edge() {
    let manifest_path = fixture_workspace(
        "intermediate_dev_dependency",
        &[
            ("cockpit-cli", &depends_on("cockpit-client")),
            (
                "cockpit-client",
                "[dev-dependencies]\ncockpit-test-daemon = { path = \"../cockpit-test-daemon\" }",
            ),
            ("cockpit-test-daemon", &depends_on("cockpit-turso")),
            ("cockpit-turso", ""),
        ],
    );
    let violations = forbidden_edges_in(&manifest_path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0]
            .starts_with("rule `clients -> persistence`: cockpit-client depends on cockpit-turso"),
        "{violations:?}"
    );
}
