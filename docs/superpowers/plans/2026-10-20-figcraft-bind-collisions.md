# FigCraft Bind Collision Implementation Plan

> **For agentic workers:** Use `executing-plans` to implement this small fix inline.

**Goal:** Avoid selecting ports which refuse connections but cannot be bound, without increasing startup timeouts or removing concurrent tests.

**Architecture:** Exclude the kernel's ephemeral source-port range from new allocations, then probe candidate ports with a real IPv4 bind before reserving them. Preserve existing checkout reservations and fail explicitly if their ports cannot be bound. Keep startup output in the instance's private configuration directory. This preflight cannot eliminate another unrelated process explicitly binding between the probe and FigCraft startup; atomic socket transfer would require upstream support.

**Tech Stack:** Bash, Python 3 standard-library sockets (Python 3 is already used by installed dotfiles tools), Rust integration tests.

**Spec:** Option A approved in this conversation: fix host-side port allocation and add a deterministic regression, retaining reservations, confinement, concurrent coverage and existing timeouts.

## Global Constraints

- No dependency manifest changes or new packages.
- Keep `CRAFT_OPEN_WAIT_S=2` in the test fixture.
- Never recycle existing reservations or adopt an unverified listener.
- Leave the separately proposed stale-forward fixture race out of scope.
- `craft-open` runs on the host; its change requires the normal reviewed dotfiles apply.

## Review Focus

- A TIME_WAIT socket is not a listener but can still prevent a bind.
- An explicit or permanently reserved unbindable port must not silently change.
- Existing listener and stale-forward rejection must remain intact.
- An unavailable bind-probe interpreter must fail clearly rather than silently accepting ports.
- Startup errors must remain inspectable after the readiness timeout.

## Task 1: Reject unbindable ports and preserve startup diagnostics

**Files:** `config/bin/.local/bin/craft-open`, `apps/cockpit/tests/craft_open.rs`.

**Interfaces:** Keep the existing CLI, reservation storage and environment variables. A new `can_bind` shell helper probes the current `PORT` using Python's `socket.bind`; false means the port cannot be used. A private `startup.log` records the launched application's output.

- [x] Add a real TIME_WAIT fixture using a loopback TCP client which closes first; verify the candidate refuses connections and cannot be rebound. Feed that candidate first through the existing `shuf` boundary, then a usable candidate; assert startup succeeds without reserving the unusable port.
- [x] Add regression coverage for an explicit and an existing reserved TIME_WAIT port: exit 3, no new window or session restart, existing reservation unchanged.
- [x] Add startup failure coverage: a fixture exits with an error, the invocation fails, and the reported private log contains that error.
- [x] Run the new tests against unchanged production code and confirm the expected failures.
- [x] Add bind preflight, a clear missing-Python error, and private startup output capture to `craft-open`.
- [x] Run all `craft_open` tests and repeat concurrent test binaries. Run `make test`, reporting unrelated failures by name rather than changing them.
- [ ] Review the diff, commit only these files and this plan, export patches, and request the host-reviewed apply followed by `make -C ~/dotfiles cockpit test` and cockpit reload.

## Verification record

- RED: `cargo test --offline --test craft_open`: `18 passed; 4 failed`; all four new regressions failed at their intended assertions.
- GREEN: same command: `22 passed; 0 failed`.
- `bash -n config/bin/.local/bin/craft-open` and `git diff --check`: exit 0.
- Fresh-context code review: no findings.
- The optional skill workspace helper is missing from this installation (exit 127), reported through `claude-todo blocked`; it was not available for recording execution metadata.
- First stress run after bind preflight: `99 passed, 1 failed`; `allocation_skips_ports_that_refuse_connections_but_cannot_be_bound` found a candidate which became unbindable between the allocation check and startup check. Nothing applied.
- Task 1 Ruling: also exclude the kernel's actual ephemeral source-port range from new allocations — automatic outgoing connections can claim a checked candidate before FigCraft binds it — this reduces the available candidate pool; explicit ports and permanent reservations remain supported unchanged.
- Added `new_ports_stay_out_of_the_kernels_ephemeral_range`; RED: `0 passed; 1 failed`, because the allocator selected the forced ephemeral candidate.
- First full `make test` run passed, before the ephemeral-range regression was added; rerun required after the follow-up.
- Follow-up GREEN: `cargo test --offline --test craft_open`: `23 passed; 0 failed`.
- Follow-up stress: `100 parallel-suite repetitions: 100 passed, 0 failed; 56.7s` (four concurrent test binaries, retaining each binary's normal parallel tests).
- Final `make test`: exit 0; all 61 test groups passed, totaling `1227 passed; 0 failed; 0 ignored`.
- Existing compiler warnings were retained: `unused_mut` in `cockpit/src/tests.rs:260`; `unused_must_use` in `desktop/src/tests.rs:1551,1556`. No failing tests were disabled.
- Follow-up range exclusion self-reviewed against the existing-reservation and explicit-port branches; it applies only to new automatic allocations.
