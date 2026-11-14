# A project's engine reaches out only through a proxy the daemon serves on the relay's listener

Status: accepted

Issue #390, part 6 (the egress design of #856, Q4 A and Q5 A). Decided by Theo through the
coordinator, answering the worker's proposals. Built so far: the relay's hand-off, the
`engine-proxy` verb and the engine unit (part 6b), and the daemon running one proxy per
recorded engine, ended by its pipe when the daemon stops (part 6c,
`crates/cockpit-daemon/src/engine_proxies.rs`), and the containers' proxy configuration
(part 6d): a session reaching its engine gets a Docker client `config.json` whose `proxies`
give the containers and builds it runs `HTTP(S)_PROXY` at the engine's proxy, with
`NO_PROXY` loopback plus the compose service names its checkout defines at launch
(`cockpit_containers::engine_client_config`), and Docker Hub's pull hosts are one
revocable per-project grant the owner gives with `torlek network grant PROJECT docker-hub`
or at `torlek containers record` (`cockpit_sandbox::grant_docker_hub`).

- **The daemon runs one proxy per engine; the relay hands it the listener** (A). The relay
  unit, in the engine's network namespace, listens on `127.0.0.1:3128` there when the owner's
  `torlek-sandbox engine-proxy` asks, and sends the listener over `SCM_RIGHTS`, keeping no
  copy (`crates/cockpit-sandbox-launcher/src/engine_relay.rs`, `engine_proxy.rs`). The port
  is open exactly while that proxy runs: with the daemon stopped, pulls and containers fail
  closed. The proxy resolves and connects on the owner's network, which the engine's
  namespace never reaches.
- **Containers reach it through slirp4netns with host loopback on.** The engine unit sets
  `DOCKERD_ROOTLESS_ROOTLESSKIT_NET=slirp4netns` and
  `DOCKERD_ROOTLESS_ROOTLESSKIT_DISABLE_HOST_LOOPBACK=false`, so `10.0.2.2` is the engine
  namespace's own `127.0.0.1`, and points dockerd's `HTTP(S)_PROXY` at `10.0.2.2:3128`. The
  consequence: the engine's containers reach every port on that loopback, which holds the
  proxy and the engine's own published ports, never the host's or another engine's, because
  `PrivateNetwork=yes` stays. An engine set up before this needs its setup run again; its
  relay refuses the listener until then, naming the setup. The engine unit is a template
  shared by every project's engine, so one project's setup changes the others' at their next
  restart; theirs still need their own run for the port.
- **A container can take the port.** A container publishing 3128 binds it before the relay
  does, if the proxy was not holding it: that project then has no egress, and dockerd's
  pulls reach the container, whose own way out is the same loopback. No other project is
  affected.
- **Rejected:** a unit running the proxy as the engine's user (B): that user would hold the
  owner's decisions and need a route out of the namespace it must not have.
