import { describe, expect, it } from "vitest";
import { formatDisplayCurrency, sumDisplayCurrencyAmounts } from "./currency";

// Rust owns the currency model (`codexbar::currency`); these rates mirror the
// merged snapshot the backend returns so the format tests stay deterministic.
const RATES: Record<string, number> = {
  USD: 1,
  GBP: 0.79,
  EUR: 0.92,
  TRY: 48.5,
  // The twelve 0.67.0 additions (upstream fallback rates, 2026-09-24).
  NZD: 1.761,
  SEK: 9.908,
  NOK: 9.48,
  DKK: 6.554,
  PLN: 3.838,
  BRL: 5.117,
  MXN: 17.47,
  ZAR: 16.36,
  THB: 33.37,
  IDR: 17836,
  VND: 25962,
  UAH: 44.86,
  // Upstream ILS fallback rate (open.er-api.com, 2026-10-19).
  ILS: 3.109,
};

describe("preferred currency display", () => {
  it("converts through the USD pivot and formats with Intl", () => {
    const display = formatDisplayCurrency(10, "USD", "TRY", RATES);
    expect(display).not.toContain("10.00");
    expect(display).toMatch(/485/);
  });

  it("converts each added currency through the USD pivot", () => {
    for (const code of ["NZD", "SEK", "NOK", "DKK", "PLN", "BRL", "MXN", "ZAR", "THB", "IDR", "VND", "UAH", "ILS"]) {
      const rate = RATES[code];
      // The rendered string carries the Intl symbol prefix (e.g. "NZ$17.61",
      // "17 618 Rp"), so assert on the exact converted digits.
      const expected = new Intl.NumberFormat(undefined, {
        style: "currency",
        currency: code,
      }).format(10 * rate);
      expect(formatDisplayCurrency(10, "USD", code, RATES)).toBe(expected);
      // Round trip back through the pivot.
      expect(formatDisplayCurrency(10 * rate, code, "USD", RATES)).toMatch(/10(\.00)?$/);
    }
    expect(formatDisplayCurrency(10, "EUR", "SEK", RATES)).toMatch(/107\.7/);
  });

  it("formats zero-decimal currencies without fractional units", () => {
    const vnd = formatDisplayCurrency(0.01, "USD", "VND", RATES);
    expect(vnd).toMatch(/260/);
    expect(vnd).not.toMatch(/[.,]\d{1,2}\D*$/);
    expect(formatDisplayCurrency(1, "USD", "JPY", { ...RATES, JPY: 154 })).toMatch(/154(?![.,]\d)/);
  });

  it("keeps AUTO, credits, unknown units, and missing-rate values in source units", () => {
    expect(formatDisplayCurrency(4.25, "USD", "AUTO", RATES)).toMatch(/4\.25/);
    expect(formatDisplayCurrency(4.25, "Credits", "TRY", RATES)).toBe("4.25 Credits");
    expect(formatDisplayCurrency(10, "USD", "TRY", {}, "$")).toBe("$10.00");
  });

  it("prefers the source symbol when both codes match", () => {
    expect(formatDisplayCurrency(4.25, "USD", "USD", RATES, "$")).toBe("$4.25");
  });

  it("sums only converted overview rows and reports incomplete coverage", () => {
    const result = sumDisplayCurrencyAmounts(
      [
        { amount: 10, currency: "USD" },
        { amount: 10, currency: "EUR" },
        { amount: 5, currency: "Credits" },
      ],
      "TRY",
      RATES,
    );
    expect(result.included).toBe(2);
    expect(result.considered).toBe(3);
    expect(result.total).toBeCloseTo(10 * 48.5 + (10 / 0.92) * 48.5, 8);
  });

  it("sums USD-only rows unchanged under AUTO", () => {
    const result = sumDisplayCurrencyAmounts(
      [
        { amount: 10, currency: "USD" },
        { amount: 10, currency: "EUR" },
      ],
      "AUTO",
      RATES,
    );
    expect(result.included).toBe(1);
    expect(result.total).toBe(10);
  });

  it("sums a row in each added currency into the TRY total", () => {
    const result = sumDisplayCurrencyAmounts(
      [
        { amount: 10, currency: "NZD" },
        { amount: 10, currency: "SEK" },
        { amount: 10, currency: "UAH" },
      ],
      "TRY",
      RATES,
    );
    expect(result.included).toBe(3);
    expect(result.considered).toBe(3);
    const expected = ((10 / 1.761) + (10 / 9.908) + (10 / 44.86)) * 48.5;
    expect(result.total).toBeCloseTo(expected, 6);
  });
});
