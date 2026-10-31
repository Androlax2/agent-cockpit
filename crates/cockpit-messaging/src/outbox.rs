//! The outbox: effect intents committed with the state that caused them, delivered by a
//! worker afterwards, each under one idempotency key (report 04, "Durable effects").
//!
//! Delivery states follow report 04: `queued → submitting → received | uncertain | failed |
//! expired`, plus `superseded` for a replaceable observation a newer one replaced before it
//! was sent. "Received" needs the receiver's durable receipt. A delivery whose
//! acknowledgement was lost is sent again only to a receiver that deduplicates on the effect
//! key; to any other it becomes `uncertain`, which waits for a lookup or the owner's decision
//! and is never sent again on its own.

use std::future::Future;

use cockpit_turso::sql::{Connection, Row};
use cockpit_turso::{Database, WriteTransaction};

use crate::error::{Error, Refusal, invalid, invalid_row};

const TABLE: &str = "messaging_envelopes";
const ENVELOPE_COLUMNS: &str = "id, effect_key, producer, destination, payload, redelivery, \
    replaceable_slot, state, attempts, max_attempts, due_at, expires_at, receipt, note, \
    superseded_by";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvelopeId(pub i64);

/// An effect to deliver, as its producer enqueues it. The producer owns what `payload` means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// One key per logical effect: enqueuing the same effect again finds this envelope, and
    /// the receiver deduplicates on it.
    pub effect_key: String,
    pub producer: String,
    pub destination: String,
    pub payload: String,
    pub redelivery: Redelivery,
    pub coalescing: Coalescing,
    /// Deliveries tried at most; at least 1.
    pub max_attempts: u32,
    /// Not sent after this time (seconds since 1970); `None` waits as long as it takes.
    pub expires_at: Option<i64>,
}

/// Whether sending again after a lost acknowledgement is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redelivery {
    /// The receiver deduplicates on the effect key, so a second delivery has no second
    /// effect.
    ReceiverDeduplicates,
    /// A second delivery could repeat the effect: an unacknowledged delivery is `uncertain`.
    Never,
}

/// Whether a newer envelope may replace this one before it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coalescing {
    /// Decisions, approvals, outcomes and ownership transitions: every one is delivered.
    Never,
    /// An availability observation, replaced by a newer one for the same destination and
    /// `slot` while it still waits to be sent.
    ReplaceableObservation { slot: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    Queued,
    Submitting,
    Received,
    Uncertain,
    Failed,
    Expired,
    Superseded,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryState::Queued => "queued",
            DeliveryState::Submitting => "submitting",
            DeliveryState::Received => "received",
            DeliveryState::Uncertain => "uncertain",
            DeliveryState::Failed => "failed",
            DeliveryState::Expired => "expired",
            DeliveryState::Superseded => "superseded",
        }
    }

    fn parse(text: &str) -> Result<DeliveryState, Error> {
        Ok(match text {
            "queued" => DeliveryState::Queued,
            "submitting" => DeliveryState::Submitting,
            "received" => DeliveryState::Received,
            "uncertain" => DeliveryState::Uncertain,
            "failed" => DeliveryState::Failed,
            "expired" => DeliveryState::Expired,
            "superseded" => DeliveryState::Superseded,
            other => return Err(invalid_row(TABLE, format!("unknown state {other:?}"))),
        })
    }
}

impl Redelivery {
    fn as_str(self) -> &'static str {
        match self {
            Redelivery::ReceiverDeduplicates => "receiver-deduplicates",
            Redelivery::Never => "never",
        }
    }

    fn parse(text: &str) -> Result<Redelivery, Error> {
        match text {
            "receiver-deduplicates" => Ok(Redelivery::ReceiverDeduplicates),
            "never" => Ok(Redelivery::Never),
            other => Err(invalid_row(TABLE, format!("unknown redelivery {other:?}"))),
        }
    }
}

