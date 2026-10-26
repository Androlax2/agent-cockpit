# Scoped Docker build trust

Task: `efff51d1`. Theo approved Option A in this session on 2026-10-26.

## Approved scope

Provide a shared, explicitly invoked agent build helper that generates temporary
Dockerfile/Compose overlays. Use BuildKit secret mounts for the CA bundle and
client configuration during individual `RUN` instructions. Do not maintain
per-project vendor Dockerfile patches. Verify a representative Sail 8.4 build
and inspect exported image layers for interception CA or persistent trust settings.

The launcher already configures the project-only daemon and container proxy.
The daemon's `SSL_CERT_FILE` does not configure processes inside build containers.
The captured Stadeo log confirms curl error 60 and apt's unknown certificate issuer.

## Constraints

- No system-wide CA installation, TLS-verification disablement, network-policy
  bypass, or CA baked into production images.
- Use the existing project daemon and proxy, never the user's main Docker daemon.
- Keep source Dockerfiles and Compose files unchanged; generated inputs are private,
  temporary files outside the build context.
- Cover the TLS clients used by Sail: apt, curl, Git, PHP/Composer, Node/npm.
- Fail explicitly for unsupported inputs rather than silently weakening trust or
  building a different configuration.
- Host deployment and live build execution go through `ask-to-run` approval.

## Builder decisions

Use a Python standard-library CLI named `agent-docker-build`, separate from the
CPU-affinity helper `agent-build`. Support local Dockerfile builds and one explicitly
selected Compose build service initially. Do not shadow the Docker executable.
Configure trust through per-step environment and secret-mounted configuration,
not image `ENV`, `COPY`, or edits to the container's system trust store.

The helper is scoped tooling, not a sandbox for malicious Dockerfiles: a build
command could deliberately copy any file it can read. The live acceptance check
must inspect the produced Sail image, including intermediate layers.
