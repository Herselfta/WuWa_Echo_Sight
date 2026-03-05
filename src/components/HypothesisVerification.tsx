import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  getCategoryStreakAnalysis,
  getReversionAnalysis,
  getTransitionMatrix,
} from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type {
  CategoryStreakReport,
  HypothesisFilter,
  ReversionReport,
  StatReversionSeries,
  TransitionCell,
  TransitionMatrix,
} from "../types/domain";

export type HypothesisTabKey = "transition" | "streak" | "reversion";

interface HypothesisVerificationProps {
  embedded?: boolean;
  forcedTab?: HypothesisTabKey;
  hideTabNav?: boolean;
  refreshToken?: number;
}

export function HypothesisVerification({
  embedded = false,
  forcedTab,
  hideTabNav = false,
  refreshToken = 0,
}: HypothesisVerificationProps) {
  const { statDefs } = useAppStore();
  const [message, setMessage] = useState("");
  
  /* ── Hypothesis verification state ── */
  const [hypoTab, setHypoTab] = useState<HypothesisTabKey>("transition");
  const [hypoFilter, setHypoFilter] = useState<HypothesisFilter>({});
  const [hypoLoading, setHypoLoading] = useState(false);
  const [transitionData, setTransitionData] = useState<TransitionMatrix | null>(null);
  const [streakData, setStreakData] = useState<CategoryStreakReport | null>(null);
  const [reversionData, setReversionData] = useState<ReversionReport | null>(null);
  const [revWindowSize, setRevWindowSize] = useState("10");
  const [revSelectedStats, setRevSelectedStats] = useState<string[]>([]);
  const requestIdRef = useRef(0);

  const statNameMap = useMemo(() => {
    const m: Record<string, string> = {};
    for (const sd of statDefs) m[sd.statKey] = sd.displayName;
    return m;
  }, [statDefs]);
  const activeTab = forcedTab ?? hypoTab;
  const showTabNav = !hideTabNav && !forcedTab;

  const loadHypothesisData = useCallback(async () => {
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    setHypoLoading(true);
    setMessage("");
    try {
      const [tm, sa, rv] = await Promise.all([
        getTransitionMatrix(hypoFilter),
        getCategoryStreakAnalysis(hypoFilter),
        getReversionAnalysis(hypoFilter, Number(revWindowSize) || 10),
      ]);
      if (requestId !== requestIdRef.current) return;
      setTransitionData(tm);
      setStreakData(sa);
      setReversionData(rv);
      // Auto-select up to 5 most frequent stats
      setRevSelectedStats(rv.statSeries.slice(0, 5).map((s) => s.statKey));
    } catch (error) {
      if (requestId !== requestIdRef.current) return;
      setMessage(`假设验证失败: ${String(error)}`);
    } finally {
      if (requestId === requestIdRef.current) {
        setHypoLoading(false);
      }
    }
  }, [hypoFilter, revWindowSize]);

  useEffect(() => {
    void loadHypothesisData();
  }, [
    loadHypothesisData,
    hypoFilter.costClass,
    hypoFilter.mainStatKey,
    hypoFilter.status,
    revWindowSize,
    refreshToken,
  ]);

  return (
    <div className={embedded ? "hypothesis-shell" : "card"}>
        <h3>统计验证</h3>
        <div className="inline-row" style={{ flexWrap: "wrap" }}>
          {message ? <span className="message">{message}</span> : null}
          <label style={{ fontSize: 13 }}>
            COST
            <select
              value={hypoFilter.costClass ?? ""}
              onChange={(e) =>
                setHypoFilter((f) => ({
                  ...f,
                  costClass: e.target.value ? Number(e.target.value) : undefined,
                }))
              }
              style={{ marginLeft: 4 }}
            >
              <option value="">全部</option>
              <option value="1">1</option>
              <option value="3">3</option>
              <option value="4">4</option>
            </select>
          </label>
          <label style={{ fontSize: 13 }}>
            主词条
            <select
              value={hypoFilter.mainStatKey ?? ""}
              onChange={(e) =>
                setHypoFilter((f) => ({ ...f, mainStatKey: e.target.value || undefined }))
              }
              style={{ marginLeft: 4 }}
            >
              <option value="">全部</option>
              {statDefs.map((sd) => (
                <option key={sd.statKey} value={sd.statKey}>
                  {sd.displayName}
                </option>
              ))}
            </select>
          </label>
          <label style={{ fontSize: 13 }}>
            状态
            <select
              value={hypoFilter.status ?? ""}
              onChange={(e) =>
                setHypoFilter((f) => ({ ...f, status: e.target.value || undefined }))
              }
              style={{ marginLeft: 4 }}
            >
              <option value="">全部</option>
              <option value="tracking">tracking</option>
              <option value="completed">completed</option>
              <option value="paused">paused</option>
              <option value="abandoned">abandoned</option>
            </select>
          </label>
          {activeTab === "reversion" ? (
            <label style={{ fontSize: 12, marginLeft: 8 }}>
              窗口
              <input
                type="number"
                min={5}
                max={30}
                value={revWindowSize}
                onChange={(e) => setRevWindowSize(e.target.value)}
                style={{ width: 48, marginLeft: 4 }}
              />
            </label>
          ) : null}
        </div>

        {/* Tab navigation */}
        {showTabNav ? (
        <nav className="tab-nav" style={{ marginTop: 12 }}>
          <button
            type="button"
            className={`tab-btn${activeTab === "transition" ? " active" : ""}`}
            onClick={() => setHypoTab("transition")}
          >
            转移矩阵
          </button>
          <button
            type="button"
            className={`tab-btn${activeTab === "streak" ? " active" : ""}`}
            onClick={() => setHypoTab("streak")}
          >
            区间/连档
          </button>
          <button
            type="button"
            className={`tab-btn${activeTab === "reversion" ? " active" : ""}`}
            onClick={() => setHypoTab("reversion")}
          >
            均值回归
          </button>
        </nav>
        ) : null}

        {/* ── Tab: Transition Matrix ── */}
        {activeTab === "transition" && (
          <div style={{ marginTop: 12 }}>
            {transitionData ? (
              <>
                <p style={{ fontSize: 13, marginBottom: 8 }}>
                  总转移数: <strong>{transitionData.totalTransitions}</strong> &nbsp;|&nbsp; χ²={" "}
                  <strong>{transitionData.chiSquared.toFixed(2)}</strong> &nbsp;|&nbsp; df={" "}
                  {transitionData.degreesOfFreedom} &nbsp;|&nbsp; p={" "}
                  <strong
                    style={{
                      color: transitionData.pValue < 0.05 ? "var(--danger, #e74c3c)" : "inherit",
                    }}
                  >
                    {transitionData.pValue < 0.001
                      ? transitionData.pValue.toExponential(2)
                      : transitionData.pValue.toFixed(4)}
                  </strong>
                  {transitionData.pValue < 0.01 && (
                    <span style={{ color: "var(--danger, #e74c3c)", marginLeft: 8 }}>
                      ⚠ 序列非独立（p&lt;0.01）
                    </span>
                  )}
                  {transitionData.pValue >= 0.05 && (
                    <span style={{ color: "var(--ok, #27ae60)", marginLeft: 8 }}>
                      ✓ 未发现显著偏离独立性
                    </span>
                  )}
                </p>
                <TransitionHeatmap
                  data={transitionData}
                  nameMap={statNameMap}
                />
              </>
            ) : (
              <p style={{ fontSize: 13, color: "var(--ink-dim)" }}>
                {hypoLoading ? "数据加载中…" : "暂无数据"}
              </p>
            )}
          </div>
        )}

        {/* ── Tab: Category Streak / Zone ── */}
        {activeTab === "streak" && (
          <div style={{ marginTop: 12 }}>
            {streakData ? (
              <>
                {/* Tier adjacency summary */}
                <p style={{ fontSize: 13, marginBottom: 8 }}>
                  相邻档位对数: <strong>{streakData.tierTotalPairs}</strong> &nbsp;|&nbsp; 停档
                  (同级): <strong>{(streakData.tierStopRatio * 100).toFixed(1)}%</strong> &nbsp;|&nbsp;
                  连档 (±1): <strong>{(streakData.tierStepRatio * 100).toFixed(1)}%</strong>{" "}
                  &nbsp;|&nbsp; 跳档 (≥2):{" "}
                  <strong>{(streakData.tierJumpRatio * 100).toFixed(1)}%</strong>
                </p>
                {streakData.tierExpectedStopRatio !== null &&
                streakData.tierExpectedStepRatio !== null &&
                streakData.tierExpectedJumpRatio !== null ? (
                  <p style={{ fontSize: 12, color: "var(--ink-dim)", marginBottom: 12 }}>
                    文档基线（非均匀档位）: 停档 {(streakData.tierExpectedStopRatio * 100).toFixed(1)}% |
                    连档 {(streakData.tierExpectedStepRatio * 100).toFixed(1)}% |
                    跳档 {(streakData.tierExpectedJumpRatio * 100).toFixed(1)}%
                  </p>
                ) : null}

                {/* Zone visits */}
                {streakData.zoneVisits.length > 0 && (
                  <div style={{ marginBottom: 12 }}>
                    <strong style={{ fontSize: 13 }}>区间访问统计</strong>
                    <div className="inline-row" style={{ flexWrap: "wrap", marginTop: 4 }}>
                      {streakData.zoneVisits.map(([zone, cnt]) => (
                        <span
                          key={zone}
                          style={{
                            fontSize: 12,
                            padding: "2px 8px",
                            borderRadius: 4,
                            background: "var(--panel)",
                            border: "1px solid var(--line)",
                          }}
                        >
                          {zone}: {cnt}
                        </span>
                      ))}
                    </div>
                  </div>
                )}

                {/* Zone transitions */}
                {streakData.zoneTransitions.length > 0 && (
                  <div style={{ marginBottom: 12 }}>
                    <strong style={{ fontSize: 13 }}>区间转移</strong>
                    <div className="inline-row" style={{ flexWrap: "wrap", marginTop: 4 }}>
                      {streakData.zoneTransitions.map(([from, to, cnt]) => (
                        <span
                          key={`${from}-${to}`}
                          style={{
                            fontSize: 12,
                            padding: "2px 8px",
                            borderRadius: 4,
                            background: "var(--panel)",
                            border: "1px solid var(--line)",
                          }}
                        >
                          {from} → {to}: {cnt}
                        </span>
                      ))}
                    </div>
                  </div>
                )}

                {/* Streak details table */}
                <strong style={{ fontSize: 13 }}>
                  同类连续段 (≥2)（共 {streakData.streaks.length} 段）
                </strong>
                {streakData.streaks.length > 0 ? (
                  <table className="compact-table table" style={{ marginTop: 4 }}>
                    <thead>
                      <tr>
                        <th>声骸</th>
                        <th>类别</th>
                        <th>槽位</th>
                        <th>长度</th>
                        <th>词条序列</th>
                        <th>档位序列</th>
                        <th>推测区间</th>
                      </tr>
                    </thead>
                    <tbody>
                      {streakData.streaks.map((s, i) => (
                        <tr key={i}>
                          <td className="mono-cell">{s.echoId.slice(0, 8)}</td>
                          <td>{s.category}</td>
                          <td>
                            {s.startSlot}–{s.endSlot}
                          </td>
                          <td>{s.length}</td>
                          <td>
                            {s.stats.map((sk) => statNameMap[sk] ?? sk).join("→")}
                          </td>
                          <td>{s.tiers.join("→")}</td>
                          <td>{s.possibleZones.join(", ") || "—"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                ) : (
                  <p style={{ fontSize: 13 }}>无连续同类段（需更多数据）</p>
                )}
              </>
            ) : (
              <p style={{ fontSize: 13, color: "var(--ink-dim)" }}>
                {hypoLoading ? "数据加载中…" : "暂无数据"}
              </p>
            )}
          </div>
        )}
        {/* ── Tab: Mean Reversion ── */}
        {activeTab === "reversion" && (
          <div style={{ marginTop: 12 }}>
            {reversionData ? (
              <ReversionPanel
                data={reversionData}
                selectedStats={revSelectedStats}
                onToggleStat={(key) =>
                  setRevSelectedStats((prev) =>
                    prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
                  )
                }
              />
            ) : (
              <p style={{ fontSize: 13, color: "var(--ink-dim)" }}>
                {hypoLoading ? "数据加载中…" : "暂无数据"}
              </p>
            )}
          </div>
        )}
    </div>
  );
}

/* ═══ Sub-components ═══ */

/** ─── Reversion analysis panel ─── */
function ReversionPanel({
  data,
  selectedStats,
  onToggleStat,
}: {
  data: ReversionReport;
  selectedStats: string[];
  onToggleStat: (key: string) => void;
}) {
  const active = data.statSeries.filter((s) => s.totalCount > 0);
  const selected = active.filter((s) => selectedStats.includes(s.statKey));

  return (
    <div>
      {/* Stat selector chips */}
      <div className="inline-row" style={{ flexWrap: "wrap", marginBottom: 10 }}>
        <span style={{ fontSize: 12 }}>选择词条:</span>
        {active.map((s) => {
          const on = selectedStats.includes(s.statKey);
          return (
            <button
              key={s.statKey}
              type="button"
              onClick={() => onToggleStat(s.statKey)}
              style={{
                fontSize: 11,
                padding: "2px 8px",
                borderRadius: 4,
                border: "1px solid var(--line)",
                background: on ? "var(--accent)" : "var(--panel)",
                color: on ? "#fff" : "var(--ink)",
                cursor: "pointer",
              }}
            >
              {s.displayName} ({s.totalCount})
            </button>
          );
        })}
      </div>

      {/* Deviation line chart */}
      {selected.length > 0 && (
        <DeviationChart totalEvents={data.totalEvents} series={selected} />
      )}

      <div className="reversion-two-col">
        {/* Gap statistics table */}
        <div className="reversion-half-panel">
          <strong style={{ fontSize: 13 }}>到达间隔统计（均值回归判断）</strong>
          <p style={{ fontSize: 11, color: "var(--ink-dim)", margin: "2px 0 6px" }}>
            离散指数 = Var/Mean。几何分布基准 ≈ (1-p)/p。若实际 &lt; 基准 → 均匀化（均值回归）；若 &gt; 基准 → 聚集爆发。
          </p>
          <table className="compact-table table" style={{ fontSize: 12 }}>
            <thead>
              <tr>
                <th>词条</th>
                <th>出现次数</th>
                <th>基准频率</th>
                <th>实际均值间隔</th>
                <th>期望间隔 (i.i.d.)</th>
                <th>间隔方差</th>
                <th>离散指数</th>
                <th>几何基准</th>
                <th>判断</th>
              </tr>
            </thead>
            <tbody>
              {active.map((s) => {
                const hasGaps = s.gaps.length >= 2;
                const di = hasGaps ? s.dispersionIndex : null;
                const gd = s.geometricDispersion;
                let verdict = "—";
                let color = "inherit";
                if (di !== null && !isNaN(di) && !isNaN(gd)) {
                  if (di < gd * 0.7) { verdict = "✓ 均匀化"; color = "var(--ok, #27ae60)"; }
                  else if (di > gd * 1.4) { verdict = "⚠ 聚集爆发"; color = "var(--danger, #e74c3c)"; }
                  else { verdict = "≈ 随机"; }
                }
                return (
                  <tr key={s.statKey}>
                    <td>{s.displayName}</td>
                    <td>{s.totalCount}</td>
                    <td>{(s.baseFreq * 100).toFixed(1)}%</td>
                    <td>{s.meanGap > 0 ? s.meanGap.toFixed(1) : "—"}</td>
                    <td>{s.expectedGap > 0 ? s.expectedGap.toFixed(1) : "—"}</td>
                    <td>{hasGaps && !isNaN(s.gapVariance) ? s.gapVariance.toFixed(1) : "—"}</td>
                    <td>{di !== null && !isNaN(di) ? di.toFixed(2) : "—"}</td>
                    <td>{!isNaN(gd) ? gd.toFixed(2) : "—"}</td>
                    <td style={{ color }}>{verdict}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>

        {/* Lag autocorrelation table */}
        <div className="reversion-half-panel">
          <strong style={{ fontSize: 13 }}>滞后自相关（负值 = 负反馈均值回归）</strong>
          <p style={{ fontSize: 11, color: "var(--ink-dim)", margin: "2px 0 6px" }}>
            Lag-5 ≈ 1 声骸，Lag-10 ≈ 2 声骸。数据少时置信度低。
          </p>
          <table className="compact-table table" style={{ fontSize: 12 }}>
            <thead>
              <tr>
                <th>词条</th>
                <th>Lag-1</th>
                <th>Lag-5</th>
                <th>Lag-10</th>
                <th>Lag-13</th>
              </tr>
            </thead>
            <tbody>
              {active.map((s) => {
                const ac: Record<number, number> = {};
                for (const [lag, v] of s.lagAutocorrs) ac[lag] = v;
                const cell = (lag: number) => {
                  const v = ac[lag];
                  if (v === undefined) return <td>—</td>;
                  const color = v < -0.15 ? "var(--ok, #27ae60)" : v > 0.25 ? "var(--danger, #e74c3c)" : "inherit";
                  return <td style={{ color }}>{v.toFixed(3)}</td>;
                };
                return (
                  <tr key={s.statKey}>
                    <td>{s.displayName}</td>
                    {cell(1)}{cell(5)}{cell(10)}{cell(13)}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>

      {/* Window conditional table */}
      {selected.length > 0 && (
        <div style={{ overflowX: "auto", marginTop: 14 }}>
          <strong style={{ fontSize: 13 }}>窗口条件频率（前 W 次出现次数 → 后 W 次出现率）</strong>
          <p style={{ fontSize: 11, color: "var(--ink-dim)", margin: "2px 0 6px" }}>
            若在前 W 事件中出现越多，后 W 出现率越低 → 均值回归；反之 → 聚集。
          </p>
          {selected.map((s) => (
            <div key={s.statKey} style={{ marginBottom: 10 }}>
              <span style={{ fontSize: 12 }}>
                <strong>{s.displayName}</strong>（基准 {(s.baseFreq * 100).toFixed(1)}%/事件）
              </span>
              <div className="inline-row" style={{ flexWrap: "wrap", marginTop: 4 }}>
                {s.windowBuckets.map((b) => (
                  <div
                    key={b.prevWindowCount}
                    style={{
                      fontSize: 11,
                      padding: "4px 10px",
                      borderRadius: 4,
                      border: "1px solid var(--line)",
                      background: "var(--panel)",
                      textAlign: "center",
                    }}
                  >
                    <div style={{ fontWeight: 600 }}>前={b.prevWindowCount}{b.prevWindowCount >= 3 ? "+" : ""}</div>
                    <div>n={b.sampleCount}</div>
                    <div
                      style={{
                        color:
                          b.prevWindowCount >= 2 && b.meanNextFreq < s.baseFreq * 0.6
                            ? "var(--ok, #27ae60)"
                            : "inherit",
                      }}
                    >
                      后={( b.meanNextFreq * 100).toFixed(1)}%
                    </div>
                  </div>
                ))}
                {s.windowBuckets.length === 0 && (
                  <span style={{ fontSize: 11, color: "var(--ink-dim)" }}>数据不足</span>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** ECharts line chart: running cumulative frequency deviation per stat */
function DeviationChart({
  totalEvents,
  series,
}: {
  totalEvents: number;
  series: StatReversionSeries[];
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!containerRef.current || series.length === 0) return;

    const chart = echarts.init(containerRef.current);
    const palette = [
      "#e74c3c", "#3498db", "#2ecc71", "#f39c12", "#9b59b6",
      "#1abc9c", "#e67e22", "#34495e", "#e91e63", "#00bcd4",
    ];
    const baselineColor = "#9ca3af";
    const statColors = series.map((_, idx) => palette[idx % palette.length]);

    chart.setOption({
      animation: false,
      color: [baselineColor, ...statColors],
      tooltip: {
        trigger: "axis",
        formatter: (params: unknown) => {
          const p = params as { seriesName: string; value: [number, number] }[];
          return p
            .map((item) => `${item.seriesName}: ${(item.value[1] * 100).toFixed(2)}%`)
            .join("<br/>");
        },
      },
      legend: { top: 4, type: "scroll", textStyle: { fontSize: 11 } },
      grid: { left: 48, right: 16, top: 36, bottom: 36 },
      xAxis: {
        type: "value",
        name: "事件序号",
        min: 0,
        max: totalEvents - 1,
        nameTextStyle: { fontSize: 11 },
      },
      yAxis: {
        type: "value",
        name: "频率偏差",
        nameTextStyle: { fontSize: 11 },
        axisLabel: {
          formatter: (v: number) => `${(v * 100).toFixed(1)}%`,
          fontSize: 10,
        },
        splitLine: { lineStyle: { type: "dashed" } },
      },
      series: [
        // Y=0 reference line
        {
          name: "基准线",
          type: "line",
          data: [[0, 0], [totalEvents - 1, 0]],
          color: baselineColor,
          lineStyle: { color: baselineColor, type: "dashed", width: 1 },
          itemStyle: { color: baselineColor },
          symbol: "none",
          emphasis: { disabled: true },
        },
        ...series.map((s, idx) => {
          const lineColor = statColors[idx];
          return {
            name: s.displayName,
            type: "line" as const,
            color: lineColor,
            data: s.deviations.map((d, i) => [i, d] as [number, number]),
            lineStyle: { color: lineColor, width: 2 },
            itemStyle: { color: lineColor },
            symbol: "none",
            smooth: false,
          };
        }),
      ],
    });

    const obs = new ResizeObserver(() => chart.resize());
    obs.observe(containerRef.current);
    return () => { obs.disconnect(); chart.dispose(); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, totalEvents]);

  return (
    <div
      ref={containerRef}
      style={{ width: "100%", height: 260, border: "1px solid var(--line)", borderRadius: 4 }}
    />
  );
}

/** Transition matrix heatmap rendered as an HTML table with color-coded residuals */
function TransitionHeatmap({
  data,
  nameMap,
}: {
  data: TransitionMatrix;
  nameMap: Record<string, string>;
}) {
  const [hoverTip, setHoverTip] = useState<{
    x: number;
    y: number;
    text: string;
  } | null>(null);
  const keys = data.statKeys.map(([k]) => k);
  const cellMap = useMemo(() => {
    const m = new Map<string, TransitionCell>();
    for (const c of data.cells) m.set(`${c.fromStat}|${c.toStat}`, c);
    return m;
  }, [data.cells]);

  const maxAbsResidual = useMemo(() => {
    let mx = 1;
    for (const c of data.cells) {
      const a = Math.abs(c.residual);
      if (a > mx) mx = a;
    }
    return mx;
  }, [data.cells]);

  const cellColor = (residual: number) => {
    const norm = Math.min(Math.abs(residual) / maxAbsResidual, 1);
    if (residual > 0) return `rgba(231, 76, 60, ${norm * 0.6})`;
    if (residual < 0) return `rgba(52, 152, 219, ${norm * 0.6})`;
    return "transparent";
  };
  const updateTipPosition = (x: number, y: number) => {
    const maxWidth = 260;
    const pad = 14;
    const vw = typeof window !== "undefined" ? window.innerWidth : 1280;
    const vh = typeof window !== "undefined" ? window.innerHeight : 720;
    return {
      x: Math.min(x + pad, vw - maxWidth),
      y: Math.min(y + pad, vh - 100),
    };
  };

  return (
    <div style={{ overflowX: "auto" }}>
      <table className="compact-table table" style={{ fontSize: 11, textAlign: "center" }}>
        <thead>
          <tr>
            <th style={{ minWidth: 48 }}>→</th>
            {keys.map((k) => (
              <th key={k} style={{ minWidth: 40, writingMode: "vertical-lr", fontSize: 10 }}>
                {nameMap[k] ?? k}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {keys.map((fromK) => (
            <tr key={fromK}>
              <td style={{ fontWeight: 600, textAlign: "left", fontSize: 10, whiteSpace: "nowrap" }}>
                {nameMap[fromK] ?? fromK}
              </td>
              {keys.map((toK) => {
                const cell = cellMap.get(`${fromK}|${toK}`);
                const cellTip =
                  cell
                    ? `${nameMap[fromK] ?? fromK}→${nameMap[toK] ?? toK}\n观测: ${cell.count}\n期望: ${cell.expected.toFixed(1)}\n残差: ${cell.residual.toFixed(2)}`
                    : "";
                return (
                  <td
                    key={toK}
                    style={{
                      background: cell ? cellColor(cell.residual) : "transparent",
                      cursor: "default",
                    }}
                    onMouseMove={(e) => {
                      if (!cellTip) {
                        setHoverTip(null);
                        return;
                      }
                      const pos = updateTipPosition(e.clientX, e.clientY);
                      setHoverTip({ x: pos.x, y: pos.y, text: cellTip });
                    }}
                    onMouseLeave={() => setHoverTip(null)}
                  >
                    {cell?.count ?? 0}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      <p style={{ fontSize: 11, color: "var(--ink-dim)", marginTop: 4 }}>
        🔴 观测 &gt; 期望 &nbsp; 🔵 观测 &lt; 期望 &nbsp; 悬停查看详情
      </p>
      {hoverTip ? (
        <div
          style={{
            position: "fixed",
            left: hoverTip.x,
            top: hoverTip.y,
            zIndex: 4000,
            maxWidth: 260,
            padding: "6px 8px",
            borderRadius: 6,
            border: "1px solid rgba(148, 163, 184, 0.35)",
            background: "rgba(15, 23, 42, 0.95)",
            color: "#fff",
            fontSize: 11,
            lineHeight: 1.35,
            whiteSpace: "pre-line",
            pointerEvents: "none",
            boxShadow: "0 8px 20px rgba(15, 23, 42, 0.22)",
          }}
        >
          {hoverTip.text}
        </div>
      ) : null}
    </div>
  );
}
