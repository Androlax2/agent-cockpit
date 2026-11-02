import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateBase } from "./content.mjs";

export function checkLinks(dist, base) {
  validateBase(base);
  const root = mkdtempSync(path.join(tmpdir(), "cockpit-site-links-"));
  try {
    const mounted = path.join(root, base);
    cpSync(dist, mounted, { recursive: true });
    const result = spawnSync(
      "lychee",
      [
        "--offline",
        "--no-progress",
        "--no-ignore",
        "--include-fragments=anchor-only",
        "--index-files",
        "index.html",
        "--root-dir",
        root,
        "--threads",
        "2",
        `${mounted}/**/*.html`,
      ],
      { encoding: "utf8", timeout: 60_000, maxBuffer: 4 * 1024 * 1024 },
    );
    if (result.error || result.status !== 0)
      throw new Error(
        `Rendered links failed: ${result.error ?? ""}\n${result.stdout}\n${result.stderr}`,
      );
    return result.stdout + result.stderr;
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const site = fileURLToPath(new URL("../", import.meta.url));
  const { base } = JSON.parse(readFileSync(path.join(site, ".source-manifest.json"), "utf8"));
  console.log(checkLinks(process.argv[2] ?? path.join(site, "dist"), process.argv[3] ?? base));
}
