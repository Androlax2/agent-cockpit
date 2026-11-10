# cockpit-containers

Owns the container engines Torlek uses: what an engine says it is, and the rootless Docker
engine each project gets under a user of its own, verified before use. Design: report 02
and issue #390, decided in #856.

`probe_docker` asks the engine the owner's `docker` program reaches for its identity with
`docker info` (10 s at most): its ID, name and version, whether it is rootless, and whether
it runs in Docker Desktop's VM. A `docker` program is not an engine. The daemon probes it
for the readiness report of a project that runs in Docker while agents run unsandboxed.

## A project's own engine

Sandboxed sessions never reach the owner's engine. Each project gets rootless Docker under a
system user of its own, shared by its clones:

- `EngineUser::for_project` names it after the profile and the project's id: `torlek-e3`,
  `torlek-next-e12` (at most 32 characters, as `useradd` takes). Its unit is
  `torlek-engine@<user>.service`; its socket `/run/<user>/docker.sock`.
- `ask_socket` asks that socket who listens (the kernel's `SO_PEERCRED`) and for `/info`.
  `verify` accepts only the project user's rootless engine, never root's or the owner's, and
  only the engine ID the owner recorded. Each refusal names its remedy: set up, start, record.
- `record_engine` keeps the ID the owner recorded with `torlek containers record PROJECT`;
  recording again replaces it, which is how a reinstalled engine is accepted.
- `setup_script` writes a POSIX `sh` script the owner reviews and runs with `sudo`: the
  system user, its subordinate ids, access to the project's checkout and clones folder, and
  the unit. The unit hides the owner's home but for those folders and has no network until
  the egress proxy exists. A second unit, `torlek-engine-relay@`, joins the engine's network
  namespace and runs the owner's installed `torlek-sandbox engine-relay`, bound read-only,
  on `EngineUser::relay_socket`: it relays the owner to one port the engine published.

The daemon finds the project, its clones folder and the owner, and passes them in.

**Depends on:** `cockpit-turso`, for the recorded engines; the
`containers -> the database only` rule of `cockpit-workspace-rules/tests/crate_graph.rs`
keeps every other Cockpit crate out. **Used by:** `cockpit-daemon`.
