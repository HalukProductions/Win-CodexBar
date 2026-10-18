// Type declarations for third-party-notices-lib.mjs, so the TypeScript test under src/ can import it.

export interface NoticeRef {
  file: string;
  textId: string;
  /** SPDX id whose standard text was used, when the package ships no license file. */
  standard?: string;
}

export interface NoticeComponent {
  name: string;
  ecosystem: string;
  versions: string[];
  authors: string[];
  url: string;
  license: string;
  notices: NoticeRef[];
}

export const TEMPLATE_DIR: string;
export const STANDARD_TEMPLATES: Readonly<Record<string, string>>;

export function normalizeText(text: string): string;
export function textIdOf(text: string): string;
export function licenseIds(license: string): string[];
export function copyrightLine(name: string, authors: string[]): string;
export function renderStandardText(template: string, copyright: string): string;
export function loadTemplate(id: string): string | undefined;
export function standardNotices(
  license: string,
  name: string,
  authors: string[],
  readTemplate?: (id: string) => string | undefined,
): Array<{ id: string; text: string }>;
export function mergeComponent(
  map: Map<string, NoticeComponent>,
  key: string,
  component: NoticeComponent,
): void;
export function compareVersions(a: string, b: string): number;
export function sortNotices(notices: NoticeRef[]): NoticeRef[];
export function finalizeComponents(components: Iterable<NoticeComponent>): NoticeComponent[];
export function buildDocument(components: NoticeComponent[], texts: Map<string, string>): string;
