# Development

## Workspace and prerequisites

The Rust workspace is [apps/Cargo.toml](../apps/Cargo.toml), with `cockpit`, `osd` and
`crates/cockpit-daemon` members. This project targets Linux and integrates with GTK,
terminals and host tools.

The Rust toolchain is pinned in [rust-toolchain.toml](../rust-toolchain.toml). Distro
Rust installations do not read that file; use the same version when running checks.
Baseline environment inventory: Rust/Cargo 1.99.1, GTK4 4.24.1, gtk4-layer-shell 1.3.0,
Node 26.12.0 and fish 4.9.3. These are observed versions, not a tested minimum-version
promise. Rust compilation needs the system GTK libraries and `pkg-config`; full tests
also use Python, `cargo-nextest`, shell utilities, Node, fish and Lua. Browser checks
have separate Playwright/Chromium prerequisites described in the root README.

An unavailable prerequisite is a reported verification gap. Check before installing;
agent sessions need explicit installation approval.

## Existing commands

Run from the repository root:

```sh
make build
make test
make fmt-check clippy doc
make lint-python
make lint-shell
make lint-js
make lint-markdown
make lint-lua
make deny
make check-links
make typos
make ci
```

`make build` builds release binaries through the low-priority `agent-build` wrapper.
`make test` takes the shared full-suite lock, runs the Python suite, builds the OSD
classifier, then runs release Rust tests with nextest. Use that entry point for a full
run; do not bypass the lock to compete with another clone's suite.

Rust checks cover every workspace member and target, with warnings treated as errors.
The language targets use [tools/quality.py](../tools/quality.py) to select tracked files,
including extensionless commands by shebang. Stage newly created source before the
final gate. Symlinks are excluded. `python3 tools/quality.py python --list` shows the
selection as JSON without invoking a linter.

Python uses the pinned Ruff version in [ruff.toml](../ruff.toml). The other language
tools currently used are ShellCheck 0.11.0, shfmt 3.14.1, Biome 2.6.3 and
markdownlint-cli2 0.24.0, plus the installed fish/fish_indent and Lua compiler. Shell
fragments declare their dialect with `# shellcheck shell=sh` or `shell=bash`; otherwise
fragments default to POSIX shell. Fish uses its own syntax and formatting checks.
Lua checks parse the two desktop configuration files without running them; their
runtime integration is covered separately by Rust fixtures.

JavaScript and TypeScript use [biome.jsonc](../biome.jsonc). Browser assets retain
their classic-script strict-mode directives; browser and agent-integration globals
are checked in separate overrides. Biome's recommended warnings stay visible at
their default severity. The check does not automatically rewrite code.

Markdown follows [.markdownlint-cli2.jsonc](../.markdownlint-cli2.jsonc): semantic
prose lines and wide tables are allowed, while headings, lists and code fences are
checked. `make check-links` uses lychee 0.24.2 in offline mode to check local files
and heading anchors; it makes no network requests. `make typos` uses typos 1.51.1
and [.config/typos.toml](../.config/typos.toml), selecting only tracked regular files.
Binary design archives/previews and documented encoded fixtures are excluded from
spelling, not source code. The quality-runner tests require these installed tools,
including Biome, lychee, typos, ShellCheck and fish.

During iteration, select the actual test file or behavior you changed:

```sh
python3 -m unittest discover -s tools/tests -p 'test_agent_build.py'
cargo nextest run --release --manifest-path apps/Cargo.toml --test team_protocol
cargo check --manifest-path apps/Cargo.toml -p cockpit --locked
```

These commands are checked against the current Makefile, project instructions and test
paths. Execution results are tracked in the engineering baseline; this guide does not
assert that the baseline passes.

## Verification policy

Use focused tests while editing and affected checks after the source settles. A change
to a shared contract also needs its consumers checked. Preserve failure-path tests;
fix the cause rather than weakening assertions or hiding warnings in a baseline.

Dependency policy lives in [.cargo/deny.toml](../.cargo/deny.toml). `make deny` rejects
advisories, yanked crates, wildcard requirements, unknown sources and licenses outside
its reviewed allowlist. Only unpublished workspace crates skip the license check;
the policy assigns no license to Cockpit itself. Duplicate dependency versions remain
visible warnings, matching the reference policy.

`make ci` runs all checks and the shared full regression suite serially. JavaScript's
default warnings remain visible. The [CI workflow](../.github/workflows/ci.yml) calls this same target on an
Ubuntu 26.04 runner with pinned Rust, Node and checker versions, read-only repository
permissions and immutable action revisions. Linux runtime libraries, fish and Lua come
from that Ubuntu release. This workflow has not yet been run on GitHub; activation
requires the normal separately approved push. Local fixture tests do not establish
live desktop or browser integration coverage.
Formatting checks should be read-only; formatting writes remain deliberate commands.
Tool versions and source selection must match locally and in CI, including extensionless
scripts and TypeScript integrations.

## Applying changes

Follow [CLAUDE.md](../CLAUDE.md) for authorization and the reviewed host-application
workflow. Building a clone does not deploy it. A successful unit test does not authorize
restarting host services, exporting credentials or changing approval gates.

See [Architecture](ARCHITECTURE.md) before changing crate, binary or fixture locations.
