//! Torlek's own sound for each event, rendered by the owner (#1130) and embedded in the
//! binary. A player needs a file, so the notifier writes them under its state directory at
//! start.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::event::Event;

/// The sound folder inside the notifier's state directory.
const SOUNDS_DIR: &str = "sounds";

/// The embedded Ogg Vorbis file for `event`; `disk` and `turn` have none and keep their theme sound.
fn bytes(event: Event) -> Option<&'static [u8]> {
    match event {
        Event::Failure => Some(include_bytes!("../assets/sounds/failure.oga")),
        Event::Hold => Some(include_bytes!("../assets/sounds/hold.oga")),
        Event::Question => Some(include_bytes!("../assets/sounds/question.oga")),
        Event::Quota => Some(include_bytes!("../assets/sounds/quota.oga")),
        Event::Finished => Some(include_bytes!("../assets/sounds/finished.oga")),
        Event::Disk | Event::Turn => None,
    }
}

/// Writes each embedded sound under `state_dir` and returns where. An event whose file cannot
/// be written is logged and left out, so it plays its theme sound.
pub fn install(state_dir: &Path) -> BTreeMap<Event, PathBuf> {
    let directory = state_dir.join(SOUNDS_DIR);
    let mut files = BTreeMap::new();
    for event in Event::ALL {
        let Some(content) = bytes(event) else {
            continue;
        };
        let path = directory.join(format!("{event}.oga"));
        match write_if_changed(&path, content) {
            Ok(()) => {
                files.insert(event, path);
            }
            Err(error) => tracing::warn!(
                %event,
                path = %path.display(),
                %error,
                "cannot write the bundled sound; the theme's sound plays instead"
            ),
        }
    }
    files
}

/// Replaces the file through a rename, so a player never reads half of it.
fn write_if_changed(path: &Path, content: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Per process: the service and `torlek notify test` may install at the same time.
    let partial = path.with_extension(format!("oga.{}.partial", std::process::id()));
    fs::write(&partial, content)?;
    fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_but_disk_and_turn_gets_an_ogg_file_in_the_state_directory() {
        let state_dir = tempfile::tempdir().unwrap();

        let files = install(state_dir.path());

        assert_eq!(files.len(), Event::ALL.len() - 2);
        assert!(!files.contains_key(&Event::Disk));
        assert!(!files.contains_key(&Event::Turn));
        for (event, path) in &files {
            assert!(path.starts_with(state_dir.path().join("sounds")), "{event}");
            assert!(fs::read(path).unwrap().starts_with(b"OggS"), "{event}");
        }
    }

    #[test]
    fn a_file_the_owner_damaged_is_restored_at_the_next_start() {
        let state_dir = tempfile::tempdir().unwrap();
        let path = install(state_dir.path())[&Event::Question].clone();
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"damaged").unwrap();

        install(state_dir.path());

        assert_eq!(fs::read(&path).unwrap(), original);
    }
}
