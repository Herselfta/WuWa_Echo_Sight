import { useEffect, useMemo, useState } from "react";
import "./App.css";
import { EchoPoolPage } from "./pages/EchoPoolPage";
import { RecordPage } from "./pages/RecordPage";
import { AnalysisPage } from "./pages/AnalysisPage";
import { useAppStore } from "./store/useAppStore";

type TabKey = "record" | "echoPool" | "analysis";

const TABS: Array<{ key: TabKey; label: string }> = [
  { key: "record", label: "统一看板" },
  { key: "echoPool", label: "声骸池管理" },
  { key: "analysis", label: "数据工具" },
];

const TAB_NOTES: Record<TabKey, string> = {
  record: "在同一页完成新增声骸、强化录入与实时概率观察。",
  echoPool: "集中处理已有声骸的资料维护、补录与误录删除。",
  analysis: "用于快照、导出和历史事件修正。",
};

function App() {
  const [activeTab, setActiveTab] = useState<TabKey>("record");
  const { loading, error, loadBootData } = useAppStore();

  useEffect(() => {
    void loadBootData();
  }, [loadBootData]);

  const content = useMemo(() => {
    if (activeTab === "echoPool") {
      return <EchoPoolPage />;
    }
    if (activeTab === "analysis") {
      return <AnalysisPage />;
    }
    return <RecordPage />;
  }, [activeTab]);

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
                onClick={() => setActiveTab(tab.key)}
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
      {content}
    </main>
  );
}

export default App;
