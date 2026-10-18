#!/usr/bin/env node
// Third-party notices generator for Settings -> About -> Acknowledgements.
// Collects every component compiled into the shipped binaries and the
// frontend bundle, with the verbatim license and NOTICE files found in their
// published sources, and writes a deterministic, sorted JSON file.
//
//   pnpm run notices          regenerate src/generated/third-party-notices.json
//   pnpm run notices:check    fail when the committed file is stale
//
// Sources (no extra tools):
//   cargo metadata --format-version 1 --filter-platform x86_64-pc-windows-msvc --locked
//     normal (non-dev, non-build) dependency closure of `codexbar` and
//     `codexbar-desktop-tauri`; texts come from the local Cargo registry.
//   pnpm licenses list --json --prod
//     production frontend dependencies; texts come from node_modules.
//   NOTICE (repo root)
//     vendored code that carries its own notice.
//
// Exit codes: 0 ok, 1 stale (--check), 2 collection failed.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(appDir, "..", "..");
const outPath = join(appDir, "src", "generated", "third-party-notices.json");
const TARGET = "x86_64-pc-windows-msvc";
const ROOT_CRATES = ["codexbar", "codexbar-desktop-tauri"];
const NOTICE_FILE = /^(licen[cs]e|copying|notice|copyright|unlicense)/i;
const NOT_A_NOTICE = /\.(rs|toml|json|js|cjs|mjs|ts|html|py|sh|yml|yaml)$/i;

function die(code, msg) {
  console.error(`[notices] ${msg}`);
  process.exit(code);
}

function run(cmd, args, cwd) {
  try {
    return execFileSync(cmd, args, {
      cwd,
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
      // pnpm is a .cmd shim on Windows, which execFile cannot start directly.
      shell: process.platform === "win32" && cmd === "pnpm",
    });
  } catch (err) {
    die(2, `\`${cmd} ${args.join(" ")}\` failed: ${err.stderr || err.message}`);
  }
}

function normalizeText(text) {
  return text
    .replace(/^\uFEFF/, "")
    .replace(/\r\n?/g, "\n")
    .replace(/[ \t]+$/gm, "")
    .trim();
}

const texts = new Map();

function internText(raw) {
  const text = normalizeText(raw);
  if (!text) return null;
  const id = createHash("sha256").update(text).digest("hex").slice(0, 16);
  const existing = texts.get(id);
  if (existing !== undefined && existing !== text) die(2, `text id collision on ${id}`);
  texts.set(id, text);
  return id;
}

function noticeFiles(dir, extra = []) {
  const found = new Map();
  let entries = [];
  try {
    entries = readdirSync(dir);
  } catch {
    return [];
  }
  for (const name of entries) {
    if (!NOTICE_FILE.test(name) || NOT_A_NOTICE.test(name)) continue;
    const path = join(dir, name);
    if (statSync(path).isFile()) found.set(name, path);
  }
  for (const rel of extra) {
    const path = resolve(dir, rel);
    if (existsSync(path) && statSync(path).isFile()) {
      found.set(rel.replace(/\\/g, "/").replace(/^(\.\.\/)+/, ""), path);
    }
  }
  const notices = [];
  for (const [file, path] of [...found].sort(([a], [b]) => a.localeCompare(b, "en"))) {
    const textId = internText(readFileSync(path, "utf8"));
    if (textId) notices.push({ file, textId });
  }
  return notices;
}

function mergeComponent(map, key, component) {
  const prev = map.get(key);
  if (!prev) {
    map.set(key, component);
    return;
  }
  prev.versions = [...new Set([...prev.versions, ...component.versions])];
  for (const author of component.authors) {
    if (!prev.authors.includes(author)) prev.authors.push(author);
  }
  for (const notice of component.notices) {
    if (!prev.notices.some((n) => n.file === notice.file && n.textId === notice.textId)) {
      prev.notices.push(notice);
    }
  }
}

function compareVersions(a, b) {
  return a.localeCompare(b, "en", { numeric: true });
}

