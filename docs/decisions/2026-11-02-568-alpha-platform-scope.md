# Alpha 1 supports Linux and WSL2 through archive installation

Issue #568. Decided by Theo on 2026-11-02 (Option A).

- **Alpha 1 supports Linux and WSL2 only.** The release route is archive installation. Linux/WSL2 installation, sandbox enforcement, clean-home onboarding with and without Docker, provider/context criteria and human acceptance remain required by #52/#137.
- **macOS delivery, Homebrew distribution and macOS acceptance are deferred beyond Alpha 1, not passed.** #568 tracks restoration, including the unresolved macOS public-CA isolation work (#527) and Homebrew delivery (#564). No experimental macOS download or restoration milestone is promised. Support returns only after the issue's evidence requirements and an explicit owner decision naming a release.
- **Other packaging is later work.** #562/#563/#565 move to Alpha 2; they do not block Alpha 1's archive route. This does not waive archive-install instructions, checksums, sandbox safety or live acceptance.
- **Historical evidence remains historical.** Existing macOS implementation, test successes and failures are retained. This release-scope decision does not remove ordinary repository-required development checks or authorize a release, publication, pin change or installation.
- **Rejected:** keeping macOS/Homebrew as Alpha 1 promises, because Theo chose a smaller Linux/WSL2 release while the macOS delivery and acceptance gaps remain unresolved. Treating deferral as successful acceptance would conceal those gaps.

Current promises live in [README](../../README.md#platforms-and-prerequisites) and [Releasing](../RELEASING.md#channels-and-what-each-promises); the [Alpha guide](../acceptance/m3.md) and [gate](../gates/m3-acceptance.md) retain the in-scope acceptance requirements. This decision alone closes none of #568, #137 or #52.
