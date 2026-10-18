import { useId, useState } from "react";
import type { LocaleKey } from "../i18n/keys";
import {
  hasSpendDays,
  type ProviderChartMode,
  readChartMode,
  spendDaySummary,
  storeChartMode,
} from "../lib/openaiSpend";
import type { OpenAiApiUsageSnapshot, ProviderThroughputSnapshot } from "../types/bridge";
import { ScrubBarStrip } from "./charts/ScrubBarStrip";
import { OpenAITokensPerMinuteChart } from "./OpenAITokensPerMinuteChart";

/**
 * OpenAI API card chart with the upstream 0.75.0 Live · Spend switch
 * (`ProviderMenuChartContent`). The switch only appears when there is both a
 * Tokens/min window and daily spend to show; otherwise the card keeps the plain
 * Tokens/min grid. Both layers stay mounted in one grid cell so switching never
 * changes the card height, and the choice persists per provider.
 */

type T = (key: LocaleKey) => string;

const MODES: readonly { id: ProviderChartMode; label: LocaleKey }[] = [
  { id: "live", label: "OpenAiChartModeLive" },
  { id: "spend", label: "OpenAiChartModeSpend" },
];

export interface OpenAILiveSpendChartProps {
  providerId: string;
  throughput: ProviderThroughputSnapshot;
  usage: OpenAiApiUsageSnapshot | null | undefined;
  t: T;
}

export function OpenAILiveSpendChart({ providerId, throughput, usage, t }: OpenAILiveSpendChartProps) {
  const [mode, setModeState] = useState<ProviderChartMode>(() => readChartMode(providerId));
  const spendUsage = usage && hasSpendDays(usage) ? usage : null;
  if (!spendUsage) return <OpenAITokensPerMinuteChart throughput={throughput} t={t} />;

  const setMode = (next: ProviderChartMode) => {
    setModeState(next);
    storeChartMode(providerId, next);
  };

  return (
    <div className="openai-live-spend" data-mode={mode}>
      <div
        className="provider-detail-segmented openai-live-spend__switch"
        role="radiogroup"
        aria-label={t("OpenAiChartModeLabel")}
      >
        {MODES.map((option) => (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={option.id === mode}
            className={`provider-detail-segmented__option${option.id === mode ? " is-active" : ""}`}
            onClick={() => setMode(option.id)}
          >
            {t(option.label)}
          </button>
        ))}
      </div>
      <div className="openai-live-spend__stack">
        <div
          className={`openai-live-spend__layer${mode === "live" ? "" : " is-hidden"}`}
          aria-hidden={mode !== "live"}
          data-testid="openai-live-layer"
        >
          <OpenAITokensPerMinuteChart throughput={throughput} t={t} />
        </div>
        <div
          className={`openai-live-spend__layer${mode === "spend" ? "" : " is-hidden"}`}
          aria-hidden={mode !== "spend"}
          data-testid="openai-spend-layer"
        >
          <OpenAISpendChart usage={spendUsage} t={t} />
        </div>
      </div>
    </div>
  );
}

function OpenAISpendChart({ usage, t }: { usage: OpenAiApiUsageSnapshot; t: T }) {
  const summaryId = useId();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const bars = usage.daily.map((day) => ({ id: String(day.startTime), value: day.costUsd }));
  const selectedDay = usage.daily.find((day) => String(day.startTime) === selectedId) ?? null;
  const title = t("OpenAiSpendChartTitle").replace("{}", String(usage.historyDays));

  return (
    <section className="openai-tpm openai-spend" aria-label={title}>
      <div className="openai-tpm__title">{title}</div>
      <ScrubBarStrip
        bars={bars}
        selectedId={selectedId}
        onSelect={setSelectedId}
        classPrefix="openai-tpm"
        className="openai-tpm__chart"
        label={title}
        describedBy={summaryId}
        testId="openai-spend-chart"
      />
      <p id={summaryId} className="openai-tpm__summary" aria-live="polite">
        {selectedDay ? spendDaySummary(selectedDay, t) : "\u00a0"}
      </p>
    </section>
  );
}
