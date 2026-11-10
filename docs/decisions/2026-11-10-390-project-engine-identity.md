# A project's engine is the project user's rootless Docker whose ID the owner recorded

Status: accepted

Issue #390, on #856's decisions (one rootless engine per project under a system user of its
own). Decided by Theo, answering the worker's three proposals.

- **The engine user is named after the project's id: `<profile>-e<id>`** (`torlek-e3`,
  `torlek-next-e12`). Project names do not fit a Linux user name (length, case, dots); the id
  does, and the profile keeps two profiles' engines apart. A name over 32 characters is
  refused, not shortened.
- **Identity is what the kernel and the owner say, never the socket's path.** The daemon
  reads the listening process's user with `SO_PEERCRED`, requires `name=rootless` in its
  `/info` and refuses root and the owner. The owner records the engine ID with
  `torlek containers record PROJECT`; any other ID is refused until recorded again, so a
  replaced engine is never trusted silently (`crates/cockpit-containers/src/identity.rs`).
- **Torlek never runs anything as root: it writes a setup script the owner reviews and runs
  with `sudo`.** The script creates the user, its subordinate ids, the checkout ACLs and the
  `torlek-engine@.service` unit; `torlek doctor` writes it and names it.
- **The unit has no network (`PrivateNetwork=yes`) until the per-engine egress proxy exists,**
  and sessions do not reach the engine until the clone masks exist: a verified engine reads
  `unsupported` in `torlek doctor`. Failing closed beats an engine that reaches the network
  or other checkouts unchecked.
- **Rejected:** trusting whatever listens on `/run/<user>/docker.sock` (a path says neither
  who runs the engine, whether it is rootless, nor whether it is the install the owner set
  up), and a root helper in Torlek (a standing root process for a one-time setup).
