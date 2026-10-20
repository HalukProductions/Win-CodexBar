import { useId, useState } from "react";
import type { LocaleKey } from "../i18n/keys";
import {
  formatClock,
  formatTokens,
  hasReportedTraffic,
  throughputLimitLine,
  tpmBarSummary,
  tpmBars,
} from "../lib/openaiThroughput";
import { ScrubBarStrip } from "./charts/ScrubBarStrip";
import type {
  ModelRateLimitSnapshot,
  ProviderThroughputSnapshot,
} from "../types/bridge";

/**
 * Last-hour Tokens/min grid for the OpenAI API (upstream 0.75.0 #4495). Reported
 * minutes are solid bars; minutes after the reporting frontier are drawn as
 * pending outlines and never count as zero or as a peak. Hovering, dragging or
 * arrowing across the grid inspects one minute in the summary line, which keeps
 * its height while empty so the card does not jump.
 */

type T = (key: LocaleKey) => string;

export interface OpenAITokensPerMinuteChartProps {
  throughput: ProviderThroughputSnapshot;
  t: T;
  /** Manual organization limits; the limit line needs an organization-scoped snapshot. */
  limits?: readonly ModelRateLimitSnapshot[];
  /** Per-model rows; the card omits them, the Live Usage view shows them. */
  showModels?: boolean;
}

const NO_LIMITS: readonly ModelRateLimitSnapshot[] = [];

export function OpenAITokensPerMinuteChart({
  throughput,
  t,
  limits = NO_LIMITS,
  showModels = false,
}: OpenAITokensPerMinuteChartProps) {
  const summaryId = useId();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const bars = tpmBars(throughput);
  const stripBars = bars.map((bar) => ({
    id: String(bar.start),
    value: bar.tokens,
    isPending: bar.isPending,
  }));
  const selectedBar = bars.find((bar) => String(bar.start) === selectedId) ?? null;
  const reported = hasReportedTraffic(throughput);
  const hasPending = bars.some((bar) => bar.isPending);
  const limit = throughputLimitLine(throughput, limits);

  return (
    <section className="openai-tpm" aria-label={t("OpenAiTpmChartTitle")}>
      <div className="openai-tpm__title">{t("OpenAiTpmChartTitle")}</div>
      <dl className="openai-tpm__stats">
        <div>
          <dt>{t("OpenAiTpmLatest")}</dt>
          <dd>
            {throughput.latestTokensPerMinute == null
              ? "—"
              : formatTokens(throughput.latestTokensPerMinute)}
          </dd>
        </div>
        <div>
          <dt>{t("OpenAiTpmPeak")}</dt>
          <dd>
            {formatTokens(throughput.peakTokensPerMinute)}
            {throughput.peakAt != null && (
              <span className="openai-tpm__muted"> {formatClock(throughput.peakAt)}</span>
            )}
          </dd>
        </div>
        <div>
          <dt>{t("OpenAiTpmFiveMinuteAverage")}</dt>
          <dd>{formatTokens(throughput.fiveMinuteAverageTokensPerMinute)}</dd>
        </div>
        {throughput.frontier != null && (
          <div>
            <dt>{t("OpenAiTpmFrontier")}</dt>
            <dd>{formatClock(throughput.frontier)}</dd>
          </div>
        )}
      </dl>

      {reported && bars.length > 0 ? (
        <>
          <ScrubBarStrip
            bars={stripBars}
            selectedId={selectedId}
            onSelect={setSelectedId}
            classPrefix="openai-tpm"
            className="openai-tpm__chart"
            label={t("OpenAiTpmChartTitle")}
            describedBy={summaryId}
            testId="openai-tpm-chart"
          />
          <p id={summaryId} className="openai-tpm__summary" aria-live="polite">
            {selectedBar ? tpmBarSummary(selectedBar, t) : "\u00a0"}
          </p>
        </>
      ) : (
        <p className="openai-tpm__empty">{t("OpenAiTpmEmpty")}</p>
      )}

      {hasPending && (
        <div className="openai-tpm__legend">
          <span className="openai-tpm__swatch openai-tpm__swatch--pending" aria-hidden="true" />
          {t("OpenAiTpmPendingLegend")}
        </div>
      )}

      {limit && (
        <p className="openai-tpm__limit">
          {t("OpenAiTpmLimitLine")
            .replace("{}", limit.model)
            .replace("{}", String(Math.round(limit.percent)))
            .replace("{}", formatTokens(limit.tokensPerMinuteLimit))}
        </p>
      )}

      {showModels && throughput.models.length > 0 && (
        <div className="openai-tpm__models">
          <div className="openai-tpm__title">{t("OpenAiTpmModels")}</div>
          <ul>
            {throughput.models.map((model) => (
              <li key={model.name}>
                <span className="openai-tpm__model-name">{model.name}</span>
                <span className="openai-tpm__muted">
                  {t("OpenAiTpmLatest")} {formatTokens(model.latestTpm)} · {t("OpenAiTpmPeak")}{" "}
                  {formatTokens(model.peakTpm)}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
