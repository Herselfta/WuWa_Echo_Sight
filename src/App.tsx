import { useEffect, useMemo, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import { EchoPoolPage } from "./pages/EchoPoolPage";
import { RecordPage } from "./pages/RecordPage";
import { DataToolsPage } from "./pages/DataToolsPage";
import { useAppStore } from "./store/useAppStore";

type TabKey = "record" | "echoPool" | "dataTools";

const TABS: Array<{ key: TabKey; label: string }> = [
  { key: "record", label: "工作台" },
  { key: "echoPool", label: "声骸池" },
  { key: "dataTools", label: "数据库" },
];

const TAB_NOTES: Record<TabKey, string> = {
  record: "统一新增声骸、强化录入与数据观察&分析。",
  echoPool: "集中处理已有声骸的数据维护。",
  dataTools: "用于数据导入/导出和历史事件修正。",
};

function App() {
  const [activeTab, setActiveTab] = useState<TabKey>("record");
  const [visitedTabs, setVisitedTabs] = useState<Record<TabKey, boolean>>({
    record: true,
    echoPool: false,
    dataTools: false,
  });
  const { loading, error, loadBootData, notifyExternalSync } = useAppStore();

  useEffect(() => {
    void loadBootData();
    
    // Register global listener for cross-app IPC (e.g. from ok-wuthering-waves)
    let syncTimeout: number | undefined;
    const unlistenPromise = listen("echo_updated", (event) => {
      console.log("[EchoSync-UI] New echo data received from background! Debouncing refresh...", event.payload);
      if (syncTimeout !== undefined) {
        window.clearTimeout(syncTimeout);
      }
      syncTimeout = window.setTimeout(() => {
        void (async () => {
          await loadBootData();
          notifyExternalSync();
        })();
      }, 500); // Debounce 500ms to prevent extreme lockups and CPU spikes
    });

    return () => {
      if (syncTimeout !== undefined) {
        window.clearTimeout(syncTimeout);
      }
      unlistenPromise.then(unlisten => unlisten());
    };
  }, [loadBootData, notifyExternalSync]);

  const switchTab = (tab: TabKey) => {
    setActiveTab(tab);
    setVisitedTabs((current) => (current[tab] ? current : { ...current, [tab]: true }));
  };

  const mountedTabs = useMemo(
    () =>
      ({
        record: visitedTabs.record ? <RecordPage /> : null,
        echoPool: visitedTabs.echoPool ? <EchoPoolPage /> : null,
        dataTools: visitedTabs.dataTools ? <DataToolsPage /> : null,
      }) satisfies Record<TabKey, ReactNode>,
    [visitedTabs],
  );

  return (
    <main className="app-shell">
      <header className="app-header">
        <div className="app-header-right">
          <nav className="tab-nav">
            {TABS.map((tab) => (
              <button
                key={tab.key}
                className={tab.key === activeTab ? "tab-btn active" : "tab-btn"}
                type="button"
                onClick={() => switchTab(tab.key)}
              >
                {tab.label}
              </button>
            ))}
          </nav>
          <div className="page-note">{TAB_NOTES[activeTab]}</div>
          {loading ? <span className="status-chip">加载中...</span> : null}
        </div>
      </header>

      {error ? <p className="error-banner">{error}</p> : null}
      {TABS.map((tab) =>
        mountedTabs[tab.key] ? (
          <section key={tab.key} className="tab-panel" hidden={activeTab !== tab.key}>
            {mountedTabs[tab.key]}
          </section>
        ) : null,
      )}
    </main>
  );
}

export default App;
