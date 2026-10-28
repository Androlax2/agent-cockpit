# Scoped Docker Build Trust Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Sail through the isolated daemon with verified HTTPS and no persistent interception trust in its image.

**Architecture:** An explicit Python CLI generates temporary Dockerfile/Compose overlays, adds BuildKit secrets to each RUN, and invokes Docker against the existing project daemon. The launcher distributes the helper with its existing read-only shims and supplies its CA bundle. Source build files are never patched.

**Tech Stack:** Python standard library, Docker BuildKit secrets, Docker Compose JSON configuration, existing Rust launcher tests.

**Spec:** `docs/superpowers/specs/2026-10-26-scoped-docker-build-trust.md`

## Global Constraints

- No system-wide CA installation, disabled TLS verification, network-policy bypass, or CA in exported image layers.
- Use only the existing project's isolated Docker socket and existing proxy configuration.
- No dependency-manifest changes or new dependencies.
- Never modify vendor Dockerfiles or source Compose files.
- Host-side launcher/helper changes must be identified in the handoff and approved through `ask-to-run` when deployed.

## Review Focus

- Dockerfile syntax: multiline RUN, exec RUN, multiple stages, and shell semantics must survive; unsupported syntax must fail before building.
- Compose resolution: relative paths, build arguments, service selection, and existing secrets must retain their meaning.
- Bootstrap: apt upgrades ca-certificates during Sail's long RUN; trust must work before and after that upgrade.
- Cleanup: failed builds and interruptions must not leave generated configuration in the project or build context.
- Isolation: inherited Docker contexts, remote builders, missing CA inputs, and caller flags must not redirect the build to another daemon.

## Task 1: Shared helper and generated build inputs

**Files:**

- Create `config/bin/.local/bin/agent-docker-build`.
- Create `tools/tests/test_agent_docker_build.py`.

**Interfaces:**

- `agent-docker-build build [-f DOCKERFILE] [-t TAG] [--build-arg KEY=VALUE] CONTEXT`.
- `agent-docker-build compose [-f COMPOSE_FILE ...] SERVICE` builds one selected service.
- Consumes launcher-provided `DOCKER_HOST`, `DOCKER_CONFIG`, and `AGENT_DOCKER_CA_BUNDLE`.
- Uses `docker compose config --format json` for resolved build settings; no YAML parser dependency.
- Produces no permanent configuration. Returns Docker's failure status and streams its output.

- [ ] Establish an isolated feature checkout from current `origin/main` without rebasing or changing the unrelated existing feature branch. Preserve user files and use host approval if a parallel clone is needed.
- [ ] Add CLI-level tests with a fake Docker executable recording arguments and reading generated inputs while they exist. Assert original input bytes remain unchanged and temporary files disappear after success and failure.
- [ ] Test local builds: the existing project socket is selected explicitly; each RUN gets required CA/config secret mounts; no trust-related ENV/COPY instruction is added; build arguments and tags survive.
- [ ] Test continued shell RUN, exec RUN, multiple stages, comments, spaces in paths, existing RUN mounts, and explicit refusal of unsupported syntax (including heredocs or custom SHELL until deliberately supported).
- [ ] Test Compose: selected service only, resolved relative context/Dockerfile paths, build arguments, image name, existing secrets, and no accidental build of unrelated services. Reject unsupported remote/inline build inputs or builder selection before invoking a build.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_agent_docker_build.py' -v`; confirm the new tests fail before implementation.
- [ ] Implement the CLI using argparse, subprocess argument arrays, and TemporaryDirectory outside the build context. Validate the project socket and CA bundle, pin the Docker endpoint and local builder, and reject conflicting overrides.
- [ ] Mount the bundle at a private build-only path. Supply CURL_CA_BUNDLE, SSL_CERT_FILE, GIT_SSL_CAINFO, NODE_EXTRA_CA_CERTS, and REQUESTS_CA_BUNDLE only to RUN processes. Supply APT_CONFIG and PHP_INI_SCAN_DIR through secret-mounted configuration for apt and PHP/Composer. Keep the image's original trust files writable for package upgrades.
- [ ] Preserve supported RUN semantics and existing flags. Fail with a filename/line diagnostic for syntax that cannot be transformed safely. Refuse collisions with the helper's secret IDs and mount targets.
- [ ] Generate a minimal Compose override using resolved absolute paths and build secrets; preserve existing service settings by merging with original Compose inputs. Never serialize the entire resolved runtime environment into the temporary override.
- [ ] Repeat the focused tests; require PASS. Commit the helper and its tests on the feature branch.

## Task 2: Launcher distribution and usage

**Files:**

- Modify `config/bin/.local/bin/greywall-agents/launcher-common.sh`.
- Modify `apps/cockpit/tests/greywall_launcher.rs`.
- Create `docs/agent-docker-build.md`.

**Interfaces:**

- Copy the helper into the existing read-only shim directory using the launcher's checked atomic-copy pattern.
- Export `AGENT_DOCKER_CA_BUNDLE` for sessions with a project Docker socket.
- Preserve the existing proxy identity, Docker config, and socket selection.

- [ ] Add launcher tests for helper availability and bundle wiring with project and parallel-clone sockets, plus explicit failure when helper installation fails.
- [ ] Run the focused launcher tests before implementation and confirm the new regression fails.
- [ ] Implement the launcher wiring; document local and Compose invocation, supported syntax, failure messages, CA rotation, and build-only trust scope.
- [ ] Run the helper tests, launcher tests, shell syntax checks, and `git diff --check`; then run the repository-required `make test` with any required build approval.
- [ ] Commit verified wiring and documentation, including `Todo: efff51d1` only when the full task's acceptance checks are satisfied.

## Task 3: Live verification, deployment, and Stadeo handoff

**Files:**

- Create `tools/tests/verify_agent_docker_build.py`, an opt-in image-save inspector accepting an image archive and CA certificate path.

**Interfaces:**

- Inspector reads Docker-save image config and every layer tar; rejects the CA fingerprint/PEM content and helper trust overrides persisted in image configuration or files.

- [ ] Add fixture tests proving the inspector catches a CA added in an earlier layer then deleted, plus persistent trust environment/configuration, and accepts a clean image. Run them before implementing the inspector, then require PASS.
- [ ] Through `ask-to-run`, inspect Stadeo's actual Sail Dockerfile and Compose service configuration, and run the helper against Stadeo's isolated daemon with plain progress output. Respect any newly encountered Greyproxy policy decision; report blocked hosts without retries around policy.
- [ ] Verify curl and apt HTTPS, PHP/Composer, and Node/npm during the representative Sail build. Export the image and run the inspector against every layer; do not accept only a final-rootfs inspection.
- [ ] Review the final diff and report exact test outcomes and diff stat. Commit only requested files; export feature commits with `git format-patch origin/main -o .outgoing`.
- [ ] Use `ask-to-run` for `cockpit apply <clone-root>` and `make -C ~/agent-cockpit test`. Identify the new host-executed helper and launcher changes in the approval explanation.
- [ ] Check `cockpit-todo list`; complete `efff51d1` with the commit, deployment, live verification results, and the exact Stadeo command in its summary. Do not mark unrelated blockers done without verification.
