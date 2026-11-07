//! A plan's usage windows as a provider reported them (report 18). An adapter builds a
//! [`QuotaReading`] from its harness's raw data; `cockpit-usage` stores and judges it.

use std::collections::BTreeSet;

use crate::InvalidValue;

/// Longest limit key: a window's kind and, for a scoped limit, its model.
const MAX_LIMIT_BYTES: usize = 64;
/// Longest plan or scope name, in the provider's own words.
const MAX_NAME_BYTES: usize = 64;
/// Most windows one reading may carry; providers report two to five.
pub const MAX_WINDOWS: usize = 32;

/// One reading of an account's plan: its windows and whether the provider refuses use now.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaReading {
    plan: Option<String>,
    windows: Vec<QuotaWindow>,
    is_blocked: bool,
}

impl QuotaReading {
    /// `plan` is the provider's own name for it ("max", "pro"), when the reading says.
    /// `is_blocked` is the provider's word that ordinary use is refused right now, whatever
    /// the percentages say. Refuses an empty plan name, more than [`MAX_WINDOWS`] windows,
    /// and two windows with the same limit.
    pub fn new(
        plan: Option<String>,
        windows: Vec<QuotaWindow>,
        is_blocked: bool,
    ) -> Result<QuotaReading, InvalidValue> {
        if let Some(name) = &plan {
            check_name("plan name", name)?;
        }
        if windows.len() > MAX_WINDOWS {
            return Err(InvalidValue {
                kind: "quota reading",
                value: format!("{} windows", windows.len()),
                rule: "at most 32 windows",
            });
        }
        let mut limits = BTreeSet::new();
        if let Some(repeated) = windows
            .iter()
            .find(|window| !limits.insert(window.limit().as_str()))
        {
            return Err(InvalidValue {
                kind: "quota reading",
                value: repeated.limit().as_str().to_owned(),
                rule: "each limit appears once",
            });
        }
        Ok(QuotaReading {
            plan,
            windows,
            is_blocked,
        })
    }

    pub fn plan(&self) -> Option<&str> {
        self.plan.as_deref()
    }

    pub fn windows(&self) -> &[QuotaWindow] {
        &self.windows
    }

    pub fn is_blocked(&self) -> bool {
        self.is_blocked
    }
}

/// One usage window of a plan: how much of it is used and when it starts over.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    limit: LimitKey,
    scope: Option<String>,
    length_seconds: Option<u32>,
    used_percent: f64,
    resets_at: Option<i64>,
}

impl QuotaWindow {
    /// `scope` is the model or surface a scoped limit counts, in the provider's words
    /// ("Sonnet"), `None` for a limit on every model. `used_percent` is the share of the
    /// limit used: 0 to 100, above 100 once a provider counts past its limit. `resets_at` is
    /// when the window starts over, in Unix seconds. Refuses a used share that is negative or
    /// not a number, a zero length and an empty or overlong scope.
    pub fn new(
        limit: LimitKey,
        scope: Option<String>,
        length_seconds: Option<u32>,
        used_percent: f64,
        resets_at: Option<i64>,
    ) -> Result<QuotaWindow, InvalidValue> {
        if !used_percent.is_finite() || used_percent < 0.0 {
            return Err(InvalidValue {
                kind: "used share",
                value: used_percent.to_string(),
                rule: "a finite percentage, 0 or more",
            });
        }
        if length_seconds == Some(0) {
            return Err(InvalidValue {
                kind: "window length",
                value: "0".to_owned(),
                rule: "a length of at least one second",
            });
        }
        if let Some(name) = &scope {
            check_name("limit scope", name)?;
        }
        Ok(QuotaWindow {
            limit,
            scope,
            length_seconds,
            used_percent,
            resets_at,
        })
    }

    pub fn limit(&self) -> &LimitKey {
        &self.limit
    }

    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    pub fn length_seconds(&self) -> Option<u32> {
        self.length_seconds
    }

    pub fn used_percent(&self) -> f64 {
        self.used_percent
    }

    pub fn resets_at(&self) -> Option<i64> {
        self.resets_at
    }
}

/// Names a window within one provider's plan, stable across readings: `five-hour`,
/// `seven-day`, `seven-day:sonnet`. Lower-case ASCII letters, digits, `-` and `:`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LimitKey(String);

impl LimitKey {
    pub fn new(key: impl Into<String>) -> Result<LimitKey, InvalidValue> {
        let key = key.into();
        let is_plain = key.len() <= MAX_LIMIT_BYTES
            && key.starts_with(|first: char| first.is_ascii_lowercase())
            && key.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | ':')
            });
        if !is_plain {
            return Err(InvalidValue {
                kind: "limit key",
                value: key,
                rule: "1 to 64 lower-case ASCII letters, digits, `-` and `:`, starting with a letter",
            });
        }
        Ok(LimitKey(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn check_name(kind: &'static str, name: &str) -> Result<(), InvalidValue> {
    let is_plain = !name.trim().is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.chars().any(char::is_control);
    if is_plain {
        return Ok(());
    }
    Err(InvalidValue {
        kind,
        value: name.to_owned(),
        rule: "1 to 64 bytes of text without control characters",
    })
}
