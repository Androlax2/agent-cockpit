# Greenfield engine pins name the published fork releases

Issue #476, preparing #470. Decided by Theo in #470 and its release comment.

- **The release record owns the pins.** `crates/cockpit-sandbox/engine-pins.json` copies the
  tags, commits, exact per-target URLs and SHA-256 values published in
  [the coordinator's comment](https://github.com/Androlax2/agent-cockpit/issues/470#issuecomment-6564069087).
  `greywall::the_pins_are_the_commits_the_release_manifest_names` checks the Rust constants
  against it. The installer consuming the assets remains #470.
- **Move both engines together.** Greywall `v0.3.7-cockpit.1` lists `--mount`; greyproxy
  `v0.4.5-cockpit.1` is the accompanying release. Any other commit is still refused.
- **Legacy builds coexist until cutover.** Greenfield no longer treats
  `packages/*/build.sh` as its pin authority. Those scripts stay unchanged for existing
  sessions; their removal from Theo's machine belongs to #66.
