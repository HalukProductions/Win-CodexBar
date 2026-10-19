import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type {
  BootstrapState,
  SettingsSurfaceTarget,
  SettingsTabId,
  SettingsUpdate,
} from "../types/bridge";
import { useSettings } from "../hooks/useSettings";
import { useSurfaceTarget } from "../hooks/useSurfaceMode";
import { useLocale } from "../hooks/useLocale";
import { useProviders } from "../hooks/useProviders";
import { closeSettingsWindow, getWorkAreaRect, setSurfaceMode } from "../lib/tauri";
import {
  TAB_META,
  filterProvidersByQuery,
  parseSettingsTabRequest,
  providersTabRequest,
  type SettingsTabRequest,
} from "./settings/settingsTabs";
import { SettingsSidebar } from "./settings/SettingsSidebar";
import { useSettingsHistory } from "./settings/useSettingsHistory";
import GeneralTab from "./settings/tabs/GeneralTab";
import MenuBarTab from "./settings/tabs/MenuBarTab";
import MenuTab from "./settings/tabs/MenuTab";
import AdvancedTab from "./settings/tabs/AdvancedTab";
import HooksTab from "./settings/tabs/HooksTab";
import AboutTab from "./settings/tabs/AboutTab";
import ProvidersTab from "./settings/tabs/ProvidersTab";
import UsageSpendTab from "./settings/tabs/UsageSpendTab";

// Mirrors `SETTINGS_WINDOW_WIDTH` / `SETTINGS_WINDOW_HEIGHT` in src-tauri `surface.rs`
// (macOS `PreferencesView` 880x620).
const SETTINGS_WINDOW_HEIGHT = 620;
const SETTINGS_WINDOW_WIDTH = 880;

async function applySettingsWindowSize() {
  const workArea = await getWorkAreaRect().catch(() => null);
  const screenWidth = window.screen.availWidth || window.innerWidth || SETTINGS_WINDOW_WIDTH;
  const screenHeight = window.screen.availHeight || window.innerHeight || SETTINGS_WINDOW_HEIGHT;
  const maxWidth = Math.min(workArea?.width ?? screenWidth, screenWidth);
  const maxHeight = Math.min(workArea?.height ?? screenHeight, screenHeight);
  const width = Math.max(
    360,
    Math.min(SETTINGS_WINDOW_WIDTH, maxWidth - 16),
  );
  const height = Math.max(
    360,
    Math.min(SETTINGS_WINDOW_HEIGHT, maxHeight - 16),
  );
  const win = getCurrentWindow();
  await win.setSize(new LogicalSize(width, height)).catch(() => {});
  const screenOrigin = window.screen as Screen & {
    availLeft?: number;
    availTop?: number;
  };
  const left = screenOrigin.availLeft ?? workArea?.x ?? 0;
  const top = screenOrigin.availTop ?? workArea?.y ?? 0;
  await win
    .setPosition(
      new LogicalPosition(
        left + Math.max(8, Math.round((screenWidth - width) / 2)),
        top + Math.max(8, Math.round((screenHeight - height) / 2)),
      ),
    )
    .catch(() => {});
}

