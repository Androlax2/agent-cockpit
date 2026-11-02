# Proposal credential isolation (#382)

Approved Option A, implemented inline by the original #382 owner. Base:
`6a35822ab452fe42aa092a109f58b30f181e7975`. No design approval is outstanding.

## 1. Independent bootstrap

- Add `tools/setup_proposal_app.py`: private manifest registration, selected-repository
  verification, separate owner-only recovery state, Git-only deploy key, and
  `release-proposal` environment storage. Reuse the release bootstrap's validated
  storage, transport and callback primitives without changing its defaults.
- First add isolated entry-point tests in `tools/tests/test_setup_proposal_app.py`:
  contents/read only, wrong owner/scope, callback replay, environment refs, secret
  stdin, partial writes and no automatic retry of uncertain mutations.
- Verify with both setup test files and changed-file Ruff. Record actual installed
  `code-review medium` and independent OpenAI review before host execution.

## 2. Approved host registration and identity

- Ask browser readiness only after the bootstrap is tested/reviewed. Use exact
  `ask-to-run` and supported `ask-to-guide`; no browser opens automatically.
- Record actual App ID/slug/bot and deploy-key ID from verified responses. Keep
  credentials outside Git, never in command arguments or output.
- Provision/reconcile approved environments and rules using the supplied eight
  payloads. Disable release admin bypass through the owner UI, then read it back.
  Migrate the existing release key through a separate reviewed operation.

## 3. Coordinated source integration

- Release owner `ses_e63e1fbcbffeuwhWvympF3Xe9i` owns only the `release.yml`
  draft/publish environment patch. Agree its exact hash and integration order.
  #523 head `17440c23504e9b3713cc8617260d614b4e9846e1` must merge first;
  its candidate run is already triggered. Coordinate #560 overlap afterward.
- Update `tools/release_pr.py` and publication/conformance fixtures: fixed-repository
  SSH push only, strict host checking, temporary-key cleanup, unchanged immutable
  proposal validation, one verified new bot. Read/API credentials remain read/PR-only.
- Route `release-plz.yml` through `release-proposal`; route prerelease tagging and
  agreed release draft/publish jobs through `release`. Preserve all nine publication
  dependencies, main/ref guards, #561 draft guards and automatic proposal generation.
- Coordinate `docs/RELEASING.md`; update `docs/security/repository.md` with measured
  evidence rather than treating prepared payloads as applied controls.

## 4. Review, delivery and acceptance

- Run focused release/setup/conformance and workflow checks, changed-file lint and
  docs checks. Run the installed eight-angle medium review and fresh independent
  OpenAI full-delta review; resolve findings before protected-workflow publication.
- Source completion requires the reviewed PR to merge. No main writes, rebase,
  force push, tag/release creation or branch deletion are authorized.
- Scanner preparation remains retained scope; exact source activation, scanner
  runs, proposal transition cleanup and live negative tests need their own approval.
  #382 remains open until separately measured external/live acceptance is satisfied.
