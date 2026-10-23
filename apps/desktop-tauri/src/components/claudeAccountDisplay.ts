import type { ClaudeAccount } from "../types/bridge";

export interface PrivateClaudeAccountLabel {
  label: string;
  tooltip: string;
}

/**
 * Number accounts 1..n in the order they are listed, so the first row shown is
 * always "Account 1". Pass the list in display order. Saved accounts keep their
 * stored order across switches, so a label only changes when the list itself
 * changes. The tray numbers Claude and Grok accounts by the same rule
 * (`ordinals_in_list_order`).
 */
export function buildAccountOrdinals(
  accounts: readonly { id: string }[],
): Record<string, number> {
  const ids = [...new Set(accounts.map((account) => account.id))];
  return Object.fromEntries(ids.map((id, index) => [id, index + 1]));
}

export function buildPrivateClaudeAccountLabel(
  account: ClaudeAccount,
  ordinal: number | undefined,
  hidePersonalInfo: boolean,
  accountWord: string,
): PrivateClaudeAccountLabel {
  if (hidePersonalInfo) {
    const label = ordinal === undefined ? "••••" : `${accountWord} ${ordinal}`;
    return { label, tooltip: label };
  }

  return { label: account.email, tooltip: account.email };
}
