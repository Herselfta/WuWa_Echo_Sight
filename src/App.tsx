import { useEffect, useMemo, useState } from "react";
import "./App.css";
import { EchoPoolPage } from "./pages/EchoPoolPage";
import { RecordPage } from "./pages/RecordPage";
import { AnalysisPage } from "./pages/AnalysisPage";
import { useAppStore } from "./store/useAppStore";

type TabKey = "record" | "echoPool" | "analysis";

const TABS: Array<{ key: TabKey; label: string }> = [
  { key: "record", label: "工作台" },
  { key: "echoPool", label: "声骸池" },
  { key: "analysis", label: "数据库" },
];

const TAB_NOTES: Record<TabKey, string> = {
  record: "统一新增声骸、强化录入与数据观察&分析。",
  echoPool: "集中处理已有声骸的数据维护。",
  analysis: "用于数据导入/导出和历史事件修正。",
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
