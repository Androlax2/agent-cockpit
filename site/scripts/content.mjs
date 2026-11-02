import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateVersion } from "./versions.mjs";

const site = path.resolve(fileURLToPath(new URL("../", import.meta.url)));
const repository = path.dirname(site);
const github = "https://github.com/Androlax2/agent-cockpit";
const digest = (value) => createHash("sha256").update(value).digest("hex");

export function validateBase(value) {
  if (
    !/^\/(?:[A-Za-z0-9_.-]+\/)+$/.test(value) ||
    value.split("/").some((part) => part === "." || part === "..")
  ) {
    throw new Error(`Invalid documentation base: ${value}`);
  }
  return value;
}

function outsideFences(markdown, transform) {
  let fence = null;
  let prose = [];
  const result = [];
  const flush = () => {
    result.push(transform(prose.join("\n")));
    prose = [];
  };
  for (const line of markdown.split("\n")) {
    const marker = line.match(/^ {0,3}(`{3,}|~{3,})/);
    if (fence) {
      result.push(line);
      if (
        marker &&
        marker[1][0] === fence[0] &&
        marker[1].length >= fence.length &&
        /^[\t ]*\r?$/.test(line.slice(marker[0].length))
      )
        fence = null;
    } else if (marker) {
      flush();
      result.push(line);
      fence = marker[1];
    } else prose.push(line);
  }
  flush();
  return result.filter((part) => part !== "").join("\n");
}

export function toMdx(markdown) {
  return outsideFences(markdown, (block) => {
    let position = 0;
    let result = "";
    const escapeProse = (value) =>
      value
        .replace(/<!--[\s\S]*?-->/g, "")
        .replace(/<(https?:\/\/[^>]+)>/g, "[$1]($1)")
        .replaceAll("<", "&lt;")
        .replaceAll("{", "&#123;")
        .replaceAll("}", "&#125;");
    for (const match of block.matchAll(/(`+)([\s\S]*?)\1/g)) {
      result += escapeProse(block.slice(position, match.index)) + match[0];
      position = match.index + match[0].length;
    }
    return result + escapeProse(block.slice(position));
  });
}

export function rewriteLinks(markdown, source, pages, base, revision, anchors = new Map()) {
  validateBase(base);
  return outsideFences(markdown, (block) =>
    block.replace(
      /(`+)([\s\S]*?)\1|(!?)\[([^\]]*)\]\(([^)\s]+)\)/g,
      (all, code, _literal, image, label, target) => {
        if (code || /^(?:https?:|mailto:)/.test(target)) return all;
        if (/^[A-Za-z][A-Za-z\d+.-]*:/.test(target))
          throw new Error(`Unsupported link protocol in ${source}`);
        const [file, fragment] = target.split("#", 2);
        if (!file && !anchors.has(`${source}#${fragment}`)) return all;
        const canonical = file
          ? path.posix.normalize(path.posix.join(path.posix.dirname(source), file))
          : source;
        if (canonical.startsWith("../") || path.posix.isAbsolute(canonical))
          throw new Error(`Link escapes repository: ${target}`);
        const route = anchors.get(`${canonical}#${fragment}`) ?? pages.get(canonical);
        const url =
          route !== undefined
            ? `${base}${route}/`.replace(/\/index\/$/, "/") +
              (fragment && !anchors.has(`${canonical}#${fragment}`) ? `#${fragment}` : "")
            : `${github}/blob/${revision}/${canonical}${image ? "?raw=true" : ""}${fragment ? `#${fragment}` : ""}`;
        return `${image}[${label}](${url})`;
      },
    ),
  );
}

