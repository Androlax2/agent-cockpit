# Quota windows are stored per provider account and alert once per threshold and window

Issue #58. Decided by the worker, following report 18 and the owner's note that one provider
subscription may have two accounts (2026-11-03).

- **Adapters build typed facts; `cockpit-usage` never parses.** `cockpit-provider-api` holds
  `QuotaReading` and `QuotaWindow` (plan, limit key, scope, length, used share, reset time,
  the provider's block), validated where they are made. Each adapter turns its harness's raw
  data into one; `cockpit-usage` imports the contract and the database only (rule `usage ->
  provider-api/database only`).
- **Keyed by provider and the owner's account name.** `AccountName` is the owner's word for an
  account, never the provider's account identifier, so two accounts on one subscription keep
  their own windows and account routing (#59) can pick between them.
- **Only a current reading has room.** A reading older than the policy's freshness, a failed
  refresh after it or a reset time already past make a window `stale`; the provider's block
  makes it `blocked` whatever the percentages say. A late reading older than the recorded one
  is refused instead of overwriting it.
- **An alert fires once per threshold and window instance.** The instance is the reset time
  to the nearest minute (the legacy cockpit's fraction-of-a-second jitter made every fetch a
  new window); fired thresholds are stored with the reading, in the same transaction, so a
  restart does not repeat them. A jump past several thresholds fires the highest only. A
  window without a reset time re-arms once a reading shows it below.
- **Thresholds and freshness are the owner's.** `QuotaPolicy` takes them; the defaults are
  report 18's 80 % and 95 % and 600 s. The daemon reads them from configuration with the
  refresh worker.
- **Rejected:** a used share rounded to whole percent (Claude reports fractions, and #412's
  pacing needs them); firing every threshold a jump passes (three banners for one reading);
  keying alerts by the window's reset alone with no stored state (a restart repeats them).

Evidence: `crates/cockpit-usage/tests/windows.rs`, `crates/cockpit-usage/tests/alerts.rs`,
`crates/cockpit-provider-api/tests/quota.rs`, and the rule in
`crates/cockpit-workspace-rules/tests/crate_graph.rs`.
