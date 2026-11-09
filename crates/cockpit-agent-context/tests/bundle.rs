//! The instruction bundle shipped in the binary: its topics, the one-read manual and the
//! manifest that names every packet's provenance.

use cockpit_agent_context::{
    Precedence, Scope, Trust, catalog, document, documents, manifest, manual,
};

#[test]
fn the_catalog_lists_every_topic_with_its_summary() {
    let names: Vec<&str> = catalog().iter().map(|entry| entry.name).collect();

    assert_eq!(
        names,
        [
            "overview",
            "workflow",
            "checks",
            "approvals",
            "coordination",
            "sandbox"
        ]
    );
    assert!(catalog().iter().all(|entry| !entry.summary.is_empty()));
}

#[test]
fn every_topic_the_overview_names_is_a_document() {
    let overview = document("overview").expect("the overview ships");
    for entry in catalog().iter().skip(1) {
        assert!(
            overview.body.contains(&format!("`{}`", entry.name)),
            "the overview does not name {}",
            entry.name
        );
    }
}

#[test]
fn an_unknown_topic_is_none() {
    assert!(document("teleport").is_none());
}

#[test]
fn the_manual_holds_every_document_once_in_catalog_order() {
    let manual = manual();
    let mut from = 0;
    for document in documents() {
        let body = document.body.trim();
        let at = manual[from..]
            .find(body)
            .unwrap_or_else(|| panic!("{} is missing or out of order", document.name));
        from += at + body.len();
        assert_eq!(manual.matches(body).count(), 1, "{}", document.name);
    }
}

#[test]
fn the_manual_starts_with_the_version_stamp() {
    assert!(
        manual().starts_with(&format!(
            "<!-- Torlek {} instruction bundle",
            env!("CARGO_PKG_VERSION")
        )),
        "{}",
        &manual()[..80]
    );
}

#[test]
fn the_overview_is_an_agent_skill_with_a_name_and_description() {
    let skill = document("overview").unwrap().file_text();

    let frontmatter = skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(frontmatter, _)| frontmatter)
        .expect("SKILL.md opens with YAML frontmatter");
    assert!(frontmatter.lines().any(|line| line == "name: torlek"));
    let description = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .expect("a description");
    // Agent Skills: at most 1024 characters, loaded into every conversation's catalog.
    assert!(description.len() <= 1024, "{}", description.len());
}

#[test]
fn the_manifest_names_every_file_with_its_hash_and_product_provenance() {
    let manifest = manifest();

    let paths: Vec<&str> = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "torlek/SKILL.md",
            "torlek/references/workflow.md",
            "torlek/references/checks.md",
            "torlek/references/approvals.md",
            "torlek/references/coordination.md",
            "torlek/references/sandbox.md",
        ]
    );
    for (entry, document) in manifest.entries.iter().zip(documents()) {
        assert_eq!(entry.source, "torlek");
        assert_eq!(entry.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(entry.scope, Scope::Product);
        assert_eq!(entry.trust, Trust::Product);
        assert_eq!(entry.precedence, Precedence::ProductInvariant);
        assert_eq!(
            entry.sha256.as_deref(),
            Some(sha256_hex(document.file_text().as_bytes()).as_str())
        );
        assert!(entry.supersedes.is_empty());
    }
}

#[test]
fn the_manifest_id_changes_with_any_document() {
    let manifest = manifest();
    let mut changed = manifest.entries.clone();
    changed[3].sha256 = Some("0".repeat(64));

    assert!(manifest.id.starts_with("m-"), "{}", manifest.id);
    assert_eq!(manifest.id.len(), 2 + 16);
    assert_ne!(cockpit_agent_context::manifest_id(&changed), manifest.id);
    assert_eq!(
        cockpit_agent_context::manifest_id(&manifest.entries),
        manifest.id
    );
}

/// #1248: an agent that meets `EROFS` or `command not found` cannot tell a deliberate limit
/// from a broken setup unless the manual says which symptoms are deliberate.
#[test]
fn the_sandbox_topic_says_which_symptoms_are_deliberate_and_what_to_do() {
    let body = document("sandbox").expect("the sandbox topic ships").body;

    assert!(
        body.contains("| What you see | What it means | The one action |"),
        "{body}"
    );
    for row in [
        "| `~` holds no config, caches or tools from an earlier session | The session home starts empty and is removed when the session ends. |",
        "| `Read-only file system` (`EROFS`) writing `.git/config`, `.git/hooks`, `.torlek` or `.cockpit` | These paths are read-only on purpose",
        "| A `.env` file reads as empty or is missing | `.env` files are hidden on purpose. |",
        "| `command not found` for a tool in `~/.local/bin`, or in another folder outside the toolchain table | That folder is not mounted",
        "| `git push` is refused | Pushes go to the owner. |",
    ] {
        assert!(body.contains(row), "missing {row:?} in\n{body}");
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