export function readReferences(binary) {
  const home = mkdtempSync(path.join(tmpdir(), "cockpit-docs-"));
  try {
    const env = { HOME: home, PATH: "/usr/bin:/bin" };
    const run = (args, expected) => {
      const result = spawnSync(binary, args, {
        env,
        encoding: "utf8",
        timeout: 15_000,
        maxBuffer: 1024 * 1024,
      });
      if (result.error || result.status !== expected)
        throw new Error(
          `Reference command ${args.join(" ")} failed: ${result.error ?? result.stderr}`,
        );
      return (result.stdout + result.stderr).replaceAll(home, "~").trim();
    };
    const help = run(["--help"], 2);
    if (!help.startsWith("usage: cockpit")) throw new Error("CLI did not return cockpit help");
    return {
      help,
      config: run(["--profile", "cockpit", "config", "show"], 0),
      version: run(["version", "--all"], 0),
      binaryHash: digest(readFileSync(binary)),
    };
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
}

export function stage({
  root = repository,
  out = path.join(site, "content/docs"),
  binary = path.join(root, "target/debug/cockpit"),
  base = "/agent-cockpit/latest/",
  version = "latest",
  versions = ["latest"],
} = {}) {
  validateBase(base);
  for (const name of [version, ...versions]) validateVersion(name);
  const revision = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim();
  const entries = [];
  const read = (file) => readFileSync(path.join(root, file), "utf8");
  const add = (source, route, title, body, extra = {}) =>
    entries.push({ source, route, title, body, sourceHash: digest(read(source)), ...extra });
  const readme = read("README.md");
  const sections = [...readme.matchAll(/^## (.+)$/gm)];
  const section = (name) => {
    const i = sections.findIndex((match) => match[1] === name);
    if (i < 0) throw new Error(`README section missing: ${name}`);
    return readme.slice(sections[i].index + sections[i][0].length, sections[i + 1]?.index).trim();
  };
  add(
    "README.md",
    "get-started/install",
    "Install status",
    readme
      .slice(0, sections.find((match) => match[1] === "First project and session").index)
      .replace(/^# .+\n/, ""),
  );
  for (const [name, route] of [
    ["First project and session", "get-started/first-session"],
    ["Approvals", "guide/approvals"],
    ["Providers and capability evidence", "guide/providers"],
    ["Configuration and state", "guide/configuration"],
    ["Help and support", "guide/support"],
  ])
    add("README.md", route, name, section(name));
  const files = (dir) =>
    readdirSync(path.join(root, dir))
      .filter((name) => name.endsWith(".md"))
      .sort()
      .map((name) => `${dir}/${name}`);
  for (const [dir, prefix] of [
    ["docs/design/greenfield", "project/design"],
    ["docs/decisions", "project/decisions"],
    ["crates/cockpit-agent-context/bundle/references", "reference/instructions"],
  ]) {
    for (const source of files(dir)) {
      const body = read(source);
      add(
        source,
        `${prefix}/${path.basename(source, ".md").toLowerCase()}`,
        body.match(/^# (.+)/m)?.[1] ?? path.basename(source, ".md"),
        body.replace(/^# .+\n/, ""),
      );
    }
  }
  for (const [source, route] of [
    ["docs/DEVELOPMENT.md", "project/development"],
    ["CONTRIBUTING.md", "project/contributing"],
    ["crates/cockpit-agent-context/bundle/SKILL.md", "reference/agent-instructions"],
  ]) {
    const body = read(source).replace(/^---\n[\s\S]*?\n---\n/, "");
    add(
      source,
      route,
      body.match(/^# (.+)/m)?.[1] ?? "Agent instructions",
      body.replace(/^# .+\n/, ""),
    );
  }
  const refs = readReferences(binary);
  const reference = (command, output, note) =>
    `Generated from \`${command}\` using the actual CLI binary. ${note}\n\n\`\`\`text\n${output}\n\`\`\`\n`;
  add(
    "crates/cockpit-cli/src/main.rs",
    "reference/cli",
    "CLI reference",
    reference("cockpit --help", refs.help, "Help exits 2, as usage output."),
    { generated: true },
  );
  add(
    "crates/cockpit-config/src/lib.rs",
    "reference/configuration",
    "Configuration defaults",
    reference(
      "cockpit --profile cockpit config show",
      refs.config,
      "Captured offline in an empty isolated home. These are effective defaults, not a complete configuration schema. Home paths are normalized to `~`.",
    ),
    { generated: true },
  );
  add(
    "site/content/protocol.md",
    "reference/protocol",
    "Protocol schema (planned)",
    read("site/content/protocol.md").replace(/^# .+\n/, ""),
  );
  add(
    "site/content/comparison.md",
    "project/comparison",
    "Where Cockpit fits",
    read("site/content/comparison.md").replace(/^# .+\n/, ""),
  );
  const pages = new Map(entries.map((entry) => [entry.source, entry.route]));
  pages.set("README.md", "get-started/install");
  const anchors = new Map();
  for (const [heading, route] of [
    ["first-project-and-session", "get-started/first-session"],
    ["approvals", "guide/approvals"],
    ["providers-and-capability-evidence", "guide/providers"],
    ["configuration-and-state", "guide/configuration"],
    ["help-and-support", "guide/support"],
  ])
    anchors.set(`README.md#${heading}`, route);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const manifest = {
    revision,
    version,
    base,
    versions,
    cli: refs.version,
    binaryHash: refs.binaryHash,
    pages: [],
  };
  for (const entry of entries) {
    const file = path.join(out, `${entry.route}.mdx`);
    mkdirSync(path.dirname(file), { recursive: true });
    const header = {
      title: entry.title,
      description: `${entry.title} for Agent Cockpit`,
      sourcePath: entry.source,
      sourceRevision: revision,
      sourceHash: entry.sourceHash,
      generated: Boolean(entry.generated),
    };
    writeFileSync(
      file,
      `---\n${Object.entries(header)
        .map(([key, value]) => `${key}: ${JSON.stringify(value)}`)
        .join(
          "\n",
        )}\n---\n\n${toMdx(rewriteLinks(entry.body, entry.source, pages, base, revision, anchors))}\n`,
    );
    manifest.pages.push({ path: entry.route, ...header });
  }
  const meta = (dir, value) => {
    const file = path.join(out, dir, "meta.json");
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
  };
  meta("", { pages: ["get-started", "guide", "reference", "project"] });
  meta("get-started", { title: "Get started", pages: ["install", "first-session"] });
  meta("guide", {
    title: "User guide",
    pages: ["approvals", "providers", "configuration", "support"],
  });
  meta("reference", {
    title: "Reference",
    pages: ["cli", "configuration", "agent-instructions", "protocol", "instructions"],
  });
  meta("reference/instructions", { title: "Instruction topics" });
  meta("project", {
    title: "Project",
    pages: ["comparison", "design", "decisions", "development", "contributing"],
  });
  meta("project/design", { title: "Design reports" });
  meta("project/decisions", { title: "Decision records" });
  writeFileSync(path.join(site, ".source-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const manifest = stage({
    binary: process.env.COCKPIT_DOCS_BINARY,
    base: process.env.COCKPIT_DOCS_BASE,
    version: process.env.COCKPIT_DOCS_VERSION,
    versions: process.env.COCKPIT_DOCS_VERSIONS
      ? JSON.parse(process.env.COCKPIT_DOCS_VERSIONS)
      : undefined,
  });
  console.log(`Staged ${manifest.pages.length} source-grounded pages at ${manifest.base}`);
}
