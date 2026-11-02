import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, rmSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const site = fileURLToPath(new URL("../", import.meta.url));
const output = `${site}validation/dist`;

test("Astro exports Fumadocs MDX, a React island and CSS without a server", () => {
  rmSync(output, { recursive: true, force: true });
  const built = spawnSync(
    process.execPath,
    ["node_modules/astro/bin/astro.mjs", "build", "--root", "validation"],
    {
      cwd: site,
      env: { ...process.env, ASTRO_TELEMETRY_DISABLED: "1" },
      encoding: "utf8",
      timeout: 120_000,
    },
  );
  if (built.stderr) process.stderr.write(built.stderr);
  assert.equal(built.status, 0, `${built.error ?? ""}\n${built.stdout}\n${built.stderr}`);
  assert.ok(existsSync(`${output}/index.html`), "the build must export a document");
  const html = readFileSync(`${output}/index.html`, "utf8");
  assert.match(html, /Dependency integration probe/);
  assert.match(html, /Static Fumadocs content/);
  assert.match(html, /<astro-island\b/);
  assert.match(html, /<button[^>]*>React integration probe<\/button>/);
  assert.match(html, /<link[^>]+href="[^"]+\.css"/);
  assert.ok(existsSync(`${output}/search.json`), "search must be emitted as a static file");
  const search = JSON.parse(readFileSync(`${output}/search.json`, "utf8"));
  assert.equal(search.type, "simple");
  assert.match(JSON.stringify(search), /Dependency integration probe/);
  assert.ok(readdirSync(`${output}/_astro`).some((name) => name.endsWith(".js")));
});
