# cockpit-containers

Owns the container engines Torlek uses (report 02, #390).

`probe_docker` asks the engine the owner's `docker` program reaches for its identity with
`docker info` (10 s at most): its ID, name and version, whether it is rootless, and whether
it runs in Docker Desktop's VM. A `docker` program is not an engine. The daemon probes it
for the readiness report of a project that runs in Docker.

**Depends on:** no other Cockpit crate. **Used by:** `cockpit-daemon`.
