import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const site = fileURLToPath(new URL("../", import.meta.url));

test("real site exports the approved guide and generated references under its version prefix", () => {
  const env = {
    ...process.env,
    ASTRO_TELEMETRY_DISABLED: "1",
    COCKPIT_DOCS_BASE: process.env.COCKPIT_DOCS_BASE ?? "/agent-cockpit/latest/",
  };
  for (const args of [["scripts/content.mjs"], ["node_modules/astro/bin/astro.mjs", "build"]]) {
    const result = spawnSync(process.execPath, args, {
      cwd: site,
      env,
      encoding: "utf8",
      timeout: 180_000,
      maxBuffer: 8 * 1024 * 1024,
    });
    if (result.stderr) process.stderr.write(result.stderr);
    assert.equal(result.status, 0, `${result.error ?? ""}\n${result.stdout}\n${result.stderr}`);
  }
  const quickstart = `${site}dist/get-started/first-session/index.html`;
  assert.ok(existsSync(quickstart), "the product quickstart must be exported");
  const html = readFileSync(quickstart, "utf8");
  assert.match(html, /Register a Git repository/);
  assert.match(html, /cockpit.*session.*new/s);
  assert.ok(html.includes(`${env.COCKPIT_DOCS_BASE}guide/approvals`));
  assert.match(html, /Edit.*GitHub/);
  assert.match(html, /rel="canonical"/);
  const cli = readFileSync(`${site}dist/reference/cli/index.html`, "utf8");
  assert.match(cli, /Generated from/);
  assert.match(cli, /cockpit.*approval.*approve/s);
  const index = readFileSync(`${site}dist/search.json`, "utf8");
  assert.match(index, /First project and session/);
  assert.ok(index.includes(`${env.COCKPIT_DOCS_BASE}get-started/first-session`));
  const manifest = JSON.parse(readFileSync(`${site}dist/build.json`, "utf8"));
  assert.equal(manifest.base, env.COCKPIT_DOCS_BASE);
  assert.equal(manifest.version, process.env.COCKPIT_DOCS_VERSION ?? "latest");
  assert.match(manifest.binaryHash, /^[0-9a-f]{64}$/);
});