export default function Settings({
  state,
  initialTab: propTab,
  tabRequestId = 0,
}: {
  state: BootstrapState;
  /** Raw tab request: a tab id, or `providers:<id>` to select a provider. */
  initialTab?: string;
  /** Changes on every window request, so a repeated deep link applies again. */
  tabRequestId?: number;
}) {
  const { settings, saving, error, update } = useSettings(state.settings);
  const { providers: snapshots } = useProviders();
  const { t } = useLocale();
  const shellTarget = useSurfaceTarget("settings");
  const propRequest = parseSettingsTabRequest(propTab);
  // Proof mode and the main-window surface carry the same raw tab string.
  const rawShellTab = shellTarget?.kind === "settings" ? shellTarget.tab : null;
  const shellRequest = parseSettingsTabRequest(rawShellTab);
  const firstRequest: SettingsTabRequest | null = propRequest ?? shellRequest;
  const [activeTab, setActiveTab] = useState<SettingsTabId>(firstRequest?.tab ?? "general");
  // Last provider picked; a bare `providers` request keeps it.
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(
    firstRequest?.providerId ?? null,
  );
  const [searchText, setSearchText] = useState("");
  const [prevPropRequest, setPrevPropRequest] = useState({ tab: propTab, id: tabRequestId });
  const [prevShellTab, setPrevShellTab] = useState(rawShellTab);

  // Adjust local state during render when external drivers change (no effect sync).
  if (propTab !== prevPropRequest.tab || tabRequestId !== prevPropRequest.id) {
    setPrevPropRequest({ tab: propTab, id: tabRequestId });
    if (propRequest) {
      setActiveTab(propRequest.tab);
      if (propRequest.providerId) {
        setSelectedProviderId(propRequest.providerId);
        // A deep link must not land on a provider the search hides.
        setSearchText("");
      }
    }
  }
  if (rawShellTab !== prevShellTab) {
    setPrevShellTab(rawShellTab);
    if (shellRequest) {
      setActiveTab(shellRequest.tab);
      const id = shellRequest.providerId;
      if (id) {
        setSelectedProviderId(id);
        // Clear a search that hides the target. The main window's echo of the
        // user's own row click names a visible row, so that search is kept.
        const target = state.providers.filter((p) => p.id === id);
        if (filterProvidersByQuery(target, searchText).length === 0) setSearchText("");
      }
    }
  }

  useEffect(() => {
    void applySettingsWindowSize();
  }, []);

  const catalog = state.providers;
  const providerId =
    selectedProviderId && catalog.some((p) => p.id === selectedProviderId)
      ? selectedProviderId
      : (catalog[0]?.id ?? null);
  const providerName = catalog.find((p) => p.id === providerId)?.displayName ?? null;
  const paneLabelKey = TAB_META.find((tab) => tab.id === activeTab)?.labelKey ?? "TabGeneral";
  const title = activeTab === "providers" && providerName ? providerName : t(paneLabelKey);

  const set = (patch: SettingsUpdate) => void update(patch);
  const showInMainWindow = useCallback((tab: SettingsSurfaceTarget["tab"]) => {
    // Only drive the main window's surface when this is not the detached window.
    if (getCurrentWebviewWindow().label !== "settings") {
      void setSurfaceMode("settings", { kind: "settings", tab });
    }
  }, []);
  const handleSelectPane = useCallback(
    (tab: SettingsTabId) => {
      setActiveTab(tab);
      showInMainWindow(tab);
    },
    [showInMainWindow],
  );
  const handleSelectProvider = useCallback(
    (id: string) => {
      setActiveTab("providers");
      setSelectedProviderId(id);
      showInMainWindow(providersTabRequest(id));
    },
    [showInMainWindow],
  );
  const history = useSettingsHistory(
    { tab: activeTab, providerId: activeTab === "providers" ? providerId : null },
    (pane) => {
      if (pane.tab !== "providers" || !pane.providerId) {
        handleSelectPane(pane.tab);
        return;
      }
      handleSelectProvider(pane.providerId);
      const target = catalog.filter((p) => p.id === pane.providerId);
      if (filterProvidersByQuery(target, searchText).length === 0) setSearchText("");
    },
  );

  return (
    <div className="settings settings--sidebar">
      {/* custom title bar (decorations disabled for guaranteed dark theme) */}
      <div className="settings-titlebar" data-tauri-drag-region>
        <div className="settings-titlebar__history">
          <button
            type="button"
            className="settings-titlebar__control settings-titlebar__control--history"
            onClick={history.goBack}
            disabled={!history.canGoBack}
            aria-label={t("SettingsHistoryBack")}
            title={t("SettingsHistoryBack")}
            data-testid="settings-history-back"
          >
            <svg aria-hidden viewBox="0 0 16 16" focusable="false">
              <path d="M10 3.5L5.5 8l4.5 4.5" />
            </svg>
          </button>
          <button
            type="button"
            className="settings-titlebar__control settings-titlebar__control--history"
            onClick={history.goForward}
            disabled={!history.canGoForward}
            aria-label={t("SettingsHistoryForward")}
            title={t("SettingsHistoryForward")}
            data-testid="settings-history-forward"
          >
            <svg aria-hidden viewBox="0 0 16 16" focusable="false">
              <path d="M6 3.5l4.5 4.5L6 12.5" />
            </svg>
          </button>
        </div>
        <span
          className="settings-titlebar__title"
          data-tauri-drag-region
          data-testid="settings-title"
        >
          {title}
        </span>
        <div className="settings-titlebar__controls">
          <button
            type="button"
            className="settings-titlebar__control settings-titlebar__control--minimize"
            onClick={() => void getCurrentWindow().minimize()}
            aria-label={t("WindowMinimize")}
            title={t("WindowMinimize")}
          />
          <button
            type="button"
            className="settings-titlebar__control settings-titlebar__control--close"
            onClick={() => void closeSettingsWindow()}
            aria-label={t("WindowClose")}
            title={t("WindowClose")}
          >
            <svg aria-hidden viewBox="0 0 16 16" focusable="false">
              <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
            </svg>
          </button>
        </div>
      </div>

      <div className="settings-shell">
        <SettingsSidebar
          settings={settings}
          providers={catalog}
          snapshots={snapshots}
          set={set}
          saving={saving}
          activeTab={activeTab}
          selectedProviderId={providerId}
          searchText={searchText}
          onSearchTextChange={setSearchText}
          onSelectPane={handleSelectPane}
          onSelectProvider={handleSelectProvider}
        />

        <section className="settings-detail" aria-label={title}>
          <div
            className={`settings-body${activeTab === "providers" ? " settings-body--provider" : ""}`}
          >
            {activeTab === "general" && (
              <GeneralTab mode="general" settings={settings} set={set} saving={saving} />
            )}
            {activeTab === "providers" && (
              <ProvidersTab
                settings={settings}
                providers={catalog}
                set={set}
                saving={saving}
                providerId={providerId}
                snapshots={snapshots}
              />
            )}
            {activeTab === "notifications" && (
              <GeneralTab mode="notifications" settings={settings} set={set} saving={saving} />
            )}
            {activeTab === "menuBar" && (
              <MenuBarTab
                settings={settings}
                providers={catalog}
                set={set}
                saving={saving}
              />
            )}
            {activeTab === "menu" && (
              <MenuTab settings={settings} set={set} saving={saving} />
            )}
            {activeTab === "usageSpend" && (
              <UsageSpendTab settings={settings} set={set} saving={saving} />
            )}
            {activeTab === "advanced" && (
              <AdvancedTab settings={settings} set={set} saving={saving} />
            )}
            {activeTab === "hooks" && <HooksTab providers={catalog} />}
            {activeTab === "about" && (
              <AboutTab settings={settings} set={set} saving={saving} />
            )}
          </div>

          {/* Save status at the foot of the detail column. "Saving" floats over
              the pane so it never shifts the layout; an error stays in flow. */}
          <div className="settings-status-slot">
            {(saving || error) && (
              <div
                role="status"
                className={`settings-status ${saving ? "settings-status--floating" : "settings-status--error"}`}
              >
                {saving ? t("SettingsStatusSaving") : error}
              </div>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