/// What [`enqueue`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    /// A new envelope, and the waiting observation it replaced, if any.
    Queued {
        id: EnvelopeId,
        superseded: Option<EnvelopeId>,
    },
    /// The same effect was enqueued before: nothing new is delivered.
    AlreadyEnqueued {
        id: EnvelopeId,
        state: DeliveryState,
    },
}

/// Commits the intent to deliver `envelope` with the caller's transaction. The same effect
/// key with the same content finds the envelope already there; with other content it is
/// refused, since one key names one effect.
pub async fn enqueue(
    write: &WriteTransaction<'_>,
    envelope: &Envelope,
    now: i64,
) -> Result<Enqueued, Error> {
    validate(envelope)?;
    let slot = match &envelope.coalescing {
        Coalescing::Never => None,
        Coalescing::ReplaceableObservation { slot } => Some(slot.as_str()),
    };
    if let Some(existing) = by_effect_key(write, &envelope.effect_key).await? {
        let is_same_effect = existing.producer == envelope.producer
            && existing.destination == envelope.destination
            && existing.payload == envelope.payload
            && existing.redelivery == envelope.redelivery
            && existing.replaceable_slot.as_deref() == slot;
        if !is_same_effect {
            return Err(Refusal::EffectKeyReused {
                effect_key: envelope.effect_key.clone(),
            }
            .into());
        }
        return Ok(Enqueued::AlreadyEnqueued {
            id: existing.id,
            state: existing.state,
        });
    }
    let waiting_observation = match slot {
        Some(slot) => waiting_in_slot(write, &envelope.destination, slot).await?,
        None => None,
    };
    write
        .execute(
            "INSERT INTO messaging_envelopes (effect_key, producer, destination, payload,
                 redelivery, replaceable_slot, state, attempts, max_attempts, due_at,
                 expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', 0, ?7, ?8, ?9, ?8, ?8)",
            (
                envelope.effect_key.as_str(),
                envelope.producer.as_str(),
                envelope.destination.as_str(),
                envelope.payload.as_str(),
                envelope.redelivery.as_str(),
                slot,
                i64::from(envelope.max_attempts),
                now,
                envelope.expires_at,
            ),
        )
        .await?;
    let id = EnvelopeId(write.last_insert_rowid());
    if let Some(replaced) = waiting_observation {
        write
            .execute(
                "UPDATE messaging_envelopes SET state = 'superseded', superseded_by = ?2,
                     updated_at = ?3
                 WHERE id = ?1",
                (replaced.0, id.0, now),
            )
            .await?;
    }
    Ok(Enqueued::Queued {
        id,
        superseded: waiting_observation,
    })
}

/// One delivery attempt a worker took: the envelope is `submitting` until its outcome is
/// recorded with [`record_delivery`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub id: EnvelopeId,
    /// 1 for the first delivery; outcomes are fenced on it.
    pub attempt: u32,
    pub effect_key: String,
    pub producer: String,
    pub destination: String,
    pub payload: String,
}

/// Marks queued envelopes past their expiry `expired`, then takes up to `limit` due ones,
/// oldest due first, for delivery. Commit before delivering: no delivery runs inside a
/// transaction (report 02).
pub async fn claim_due(
    write: &WriteTransaction<'_>,
    now: i64,
    limit: usize,
) -> Result<Vec<Claim>, Error> {
    write
        .execute(
            "UPDATE messaging_envelopes SET state = 'expired',
                 note = 'expired before it could be delivered', updated_at = ?1
             WHERE state = 'queued' AND expires_at IS NOT NULL AND expires_at <= ?1",
            [now],
        )
        .await?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut rows = write
        .query(
            &format!(
                "SELECT {ENVELOPE_COLUMNS} FROM messaging_envelopes
                 WHERE state = 'queued' AND due_at <= ?1 ORDER BY due_at, id LIMIT ?2"
            ),
            (now, limit),
        )
        .await?;
    let mut due = Vec::new();
    while let Some(row) = rows.next().await? {
        due.push(stored(&row)?);
    }
    let mut claims = Vec::with_capacity(due.len());
    for envelope in due {
        let attempt = envelope.attempts + 1;
        write
            .execute(
                "UPDATE messaging_envelopes SET state = 'submitting', attempts = ?2,
                     updated_at = ?3
                 WHERE id = ?1",
                (envelope.id.0, i64::from(attempt), now),
            )
            .await?;
        claims.push(Claim {
            id: envelope.id,
            attempt,
            effect_key: envelope.effect_key,
            producer: envelope.producer,
            destination: envelope.destination,
            payload: envelope.payload,
        });
    }
    Ok(claims)
}

