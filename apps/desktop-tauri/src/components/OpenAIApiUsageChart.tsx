import { type KeyboardEvent, useEffect, useId, useMemo, useRef, useState } from "react";
import type { LocaleKey } from "../i18n/keys";
import type {
  OpenAiApiDailyUsageSnapshot,
  OpenAiApiUsageSnapshot,
} from "../types/bridge";
import { dayLabel, formatUsd } from "../lib/openaiSpend";
import { BarChart } from "./charts/BarChart";
import { providerCostColor } from "./charts/chartPalette";

/**
 * Per-UTC-day chart for the OpenAI Admin API (upstream 0.66.0). Cost or token
 * bars, one per day; hovering or focusing a bar selects the day and fills the
 * detail panel (requests, token split, line items, models). USD only.
 */

type Metric = "cost" | "tokens";
type T = (key: LocaleKey) => string;

const METRICS: readonly Metric[] = ["cost", "tokens"];
const MAX_DETAIL_ROWS = 5;

const compactFormat = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const countFormat = new Intl.NumberFormat("en-US");

function metricValue(day: OpenAiApiDailyUsageSnapshot, metric: Metric): number {
  return metric === "cost" ? day.costUsd : day.totalTokens;
}

function formatMetric(value: number, metric: Metric): string {
  return metric === "cost" ? formatUsd(value) : compactFormat.format(value);
}

function windowLabel(days: number, t: T): string {
  return days === 1
    ? t("OpenAIChartWindowToday")
    : t("OpenAIChartWindowDays").replace("{}", String(days));
}

interface DetailRow {
  key: string;
  name: string;
  value: string;
}

function DetailList({
  title,
  rows,
  t,
}: {
  title: string;
  rows: DetailRow[];
  t: T;
}) {
  if (rows.length === 0) return null;
  const hidden = rows.length - MAX_DETAIL_ROWS;
  return (
    <div className="openai-usage__list">
      <div className="openai-usage__list-title">{title}</div>
      <ul>
        {rows.slice(0, MAX_DETAIL_ROWS).map((row) => (
          <li key={row.key}>
            <span className="openai-usage__name">{row.name}</span>
            <span className="openai-usage__amount">{row.value}</span>
          </li>
        ))}
        {hidden > 0 && (
          <li className="openai-usage__more">
            {t("OpenAIChartMore").replace("{}", String(hidden))}
          </li>
        )}
      </ul>
    </div>
  );
}

