//! The receiver's side: a durable receipt per (receiver, effect key), so a delivery that
//! arrives twice has its effect once (report 04: receivers deduplicate on a stable effect id).

use cockpit_turso::WriteTransaction;

use crate::error::{Error, Refusal, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReceiptId(pub i64);

/// What [`receive`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// The first delivery of the effect: apply it in this transaction.
    First(ReceiptId),
    /// Delivered before: do nothing, and answer with the same receipt.
    Duplicate(ReceiptId),
}

impl Received {
    pub fn receipt(self) -> ReceiptId {
        match self {
            Received::First(receipt) | Received::Duplicate(receipt) => receipt,
        }
    }
}

/// Records that `receiver` got the effect `effect_key` with `content`, in the transaction
/// that applies it. The same key with other content is refused: one key is one effect.
pub async fn receive(
    write: &WriteTransaction<'_>,
    receiver: &str,
    effect_key: &str,
    content: &str,
    now: i64,
) -> Result<Received, Error> {
    if receiver.is_empty() {
        return Err(invalid("receiver", "it is not empty"));
    }
    if effect_key.is_empty() {
        return Err(invalid("effect_key", "it is not empty"));
    }
    let mut rows = write
        .query(
            "SELECT id, content FROM messaging_receipts WHERE receiver = ?1 AND effect_key = ?2",
            (receiver, effect_key),
        )
        .await?;
    if let Some(row) = rows.next().await? {
        if row.get::<String>(1)? != content {
            return Err(Refusal::EffectKeyReused {
                effect_key: effect_key.to_owned(),
            }
            .into());
        }
        return Ok(Received::Duplicate(ReceiptId(row.get(0)?)));
    }
    drop(rows);
    write
        .execute(
            "INSERT INTO messaging_receipts (receiver, effect_key, content, received_at)
             VALUES (?1, ?2, ?3, ?4)",
            (receiver, effect_key, content, now),
        )
        .await?;
    Ok(Received::First(ReceiptId(write.last_insert_rowid())))
}
