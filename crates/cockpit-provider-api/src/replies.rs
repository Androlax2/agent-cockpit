//! The model replies a harness recorded, as typed facts for the cost archive (report 18,
//! #699). An adapter reads its harness's own records and builds [`ModelReply`] values;
//! `cockpit-usage` prices, stores and totals them.

use crate::InvalidValue;

const MAX_KEY_BYTES: usize = 256;
const MAX_MODEL_BYTES: usize = 128;
const MAX_WORKING_DIR_BYTES: usize = 4096;
const MAX_SOURCE_BYTES: usize = 1024;

/// What one reply consumed, in tokens, as the harness counted them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplyTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Written to the five-minute prompt cache.
    pub cache_write_5m: u64,
    /// Written to the one-hour prompt cache.
    pub cache_write_1h: u64,
    /// The reply ran in the provider's fast mode, which bills at a premium.
    pub is_fast: bool,
}

/// One model reply: the unit a provider bills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReply {
    key: String,
    at: i64,
    model: String,
    working_dir: Option<String>,
    tokens: ReplyTokens,
}

impl ModelReply {
    /// `key` names the reply within its provider and is the same whenever a harness records
    /// the reply again: a resumed or copied conversation repeats its earlier replies, which
    /// the provider billed once. `at` is when the reply was made, in Unix seconds.
    /// `working_dir` is where the agent ran, as the harness recorded it. Refuses an empty
    /// or overlong key, model or working directory, and control characters in any of them.
    pub fn new(
        key: impl Into<String>,
        at: i64,
        model: impl Into<String>,
        working_dir: Option<String>,
        tokens: ReplyTokens,
    ) -> Result<ModelReply, InvalidValue> {
        let key = key.into();
        let model = model.into();
        check_text("reply key", &key, MAX_KEY_BYTES)?;
        check_text("reply model", &model, MAX_MODEL_BYTES)?;
        if let Some(working_dir) = &working_dir {
            check_text(
                "reply working directory",
                working_dir,
                MAX_WORKING_DIR_BYTES,
            )?;
        }
        Ok(ModelReply {
            key,
            at,
            model,
            working_dir,
            tokens,
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn at(&self) -> i64 {
        self.at
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    pub fn tokens(&self) -> ReplyTokens {
        self.tokens
    }
}

/// How far a scan of one record source got: `offset` is the byte after the last complete
/// record counted, so the next scan starts there and never reads again what it counted.
/// `source` names the record file relative to the home folder scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCursor {
    source: String,
    offset: u64,
}

impl SourceCursor {
    pub fn new(source: impl Into<String>, offset: u64) -> Result<SourceCursor, InvalidValue> {
        let source = source.into();
        check_text("cursor source", &source, MAX_SOURCE_BYTES)?;
        Ok(SourceCursor { source, offset })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }
}

/// What [`crate::Provider::scan_replies`] found past the cursors it was given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyScan {
    /// Replies past the old cursors, oldest records first. A reply may repeat one an earlier
    /// scan returned; its key says so.
    pub replies: Vec<ModelReply>,
    /// The cursor of every source that moved, which the caller keeps for the next scan.
    pub cursors: Vec<SourceCursor>,
    /// What the scan could not read, one line each, for the owner's log. A source it names
    /// keeps its old cursor, so the next scan tries it again.
    pub problems: Vec<String>,
}

fn check_text(kind: &'static str, text: &str, limit: usize) -> Result<(), InvalidValue> {
    if !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control) {
        return Ok(());
    }
    Err(InvalidValue {
        kind,
        value: text.chars().take(80).collect(),
        rule: "1 to its length limit in bytes of text without control characters",
    })
}