function DayDetail({ day, t }: { day: OpenAiApiDailyUsageSnapshot; t: T }) {
  const stats: Array<[LocaleKey, string]> = [
    ["OpenAIChartMetricCost", formatUsd(day.costUsd)],
    ["OpenAIChartRequests", countFormat.format(day.requests)],
    ["OpenAIChartInputTokens", countFormat.format(day.inputTokens)],
    ["OpenAIChartCachedTokens", countFormat.format(day.cachedInputTokens)],
    ["OpenAIChartOutputTokens", countFormat.format(day.outputTokens)],
  ];
  const lineItems = day.lineItems.map((item, index) => ({
    key: `${item.name}-${index}`,
    name: item.name,
    value: formatUsd(item.costUsd),
  }));
  const models = day.models.map((model, index) => ({
    key: `${model.name}-${index}`,
    name: model.name,
    value: `${compactFormat.format(model.totalTokens)} · ${countFormat.format(model.requests)} ${t(
      "UsageSpendRequests",
    )}`,
  }));
  return (
    <div className="openai-usage__detail" aria-live="polite">
      <div className="openai-usage__day">{dayLabel(day)}</div>
      <dl className="openai-usage__stats">
        {stats.map(([labelKey, value]) => (
          <div key={labelKey}>
            <dt>{t(labelKey)}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      <DetailList title={t("OpenAIChartLineItems")} rows={lineItems} t={t} />
      <DetailList title={t("OpenAIChartModels")} rows={models} t={t} />
    </div>
  );
}

export interface OpenAIApiUsageChartProps {
  usage: OpenAiApiUsageSnapshot;
  animations: boolean;
  t: T;
  /** Popover hosts resize to content; called after the detail panel height may change. */
  onLayoutChange?: () => void;
}

export function OpenAIApiUsageChart({
  usage,
  animations,
  t,
  onLayoutChange,
}: OpenAIApiUsageChartProps) {
  const [metric, setMetric] = useState<Metric>("cost");
  // Selection is keyed by bucket start so it survives a refresh; null = latest day.
  const [selectedStart, setSelectedStart] = useState<number | null>(null);
  const baseId = useId();
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const days = usage.daily;
  const points = useMemo(
    () =>
      days.map((day) => ({
        label: dayLabel(day),
        value: metricValue(day, metric),
      })),
    [days, metric],
  );

  const layoutKey = `${metric}:${selectedStart ?? "latest"}:${days.length}`;
  const layoutCallback = useRef(onLayoutChange);
  layoutCallback.current = onLayoutChange;
  useEffect(() => {
    const frame = requestAnimationFrame(() => layoutCallback.current?.());
    return () => cancelAnimationFrame(frame);
  }, [layoutKey]);

  if (days.length === 0) {
    return (
      <div className="chart chart--bar">
        <div className="chart__empty">{t("DetailChartEmpty")}</div>
      </div>
    );
  }

  const found = days.findIndex((day) => day.startTime === selectedStart);
  const selectedIndex = found >= 0 ? found : days.length - 1;
  const total = days.reduce((sum, day) => sum + metricValue(day, metric), 0);
  const tabLabel = (m: Metric) =>
    t(m === "cost" ? "OpenAIChartMetricCost" : "OpenAIChartMetricTokens");
  const tabId = (m: Metric) => `${baseId}-tab-${m}`;
  const panelId = `${baseId}-panel`;

  const onTabKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    const current = METRICS.indexOf(metric);
    let next: number;
    if (e.key === "ArrowLeft") next = (current + METRICS.length - 1) % METRICS.length;
    else if (e.key === "ArrowRight") next = (current + 1) % METRICS.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = METRICS.length - 1;
    else return;
    e.preventDefault();
    setMetric(METRICS[next]);
    tabRefs.current[next]?.focus();
  };

  return (
    <div className="openai-usage">
      <div className="openai-usage__tabs" role="tablist" aria-label={t("OpenAIChartTitle")}>
        {METRICS.map((m, index) => (
          <button
            key={m}
            ref={(node) => {
              tabRefs.current[index] = node;
            }}
            id={tabId(m)}
            type="button"
            role="tab"
            aria-selected={m === metric}
            aria-controls={panelId}
            tabIndex={m === metric ? 0 : -1}
            className="openai-usage__tab"
            data-active={m === metric ? "true" : "false"}
            onClick={() => setMetric(m)}
            onKeyDown={onTabKeyDown}
          >
            {tabLabel(m)}
          </button>
        ))}
      </div>
      <div id={panelId} role="tabpanel" aria-labelledby={tabId(metric)} className="openai-usage__panel">
        <BarChart
          data={points}
          color={providerCostColor("openaiapi")}
          ariaLabel={`${t("OpenAIChartTitle")}: ${tabLabel(metric)}`}
          valueFormatter={(v) => formatMetric(v, metric)}
          animations={animations}
          emptyMessage={t("DetailChartEmpty")}
          scrollable
          selection={{
            index: selectedIndex,
            onSelect: (index) => setSelectedStart(days[index]?.startTime ?? null),
          }}
        />
        <div className="openai-usage__hint">{t("OpenAIChartHint")}</div>
        <DayDetail day={days[selectedIndex]} t={t} />
        <div className="openai-usage__footer">
          <span>{windowLabel(usage.historyDays, t)}</span>
          <strong>{formatMetric(total, metric)}</strong>
        </div>
      </div>
    </div>
  );
}
