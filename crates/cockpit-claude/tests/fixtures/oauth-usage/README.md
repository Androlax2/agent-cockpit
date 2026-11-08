# OAuth usage answers

Made-up answers of `https://api.anthropic.com/api/oauth/usage`, in the two shapes the owner's
`config/waybar/.config/waybar/scripts/claude-usage.sh` and the old cockpit's
`apps/crates/cockpit-core/src/usage.rs` read: the `limits` list, and the older flat keys.

`limits-spent.json` (#249) is the `limits` answer with the five-hour window at its whole
limit, as the endpoint reports it after Claude Code stops a turn with `rate_limit` for a spent
window; `limits.json` has headroom in every window.
