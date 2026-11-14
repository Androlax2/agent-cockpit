# A project's engine reaches out only through a proxy the daemon serves on the relay's listener

Status: accepted

Issue #390, part 6 (the egress design of #856, Q4 A and Q5 A). Decided by Theo through the
coordinator, answering the worker's proposals.

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
  relay refuses the listener until then, naming the setup.
- **Rejected:** a unit running the proxy as the engine's user (B): that user would hold the
  owner's decisions and need a route out of the namespace it must not have.