/// What a delivery attempt observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The receiver's durable receipt.
    Received {
        receipt: String,
    },
    Failed(Failure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Nothing reached the receiver (it refused the connection, it was not running), so
    /// sending again is safe; retried at `retry_at` while attempts remain.
    NotSent { reason: String, retry_at: i64 },
    /// The receiver refused the effect for good.
    Rejected { reason: String },
    /// It may have reached the receiver, but no receipt came back: a lost acknowledgement, a
    /// timeout, a crash. Sent again at `retry_at` only to a receiver that deduplicates.
    Unacknowledged { reason: String, retry_at: i64 },
}

/// Records what attempt `claim.attempt` of the envelope observed, and returns its state.
///
/// A receipt is recorded whenever it arrives, late ones included: it proves the receiver has
/// the effect. A failure counts only for the envelope's current attempt; a stale one leaves
/// the envelope as it is.
pub async fn record_delivery(
    write: &WriteTransaction<'_>,
    id: EnvelopeId,
    attempt: u32,
    delivery: &Delivery,
    now: i64,
) -> Result<DeliveryState, Error> {
    let envelope = envelope(write, id)
        .await?
        .ok_or(Refusal::NotFound { envelope: id.0 })?;
    match delivery {
        Delivery::Received { receipt } => {
            if receipt.is_empty() {
                return Err(invalid("receipt", "a receipt is not empty"));
            }
            if envelope.state == DeliveryState::Received {
                return Ok(DeliveryState::Received);
            }
            set_received(write, id, receipt, now).await?;
            Ok(DeliveryState::Received)
        }
        Delivery::Failed(failure) => {
            if envelope.state != DeliveryState::Submitting || envelope.attempts != attempt {
                return Ok(envelope.state);
            }
            fail(write, &envelope, failure, now).await
        }
    }
}

/// Restart contract: an envelope a stopped daemon left `submitting` may or may not have
/// reached its receiver. It is retried only when the receiver deduplicates; otherwise it is
/// `uncertain`. Returns each envelope with the state it was left in.
pub async fn reconcile_deliveries(
    write: &WriteTransaction<'_>,
    now: i64,
) -> Result<Vec<(EnvelopeId, DeliveryState)>, Error> {
    let mut rows = write
        .query(
            &format!(
                "SELECT {ENVELOPE_COLUMNS} FROM messaging_envelopes
                 WHERE state = 'submitting' ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut interrupted = Vec::new();
    while let Some(row) = rows.next().await? {
        interrupted.push(stored(&row)?);
    }
    let mut reconciled = Vec::with_capacity(interrupted.len());
    for envelope in interrupted {
        let failure = Failure::Unacknowledged {
            reason: "the daemon stopped during the delivery".to_owned(),
            retry_at: now,
        };
        let state = fail(write, &envelope, &failure, now).await?;
        reconciled.push((envelope.id, state));
    }
    Ok(reconciled)
}

/// What became of an uncertain delivery, found by a lookup at the receiver or decided by the
/// owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The receiver has it: its receipt.
    Received { receipt: String },
    /// The owner decided it did not arrive: one more delivery.
    SendAgain,
    /// The owner gave it up.
    Abandon { note: String },
}

/// Settles an `uncertain` delivery; any other state is refused.
pub async fn resolve_uncertain(
    write: &WriteTransaction<'_>,
    id: EnvelopeId,
    resolution: &Resolution,
    now: i64,
) -> Result<DeliveryState, Error> {
    let envelope = envelope(write, id)
        .await?
        .ok_or(Refusal::NotFound { envelope: id.0 })?;
    if envelope.state != DeliveryState::Uncertain {
        return Err(Refusal::NotUncertain {
            state: envelope.state.as_str(),
        }
        .into());
    }
    match resolution {
        Resolution::Received { receipt } => {
            if receipt.is_empty() {
                return Err(invalid("receipt", "a receipt is not empty"));
            }
            set_received(write, id, receipt, now).await?;
            Ok(DeliveryState::Received)
        }
        Resolution::SendAgain => {
            write
                .execute(
                    "UPDATE messaging_envelopes SET state = 'queued', due_at = ?2,
                         max_attempts = attempts + 1, note = NULL, updated_at = ?2
                     WHERE id = ?1",
                    (id.0, now),
                )
                .await?;
            Ok(DeliveryState::Queued)
        }
        Resolution::Abandon { note } => {
            set_ended(write, id, DeliveryState::Failed, note, now).await?;
            Ok(DeliveryState::Failed)
        }
    }
}

/// An envelope as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEnvelope {
    pub id: EnvelopeId,
    pub effect_key: String,
    pub producer: String,
    pub destination: String,
    pub payload: String,
    pub redelivery: Redelivery,
    pub replaceable_slot: Option<String>,
    pub state: DeliveryState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub due_at: i64,
    pub expires_at: Option<i64>,
    pub receipt: Option<String>,
    /// Why it failed, expired or is uncertain.
    pub note: Option<String>,
    pub superseded_by: Option<EnvelopeId>,
}

