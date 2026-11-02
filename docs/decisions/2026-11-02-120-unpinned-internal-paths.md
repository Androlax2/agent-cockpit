# Internal path dependencies do not pin the workspace version

Issue #120. Decided by the worker within Theo's approved manifest scope.

- **Remove internal path versions directly.** Every greenfield crate is unpublished and
  inherits the workspace version. Repeating it on dependency edges prevents prerelease
  resolution and makes stamping rewrite unrelated manifests.
- **Keep external dependency checks.** Cargo-deny's `wildcards = "deny"` remains enabled;
  `allow-wildcard-paths = true` exempts private path dependencies. License, advisory and
  source checks stay unchanged. The option also covers private Git dependencies, whose
  sources remain governed by `unknown-git = "deny"`.
- **Stamp one root line.** Cargo updates workspace lockfile entries afterward. The regression checks
  member manifests stay unchanged. The release workflow then resolves and builds the
  stamped workspace; local verification also resolved a copied prerelease workspace.
- **Rejected:** adding a workspace dependency table solely to centralize pins that carry
  no constraint for these unpublished, same-workspace crates.

Option semantics: [cargo-deny configuration](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html#allow-wildcard-paths).
