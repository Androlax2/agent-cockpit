//! Writing a file in a folder an agent may have written, without following either anywhere
//! else (#390 part 6d): the daemon writes a sandboxed session's Docker client configuration so.

use std::path::Path;

use cockpit_projects::replace_agent_folder_file;

fn session_temp() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("docker-config")).unwrap();
    temp
}

fn read(temp: &tempfile::TempDir) -> String {
    std::fs::read_to_string(temp.path().join("docker-config/config.json")).unwrap()
}

#[test]
fn the_file_is_written_replaced_and_removed() {
    let temp = session_temp();

    replace_agent_folder_file(temp.path(), "docker-config", "config.json", Some("{}")).unwrap();
    assert_eq!(read(&temp), "{}");
    replace_agent_folder_file(
        temp.path(),
        "docker-config",
        "config.json",
        Some("{\"a\":1}"),
    )
    .unwrap();
    assert_eq!(read(&temp), "{\"a\":1}");
    replace_agent_folder_file(temp.path(), "docker-config", "config.json", None).unwrap();
    replace_agent_folder_file(temp.path(), "docker-config", "config.json", None).unwrap();

    assert!(!temp.path().join("docker-config/config.json").exists());
}

#[test]
fn a_folder_linked_elsewhere_is_refused_and_its_target_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let owners = tempfile::tempdir().unwrap();
    std::fs::write(owners.path().join("config.json"), "owner's").unwrap();
    std::os::unix::fs::symlink(owners.path(), temp.path().join("docker-config")).unwrap();

    let written =
        replace_agent_folder_file(temp.path(), "docker-config", "config.json", Some("{}"));

    assert!(written.is_err(), "{written:?}");
    assert_eq!(
        std::fs::read_to_string(owners.path().join("config.json")).unwrap(),
        "owner's"
    );
}

#[test]
fn a_file_linked_elsewhere_is_replaced_and_its_target_untouched() {
    let owners = tempfile::tempdir().unwrap();
    let target = owners.path().join("bashrc");
    let links: [fn(&Path, &Path) -> std::io::Result<()>; 2] = [
        |original, link| std::os::unix::fs::symlink(original, link),
        |original, link| std::fs::hard_link(original, link),
    ];
    for link in links {
        let temp = session_temp();
        std::fs::write(&target, "owner's").unwrap();
        link(&target, &temp.path().join("docker-config/config.json")).unwrap();

        replace_agent_folder_file(temp.path(), "docker-config", "config.json", Some("{}")).unwrap();

        assert_eq!(read(&temp), "{}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "owner's");
    }
}
