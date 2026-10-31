//! Images the owner pastes into an item's notes, where `[Image N]` names image N. They are
//! part of what is asked, so adding one is an intent revision.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

use crate::items::{bump_intent, invalid_row, owner_only, required};
use crate::{Actor, Error, ImageInfo, ItemId, Refusal};

pub const IMAGE_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const IMAGES_PER_ITEM_MAX: i64 = 20;

/// The formats a note image may have, by the bytes they start with. The type is read from
/// the content, never taken from the sender.
const SIGNATURES: [(&str, &[u8]); 5] = [
    ("image/png", b"\x89PNG\r\n\x1a\n"),
    ("image/jpeg", b"\xff\xd8\xff"),
    ("image/gif", b"GIF87a"),
    ("image/gif", b"GIF89a"),
    ("image/webp", b"RIFF"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub number: i64,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Attaches `bytes` to the item as its next image number.
pub async fn add_image(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    bytes: Vec<u8>,
    now: i64,
) -> Result<ImageInfo, Error> {
    owner_only(actor, "attach an image")?;
    let media_type = media_type_of(&bytes)?;
    let size = bytes.len() as u64;
    let item = required(write, id).await?;
    let count = i64::try_from(item.images.len()).unwrap_or(i64::MAX);
    if count >= IMAGES_PER_ITEM_MAX {
        return Err(Refusal::InvalidImage {
            reason: format!("the item has {count} images, the most it may have"),
        }
        .into());
    }
    let number = item.images.last().map_or(1, |image| image.number + 1);
    write
        .execute(
            "INSERT INTO tasks_images (item, number, media_type, bytes, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (id.0, number, media_type, bytes, now),
        )
        .await?;
    bump_intent(write, id, now).await?;
    Ok(ImageInfo {
        number,
        media_type,
        bytes: size,
    })
}

/// Image `number` of the item, with its bytes.
pub async fn image(
    connection: &Connection,
    actor: &Actor,
    id: ItemId,
    number: i64,
) -> Result<Image, Error> {
    owner_only(actor, "read an image")?;
    required(connection, id).await?;
    let mut rows = connection
        .query(
            "SELECT media_type, bytes FROM tasks_images WHERE item = ?1 AND number = ?2",
            (id.0, number),
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Err(Refusal::InvalidImage {
            reason: format!("item #{} has no image {number}", id.0),
        }
        .into());
    };
    let media_type = row.get::<String>(0)?;
    Ok(Image {
        number,
        media_type: stored_media_type(&media_type)?,
        bytes: row.get(1)?,
    })
}

pub(crate) async fn images_of(
    connection: &Connection,
    id: ItemId,
) -> Result<Vec<ImageInfo>, Error> {
    let mut rows = connection
        .query(
            "SELECT number, media_type, length(bytes) FROM tasks_images
             WHERE item = ?1 ORDER BY number",
            [id.0],
        )
        .await?;
    let mut images = Vec::new();
    while let Some(row) = rows.next().await? {
        let media_type = row.get::<String>(1)?;
        images.push(ImageInfo {
            number: row.get(0)?,
            media_type: stored_media_type(&media_type)?,
            bytes: u64::try_from(row.get::<i64>(2)?).unwrap_or(0),
        });
    }
    Ok(images)
}

fn media_type_of(bytes: &[u8]) -> Result<&'static str, Refusal> {
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(Refusal::InvalidImage {
            reason: format!(
                "it has {} bytes; the limit is {IMAGE_MAX_BYTES}",
                bytes.len()
            ),
        });
    }
    let is_webp = bytes.get(8..12) == Some(b"WEBP".as_slice());
    SIGNATURES
        .into_iter()
        .find(|(media_type, signature)| {
            bytes.starts_with(signature) && (*media_type != "image/webp" || is_webp)
        })
        .map(|(media_type, _)| media_type)
        .ok_or_else(|| Refusal::InvalidImage {
            reason: "it is not a PNG, JPEG, GIF or WebP image".to_owned(),
        })
}

fn stored_media_type(stored: &str) -> Result<&'static str, Error> {
    known_media_type(stored)
        .ok_or_else(|| invalid_row("tasks_images", format!("media type {stored:?}")))
}

fn known_media_type(stored: &str) -> Option<&'static str> {
    SIGNATURES
        .into_iter()
        .map(|(media_type, _)| media_type)
        .find(|media_type| *media_type == stored)
}
