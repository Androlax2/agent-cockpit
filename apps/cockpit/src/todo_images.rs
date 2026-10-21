//! Images Theo pastes into a TODO's notes (CMD+V in the form). The column reads the clipboard's
//! PNG with wl-paste (it runs outside the sandbox) and keeps it in todo-images/ next to todo.json,
//! which sessions can read but not write. The notes say "[Image N]" where it belongs; the item maps
//! N to its file, and claude-todo show prints the paths for the session to open.

use crate::paths;
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};
use std::fs;

const MAX_BYTES: usize = 20 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// An unreferenced file younger than this may belong to a form still open: kept until the next sweep.
const ORPHAN_GRACE: Duration = Duration::from_secs(3600);

static FILE_NAME: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"^[0-9a-f]{16}\.png$").unwrap());
static MARKER: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"\[Image (\d{1,4})\]").unwrap());

pub fn is_file_name(name: &str) -> bool { FILE_NAME.is_match(name) }

pub fn marker(number: u32) -> String { format!("[Image {number}]") }

/// Where an image lives, beside the list it belongs to (todo.json's folder).
pub fn path_in(list_folder: &Path, file: &str) -> PathBuf { list_folder.join("todo-images").join(file) }

pub fn path_of(file: &str) -> PathBuf { paths::todo_images().join(file) }

/// The clipboard's PNG image, stored; its file name. An error Theo can read otherwise.
pub fn paste_from_clipboard() -> Result<String, String> {
    let types = Command::new("wl-paste").arg("--list-types").stdin(Stdio::null()).stderr(Stdio::null()).output()
        .map_err(|error| format!("Cannot run wl-paste: {error}"))?;
    if !String::from_utf8_lossy(&types.stdout).lines().any(|line| line.trim() == "image/png") {
        return Err("No image in the clipboard".into());
    }
    let image = Command::new("wl-paste").args(["--no-newline", "--type", "image/png"]).stdin(Stdio::null()).stderr(Stdio::null()).output()
        .map_err(|error| format!("Cannot run wl-paste: {error}"))?;
    if !image.status.success() {
        return Err(format!("wl-paste failed: {}", image.status));
    }
    store(&image.stdout)
}

pub fn store(bytes: &[u8]) -> Result<String, String> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err("The clipboard image is not a PNG".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("Image too large ({} MB, at most {} MB)", bytes.len() / 1_048_576, MAX_BYTES / 1_048_576));
    }
    let name = format!("{}.png", crate::util::token_hex(8));
    fs::create_dir_all(paths::todo_images()).map_err(|error| format!("Cannot store the image: {error}"))?;
    let partial = paths::todo_images().join(format!(".{name}.partial"));
    fs::write(&partial, bytes).and_then(|()| fs::rename(&partial, path_of(&name)))
        .map_err(|error| { let _ = fs::remove_file(&partial); format!("Cannot store the image: {error}") })?;
    Ok(name)
}

/// The images whose marker is still in the notes: deleting "[Image 2]" drops image 2.
pub fn referenced(images: &BTreeMap<u32, String>, notes: &str) -> BTreeMap<u32, String> {
    let numbers: HashSet<u32> = MARKER.captures_iter(notes).filter_map(|captures| captures[1].parse().ok()).collect();
    images.iter().filter(|(number, file)| numbers.contains(number) && is_file_name(file)).map(|(number, file)| (*number, file.clone())).collect()
}

/// Removes the stored images nothing refers to any more (a cancelled form, a deleted or expired item).
pub fn sweep(kept: &HashSet<String>) {
    let Ok(entries) = fs::read_dir(paths::todo_images()) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_ours = is_file_name(&name) || (name.starts_with('.') && name.ends_with(".png.partial"));
        if !is_ours || kept.contains(&name) { continue }
        let age = entry.metadata().and_then(|metadata| metadata.modified()).ok().and_then(|modified| now.duration_since(modified).ok());
        if age.is_some_and(|age| age >= ORPHAN_GRACE) {
            if let Err(error) = fs::remove_file(entry.path()) {
                crate::util::log_event("WARN", &format!("cannot remove the TODO image {name}: {error}"));
            }
        }
    }
}