function collectCargo(components) {
  const meta = JSON.parse(
    run(
      "cargo",
      ["metadata", "--format-version", "1", "--filter-platform", TARGET, "--locked"],
      repoRoot,
    ),
  );
  const packages = new Map(meta.packages.map((pkg) => [pkg.id, pkg]));
  const nodes = new Map(meta.resolve.nodes.map((node) => [node.id, node]));
  const members = new Set(meta.workspace_members);
  const roots = meta.workspace_members.filter((id) => ROOT_CRATES.includes(packages.get(id)?.name));
  if (roots.length !== ROOT_CRATES.length) die(2, `expected workspace crates ${ROOT_CRATES.join(", ")}`);

  const seen = new Set();
  const queue = [...roots];
  while (queue.length) {
    const id = queue.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      if (dep.dep_kinds.some((kind) => kind.kind === null)) queue.push(dep.pkg);
    }
  }

  for (const id of seen) {
    if (members.has(id)) continue;
    const pkg = packages.get(id);
    const dir = dirname(pkg.manifest_path);
    const url =
      pkg.repository || pkg.homepage || `https://crates.io/crates/${pkg.name}`;
    mergeComponent(components, `cargo:${pkg.name}`, {
      name: pkg.name,
      ecosystem: "cargo",
      versions: [pkg.version],
      authors: [...(pkg.authors ?? [])],
      url,
      license: pkg.license || (pkg.license_file ? "See license file" : "Unknown"),
      notices: noticeFiles(dir, pkg.license_file ? [pkg.license_file] : []),
    });
  }
}

function authorName(author) {
  if (!author) return null;
  if (typeof author === "string") return author;
  return author.name ?? null;
}

function collectNpm(components) {
  const report = JSON.parse(run("pnpm", ["licenses", "list", "--json", "--prod"], appDir));
  for (const group of Object.values(report)) {
    for (const entry of group) {
      entry.paths.forEach((dir, index) => {
        let pkg = {};
        try {
          pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
        } catch {
          // pnpm already reported the package; fall back to its summary fields.
        }
        const author = authorName(pkg.author) ?? entry.author ?? null;
        const repository =
          typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
        const url =
          entry.homepage ||
          (repository ? repository.replace(/^git\+/, "").replace(/\.git$/, "") : null) ||
          `https://www.npmjs.com/package/${entry.name}`;
        mergeComponent(components, `npm:${entry.name}`, {
          name: entry.name,
          ecosystem: "npm",
          versions: [pkg.version ?? entry.versions[index] ?? entry.versions[0]],
          authors: author ? [author] : [],
          url,
          license: entry.license || "Unknown",
          notices: noticeFiles(dir),
        });
      });
    }
  }
}

function collectVendored(components) {
  const textId = internText(readFileSync(join(repoRoot, "NOTICE"), "utf8"));
  components.set("vendored:codexcontrol", {
    name: "codexcontrol",
    ecosystem: "vendored",
    versions: [],
    authors: ["Adem Isler"],
    url: "https://github.com/ademisler/codexcontrol",
    license: "MIT",
    notices: textId ? [{ file: "NOTICE", textId }] : [],
  });
}

function generate() {
  const components = new Map();
  collectCargo(components);
  collectNpm(components);
  collectVendored(components);

  const list = [...components.values()]
    .map((c) => ({
      ...c,
      versions: [...c.versions].sort(compareVersions),
      notices: [...c.notices].sort(
        (a, b) => a.file.localeCompare(b.file, "en") || a.textId.localeCompare(b.textId, "en"),
      ),
    }))
    .sort(
      (a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase(), "en") ||
        a.ecosystem.localeCompare(b.ecosystem, "en"),
    );

  const used = new Set(list.flatMap((c) => c.notices.map((n) => n.textId)));
  const textEntries = [...texts]
    .filter(([id]) => used.has(id))
    .sort(([a], [b]) => a.localeCompare(b, "en"));

  return `${JSON.stringify(
    { schemaVersion: 1, components: list, texts: Object.fromEntries(textEntries) },
    null,
    2,
  )}\n`;
}

const check = process.argv.includes("--check");
const output = generate();
const parsed = JSON.parse(output);
const missing = parsed.components.filter((c) => c.notices.length === 0).map((c) => c.name);
const summary = `${parsed.components.length} components, ${Object.keys(parsed.texts).length} distinct texts, ${missing.length} without a bundled license file`;

if (check) {
  const current = existsSync(outPath) ? readFileSync(outPath, "utf8").replace(/\r\n/g, "\n") : "";
  if (current !== output) {
    die(1, `src/generated/third-party-notices.json is stale; run \`pnpm run notices\` (${summary})`);
  }
  console.log(`[notices] OK - ${summary}`);
} else {
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, output);
  console.log(`[notices] wrote ${outPath} - ${summary}`);
  if (missing.length) console.log(`[notices] license id + SPDX link only: ${missing.join(", ")}`);
}
