import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { checkLinks } from "../scripts/links.mjs";

test("real offline checker resolves version paths and rejects missing anchors", () => {
  const root = mkdtempSync(path.join(tmpdir(), "cockpit-link-fixture-"));
  try {
    mkdirSync(path.join(root, "guide"));
    writeFileSync(
      path.join(root, "index.html"),
      '<a href="/agent-cockpit/v1.0.0/guide/#exists">Guide</a>',
    );
    writeFileSync(path.join(root, "guide/index.html"), '<h1 id="exists">Guide</h1>');
    assert.match(checkLinks(root, "/agent-cockpit/v1.0.0/"), /0 Errors/);
    writeFileSync(path.join(root, "guide/index.html"), '<h1 id="other">Guide</h1>');
    assert.throws(() => checkLinks(root, "/agent-cockpit/v1.0.0/"), /Rendered links failed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
