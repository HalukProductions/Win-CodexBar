// Pure helpers for the third-party notices generator (third-party-notices.mjs).
// No collection runs here, so tests can import this module safely
// (see src/generated/third-party-notices.test.ts).

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const TEMPLATE_DIR = join(dirname(fileURLToPath(import.meta.url)), "license-templates");

// Standard texts for SPDX ids whose packages publish no license file. Each one is a
// verbatim copy of a license file already on disk in the Cargo registry:
//   MIT           adler2 2.0.1 LICENSE-MIT (body only; the copyright line is added)
//   BSD-3-Clause  alloc-no-stdlib 2.0.4 LICENSE (same author as alloc-stdlib, which
//                 ships no file; the copyright line is added)
//   Apache-2.0    adler2 2.0.1 LICENSE-APACHE
//   MPL-2.0       cssparser 0.36.0 LICENSE (identical to colored 3.1.1 LICENSE)
// The `{{copyright}}` placeholder is replaced with a line built from the package authors.
export const STANDARD_TEMPLATES = Object.freeze({
  "Apache-2.0": "Apache-2.0.txt",
  "BSD-3-Clause": "BSD-3-Clause.txt",
  MIT: "MIT.txt",
  "MPL-2.0": "MPL-2.0.txt",
});

export function normalizeText(text) {
  return text
    .replace(/^\uFEFF/, "")
    .replace(/\r\n?/g, "\n")
    .replace(/[ \t]+$/gm, "")
    .trim();
}

export function textIdOf(text) {
  return createHash("sha256").update(text).digest("hex").slice(0, 16);
}

// SPDX-looking ids named by a license expression ("MIT OR Apache-2.0", "MIT/Apache-2.0").
export function licenseIds(license) {
  const ids = license
    .split(/\s+(?:OR|AND|WITH)\s+|\s*\/\s*|[()]/)
    .map((part) => part.trim())
    .filter((part) => /^[A-Za-z0-9.+-]+$/.test(part));
  return [...new Set(ids)];
}

// Copyright line for a standard text. A package with no authors metadata gets a
// generic holder rather than an invented person.
export function copyrightLine(name, authors) {
  const holders = authors.length > 0 ? authors.join(", ") : `${name} contributors`;
  return `Copyright (c) ${holders}`;
}

export function renderStandardText(template, copyright) {
  // A replacer function keeps `$` sequences in author names literal.
  return template.replaceAll("{{copyright}}", () => copyright);
}

export function loadTemplate(id) {
  if (!Object.hasOwn(STANDARD_TEMPLATES, id)) return undefined;
  return readFileSync(join(TEMPLATE_DIR, STANDARD_TEMPLATES[id]), "utf8");
}

// The standard texts for every SPDX id in a license expression that has a template.
// Ids without one (GPL-2.0+, Unicode-3.0, free text such as "Unknown") yield nothing.
export function standardNotices(license, name, authors, readTemplate = loadTemplate) {
  return licenseIds(license).flatMap((id) => {
    const template = readTemplate(id);
    if (template === undefined) return [];
    return [{ id, text: renderStandardText(template, copyrightLine(name, authors)) }];
  });
}

export function mergeComponent(map, key, component) {
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

export function compareVersions(a, b) {
  return a.localeCompare(b, "en", { numeric: true });
}

export function sortNotices(notices) {
  return [...notices].sort(
    (a, b) => a.file.localeCompare(b.file, "en") || a.textId.localeCompare(b.textId, "en"),
  );
}

// Sorted, with versions and notices in canonical order. Components sort by name
// (case-insensitive), then ecosystem.
export function finalizeComponents(components) {
  return [...components]
    .map((c) => ({
      ...c,
      versions: [...c.versions].sort(compareVersions),
      notices: sortNotices(c.notices),
    }))
    .sort(
      (a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase(), "en") ||
        a.ecosystem.localeCompare(b.ecosystem, "en"),
    );
}

// The committed JSON. `texts` is the interned table; only texts that a component
// references are written.
export function buildDocument(components, texts) {
  const used = new Set(components.flatMap((c) => c.notices.map((n) => n.textId)));
  const textEntries = [...texts]
    .filter(([id]) => used.has(id))
    .sort(([a], [b]) => a.localeCompare(b, "en"));
  return `${JSON.stringify(
    { schemaVersion: 1, components, texts: Object.fromEntries(textEntries) },
    null,
    2,
  )}\n`;
}
