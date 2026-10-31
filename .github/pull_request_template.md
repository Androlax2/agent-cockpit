<!-- markdownlint-disable-file MD041 -- the pull request title is the heading. -->
<!-- One issue, one pull request. Branch: greenfield/<issue-number>-<slug>. See CONTRIBUTING.md.
     For an apps/ stabilization fix, describe the fix under Scope and write "n/a" where a greenfield field does not apply. -->

Closes #

## Owns

<!-- The issue's Owns line: what this change is responsible for, in one sentence. -->

## Read

<!-- The reports from the issue's Read line that you loaded. -->

## Scope

<!-- What changed, by file or crate. Name anything in the issue's scope that you left out, and why. -->

## Acceptance

<!-- Each acceptance item from the issue, with the test name or recorded run that proves it. -->

- [ ]

## Commands run

<!-- Exact commands and their actual outcomes, failures included. A skipped check is not a pass. -->

```text
make greenfield-ci  # exit 0
```

## Checklist

- [ ] Nothing under `apps/` changed (or this is a stabilization fix in its own pull request)
- [ ] Coverage map rows this proves are updated with the test name
- [ ] Non-obvious decisions are recorded in `docs/DECISIONS.md`
- [ ] `validate` claims stay open unless measured here
