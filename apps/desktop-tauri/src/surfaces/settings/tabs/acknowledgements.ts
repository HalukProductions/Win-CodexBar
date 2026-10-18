/** One third-party component with its verbatim license and notice texts. */
export interface Acknowledgement {
  name: string;
  ecosystem: "cargo" | "npm" | "vendored";
  versions: string[];
  authors: string[];
  url: string;
  license: string;
  notices: Array<{ file: string; text: string }>;
}

interface NoticesFile {
  schemaVersion: number;
  components: Array<Omit<Acknowledgement, "notices"> & {
    notices: Array<{ file: string; textId: string }>;
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
      return text === undefined ? [] : [{ file: notice.file, text }];
    }),
  }));
}

/** The generated file is about 1 MB, so it is a lazy chunk loaded on first open. */
export async function loadAcknowledgements(): Promise<Acknowledgement[]> {
  const module = await import("../../../generated/third-party-notices.json?raw");
  return parseNotices(module.default);
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

/** SPDX ids in a license expression, for components that publish no license file. */
export function spdxIds(license: string): string[] {
  const ids = license
    .split(/\s+(?:OR|AND|WITH)\s+|\s*\/\s*|[()]/)
    .map((part) => part.trim())
    .filter((part) => /^[A-Za-z0-9.+-]+$/.test(part));
  return [...new Set(ids)];
}

export function spdxUrl(id: string): string {
  return `https://spdx.org/licenses/${id.replace(/\+$/, "")}.html`;
}
