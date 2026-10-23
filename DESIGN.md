---
version: alpha
name: Agent Cockpit terminal cost reporting
description: Approved Option A handoff for the existing Rust terminal cockpit.
---

## Overview

The terminal cockpit separates API-credit usage estimates, subscription API-equivalent value, and usage whose billing mode cannot be verified. Implement the approved Option A without redesigning the cockpit or implying that token-based estimates are a provider bill.

## Colors

Reuse the existing Rust terminal palette and style helpers in `apps/cockpit/src/app.rs`: `MAGENTA` for monetary amounts, `CYAN` for active time, `FG` for labels, `FG_DARK` for secondary text, and `YELLOW` for existing error messages. Keep quota colors unchanged. Category meaning must remain explicit in text, not color alone.

Do not translate ANSI palette indices into invented RGB tokens or introduce a separate CSS token system.

## Typography

Use the terminal's existing font, cell measurements, `width_of`, money formatter, and bold/dim styles. FigCraft font sizes and pixel dimensions illustrate the layout; they are not new terminal typography tokens.

## Layout

Keep the current single-line header, quota priority, clock placement, Status navigation, and spending-section placement before service health. Preserve the existing `TODAY`, `WEEK`, and `MONTH` periods and the `Active time` row's calculation and conditional visibility.

Compute column widths from the actual category labels and formatted amounts. Retain the existing stacked-period fallback when columns do not fit. Wrap explanations and long labels rather than clipping their billing meaning. Hide the header monetary fragment when it cannot fit intact; do not displace quotas to show it.

## Components

### Approved FigCraft handoff

Use `design-plans/cost-reporting-split.figcraft`. Theo approved Option A; frames `2` (header and wide Status table) and `4` (narrow stacked fallback) are marked `readyForDev`. Frame `5` contains the scope and edge-state notes. Frame `3` is the rejected combined-value header and must not be implemented.

Reuse the existing Rust header and Status renderers in `apps/cockpit/src/app.rs`. No new component framework is required.

### Header

Show `API ≈ <amount> today`, for example `API ≈ $12.00 today`, using only usage attributed to API credits by verified billing evidence. Never add subscription or unknown-billing values to this amount.

For historical-only or otherwise unverified billing, omit the header dollar fragment rather than presenting the old combined value or implying a verified zero. Update the existing header help entry to describe the API-credit estimate and direct users to Status for the other categories.

### Status spending

Use this exact section caption:

`SPENDING · token-based estimates, not your bill`

Replace the combined Cost row with these mutually exclusive rows, in this order:

- `API credits (est.)`
- `Subscription (API value)`
- `Unknown billing (API value)`

Keep all three rows when recorded usage exists, including zero-valued categories. Amounts remain token-based estimates using the existing money formatter; an API-credit estimate is not an actual credit balance or provider charge.

Use these explanations below the amounts, wrapping to available space:

- `Subscription = API-price value, not a subscription charge.`
- `Unknown = billing mode unverified; includes historical records.`

Rename `All-time cost` to `All-time API value`, retaining its aggregate scope across all three categories and showing `≈` before its amount. It is not an all-time API-credit spend total.

### Evidence and edge states

Classify billing from verified record-specific evidence. Historical or unverifiable records stay in Unknown; do not infer their billing mode from the current session, provider, or subscription settings. A record can have a known token-price estimate and still have unknown billing.

Distinguish zero recorded estimated value from missing data. With no recorded usage, state `No recorded usage` instead of fabricating a verified zero bill. Loading and archive failures must not become dollar-zero values; preserve explicit error handling and visible archive errors.

If some records cannot be priced, show a visible incomplete-estimate warning, with a count when available. Unknown token pricing and unknown billing mode are different conditions; neither may silently disappear into an apparently complete total.

### Verification handoff

Extend the existing Rust `Fixture`, `texts`, and `render` tests in `apps/cockpit/src/tests.rs`, including the spending-placement and archive-error tests. Reuse `apps/cockpit/tests/costs.rs` for billing aggregation fixtures rather than adding browser or Storybook tests.

Cover mixed billing, historical-only billing, zero-valued categories, no recorded usage, unpriced records, archive errors, large formatted amounts, and wide/narrow layouts. Assert the header excludes subscription and unknown values, the exact category labels remain readable, amounts do not overlap, and Active time and service-health ordering are preserved.

## Do's and Don'ts

- Keep implementation limited to the approved cost-reporting split and its existing terminal rendering/test patterns.
- Do not call estimates actual spend, paid charges, invoices, or provider balances.
- Do not add browser pages, Storybook, dependencies, invoice integration, or new provider-billing fetches.
- Do not modify unrelated product UI or replace the existing terminal design system.
