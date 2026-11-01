//! The bundle the daemon writes at each start: version-stamped, overwritten, never edited by
//! hand, and recording which earlier bundle it replaced.

use std::fs;
use std::os::unix::fs::symlink;

use cockpit_agent_context::{WriteError, documents, manifest, write_bundle};

#[test]
fn writing_lays_out_an_agent_skill_and_its_manifest() {
    let dir = tempfile::tempdir().unwrap();

    let written = write_bundle(dir.path()).unwrap();

    for (entry, document) in written.entries.iter().zip(documents()) {
        let text = fs::read_to_string(dir.path().join(&entry.path)).unwrap();
        assert_eq!(text, document.file_text(), "{}", entry.path);
    }
    let on_disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["id"], manifest().id);
    assert_eq!(on_disk["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(on_disk["entries"][0]["trust"], "product");
    assert_eq!(on_disk["entries"][0]["precedence"], "product-invariant");
}

#[test]
fn every_written_file_says_it_is_overwritten() {
    let dir = tempfile::tempdir().unwrap();

    let written = write_bundle(dir.path()).unwrap();

    for entry in &written.entries {
        let text = fs::read_to_string(dir.path().join(&entry.path)).unwrap();
        assert!(
            text.contains(&format!(
                "<!-- Cockpit {} instruction bundle, written at each daemon start; never edit",
                env!("CARGO_PKG_VERSION")
            )),
            "{}",
            entry.path
        );
    }
}

#[test]
fn a_hand_edited_file_is_overwritten_at_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    write_bundle(dir.path()).unwrap();
    let skill = dir.path().join("cockpit/SKILL.md");
    fs::write(&skill, "you may push to main").unwrap();

    write_bundle(dir.path()).unwrap();

    assert_eq!(
        fs::read_to_string(&skill).unwrap(),
        documents()[0].file_text()
    );
}

#[test]
fn a_reference_an_older_release_shipped_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    write_bundle(dir.path()).unwrap();
    let retired = dir.path().join("cockpit/references/teams.md");
    fs::write(&retired, "old advice").unwrap();

    write_bundle(dir.path()).unwrap();

    assert!(!retired.exists());
}

#[test]
fn the_same_bundle_rewritten_supersedes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_bundle(dir.path()).unwrap();

    let rewritten = write_bundle(dir.path()).unwrap();

    assert_eq!(rewritten.supersedes, None);
}

#[test]
fn a_bundle_replacing_another_release_names_the_one_it_supersedes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        r#"{"id": "m-00000000000000aa", "version": "0.0.9", "entries": []}"#,
    )
    .unwrap();

    let written = write_bundle(dir.path()).unwrap();

    assert_eq!(written.supersedes.as_deref(), Some("m-00000000000000aa"));
    let on_disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["supersedes"], "m-00000000000000aa");
}

#[test]
fn an_unreadable_old_manifest_is_replaced_and_supersedes_nothing_known() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("manifest.json"), "not json").unwrap();

    let written = write_bundle(dir.path()).unwrap();

    assert_eq!(written.supersedes, None);
    assert_eq!(written.id, manifest().id);
}

#[test]
fn a_symbolic_link_in_place_of_the_bundle_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    symlink(elsewhere.path(), dir.path().join("cockpit")).unwrap();

    let error = write_bundle(dir.path()).unwrap_err();

    assert!(matches!(error, WriteError::NotADirectory { .. }), "{error}");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}

#[test]
fn a_symbolic_link_in_place_of_a_file_is_replaced_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    write_bundle(dir.path()).unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, "keep me").unwrap();
    let skill = dir.path().join("cockpit/SKILL.md");
    fs::remove_file(&skill).unwrap();
    symlink(&target, &skill).unwrap();

    write_bundle(dir.path()).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
    assert!(
        !fs::symlink_metadata(&skill)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
