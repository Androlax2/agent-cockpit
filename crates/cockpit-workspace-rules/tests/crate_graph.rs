//! The forbidden dependency edges of docs/design/greenfield/02-crate-architecture.md.
//!
//! The graph comes from `cargo tree`, so renamed, workspace-inherited and target-specific
//! dependencies resolve exactly as Cargo builds them. An edge is forbidden whether it is
//! direct or goes through other crates; dev-dependencies count for the crate that declares
//! them only, because Cargo never builds an intermediate crate's dev-dependencies.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct ForbiddenEdge {
    rule: &'static str,
    from: &'static [&'static str],
    to: &'static [&'static str],
}

const CLIENTS: &[&str] = &[
    "cockpit-client",
    "cockpit-cli",
    "cockpit-tui",
    "cockpit-web",
    "cockpit-notify",
];
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
        from: CLIENTS,
        to: &["cockpit-turso"],
    },
    ForbiddenEdge {
        rule: "clients -> scheduling",
        from: CLIENTS,
        to: &["cockpit-scheduling"],
    },
    ForbiddenEdge {
        rule: "providers -> UI",
        from: PROVIDERS,
        to: USER_INTERFACES,
    },
    ForbiddenEdge {
        rule: "authorization -> chief/workflows",
        from: &["cockpit-authorization"],
        to: &["cockpit-chief", "cockpit-workflows"],
    },
    ForbiddenEdge {
        rule: "workflows -> scheduling",
        from: &["cockpit-workflows"],
        to: &["cockpit-scheduling"],
    },
    ForbiddenEdge {
        rule: "agent-channel -> wire types",
        from: &["cockpit-agent-channel"],
        to: &["cockpit-protocol", "cockpit-client"],
    },
    ForbiddenEdge {
        rule: "verification -> processes",
        from: &["cockpit-verification"],
        to: &["cockpit-processes"],
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

/// One line per forbidden edge in the workspace, naming its rule.
fn forbidden_edges_in(manifest_path: &Path) -> Vec<String> {
    let members = workspace_members(manifest_path);
    let mut reachable_by_source: HashMap<&str, Vec<String>> = HashMap::new();
    let mut violations = Vec::new();
    for edge in FORBIDDEN_EDGES {
        for source in edge.from {
            if !members.iter().any(|member| member == source) {
                continue;
            }
            let reachable = reachable_by_source
                .entry(source)
                .or_insert_with(|| packages_reachable_from(manifest_path, source));
            for target in edge.to {
                if !reachable.iter().any(|package| package == target) {
                    continue;
                }
                violations.push(format!(
                    "rule `{}`: {source} depends on {target} \
                     (`cargo tree --target all -e normal,build,dev -p {source} -i {target}` \
                     shows the path)",
                    edge.rule
                ));
            }
        }
    }
    violations
}

/// The daemon wires capabilities together, so it sits above all of them (report 02): a
/// member that depends on it, directly or through another crate, inverts that direction.
fn members_depending_on_the_daemon(manifest_path: &Path) -> Vec<String> {
    workspace_members(manifest_path)
        .into_iter()
        .filter(|member| member != DAEMON)
        .filter(|member| {
            packages_reachable_from(manifest_path, member)
                .iter()
                .any(|package| package == DAEMON)
        })
        .map(|member| {
            format!(
                "rule `nothing -> daemon`: {member} depends on {DAEMON} \
                 (`cargo tree --target all -e normal,build,dev -p {member} -i {DAEMON}` \
                 shows the path)"
            )
        })
        .collect()
}

const DAEMON: &str = "cockpit-daemon";

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
    let unknown: Vec<&str> = FORBIDDEN_EDGES
        .iter()
        .flat_map(|edge| edge.from.iter().chain(edge.to))
        .copied()
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
            ("cockpit-cli", &depends_on("cockpit-turso")),
            ("cockpit-turso", ""),
        ],
    );
    assert_eq!(
        forbidden_edges_in(&manifest_path),
        [
            "rule `clients -> persistence`: cockpit-cli depends on cockpit-turso \
          (`cargo tree --target all -e normal,build,dev -p cockpit-cli -i cockpit-turso` shows the path)"
        ]
    );
}

#[test]
fn each_rule_catches_its_forbidden_edge() {
    for (rule, source, target) in [
        ("clients -> scheduling", "cockpit-tui", "cockpit-scheduling"),
        ("providers -> UI", "cockpit-claude", "cockpit-web"),
        (
            "authorization -> chief/workflows",
            "cockpit-authorization",
            "cockpit-chief",
        ),
        (
            "authorization -> chief/workflows",
            "cockpit-authorization",
            "cockpit-workflows",
        ),
        (
            "workflows -> scheduling",
            "cockpit-workflows",
            "cockpit-scheduling",
        ),
        (
            "verification -> processes",
            "cockpit-verification",
            "cockpit-processes",
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
fn an_edge_through_an_intermediate_crate_is_forbidden() {
    let manifest_path = fixture_workspace(
        "through_intermediate",
        &[
            ("cockpit-cli", &depends_on("cockpit-middle")),
            ("cockpit-middle", &depends_on("cockpit-scheduling")),
            ("cockpit-scheduling", ""),
        ],
    );
    assert_eq!(forbidden_edges_in(&manifest_path).len(), 1);
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

#[test]
fn an_allowed_edge_passes() {
    let manifest_path = fixture_workspace(
        "allowed_edge",
        &[
            ("cockpit-cli", &depends_on("cockpit-client")),
            ("cockpit-client", ""),
        ],
    );
    assert_eq!(forbidden_edges_in(&manifest_path), Vec::<String>::new());
}
