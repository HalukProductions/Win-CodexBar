import { useState } from "react";
import type {
  MetricPreference,
  ProviderDetail,
  SettingsSnapshot,
  SettingsUpdate,
} from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";
import { localizeWindowLabel } from "../../../../lib/windowLabels";
import { Field, Select, SettingsGroup } from "../../../../components/FormControls";

interface Props {
  provider: ProviderDetail;
  providerMetrics: SettingsSnapshot["providerMetrics"];
  disabled: boolean;
  t: (key: LocaleKey) => string;
  onChange: (patch: SettingsUpdate) => void;
}

interface MetricOption {
  value: MetricPreference;
  label: string;
}

const AUTOMATIC_ONLY_PROVIDERS: ReadonlySet<string> = new Set(["aixy"]);

export function MenuBarMetricSection({
  provider,
  providerMetrics,
  disabled,
  t,
  onChange,
}: Props) {
  const [error, setError] = useState<string | null>(null);
  const selected = AUTOMATIC_ONLY_PROVIDERS.has(provider.id)
    ? "automatic"
    : providerMetrics[provider.id] ?? "automatic";
  const options = metricOptions(provider, selected, t);

  const handleChange = (value: MetricPreference) => {
    setError(null);
    try {
      onChange({
        providerMetrics: {
          ...providerMetrics,
          [provider.id]: value,
        },
      });
    } catch (e) {
      setError(String(e));
    }
  };

  // Mac shows the picker only when there is a choice to make; a saved choice
  // the provider no longer offers adds an option, so it stays visible then.
  if (options.length <= 1) return null;

  return (
    <SettingsGroup
      className="provider-detail-group provider-metric-group"
      footer={t("MenuBarMetricHelper")}
    >
      <Field label={t("MenuBarMetric")}>
        <Select
          variant="menu"
          value={selected}
          options={options}
          disabled={disabled}
          onChange={(value) => handleChange(value as MetricPreference)}
        />
      </Field>
      {error && <p className="provider-option-error">{error}</p>}
    </SettingsGroup>
  );
}

function metricOptions(
  provider: ProviderDetail,
  selected: MetricPreference,
  t: (key: LocaleKey) => string,
): MetricOption[] {
  // Aixy's primary budget depends on which limits currently apply to the key,
  // so a fixed session/weekly lane would be misleading. Offer Automatic only.
  if (AUTOMATIC_ONLY_PROVIDERS.has(provider.id)) {
    return [{ value: "automatic", label: t("Automatic") }];
  }

  const options: MetricOption[] = [
    { value: "automatic", label: t("Automatic") },
    {
      value: "session",
      label:
        provider.primaryMetricLabel ??
        (localizeWindowLabel(provider.primaryLabel ?? undefined, t) || t("ProviderSessionLabel")),
    },
  ];

  if (provider.weekly) {
    options.push({ value: "weekly", label: localizeWindowLabel(provider.secondaryLabel ?? undefined, t) || t("ProviderWeeklyLabel") });
  }
  if (provider.modelSpecific) {
    options.push({ value: "model", label: t("DetailWindowModelSpecific") });
  }
  if (provider.tertiary || provider.tertiaryLabelKey) {
    options.push({
      value: "tertiary",
      label: t(
        (provider.tertiaryLabelKey ?? "DetailWindowTertiary") as LocaleKey,
      ),
    });
  }
  // The monthly plan window has its own choice below, so it does not make
  // Extra usage available on its own.
  const extraWindows = provider.extraRateWindows.filter(
    (extra) => extra.id !== provider.monthlyPlanWindowId,
  );
  if (provider.id === "cursor" || extraWindows.length > 0) {
    options.push({ value: "extraUsage", label: t("ExtraUsage") });
  }
  // Upstream 0.70.0 (#4072): providers that publish a monthly plan
  // allowance window offer it for the menu bar and widgets.
  if (provider.monthlyPlanWindowId) {
    options.push({ value: "monthlyPlan", label: t("MetricMonthlyPlan") });
  }
  if (provider.id === "gemini" && provider.weekly) {
    options.push({ value: "average", label: t("Average") });
  }
  // Upstream 0.75.0 (#4495): the latest Tokens/min from Live usage. The tray
  // meter keeps the Automatic lane; the tray status text shows the rate.
  if (provider.tokensPerMinuteMetric) {
    options.push({ value: "tokensPerMinute", label: t("MetricTokensPerMinute") });
  }
  // Upstream 0.73.0 (#4269): provider-declared named allowances, offered
  // before the first refresh. The live window title wins over the declared one.
  for (const named of provider.namedExtraMetrics ?? []) {
    const live = provider.extraRateWindows.find((extra) => extra.id === named.id);
    options.push({
      value: `extra:${named.id}`,
      label: live?.title.trim() || named.label,
    });
  }
  if (!options.some((option) => option.value === selected)) {
    const labelKey = SAVED_ONLY_LABEL_KEYS[selected];
    options.push({
      value: selected,
      label: labelKey ? t(labelKey) : savedOnlyLabel(selected),
    });
  }

  return options;
}

/** A retired named allowance keeps its window id visible, not the storage prefix. */
function savedOnlyLabel(selected: MetricPreference): string {
  return selected.startsWith("extra:") ? selected.slice("extra:".length) : selected;
}

/** Labels for saved choices the provider no longer offers. */
const SAVED_ONLY_LABEL_KEYS: Partial<Record<MetricPreference, LocaleKey>> = {
  credits: "CreditsLabel",
  extraUsage: "ExtraUsage",
  monthlyPlan: "MetricMonthlyPlan",
  tokensPerMinute: "MetricTokensPerMinute",
};
