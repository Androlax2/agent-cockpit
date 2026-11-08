//! Owner-only: each provider's accounts, in the order a sandboxed launch tries them (#59,
//! owner decision 2026-11-08). An account's credential files live in its own folder of the
//! provider's credential folder; `default` keeps the provider's folder itself, so the single
//! credential of #968 is the `default` account and works unchanged. Read at the daemon's start.
use std::collections::BTreeMap;

use serde::Deserialize;
use toml::Spanned;

use crate::{ConfigError, Layer, Source, Sourced};

/// The account a provider has when the owner lists none: the credential files directly in
/// its folder (#968).
pub const DEFAULT_ACCOUNT: &str = "default";

/// The rule of `cockpit_usage::AccountName`, which names the same accounts in the quota
/// records: configuration depends on no product crate, so it checks the rule itself.
const MAX_ACCOUNT_BYTES: usize = 32;

/// The owner's account lists, by provider; a provider without one has [`DEFAULT_ACCOUNT`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountSettings {
    listed: BTreeMap<&'static str, Sourced<Vec<String>>>,
}

impl AccountSettings {
    /// `provider`'s accounts in the order a launch tries them.
    pub fn of(&self, provider: &str) -> Vec<String> {
        self.listed
            .get(provider)
            .map(|accounts| accounts.value.clone())
            .unwrap_or_else(|| vec![DEFAULT_ACCOUNT.to_owned()])
    }

    /// `provider`'s list with where it was set; the product default when the owner set none.
    pub fn sourced(&self, provider: &str) -> Sourced<Vec<String>> {
        self.listed.get(provider).cloned().unwrap_or_else(|| {
            Sourced::new(vec![DEFAULT_ACCOUNT.to_owned()], Source::ProductDefault)
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccountsTable {
    claude: Option<Spanned<Vec<String>>>,
    codex: Option<Spanned<Vec<String>>>,
    opencode: Option<Spanned<Vec<String>>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<AccountSettings, ConfigError> {
    let mut result = AccountSettings::default();
    for layer in layers {
        let Some(table) = &layer.settings.accounts else {
            continue;
        };
        let table = table.get_ref();
        for (provider, list) in [
            ("claude", &table.claude),
            ("codex", &table.codex),
            ("opencode", &table.opencode),
        ] {
            let Some(list) = list else {
                continue;
            };
            let source = layer.source_at(list.span());
            check_list(list.get_ref()).map_err(|reason| ConfigError::Invalid {
                key: format!("accounts.{provider}"),
                source: source.clone(),
                reason,
            })?;
            result
                .listed
                .insert(provider, Sourced::new(list.get_ref().clone(), source));
        }
    }
    Ok(result)
}

fn check_list(accounts: &[String]) -> Result<(), String> {
    if accounts.is_empty() {
        return Err(format!(
            "lists no account; leave it out for the provider's `{DEFAULT_ACCOUNT}` account"
        ));
    }
    for (position, account) in accounts.iter().enumerate() {
        if !is_account_name(account) {
            return Err(format!(
                "{account:?} is not 1 to {MAX_ACCOUNT_BYTES} lower-case ASCII letters, digits \
                 and `-`, starting with a letter"
            ));
        }
        if accounts[..position].contains(account) {
            return Err(format!("lists {account:?} twice"));
        }
    }
    Ok(())
}

fn is_account_name(name: &str) -> bool {
    name.len() <= MAX_ACCOUNT_BYTES
        && name.starts_with(|first: char| first.is_ascii_lowercase())
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}
