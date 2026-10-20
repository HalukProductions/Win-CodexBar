/** One third-party component with its verbatim license and notice texts. */
export interface Acknowledgement {
  name: string;
  ecosystem: "cargo" | "npm" | "vendored";
  versions: string[];
  authors: string[];
  url: string;
  license: string;
  /** `standard` names the SPDX id when the text is the standard one; the package ships no license file. */
  notices: Array<{ file: string; text: string; standard?: string }>;
}

interface NoticesFile {
  schemaVersion: number;
  components: Array<Omit<Acknowledgement, "notices"> & {
    notices: Array<{ file: string; textId: string; standard?: string }>;
  }>;
  texts: Record<string, string>;
}

/** Resolves the deduplicated text table into per-component notices. */
export function parseNotices(raw: string): Acknowledgement[] {
  const data = JSON.parse(raw) as NoticesFile;
  if (data.schemaVersion !== 1 || !Array.isArray(data.components)) return [];
  return data.components.map((component) => ({
    ...component,
    notices: component.notices.flatMap((notice) => {
      const text = data.texts[notice.textId];
      return text === undefined ? [] : [{ file: notice.file, text, standard: notice.standard }];
    }),
  }));
}

let acknowledgements: Promise<Acknowledgement[]> | null = null;

/**
 * The generated file is about 1 MB, so it is a lazy chunk. It is fetched and parsed
 * once and shared by every mount of the pane; a failed load is retried on the next call.
 */
export function loadAcknowledgements(): Promise<Acknowledgement[]> {
  if (!acknowledgements) {
    acknowledgements = import("../../../generated/third-party-notices.json?raw")
      .then((module) => parseNotices(module.default))
      .catch((error: unknown) => {
        acknowledgements = null;
        throw error;
      });
  }
  return acknowledgements;
}

/**
 * Up to two authors, then "N others"; a lone extra author is named rather than
 * counted. `othersTemplate` is the localized `{}` template for the count.
 */
export function authorSummary(authors: string[], othersTemplate: string): string {
  const shown =
    authors.length > 3
      ? [...authors.slice(0, 2), othersTemplate.replace("{}", String(authors.length - 2))]
      : authors;
  return shown.join(", ");
}

/**
 * SPDX ids that may link to spdx.org. Package metadata also holds free text in the
 * license field ("Unknown", "See license file", "NOASSERTION"), which must not become a link.
 */
const SPDX_IDS = new Set([
  "0BSD",
  "AGPL-3.0",
  "AGPL-3.0-only",
  "AGPL-3.0-or-later",
  "Apache-2.0",
  "Artistic-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BSL-1.0",
  "BlueOak-1.0.0",
  "CC-BY-4.0",
  "CC0-1.0",
  "CDLA-Permissive-2.0",
  "GPL-2.0",
  "GPL-2.0-only",
  "GPL-2.0-or-later",
  "GPL-3.0",
  "GPL-3.0-only",
  "GPL-3.0-or-later",
  "ISC",
  "LGPL-2.0",
  "LGPL-2.1",
  "LGPL-2.1-only",
  "LGPL-2.1-or-later",
  "LGPL-3.0",
  "LGPL-3.0-only",
  "LGPL-3.0-or-later",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "OFL-1.1",
  "OpenSSL",
  "PSF-2.0",
  "Python-2.0",
  "Unicode-3.0",
  "Unicode-DFS-2016",
  "Unlicense",
  "WTFPL",
  "Zlib",
]);

/** SPDX ids in a license expression, for components that publish no license file. */
export function spdxIds(license: string): string[] {
  const ids = license
    .split(/\s+(?:OR|AND|WITH)\s+|\s*\/\s*|[()]/)
    .map((part) => part.trim())
    .filter((part) => SPDX_IDS.has(part.replace(/\+$/, "")));
  return [...new Set(ids)];
}

export function spdxUrl(id: string): string {
  return `https://spdx.org/licenses/${id.replace(/\+$/, "")}.html`;
}
