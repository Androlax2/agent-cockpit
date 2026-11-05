# Which folders and caches outside the checkout a session may read or write

Issue [#859](https://github.com/Androlax2/agent-cockpit/issues/859), a decision issue with no
product code. Covers [#716](https://github.com/Androlax2/agent-cockpit/issues/716) (shared
build cache across clones) and [#406](https://github.com/Androlax2/agent-cockpit/issues/406)
(storage hygiene). Background: the [native sandbox report](2026-11-05-546-own-sandbox-and-traffic-control.md)
and its decisions D1 to D6. **Research and proposal, 2026-11-05; Theo's decisions are
recorded under [Decisions](#decisions) as they are made.** Merging this document accepts
research delivery only: it authorizes no product code, dependency or host setting, and it
edits no issue; the changes proposed under [Changes per issue](#changes-per-issue) are applied
by the owner's side.

## The question

The native sandbox gives a session its checkout, its session folder and its temp folder, and
nothing else of the owner's home ([#829](https://github.com/Androlax2/agent-cockpit/issues/829),
report 546 threat model). A build needs more: compilers, a package download cache, sometimes a
compiled-output cache. For each kind of folder or cache outside the checkout, which of
these does a session get: **read-only, a folder of its own, shared and writable, or nothing**?
And what do #829's mount view and Landlock rules then contain?

## What exists today

- The profile asks Greywall for the checkout, session folder and temp folder read-write
  (`--allow-path`, `crates/cockpit-sandbox/src/greywall.rs:410`) and `defaultDenyRead`
  (`:453`). Everything else the agent can read comes from Greywall's own defaults and its
  provider profile (`--auto-profile --profile <provider>`, `:335`).
- With `defaultDenyRead` Greywall mounts an empty root, then binds the system folders and
  the checkout, and **binds `~/.cache`, `~/.npm`, `~/.cargo`, `~/.rustup`, `~/.local` and
  `~/.config` read-only into every session**, with the version-manager folders (`~/.nvm`,
  `~/.pyenv`, `~/.volta`, `~/go/bin`) (`buildDenyByDefaultMounts`,
  `internal/sandbox/linux.go:939-1070`, and `GetDefaultReadablePaths`, `dangerous.go:87`, at
  the pinned commit `a9ccc58`). Only Torlek's `denyRead` list masks parts of that
  (`BASELINE_DENIED_READ`, `crates/cockpit-sandbox/src/profile.rs:17`): it names `~/.aws`,
  `~/.ssh/id_*`, `~/.netrc` and the like, **not** `~/.cargo/credentials.toml`, other tools'
  tokens under `~/.config`, or the 9.0 GB `~/.cache/sccache`. Read from source; not run,
  because the pinned Greywall talks to the live Greyproxy (report 546).
- Writes outside the checkout, session folder and temp folder fail today. Greywall's
  toolchain overlays that grant read-write to `~/.cargo`, `~/.cache/sccache`, `~/.npm` and
  more (`internal/profiles/toolchains/{rust,node,python}.go`) attach to profiles named after
  the tool, such as `cargo`. Torlek requests the provider's profile (`--profile claude`,
  `greywall.rs:335`), so they do not apply. The consequence: a build that needs a package
  its `~/.cargo/registry` lacks cannot store it. I measured the same failure with a
  read-only `CARGO_HOME` below.
- The daemon runs checks on the host, as the owner, with the owner's `~/.cargo` and sccache
  ([#352](https://github.com/Androlax2/agent-cockpit/issues/352): "the daemon runs only
  `native` checks, on the host"). Whatever a session writes into a cache the host also reads is
  executed later with the owner's rights.
- Session temp folders are created at launch, removed at session end and measured by
  `torlek storage` (`crates/cockpit-sessions/src/lifecycle.rs:2486`). Anything a session keeps
  there is cleaned and reported without new code.
- #716 was decided on 2026-11-04: no code, wait for Cargo's native shared cache. Its
  measurements stand. A shared compiled cache saved no time on this workspace (91 s with 89%
  hits against 74 s cold), and the sccache variant already carries "a per-project cache that
  every sandboxed session can write to, so one session could tamper with compiled
  dependencies another session reuses". #406's build-folder parts all merged; its live check
  moves to alpha 7. Both comments say the sandbox decision comes first, here.

## Threats, in the order they matter

1. **Sandbox to host.** Closed today by read-only binds, and it reopens with any writable
   cache. A cache or tool folder the session writes and a host-side process later reads: compiled outputs (`sccache`, `build.rs` and proc-macro results), `~/.cargo/bin`,
   `~/.cargo/config.toml` (`rustc-wrapper`, aliases), `~/.npmrc` (registry redirect,
   `script-shell`). The owner's `make test` or a #352 check then runs agent-chosen code
   outside the sandbox.
2. **Session to session.** The same, with the reader another session of the same or another
   project (#716's tampering finding).
3. **Reading other projects.** The owner's caches hold what every project built: private
   registry crates, authenticated Git dependencies, compiled objects of private code. This
   host: `~/.cargo/registry` 2.5 GB, `~/.cache/sccache` 9.0 GB, `~/.cache/uv` 2.0 GB,
   `~/go/pkg/mod` 1.1 GB. The threat table in report 546 says a session must not read other
   projects.
4. **Credentials beside caches.** `~/.cargo/credentials.toml`, `~/.npmrc` auth tokens,
   `~/.pypirc`, `~/.config/pip`: in the same folders Greywall binds read-only today, so
   threats 3 and 4 are open now, and any session can read them.
5. **Availability.** One session fills the disk or corrupts a cache the others build from.

Not a threat this decision can remove: a session's own `target/` and `node_modules` inside
the checkout are agent-writable, and the owner running a build in that checkout on the host
executes them. That is the existing clone rule, not a cache choice.

## What was measured

Host: Arch Linux, kernel 7.2.14, bubblewrap 0.13.1, Cargo 1.99.1, sccache 0.19.0, ext4. All
runs under this clone's gitignored `target/`, deleted afterwards.

| Check | Result |
| --- | --- |
| Cold `cargo fetch --locked` of the greenfield workspace into an empty `CARGO_HOME` | 5.9 s, 485 MB |
| `cargo check -p cockpit-protocol --offline` with a **read-only** `CARGO_HOME` already holding the dependencies | Passes |
| `cargo fetch` with a read-only **empty** `CARGO_HOME` | Fails: `Permission denied (os error 13)`, exit 101 |
| bubblewrap `--overlay-src SHARED --tmp-overlay DEST`, `CARGO_HOME=DEST`, same check | Passes; a file created in `DEST` is absent from `SHARED` afterwards |
| Owner's `~/.cargo/config.toml` sets `rustc-wrapper = "sccache"`; build with `RUSTC_WRAPPER=` in the environment | The wrapper is not invoked (0 of 1 invocations) |
| Build with `SCCACHE_DIR` pointing at a read-only folder | Passes (sccache falls back to compiling) |

Not measured: overlayfs inside a user namespace on Ubuntu (AppArmor) or WSL2; npm, pip, uv
and Go behaviour with a read-only or empty cache; the download size of a project that is not
this one.

## Options

### Package and download caches (cargo registry and Git, npm, pip, uv, Go modules, Bun)

| Option | Threats left | Cost |
| --- | --- | --- |
| **A. Overlay of the owner's cache.** The owner's folder is the read-only lower layer; writes go to an upper layer in the session's temp folder (bubblewrap `--overlay-src` and `--tmp-overlay`, measured) | 1 and 2 closed (writes never reach the lower layer). 3 and 4 remain: the session reads every package any project of the owner ever fetched, and a private registry's crates with them | Warm reads. Needs overlayfs in a user namespace (host kernel and Ubuntu/WSL2 unmeasured). A bubblewrap feature beyond D3's minimal use. Needs a fallback when absent |
| **B. A folder of its own per session** (recommended). Torlek points `CARGO_HOME`, `npm_config_cache`, `PIP_CACHE_DIR`, `UV_CACHE_DIR`, `GOMODCACHE` and the rest into the session's temp folder; the owner's folders are not mounted | 1, 2, 3, 4 closed by construction. Remains: each session downloads what it needs, so the package registries become hosts on the allowlist | 5.9 s and 485 MB per session on the greenfield workspace, removed at session end; five sessions are 2.4 GB against the 15.5 GiB a clone's `target/` takes (#716). The owner's private-registry and mirror settings in `~/.cargo/config.toml` are not seen; a project states them in its checkout |
| **C. Shared, read-only, warmed by the owner.** Cargo works read-only when everything is cached (measured) | 1 and 2 closed. 3 remains | A missing package fails with `Permission denied` (measured), so any new dependency needs a trusted fetch step on the host, which Torlek would own |
| **D. Shared and writable** (what Greywall's toolchain overlays would give; not applied to Torlek launches today) | None closed. Host-side checks read what sessions wrote | None |

Today's behavior is **C with the whole folders and the credentials**: read-only, nothing can be
fetched, threats 3 and 4 open.

### Compiled-output caches (sccache, ccache, Go build cache, Turbo, mypy, ruff)

| Option | Threats left | Cost |
| --- | --- | --- |
| **A. Not mounted; the wrapper is switched off** (recommended). Torlek sets `RUSTC_WRAPPER=` (measured to beat the owner's config) and gives the others a folder of the session's own | 1 to 4 closed | The session builds without a compiled cache. #716 measured the shared cache as slower than cold on this workspace; a session's own `target/` already caches its rebuilds. Other ecosystems unmeasured |
| **B. One writable cache per project** (#716's option A) | 2 open: a compiled object one session stored is reused by the next, with no check on the contents. 1 open if the owner's host builds share it | Hits across clones, needing `SCCACHE_BASEDIRS` on the server (#716) |
| **C. Overlay of the owner's cache** | 1 and 2 closed. 3 open and larger: the owner's sccache holds compiled objects of every project, 9.0 GB here | Warm builds; the overlay costs of the package option A |

Whether a compiled cache can be protected by content addressing: not as far as measured here.
A key hashes the inputs of a compilation and the stored output is what the compiler produced
then; whether it is re-checked on read was not tested, so this report does not rely on it.

### Toolchain installs, tool configuration and the rest of the home

| Option | Threats left | Cost |
| --- | --- | --- |
| **A. Installs read-only; nothing else of the home; extra folders only by a project grant** (recommended). The mount view binds the directories holding `cargo`, `rustc`, `node`, `python`, `go` (found on the launch `PATH`, read-only), including `~/.cargo/bin` and `~/.rustup` when the owner uses them. No tool configuration or credential file is mounted: Torlek writes any needed configuration into the session folder and names it in the environment (`CARGO_HOME`, `npm_config_userconfig`, `PIP_CONFIG_FILE`). A project can ask for another folder, read-only unless it names write; the owner decides per project, as hosts do (D4, #674), revocably. Some paths can never be granted: the credential and persistence paths of `BASELINE_DENIED_READ` and `BASELINE_DENIED_WRITE` | 1 closed for tool folders, 4 closed. A toolchain the project needs but the owner has not installed fails with a clear error; `rustup` cannot install a pinned toolchain from inside | The adapter-style table of toolchain detection to write and keep (seeded from Greywall's, as #830 does for providers) |
| **B. Today's binds carried over**: `~/.cache`, `~/.npm`, `~/.cargo`, `~/.rustup`, `~/.local`, `~/.config` read-only, as Greywall mounts them | 3 and 4 open (credentials in `~/.cargo`, `~/.config`, the owner's caches). 1 closed while read-only | None; a tool that must write fails |
| **C. Mount nothing of the home**: toolchains must come from system paths or the checkout | All closed | A rustup, nvm, Volta or pyenv install in the home is unusable; fails for a common developer setup |

## Recommendation (all three accepted, see Decisions)

1. **Package caches: B**, a folder of the session's own. The cost is small against what
   a clone already spends (485 MB and 6 s, measured), and it needs no kernel feature, no
   fallback and no trusted writer. Revisit A only if a measured network or time cost appears;
   the evidence line then names the cache mode.
2. **Compiled caches: A**, not mounted, wrapper off. #716's own numbers say the shared cache
   does not pay for its threats on this project.
3. **Toolchains and the rest: A.**

The rule underneath, for #829 and every later cache: **nothing a session writes outside its
own folders is ever read by the host or by another session, and nothing of the owner's other
projects' content is mounted.** A shared cache is protected by not being shared with the
writer: no agent writes it, no host build reads what an agent wrote. The one shared thing a
session reads is the toolchain, read-only.

### What #829's mount view and Landlock contain

| Path | Mode | Why |
| --- | --- | --- |
| Checkout | read-write; `.git/hooks`, `.git/config`, `.torlek`, `.envrc`, IDE run configurations read-only | Existing baseline |
| Session folder, session temp folder (holding the session's `CARGO_HOME`, package and tool caches) | read-write | Created at launch, removed at end, counted by `torlek storage` |
| System folders, toolchain directories found on `PATH` | read-only | Programs only |
| Agent socket folder, provider folders (#830) | as declared | Unchanged |
| `~/.cargo` (except `bin`), `~/.npm`, `~/.cache/*`, `~/.config/*` of tools, `~/go`, `~/.npmrc`, `~/.pypirc` | not mounted | Threats 1 to 4 |
| Granted extra folder | read-only, or read-write if the grant says | Project grant, revocable |

Environment Torlek builds for the session (part of #829's "only the environment Torlek builds"):
`CARGO_HOME`, `RUSTC_WRAPPER=`, `SCCACHE_DIR`, `npm_config_cache`, `PIP_CACHE_DIR`,
`UV_CACHE_DIR`, `GOMODCACHE`, `GOCACHE`, `XDG_CACHE_HOME`, all under the session's temp
folder. Landlock gets the same two sets (read-write: checkout, session folders; read-only:
installs); everything absent is denied by omission.

## Decisions

Recorded as Theo answers, one at a time.

- [x] **Q1, decided 2026-11-05: B.** Package and download caches: a folder of the session's own,
  nothing of the owner's mounted. (Options: A overlay, B own folder per session, C shared
  read-only, D shared writable.)
- [x] **Q2, decided 2026-11-05: A.** Compiled-output caches: not mounted, `RUSTC_WRAPPER`
  and sccache off. (Options: A not mounted, B one writable per project, C overlay.)
- [x] **Q3, decided 2026-11-05: A.** Toolchain installs read-only; no tool configuration or
  credential file mounted; other folders only by a per-project grant the owner decides
  (read-only unless the grant says write). (Options: A, B today's read-only home binds
  carried over, C nothing of the home.)

All three follow the recommendation. The rule that falls out of them: nothing a session
writes outside its own folders is read by the host or by another session, and nothing of the
owner's other projects is mounted.

### Residual risks the decisions accept

- Package registries become hosts on the allowlist (below). A registry is a read channel the
  session can also use to leak small amounts of data in request URLs; no registry token is
  inside the sandbox, so it cannot publish.
- A session's own `target/` and `node_modules` stay agent-writable and are executed if the
  owner builds in that checkout on the host. Unchanged by this decision.
- A toolchain the project pins but the owner has not installed (a `rust-toolchain.toml` for a
  missing toolchain) fails inside the sandbox, because installs are read-only. The owner
  installs it.
- Overlay (package option A) was not rejected on security: it is the later optimization if a
  measured download cost appears, with the same read-only lower layer limited to a
  Torlek-managed per-project folder and not the owner's.

## Changes per issue

Proposed text, to be applied by the owner's side. Nothing is edited on GitHub by this work.
Each block starts with the line to add to the issue.

### #829 native Linux sandbox launcher

> Folders outside the checkout (#859, owner decisions 2026-11-05): the mount view binds
> read-only only the system folders, the toolchain directories found on the launch `PATH`
> (for example `~/.cargo/bin`, `~/.rustup`, `~/.nvm`, `~/go/bin`), the agent socket folder
> and the provider folders of #830. Nothing else of the home is mounted: not `~/.cargo`
> beyond `bin`, `~/.npm`, `~/.cache`, `~/.config`, `~/.local`, `~/.npmrc`, `~/.pypirc`. The
> session's package and tool caches live in its temp folder (removed at session end, counted by
> `torlek storage`). The environment Torlek builds sets `CARGO_HOME`, `RUSTC_WRAPPER=`,
> `SCCACHE_DIR`, `npm_config_cache`, `PIP_CACHE_DIR`, `UV_CACHE_DIR`, `GOMODCACHE`,
> `GOCACHE` and `XDG_CACHE_HOME` to folders under it, and `npm_config_userconfig` and
> `PIP_CONFIG_FILE` to files Torlek writes there. Landlock holds the same two sets.
> A project's extra folder is a project grant (D4), read-only unless it says write, never
> a path in `BASELINE_DENIED_READ` or `BASELINE_DENIED_WRITE`.

Acceptance to add: through the daemon with a fake provider, `~/.cargo/credentials.toml`,
`~/.config/*` and `~/.cache/sccache` are not visible; a second session does not see the first
one's cache; the owner's `~/.cargo` is byte-identical after a session that built a crate;
`~/.cargo/bin` cannot be written; the evidence names the cache mode (session-own).

### #830 provider hosts and paths in the provider adapters

> Also (#859): a toolchain table beside the provider adapters, seeded from Greywall's
> `internal/profiles/toolchains` at the pinned commit with attribution, declares for each
> ecosystem the install directories to find on `PATH` (read-only), the cache variables above
> and the registry hosts (for example `index.crates.io`, `static.crates.io`,
> `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org`, `proxy.golang.org`). An ecosystem
> is detected from the checkout (`Cargo.lock`, `package-lock.json`, `pyproject.toml`,
> `go.sum`). The table contains no write access to the owner's home.

### #831 per-session egress proxy

> Also (#859): the registry hosts of a detected ecosystem (from #830's toolchain table) are
> allowed like a provider's hosts, so a session with an empty cache of its own can fetch its
> dependencies. They are shown in the evidence and in the host counters. A private registry
> or mirror, or a Git dependency host not in the table, is an owner decision (#674).

### #832 selective TLS interception for credential hosts

> Also (#859): a registry token (`~/.cargo/credentials.toml`, `~/.npmrc` auth, `~/.pypirc`) is
> a credential of the same kind as `GH_TOKEN`. It is not mounted into the sandbox. When a
> project needs a private registry, its token is bound to that registry host here and never
> enters the sandbox. Not required for the first release of this issue.

### #674 refuse at once, owner decides

> Also (#859): the same owner decision and project grant carries a private registry host and
> an extra read-only or read-write folder, with who, when and client recorded and revocation
> (a revoked folder grant applies at the next launch; a revoked host closes open tunnels).
> No path adds a folder or host without an owner decision.

### #716 shared build cache across clones

> Sandbox consequence (#859, owner decisions 2026-11-05): a sandboxed session gets package
> caches and compiled-output caches of its own, in its session temp folder, and none of the
> owner's. This answers open question 2: a cache survives the sandbox by not being shared
> across sessions. The deferral to Cargo's native shared cache stands; if it lands, it must
> fit that rule (no cache written by a session and read by the host or another session). The
> sccache option stays rejected.

### #406 storage and memory hygiene

> Sandbox consequence (#859): the "shared build cache across clones" bullet is #716's and is
> closed as a sandbox matter. A session's package and tool caches live in its temp folder,
> which `torlek storage` already measures and the session end removes; no new retention rule
> is needed for them.

### Not changed

- Issue #833 (default on): its probe launch should include the cache variables above.
- Issue #834 (limits): the per-session cache folders count toward a session's disk use; no
  change to its scope.
- Issue #685 (red-team): add "plant a program, config or cache entry the host later runs" to
  section 1.
- Issue #858 (provider sign-ins): the credential and tool-config rule above applies to
  provider folders as well; #830's per-provider folder list is where that decision lands.
