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
//   license-templates/ (standard texts)
//     a component that publishes no license file gets the canonical text of
//     each SPDX id it names (see third-party-notices-lib.mjs).
//
// Exit codes: 0 ok, 1 stale (--check), 2 collection failed.

import { execFileSync, execSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildDocument,
  finalizeComponents,
  mergeComponent,
  normalizeText,
  standardNotices,
  textIdOf,
} from "./third-party-notices-lib.mjs";

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
  const options = {
    cwd,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  };
  try {
    if (cmd === "pnpm" && process.platform === "win32") {
      // pnpm is a .cmd shim on Windows, which execFile cannot start directly. When
      // pnpm runs this script it sets npm_execpath to its JS entry point; run that
      // with Node. Otherwise go through the shell, with constant arguments only.
      const entry = process.env.npm_execpath;
      if (entry && /pnpm/i.test(entry)) {
        return execFileSync(process.execPath, [entry, ...args], options);
      }
      return execSync(["pnpm", ...args].join(" "), options);
    }
    return execFileSync(cmd, args, options);
  } catch (err) {
    die(2, `\`${cmd} ${args.join(" ")}\` failed: ${err.stderr || err.message}`);
  }
}

const texts = new Map();

function internText(raw) {
  const text = normalizeText(raw);
  if (!text) return null;
  const id = textIdOf(text);
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

// A component that ships no license or notice file gets the standard text of each
// SPDX id it names. These notices carry `standard` so the UI can say so.
function applyStandardTexts(components) {
  for (const component of components.values()) {
    if (component.notices.length > 0) continue;
    for (const { id, text } of standardNotices(component.license, component.name, component.authors)) {
      component.notices.push({ file: `standard-${id}`, textId: internText(text), standard: id });
    }
  }
}

function generate() {
  const components = new Map();
  collectCargo(components);
  collectNpm(components);
  collectVendored(components);
  applyStandardTexts(components);
  return buildDocument(finalizeComponents(components.values()), texts);
}

const check = process.argv.includes("--check");
const output = generate();
const parsed = JSON.parse(output);
const standard = parsed.components.filter((c) => c.notices.some((n) => n.standard)).map((c) => c.name);
const missing = parsed.components.filter((c) => c.notices.length === 0).map((c) => c.name);
const summary = `${parsed.components.length} components, ${Object.keys(parsed.texts).length} distinct texts, ${standard.length} with standard license text (no license file in the package), ${missing.length} without any license text`;

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
  if (standard.length) console.log(`[notices] standard text (no license file): ${standard.join(", ")}`);
  if (missing.length) console.log(`[notices] no license text at all: ${missing.join(", ")}`);
}
