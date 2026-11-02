import assert from "node:assert/strict";
import test from "node:test";

const content = await import("../scripts/content.mjs").catch(() => ({}));

test("MDX conversion preserves code while escaping Markdown prose", () => {
  assert.equal(typeof content.toMdx, "function", "canonical Markdown converter is required");
  const source =
    'Source <https://example.com/docs> and {a}.\n`x < y {z}`\n```sh\necho "<value>{literal}"\n```\n';
  const result = content.toMdx(source);
  assert.match(result, /\[https:\/\/example.com\/docs\]\(https:\/\/example.com\/docs\)/);
  assert.match(result, /&#123;a&#125;/);
  assert.ok(result.includes("`x < y {z}`"));
  assert.ok(result.includes('echo "<value>{literal}"'));
});

test("canonical links use the selected prefix and unmapped sources stay on GitHub", () => {
  assert.equal(typeof content.rewriteLinks, "function", "canonical link mapping is required");
  const pages = new Map([
    ["README.md", "index"],
    ["docs/DEVELOPMENT.md", "project/development"],
  ]);
  const result = content.rewriteLinks(
    "[dev](docs/DEVELOPMENT.md#checks) [code](crates/cockpit-cli/src/main.rs) [same](#install)",
    "README.md",
    pages,
    "/agent-cockpit/v0.1.0-alpha.1/",
    "abc123",
  );
  assert.match(result, /\/agent-cockpit\/v0\.1\.0-alpha\.1\/project\/development\/#checks/);
  assert.match(
    result,
    /github.com\/Androlax2\/agent-cockpit\/blob\/abc123\/crates\/cockpit-cli\/src\/main.rs/,
  );
  assert.match(result, /\[same\]\(#install\)/);
});

test("unsafe base and version values are refused", () => {
  assert.equal(typeof content.validateBase, "function", "base validation is required");
  for (const value of [
    "https://other.invalid/",
    "/docs/../secret/",
    "/docs//latest/",
    '/docs/"x/',
  ]) {
    assert.throws(() => content.validateBase(value));
  }
  assert.equal(content.validateBase("/agent-cockpit/latest/"), "/agent-cockpit/latest/");
});

test("reference generation uses an empty home and cleared environment", async () => {
  const { mkdtempSync, writeFileSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const dir = mkdtempSync(join(tmpdir(), "cockpit-reference-test-"));
  try {
    const binary = join(dir, "fake cockpit");
    writeFileSync(
      binary,
      `#!${process.execPath}
if (process.env.COCKPIT_DOCS_TEST_SENTINEL) process.exit(17);
const args = process.argv.slice(2).join(' ');
if (args === '--help') { console.log('usage: cockpit VERB'); process.exit(2); }
if (args === '--profile cockpit config show') console.log('home = "'+process.env.HOME+'"');
else if (args === 'version --all') console.log('fixture version');
else process.exit(19);
`,
      { mode: 0o755 },
    );
    process.env.COCKPIT_DOCS_TEST_SENTINEL = "fixture-only";
    const refs = content.readReferences(binary);
    assert.equal(refs.help, "usage: cockpit VERB");
    assert.equal(refs.config, 'home = "~"');
    assert.equal(refs.version, "fixture version");
  } finally {
    delete process.env.COCKPIT_DOCS_TEST_SENTINEL;
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a fence-like content line does not end code protection", () => {
  const markdown = "```text\n```shell\n{literal} [sample](README.md)\n```\n";
  assert.equal(content.toMdx(markdown), markdown.trimEnd());
});

test("split README fragments resolve to their generated page", () => {
  const anchors = new Map([["README.md#approvals", "guide/approvals"]]);
  const result = content.rewriteLinks(
    "[approve](#approvals) [local](#example)",
    "README.md",
    new Map(),
    "/agent-cockpit/latest/",
    "abc123",
    anchors,
  );
  assert.equal(result, "[approve](/agent-cockpit/latest/guide/approvals/) [local](#example)");
});

test("image raw query precedes its fragment", () => {
  assert.equal(
    content.rewriteLinks(
      "![Diagram](diagram.svg#detail)",
      "README.md",
      new Map(),
      "/agent-cockpit/latest/",
      "abc123",
    ),
    "![Diagram](https://github.com/Androlax2/agent-cockpit/blob/abc123/diagram.svg?raw=true#detail)",
  );
});

test("CRLF closing fences return to prose conversion", () => {
  const result = content.toMdx("```text\r\n{inside}\r\n```\r\n{outside}\r\n");
  assert.ok(result.includes("{inside}"));
  assert.ok(result.includes("&#123;outside&#125;"));
});
