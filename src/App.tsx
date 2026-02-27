import { useEffect, useMemo, useState } from "react";
import "./App.css";
import { EchoPoolPage } from "./pages/EchoPoolPage";
import { RecordPage } from "./pages/RecordPage";
import { DistributionPage } from "./pages/DistributionPage";
import { AnalysisPage } from "./pages/AnalysisPage";
import { useAppStore } from "./store/useAppStore";

type TabKey = "echoPool" | "record" | "distribution" | "analysis";

const TABS: Array<{ key: TabKey; label: string }> = [
  { key: "echoPool", label: "声骸池" },
  { key: "record", label: "录入工作台" },
  { key: "distribution", label: "实时分布" },
  { key: "analysis", label: "分析修正" },
];

function App() {
  const [activeTab, setActiveTab] = useState<TabKey>("record");
  const { loading, error, loadBootData } = useAppStore();

  useEffect(() => {
    void loadBootData();
  }, [loadBootData]);

  const content = useMemo(() => {
    if (activeTab === "record") {
      return <RecordPage />;
    }
    if (activeTab === "distribution") {
      return <DistributionPage />;
    }
    if (activeTab === "analysis") {
      return <AnalysisPage />;
    }
    return <EchoPoolPage />;
  }, [activeTab]);

  return (
    <main className="app-shell">
      <header className="app-header">
        <div>
          <h1>WuWa Echo Sight</h1>
          <p>离线记录与概率分析工具（v1）</p>
        </div>
        {loading ? <span className="status-chip">加载中...</span> : null}
      </header>

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

      {error ? <p className="error-banner">{error}</p> : null}
      {content}
    </main>
  );
}

export default App;
