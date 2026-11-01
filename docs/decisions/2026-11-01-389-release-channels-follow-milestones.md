# Release channels open at the release milestones' checklists, and no stable tag exists yet

Issue #389. Milestones as releases decided by Theo on 2026-11-01; the tooling choices below
by the worker, with the reasons given.

- **Alphas open when the Alpha 1 checklist (#52) closes; betas when the Beta 1 checklist
  (#57) closes.** Beta 1 is the first release that promises upgrades keep data, which is the
  beta promise; the cutover (#66, Beta 2) no longer gates the channel.
- **`tools/release.py` refuses every stable `vX.Y.Z` tag, naming #389.** No stable release
  and no 1.0 are planned until Theo decides. The stable-only checks (workspace version equal
  to the tag, above the previous stable, stable notes pattern, a stable base, the `stable`
  gate channel) were removed rather than left unreachable behind the refusal; git history
  holds them for when Theo decides.
- **A stable tag left on `main` is ignored: the base is always the bootstrap commit.** A
  refused tag stays in the repository; counting it as the base, as the old rule did ("newest
  stable, published or not"), would block every later pre-release until someone deleted it.
  `dry-run-version` lost its `--main` flag, which only served that lookup.
- **Both workflows read the issue numbers `release.py` names**; a test fails when they
  disagree, so the next renumbering cannot miss one of the three places.
- **The flags name the checklists (`--alpha-checklist`, `--beta-checklist`), not milestone
  numbers.** The old `--m3-gate`/`--m5-gate` names would mislead once M-numbers are gone.
- **A beta tag while #57 is open is refused with its own reason**, before the alpha
  checklist is consulted, so the message names the rule the tag broke.
- **Rejected:** keeping the stable rows in `docs/RELEASING.md` as "later": a promise row
  for a channel nobody may cut reads as a plan.