pub async fn envelope(
    connection: &Connection,
    id: EnvelopeId,
) -> Result<Option<StoredEnvelope>, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {ENVELOPE_COLUMNS} FROM messaging_envelopes WHERE id = ?1"),
            [id.0],
        )
        .await?;
    rows.next().await?.map(|row| stored(&row)).transpose()
}

/// The deliveries waiting for a lookup or the owner's decision, oldest first.
pub async fn uncertain_envelopes(connection: &Connection) -> Result<Vec<StoredEnvelope>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {ENVELOPE_COLUMNS} FROM messaging_envelopes
                 WHERE state = 'uncertain' ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut uncertain = Vec::new();
    while let Some(row) = rows.next().await? {
        uncertain.push(stored(&row)?);
    }
    Ok(uncertain)
}

/// Delivers one claimed envelope to its receiver and reports what it observed. It runs
/// outside every transaction.
pub trait Deliverer {
    fn deliver(&self, claim: &Claim) -> impl Future<Output = Delivery> + Send;
}

/// One pass of the delivery worker: claims up to `limit` due envelopes and commits, delivers
/// each through `deliverer`, then records each outcome in its own transaction. A crash in
/// between leaves the envelopes `submitting` for [`reconcile_deliveries`].
pub async fn deliver_due(
    database: &Database,
    deliverer: &impl Deliverer,
    now: i64,
    limit: usize,
) -> Result<Vec<(EnvelopeId, DeliveryState)>, Error> {
    let write = database.write().await?;
    let claims = claim_due(&write, now, limit).await?;
    write.commit().await?;
    let mut delivered = Vec::with_capacity(claims.len());
    for claim in claims {
        let delivery = deliverer.deliver(&claim).await;
        let write = database.write().await?;
        let state = record_delivery(&write, claim.id, claim.attempt, &delivery, now).await?;
        write.commit().await?;
        delivered.push((claim.id, state));
    }
    Ok(delivered)
}

