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
    TORLEK_DOCS_BASE: process.env.TORLEK_DOCS_BASE ?? "/agent-cockpit/latest/",
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
  const quickstart = `${site}dist/get-started/quickstart/index.html`;
  assert.ok(existsSync(quickstart), "the product quickstart must be exported");
  const html = readFileSync(quickstart, "utf8");
  assert.match(html, /torlek.*project.*add/s);
  assert.match(html, /torlek.*session.*new/s);
  assert.ok(html.includes(`${env.TORLEK_DOCS_BASE}guide/approvals`));
  assert.match(html, /Edit.*GitHub/);
  assert.match(html, /rel="canonical"/);
  const install = readFileSync(`${site}dist/get-started/install-release/index.html`, "utf8");
  assert.match(install, /sha256sum/);
  const installSource = readFileSync(`${site}content/docs/get-started/install-release.mdx`, "utf8");
  for (const command of ["sha256sum --check", "torlek daemon install", "torlek init"])
    assert.ok(installSource.includes(command), `staged install steps must include: ${command}`);
  const installStatus = readFileSync(`${site}dist/get-started/install/index.html`, "utf8");
  assert.ok(
    installStatus.includes(`${env.TORLEK_DOCS_BASE}get-started/install-release`),
    "the install status page must link to the on-site install steps, not the GitHub README",
  );
  for (const route of ["get-started/quickstart", "guide/updates", "guide/phone-status"])
    assert.ok(existsSync(`${site}dist/${route}/index.html`), `${route} must be exported`);
  assert.ok(!existsSync(`${site}dist/project/decisions`), "decision records stay out of the site");
  const cli = readFileSync(`${site}dist/reference/cli/index.html`, "utf8");
  assert.match(cli, /Generated from/);
  assert.match(cli, /torlek.*approval.*approve/s);
  const index = readFileSync(`${site}dist/search.json`, "utf8");
  assert.match(index, /First project and session/);
  assert.ok(index.includes(`${env.TORLEK_DOCS_BASE}get-started/first-session`));
  const manifest = JSON.parse(readFileSync(`${site}dist/build.json`, "utf8"));
  assert.equal(manifest.base, env.TORLEK_DOCS_BASE);
  assert.equal(manifest.version, process.env.TORLEK_DOCS_VERSION ?? "latest");
  assert.match(manifest.binaryHash, /^[0-9a-f]{64}$/);
});
