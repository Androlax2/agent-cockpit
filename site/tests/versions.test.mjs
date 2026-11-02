import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

const versions = await import("../scripts/versions.mjs").catch(() => ({}));
test("published catalog includes stable and preview versions, excluding drafts", () => {
  assert.equal(typeof versions.catalog, "function");
  assert.deepEqual(
    versions.catalog([
      { tag_name: "v0.1.0-alpha.1", draft: false },
      { tag_name: "v1.0.0", draft: false },
      { tag_name: "v0.1.0-beta.1", draft: true },
    ]),
    ["latest", "v1.0.0", "v0.1.0-alpha.1"],
  );
  assert.throws(() => versions.catalog([{ tag_name: "../../bad", draft: false }]));
});
test("version assembly retains every requested tree and refuses incomplete output", () => {
  assert.equal(typeof versions.assemble, "function");
  const root = mkdtempSync(path.join(tmpdir(), "cockpit-pages-"));
  try {
    const builds = path.join(root, "builds");
    const names = ["latest", "v0.1.0-alpha.1", "v0.1.0-beta.1"];
    for (const name of names) {
      mkdirSync(path.join(builds, name), { recursive: true });
      writeFileSync(path.join(builds, name, "index.html"), name);
      writeFileSync(
        path.join(builds, name, "build.json"),
        JSON.stringify({ version: name, base: `/agent-cockpit/${name}/`, versions: names }),
      );
    }
    const out = path.join(root, "pages");
    versions.assemble(builds, out, names);
    for (const name of names)
      assert.equal(readFileSync(path.join(out, name, "index.html"), "utf8"), name);
    assert.match(
      readFileSync(path.join(out, "index.html"), "utf8"),
      /latest\/get-started\/install\//,
    );
    assert.throws(() => versions.assemble(builds, out, [...names, "v2.0.0"]), /Missing|mismatch/);
    assert.equal(readFileSync(path.join(out, names[1], "index.html"), "utf8"), names[1]);
    assert.throws(() => versions.assemble(builds, out, ["latest", "../bad"]));
    assert.throws(() => versions.assemble(builds, builds, names), /overlap/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
