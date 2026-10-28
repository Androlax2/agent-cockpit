//! Images a session shows in its phone chat: an agent reply's line `![alt](path)` names a file in
//! the session's folder. Only such a line, in a reply the phone's conversation window holds, makes
//! a file readable, and only a PNG, JPEG, GIF or WebP inside that folder, at most IMAGE_MAX bytes.
//! The column's `I` opens the latest reply's images under the same rules (latest_reply_images).

use super::{conversation, session_row, text};
use regex::Regex;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::LazyLock;

const IMAGE_MAX: u64 = 10 * 1024 * 1024;

/// A reply's image path and either its content type/bytes or a read error.
pub type ReplyImage = (String, Result<(&'static str, Vec<u8>), String>);

/// The page's own grammar (page.js imageLine): the whole line is the image.
static IMAGE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*!\[([^\]\n]*)\]\(([^)\s]+)\)\s*$").unwrap());
static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*```").unwrap());

/// The paths of a reply's image lines, in order, outside fenced code (as the page counts them).
pub fn image_paths(message: &str) -> Vec<String> {
    let mut is_in_code = false;
    let mut paths = Vec::new();
    for line in message.split('\n') {
        if FENCE.is_match(line) {
            is_in_code = !is_in_code;
        } else if !is_in_code && let Some(found) = IMAGE_LINE.captures(line) {
            paths.push(found[2].to_string());
        }
    }
    paths
}

/// The `index`th image of reply `message_id` in session `id`: its content type and bytes.
pub fn shared_image(
    id: &str,
    message_id: &str,
    index: usize,
) -> Result<(&'static str, Vec<u8>), String> {
    let history = conversation(id)?;
    let reply = history["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|message| message["id"] == message_id && message["role"] == "assistant")
        .ok_or("no such reply in the recent conversation")?;
    let path = image_paths(reply["text"].as_str().unwrap_or(""))
        .into_iter()
        .nth(index)
        .ok_or("this reply shows no such image")?;
    let row = session_row(id).ok_or("no such session")?;
    read_inside(Path::new(&text(&row, "cwd")), &path)
}

/// The images session `id`'s latest reply names (its assistant messages since the last question),
/// each read from `folder` under the phone's rules: its path, then its type and bytes or why not.
pub fn latest_reply_images(id: &str, folder: &str) -> Result<Vec<ReplyImage>, String> {
    let history = conversation(id)?;
    let messages = history["messages"].as_array().cloned().unwrap_or_default();
    let reply: Vec<&Value> = messages
        .iter()
        .rev()
        .skip_while(|message| message["role"] != "assistant")
        .take_while(|message| message["role"] == "assistant")
        .collect();
    Ok(reply
        .into_iter()
        .rev()
        .flat_map(|message| image_paths(message["text"].as_str().unwrap_or("")))
        .map(|path| {
            let image = read_inside(Path::new(folder), &path);
            (path, image)
        })
        .collect())
}

/// `path` (relative to `folder`, or absolute) when it resolves inside `folder` and is an image.
fn read_inside(folder: &Path, path: &str) -> Result<(&'static str, Vec<u8>), String> {
    if !folder.is_absolute() {
        return Err("this session has no folder".into());
    }
    let root = folder
        .canonicalize()
        .map_err(|_| "the session's folder is unavailable")?;
    let target = root
        .join(path)
        .canonicalize()
        .map_err(|_| format!("{path} does not exist"))?;
    if !target.starts_with(&root) {
        return Err(format!("{path} is outside the session's folder"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&target)
        .map_err(|_| format!("{path} could not be read"))?;
    let metadata = file
        .metadata()
        .map_err(|_| format!("{path} could not be read"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    if metadata.len() > IMAGE_MAX {
        return Err(format!(
            "{path} is larger than {} MB",
            IMAGE_MAX / 1_048_576
        ));
    }
    let mut bytes = Vec::new();
    file.take(IMAGE_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{path} could not be read"))?;
    if bytes.len() as u64 > IMAGE_MAX {
        return Err(format!(
            "{path} is larger than {} MB",
            IMAGE_MAX / 1_048_576
        ));
    }
    let content_type = image_type(&bytes)
        .ok_or_else(|| format!("{path} is not a PNG, JPEG, GIF or WebP image"))?;
    Ok((content_type, bytes))
}

/// The type the bytes themselves say, never the file name: no SVG, whose markup could run.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_image_lines_outside_code_count_in_order() {
        let reply = "Here:\n![Login](design/login.png)\nsee ![inline](x.png) too\n```\n![code](no.png)\n```\n  ![Second](b.jpg)  ";
        assert_eq!(image_paths(reply), ["design/login.png", "b.jpg"]);
    }
}
