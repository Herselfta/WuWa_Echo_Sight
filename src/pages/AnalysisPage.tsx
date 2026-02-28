import { useEffect, useMemo, useState } from "react";
import {
  createProbabilitySnapshot,
  editOrderedEvent,
  exportCsv,
  getCategoryStreakAnalysis,
  getEventHistory,
  getSlotStatDistribution,
  getTransitionMatrix,
  importData,
} from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type {
  CategoryStreakReport,
  EventRow,
  HypothesisFilter,
  SlotStatCell,
  SlotStatDistribution,
  TransitionCell,
  TransitionMatrix,
} from "../types/domain";

function toLocalInputValue(iso: string): string {
  const date = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

export function AnalysisPage() {
  const { distributionFilter, selectedStatKey, statDefs, loadBootData } = useAppStore();
  const [events, setEvents] = useState<EventRow[]>([]);
  const [eventId, setEventId] = useState("");
  const [slotNo, setSlotNo] = useState("");
  const [statKey, setStatKey] = useState("");
  const [tierIndex, setTierIndex] = useState("");
  const [eventTime, setEventTime] = useState("");
  const [reorderMode, setReorderMode] = useState<"none" | "time_assist">("none");
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(false);
  const [importZipPath, setImportZipPath] = useState("");

  /* ── Hypothesis verification state ── */
  const [hypoTab, setHypoTab] = useState<"transition" | "slotstat" | "streak">("transition");
  const [hypoFilter, setHypoFilter] = useState<HypothesisFilter>({});
  const [hypoLoading, setHypoLoading] = useState(false);
  const [transitionData, setTransitionData] = useState<TransitionMatrix | null>(null);
  const [slotStatData, setSlotStatData] = useState<SlotStatDistribution | null>(null);
  const [streakData, setStreakData] = useState<CategoryStreakReport | null>(null);

  const statNameMap = useMemo(() => {
    const m: Record<string, string> = {};
    for (const sd of statDefs) m[sd.statKey] = sd.displayName;
    return m;
  }, [statDefs]);

  const runHypothesisVerification = async () => {
    setHypoLoading(true);
    setMessage("");
    try {
      const [tm, ss, sa] = await Promise.all([
        getTransitionMatrix(hypoFilter),
        getSlotStatDistribution(hypoFilter),
        getCategoryStreakAnalysis(hypoFilter),
      ]);
      setTransitionData(tm);
      setSlotStatData(ss);
      setStreakData(sa);
    } catch (error) {
      setMessage(`假设验证失败: ${String(error)}`);
    } finally {
      setHypoLoading(false);
    }
  };

  const loadEvents = async () => {
    const rows = await getEventHistory({ limit: 200 });
    setEvents(rows);
  };

  useEffect(() => {
    void loadEvents();
  }, []);

  const fillFromRow = (row: EventRow) => {
    setEventId(row.eventId);
    setSlotNo(String(row.slotNo));
    setStatKey(row.statKey);
    setTierIndex(String(row.tierIndex));
    setEventTime(toLocalInputValue(row.eventTime));
  };

  const submitEdit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!eventId) {
      setMessage("请先输入 eventId。");
      return;
    }
    setLoading(true);
    setMessage("");
    try {
      const result = await editOrderedEvent({
        eventId,
        slotNo: slotNo ? Number(slotNo) : undefined,
        statKey: statKey || undefined,
        tierIndex: tierIndex ? Number(tierIndex) : undefined,
        eventTime: eventTime ? new Date(eventTime).toISOString() : undefined,
        reorderMode,
      });
      await loadEvents();
      setMessage(`事件修正成功，影响范围: ${result.affectedRange}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  const snapshot = async () => {
    setLoading(true);
    setMessage("");
    try {
      const result = await createProbabilitySnapshot({
        scope: distributionFilter,
        statKey: selectedStatKey ?? undefined,
      });
      setMessage(`快照创建成功: ${result.snapshotId}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  const doExport = async () => {
    setLoading(true);
    setMessage("");
    try {
      const result = await exportCsv({
        scope: distributionFilter,
        includeSnapshots: true,
      });
      setMessage(`CSV 已导出: ${result.zipPath}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  const doImport = async () => {
    if (!importZipPath.trim()) {
      setMessage("请先输入 zip 文件路径。");
      return;
    }
    if (!window.confirm("导入会覆盖当前记录数据，确认继续？")) {
      return;
    }
    setLoading(true);
    setMessage("");
    try {
      const result = await importData(importZipPath.trim());
      await loadBootData();
      await loadEvents();
      setMessage(`导入完成，表：${result.importedTables.join(", ") || "无"}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  return (
    <section className="page">
      <div className="card inline-row">
        <button type="button" onClick={() => void snapshot()} disabled={loading}>
          生成概率快照
        </button>
        <button type="button" onClick={() => void doExport()} disabled={loading}>
          导出 CSV(zip)
        </button>
        <input
          value={importZipPath}
          onChange={(e) => setImportZipPath(e.target.value)}
          placeholder="导入 zip 绝对路径"
        />
        <button type="button" onClick={() => void doImport()} disabled={loading}>
          导入数据
        </button>
      </div>

      <form className="card form-grid" onSubmit={submitEdit}>
        <h3>事件修正</h3>
        <label>
          Event ID
          <input value={eventId} onChange={(e) => setEventId(e.target.value)} placeholder="必填" />
        </label>
        <label>
          槽位
          <input value={slotNo} onChange={(e) => setSlotNo(e.target.value)} placeholder="可选" />
        </label>
        <label>
          词条 key
          <input value={statKey} onChange={(e) => setStatKey(e.target.value)} placeholder="可选" />
        </label>
        <label>
          档位
          <input value={tierIndex} onChange={(e) => setTierIndex(e.target.value)} placeholder="可选" />
        </label>
        <label>
          时间
          <input type="datetime-local" value={eventTime} onChange={(e) => setEventTime(e.target.value)} />
        </label>
        <label>
          重排模式
          <select
            value={reorderMode}
            onChange={(e) => setReorderMode(e.target.value as "none" | "time_assist")}
          >
            <option value="none">none</option>
            <option value="time_assist">time_assist</option>
          </select>
        </label>
        <button
          type="submit"
          disabled={loading}
          onClick={(e) => {
            if (!window.confirm("确认修改事件？修改后将重算相关统计。")) {
              e.preventDefault();
            }
          }}
        >
          提交修正
        </button>
      </form>

      {message ? <p className="message">{message}</p> : null}

      <div className="card">
        <h3>最近 200 条事件（点击填充修正表单）</h3>
        <table className="table">
          <thead>
            <tr>
              <th>analysisSeq</th>
              <th>eventId</th>
              <th>声骸</th>
              <th>槽位</th>
              <th>词条</th>
              <th>档位</th>
              <th>时间</th>
              <th>动作</th>
            </tr>
          </thead>
          <tbody>
            {events.map((row) => (
              <tr key={row.eventId}>
                <td>{row.analysisSeq}</td>
                <td className="mono-cell">{row.eventId}</td>
                <td>{row.echoNickname ?? row.echoId.slice(0, 8)}</td>
                <td>{row.slotNo}</td>
                <td>{row.statKey}</td>
                <td>{row.tierIndex}</td>
                <td>{new Date(row.eventTime).toLocaleString()}</td>
                <td>
                  <button type="button" onClick={() => fillFromRow(row)}>
                    填充
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* ═══ Hypothesis Verification Section ═══ */}
      <div className="card">
        <h3>假设验证</h3>
        <div className="inline-row" style={{ flexWrap: "wrap" }}>
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
          <button type="button" onClick={() => void runHypothesisVerification()} disabled={hypoLoading}>
            {hypoLoading ? "分析中…" : "运行验证"}
          </button>
        </div>

        {/* Tab navigation */}
        <nav className="tab-nav" style={{ marginTop: 12 }}>
          <button
            type="button"
            className={`tab-btn${hypoTab === "transition" ? " active" : ""}`}
            onClick={() => setHypoTab("transition")}
          >
            转移矩阵
          </button>
          <button
            type="button"
            className={`tab-btn${hypoTab === "slotstat" ? " active" : ""}`}
            onClick={() => setHypoTab("slotstat")}
          >
            槽位分布
          </button>
          <button
            type="button"
            className={`tab-btn${hypoTab === "streak" ? " active" : ""}`}
            onClick={() => setHypoTab("streak")}
          >
            区间/连档
          </button>
        </nav>

        {/* ── Tab: Transition Matrix ── */}
        {hypoTab === "transition" && (
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
              <p style={{ fontSize: 13, color: "var(--ink-dim)" }}>点击「运行验证」获取数据</p>
            )}
          </div>
        )}

        {/* ── Tab: Slot-Stat Distribution ── */}
        {hypoTab === "slotstat" && (
          <div style={{ marginTop: 12 }}>
            {slotStatData ? (
              <>
                <p style={{ fontSize: 13, marginBottom: 8 }}>
                  总事件数: <strong>{slotStatData.totalEvents}</strong> &nbsp;|&nbsp; χ²={" "}
                  <strong>{slotStatData.chiSquared.toFixed(2)}</strong> &nbsp;|&nbsp; df={" "}
                  {slotStatData.degreesOfFreedom} &nbsp;|&nbsp; p={" "}
                  <strong
                    style={{
                      color: slotStatData.pValue < 0.05 ? "var(--danger, #e74c3c)" : "inherit",
                    }}
                  >
                    {slotStatData.pValue < 0.001
                      ? slotStatData.pValue.toExponential(2)
                      : slotStatData.pValue.toFixed(4)}
                  </strong>
                  {slotStatData.pValue < 0.01 && (
                    <span style={{ color: "var(--danger, #e74c3c)", marginLeft: 8 }}>
                      ⚠ 槽位-词条非独立
                    </span>
                  )}
                  {slotStatData.pValue >= 0.05 && (
                    <span style={{ color: "var(--ok, #27ae60)", marginLeft: 8 }}>
                      ✓ 各槽位分布一致
                    </span>
                  )}
                </p>
                <SlotStatTable data={slotStatData} nameMap={statNameMap} />
              </>
            ) : (
              <p style={{ fontSize: 13, color: "var(--ink-dim)" }}>点击「运行验证」获取数据</p>
            )}
          </div>
        )}

        {/* ── Tab: Category Streak / Zone ── */}
        {hypoTab === "streak" && (
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
                <p style={{ fontSize: 12, color: "var(--ink-dim)", marginBottom: 12 }}>
                  均匀 8 档理论值: 停档 12.5% | 连档 21.9% | 跳档 65.6%
                </p>

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
              <p style={{ fontSize: 13, color: "var(--ink-dim)" }}>点击「运行验证」获取数据</p>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

/* ═══ Sub-components ═══ */

/** Transition matrix heatmap rendered as an HTML table with color-coded residuals */
function TransitionHeatmap({
  data,
  nameMap,
}: {
  data: TransitionMatrix;
  nameMap: Record<string, string>;
}) {
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
                return (
                  <td
                    key={toK}
                    style={{
                      background: cell ? cellColor(cell.residual) : "transparent",
                      cursor: "default",
                    }}
                    title={
                      cell
                        ? `${nameMap[fromK] ?? fromK}→${nameMap[toK] ?? toK}\n观测: ${cell.count}\n期望: ${cell.expected.toFixed(1)}\n残差: ${cell.residual.toFixed(2)}`
                        : ""
                    }
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
    </div>
  );
}

/** Slot-Stat distribution table: rows = stats, columns = slot 1-5 */
function SlotStatTable({
  data,
  nameMap,
}: {
  data: SlotStatDistribution;
  nameMap: Record<string, string>;
}) {
  const keys = data.statKeys.map(([k]) => k);
  const cellMap = useMemo(() => {
    const m = new Map<string, SlotStatCell>();
    for (const c of data.cells) m.set(`${c.slotNo}|${c.statKey}`, c);
    return m;
  }, [data.cells]);

  return (
    <div style={{ overflowX: "auto" }}>
      <table className="compact-table table" style={{ fontSize: 12 }}>
        <thead>
          <tr>
            <th>词条</th>
            <th>类别</th>
            {[1, 2, 3, 4, 5].map((s) => (
              <th key={s}>
                Slot {s}
                {data.slotTotals[s - 1] != null && (
                  <div style={{ fontWeight: 400, fontSize: 10 }}>n={data.slotTotals[s - 1]}</div>
                )}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {keys.map((sk) => {
            const firstCell = data.cells.find((c) => c.statKey === sk);
            return (
              <tr key={sk}>
                <td>{nameMap[sk] ?? sk}</td>
                <td style={{ fontSize: 11 }}>{firstCell?.category ?? "—"}</td>
                {[1, 2, 3, 4, 5].map((s) => {
                  const cell = cellMap.get(`${s}|${sk}`);
                  const pct = cell ? (cell.probability * 100).toFixed(1) : "—";
                  return (
                    <td
                      key={s}
                      title={cell ? `计数: ${cell.count} / ${data.slotTotals[s - 1]}` : ""}
                    >
                      {pct}%
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
