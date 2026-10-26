# Agent Docker builds with scoped CA trust

`agent-docker-build` makes the launcher's CA bundle available to build steps using
BuildKit secret mounts. It uses the current project's rootless Docker daemon and
the existing Greyproxy policy. It does not start application containers.

After deploying the helper, restart the agent session so its launcher installs the
read-only helper and exports `AGENT_DOCKER_CA_BUNDLE`.

## Usage

Run from the project's checkout or one of its subdirectories:

```sh
agent-docker-build build -t my-dev-image --build-arg VERSION=1 ./docker
agent-docker-build build -f docker/Dev.Dockerfile .
agent-docker-build compose laravel.test
agent-docker-build compose -f docker-compose.yaml --no-cache laravel.test
```

Compose resolves the project's normal configuration, then the helper generates a
temporary build-only model for the selected service. Context, arguments, image,
platform, existing build secrets and supported build settings retain their resolved
values. Runtime environment values, volumes and unrelated services are omitted.
Without an explicit image name, Compose keeps its project/service image naming.
Compose converts that model to Bake definitions; Buildx gets explicit read access
only to the helper's private temporary directory. Other filesystem entitlements
remain enforced, including any extra accesses requested by the original build.

For Stadeo, use `agent-docker-build compose laravel.test` from its checkout. The
original Sail Dockerfile and Compose file stay unchanged.

## Trust scope

Every supported `RUN` receives required secret mounts under
`/run/agent-build-trust`. Its processes get CA settings for curl, OpenSSL, Git,
Python Requests and Node, plus temporary apt and PHP configuration. These settings
are not Dockerfile `ENV` instructions and do not modify the image's system CA
store. Installing or upgrading `ca-certificates` remains possible during the step.
PHP's existing scan directories are preserved. `NODE_USE_ENV_PROXY=1` enables
Node/Corepack's use of the existing proxy settings during build steps only.

The generated Dockerfile and build configuration live in a private temporary
directory outside the build context. They are removed when Docker exits or the
helper is interrupted. Docker build history includes the **paths** and commands,
but the certificate and client configuration are secret-mount inputs, not image
layers. A Dockerfile can deliberately copy a mounted file: this helper provides
scoped trust, not containment of malicious build instructions.

CA rotation is picked up when the agent launcher next starts. Secret contents are
not part of Docker's cache key; use `--no-cache` when verifying a rotated CA.

## Supported inputs and failures

- Local directory contexts; ordinary Linux Dockerfiles whose base shell is POSIX
  compatible, shell/exec `RUN`, backslash continuations and multiple stages.
- Existing simple `RUN --mount`, `--network` and `--security` flags are preserved.
  Dockerfile-specific `.dockerignore` files follow the generated Dockerfile.
- Custom `SHELL`, `ONBUILD`, heredocs, nonstandard frontends, alternate escape
  characters, remote/inline contexts and additional Compose contexts are rejected.
  Unsupported Compose build fields fail explicitly rather than being ignored.
- The helper requires a project socket, launcher Docker configuration, and a valid
  PEM CA bundle. It pins the default local builder and removes inherited context
  and remote-builder overrides; it never falls back to the user's main daemon.
- Network-policy refusals still require approval in Greyproxy. Do not weaken TLS
  verification to resolve them.

## Inspect an exported image

Use `docker image save` against the same isolated daemon, then run:

```sh
python3 tools/tests/verify_agent_docker_build.py image.tar ~/.local/share/greyproxy/ca-cert.pem
```

The inspector reads every referenced layer, including files deleted by subsequent
layers, and the runtime image configuration. It rejects the supplied interception
certificate (PEM or DER) and persisted helper trust paths. It does not extract the
archive or execute image contents. Scan against the single interception CA, not
the full public-root bundle.
