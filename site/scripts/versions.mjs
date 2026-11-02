import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function validateVersion(value) {
  if (value !== "latest" && !/^v\d+\.\d+\.\d+(?:-(?:alpha|beta)\.[1-9]\d*)?$/.test(value)) {
    throw new Error(`Invalid documentation version: ${value}`);
  }
  return value;
}

export function catalog(releases) {
  const names = releases
    .filter((release) => !release.draft)
    .map((release) => validateVersion(release.tag_name));
  return [
    "latest",
    ...new Set(
      names
        .filter((name) => name !== "latest")
        .sort((a, b) => b.localeCompare(a, "en", { numeric: true })),
    ),
  ];
}

export function assemble(builds, out, names) {
  const inputPath = path.resolve(builds);
  const outputPath = path.resolve(out);
  if (
    inputPath === outputPath ||
    inputPath.startsWith(`${outputPath}${path.sep}`) ||
    outputPath.startsWith(`${inputPath}${path.sep}`)
  )
    throw new Error("Build and output directories must not overlap");
  if (new Set(names).size !== names.length || names[0] !== "latest")
    throw new Error("Catalog must start with latest and contain unique versions");
  for (const name of names) {
    validateVersion(name);
    const source = path.join(builds, name);
    if (
      !existsSync(path.join(source, "index.html")) ||
      !existsSync(path.join(source, "build.json"))
    )
      throw new Error(`Missing documentation build: ${name}`);
    const manifest = JSON.parse(readFileSync(path.join(source, "build.json"), "utf8"));
    if (
      manifest.version !== name ||
      manifest.base !== `/agent-cockpit/${name}/` ||
      JSON.stringify(manifest.versions) !== JSON.stringify(names)
    )
      throw new Error(`Build catalog/prefix mismatch: ${name}`);
  }
  // Validate the complete catalog before replacing an assembled tree.
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  for (const name of names)
    cpSync(path.join(builds, name), path.join(out, name), { recursive: true });
  writeFileSync(path.join(out, ".nojekyll"), "");
  writeFileSync(path.join(out, "versions.json"), `${JSON.stringify(names)}\n`);
  writeFileSync(
    path.join(out, "index.html"),
    '<!doctype html><html lang="en"><meta charset="utf-8"><title>Agent Cockpit documentation</title><meta http-equiv="refresh" content="0;url=latest/get-started/install/"><a href="latest/get-started/install/">Latest documentation</a></html>\n',
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, input, output, versions] = process.argv.slice(2);
  if (command === "catalog")
    console.log(JSON.stringify(catalog(JSON.parse(readFileSync(input, "utf8")).flat())));
  else if (command === "assemble") assemble(input, output, JSON.parse(versions));
  else
    throw new Error(
      "Usage: versions.mjs catalog releases.json | assemble builds output versions-json",
    );
}