async fn fail(
    write: &WriteTransaction<'_>,
    envelope: &StoredEnvelope,
    failure: &Failure,
    now: i64,
) -> Result<DeliveryState, Error> {
    let attempts_remain = envelope.attempts < envelope.max_attempts;
    let (state, note, retry_at) = match failure {
        Failure::NotSent { reason, retry_at } if attempts_remain => {
            (DeliveryState::Queued, reason, *retry_at)
        }
        Failure::NotSent { reason, .. } | Failure::Rejected { reason } => {
            (DeliveryState::Failed, reason, now)
        }
        Failure::Unacknowledged { reason, retry_at }
            if attempts_remain && envelope.redelivery == Redelivery::ReceiverDeduplicates =>
        {
            (DeliveryState::Queued, reason, *retry_at)
        }
        Failure::Unacknowledged { reason, .. } => (DeliveryState::Uncertain, reason, now),
    };
    match state {
        DeliveryState::Queued => {
            write
                .execute(
                    "UPDATE messaging_envelopes SET state = 'queued', due_at = ?2, note = ?3,
                         updated_at = ?4
                     WHERE id = ?1",
                    (envelope.id.0, retry_at, note.as_str(), now),
                )
                .await?;
        }
        _ => set_ended(write, envelope.id, state, note, now).await?,
    }
    Ok(state)
}

async fn set_received(
    write: &WriteTransaction<'_>,
    id: EnvelopeId,
    receipt: &str,
    now: i64,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE messaging_envelopes SET state = 'received', receipt = ?2, note = NULL,
                 superseded_by = NULL, updated_at = ?3
             WHERE id = ?1",
            (id.0, receipt, now),
        )
        .await?;
    Ok(())
}

async fn set_ended(
    write: &WriteTransaction<'_>,
    id: EnvelopeId,
    state: DeliveryState,
    note: &str,
    now: i64,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE messaging_envelopes SET state = ?2, note = ?3, updated_at = ?4 WHERE id = ?1",
            (id.0, state.as_str(), note, now),
        )
        .await?;
    Ok(())
}

async fn by_effect_key(
    connection: &Connection,
    effect_key: &str,
) -> Result<Option<StoredEnvelope>, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {ENVELOPE_COLUMNS} FROM messaging_envelopes WHERE effect_key = ?1"),
            [effect_key],
        )
        .await?;
    rows.next().await?.map(|row| stored(&row)).transpose()
}

async fn waiting_in_slot(
    connection: &Connection,
    destination: &str,
    slot: &str,
) -> Result<Option<EnvelopeId>, Error> {
    let mut rows = connection
        .query(
            "SELECT id FROM messaging_envelopes
             WHERE state = 'queued' AND destination = ?1 AND replaceable_slot = ?2",
            (destination, slot),
        )
        .await?;
    Ok(rows
        .next()
        .await?
        .map(|row| row.get(0))
        .transpose()?
        .map(EnvelopeId))
}

fn stored(row: &Row) -> Result<StoredEnvelope, Error> {
    let count = |index: usize| -> Result<u32, Error> {
        let value: i64 = row.get(index)?;
        u32::try_from(value).map_err(|_| invalid_row(TABLE, format!("count {value}")))
    };
    Ok(StoredEnvelope {
        id: EnvelopeId(row.get(0)?),
        effect_key: row.get(1)?,
        producer: row.get(2)?,
        destination: row.get(3)?,
        payload: row.get(4)?,
        redelivery: Redelivery::parse(&row.get::<String>(5)?)?,
        replaceable_slot: row.get(6)?,
        state: DeliveryState::parse(&row.get::<String>(7)?)?,
        attempts: count(8)?,
        max_attempts: count(9)?,
        due_at: row.get(10)?,
        expires_at: row.get(11)?,
        receipt: row.get(12)?,
        note: row.get(13)?,
        superseded_by: row.get::<Option<i64>>(14)?.map(EnvelopeId),
    })
}

fn validate(envelope: &Envelope) -> Result<(), Error> {
    let required = [
        ("effect_key", &envelope.effect_key),
        ("producer", &envelope.producer),
        ("destination", &envelope.destination),
    ];
    if let Some((field, _)) = required.iter().find(|(_, value)| value.is_empty()) {
        return Err(invalid(field, "it is not empty"));
    }
    if let Coalescing::ReplaceableObservation { slot } = &envelope.coalescing
        && slot.is_empty()
    {
        return Err(invalid("slot", "a replaceable observation names its slot"));
    }
    if envelope.max_attempts == 0 {
        return Err(invalid("max_attempts", "at least one delivery is tried"));
    }
    Ok(())
}
