import { useEffect, useMemo, useState } from "react";
import {
  appendOrderedEvent,
  createEcho,
  deleteExpectationPreset,
  getEchoesForStat,
  getEventHistory,
  getGlobalDistribution,
  saveExpectationPreset,
  setExpectations,
} from "../api/tauri";
import { BarChart } from "../components/BarChart";
import { useAppStore } from "../store/useAppStore";
import type {
  DistributionFilter,
  DistributionPayload,
  EchoProbRow,
  EchoStatus,
  EventRow,
  ExpectationItem,
  ExpectationPreset,
} from "../types/domain";

interface ExpectationDraft {
  statKey: string;
  rank: number;
}

function toLocalInputValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

function normalizeLocalTime(value: string): string {
  return new Date(value).toISOString();
}

function formatScaledValue(unit: string, valueScaled: number) {
  return unit === "percent" ? `${(valueScaled / 10).toFixed(1)}%` : String(valueScaled);
}

function formatPresetSummary(items: ExpectationItem[]) {
  const sorted = [...items].sort((a, b) => a.rank - b.rank || a.statKey.localeCompare(b.statKey));
  return sorted.map((x) => `${x.statKey}(r${x.rank})`).join(" / ");
}

function toPercent(value: number): string {
  return `${(value * 100).toFixed(2)}%`;
}

export function RecordPage() {
  const { echoes, statDefs, expectationPresets, refreshEchoes, refreshExpectationPresets } = useAppStore();

  const [selectedEchoId, setSelectedEchoId] = useState<string>("");
  const [slotNo, setSlotNo] = useState<number>(1);
  const [statKey, setStatKey] = useState<string>("crit_rate");
  const [tierIndex, setTierIndex] = useState<number>(1);
  const [eventTimeLocal, setEventTimeLocal] = useState<string>(toLocalInputValue(new Date()));
  const [eventHistory, setEventHistory] = useState<EventRow[]>([]);

  const [createForm, setCreateForm] = useState({
    nickname: "",
    mainStatKey: "atk_pct",
    costClass: 1,
    status: "tracking" as EchoStatus,
    presetId: "",
  });

  const [presetName, setPresetName] = useState("");
  const [editingPresetId, setEditingPresetId] = useState<string | null>(null);
  const [presetDrafts, setPresetDrafts] = useState<ExpectationDraft[]>([
    { statKey: "crit_rate", rank: 1 },
  ]);

  const [distributionFilter, setDistributionFilter] = useState<DistributionFilter>({});
  const [distribution, setDistribution] = useState<DistributionPayload | null>(null);
  const [selectedDistStatKey, setSelectedDistStatKey] = useState<string | null>(null);
  const [echoProbRows, setEchoProbRows] = useState<EchoProbRow[]>([]);
  const [sortBy, setSortBy] = useState("pFinal");

  const [saving, setSaving] = useState(false);
  const [loadingDistribution, setLoadingDistribution] = useState(false);
  const [loadingEchoProbRows, setLoadingEchoProbRows] = useState(false);
  const [message, setMessage] = useState("");

  const selectedEcho = useMemo(
    () => echoes.find((echo) => echo.echoId === selectedEchoId) ?? null,
    [echoes, selectedEchoId],
  );

  const occupiedSlots = useMemo(
    () => new Set((selectedEcho?.currentSubstats ?? []).map((slot) => slot.slotNo)),
    [selectedEcho],
  );

  const occupiedStats = useMemo(
    () => new Set((selectedEcho?.currentSubstats ?? []).map((slot) => slot.statKey)),
    [selectedEcho],
  );

  const availableSlots = useMemo(() => {
    return [1, 2, 3, 4, 5].filter((x) => !occupiedSlots.has(x));
  }, [occupiedSlots]);

  const availableStatDefs = useMemo(() => {
    return statDefs.filter((stat) => !occupiedStats.has(stat.statKey));
  }, [statDefs, occupiedStats]);

  const selectedStat = useMemo(
    () => availableStatDefs.find((stat) => stat.statKey === statKey) ?? availableStatDefs[0] ?? null,
    [availableStatDefs, statKey],
  );

  const selectedTierValue = selectedStat?.tiers.find((x) => x.tierIndex === tierIndex)?.valueScaled ?? 0;

  const distributionChartData = useMemo(() => {
    const rows = distribution?.rows ?? [];
    return {
      labels: rows.map((row) => row.displayName),
      values: rows.map((row) => row.pGlobal),
    };
  }, [distribution]);

  useEffect(() => {
    if (!selectedEchoId && echoes.length > 0) {
      setSelectedEchoId(echoes[0].echoId);
    }
  }, [echoes, selectedEchoId]);

  useEffect(() => {
    if (availableSlots.length === 0) {
      return;
    }
    if (!availableSlots.includes(slotNo)) {
      setSlotNo(availableSlots[0]);
    }
  }, [availableSlots, slotNo]);

  useEffect(() => {
    if (!selectedStat) {
      return;
    }
    if (statKey !== selectedStat.statKey) {
      setStatKey(selectedStat.statKey);
      return;
    }
    if (!selectedStat.tiers.some((t) => t.tierIndex === tierIndex)) {
      setTierIndex(selectedStat.tiers[0]?.tierIndex ?? 1);
    }
  }, [selectedStat, statKey, tierIndex]);

  useEffect(() => {
    if (!statDefs.some((s) => s.statKey === createForm.mainStatKey) && statDefs.length > 0) {
      setCreateForm((prev) => ({ ...prev, mainStatKey: statDefs[0].statKey }));
    }
  }, [createForm.mainStatKey, statDefs]);

  const loadHistory = async () => {
    const rows = await getEventHistory({ limit: 200 });
    setEventHistory(rows);
  };

  const loadDistribution = async () => {
    setLoadingDistribution(true);
    try {
      const result = await getGlobalDistribution(distributionFilter);
      setDistribution(result);
      if (!selectedDistStatKey && result.rows.length > 0) {
        setSelectedDistStatKey(result.rows[0].statKey);
      }
    } finally {
      setLoadingDistribution(false);
    }
  };

  const loadEchoProbRows = async () => {
    if (!selectedDistStatKey) {
      setEchoProbRows([]);
      return;
    }
    setLoadingEchoProbRows(true);
    try {
      const rows = await getEchoesForStat({
        statKey: selectedDistStatKey,
        sortBy,
        ...distributionFilter,
      });
      setEchoProbRows(rows);
    } finally {
      setLoadingEchoProbRows(false);
    }
  };

  useEffect(() => {
    void loadHistory();
    void loadDistribution();
  }, []);

  useEffect(() => {
    void loadDistribution();
  }, [distributionFilter.startTime, distributionFilter.endTime, distributionFilter.mainStatKey, distributionFilter.costClass, distributionFilter.status]);

  useEffect(() => {
    void loadEchoProbRows();
  }, [selectedDistStatKey, sortBy, distributionFilter.startTime, distributionFilter.endTime, distributionFilter.mainStatKey, distributionFilter.costClass, distributionFilter.status]);

  const handleCreateEcho = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setMessage("");

    try {
      const created = await createEcho({
        nickname: createForm.nickname || undefined,
        mainStatKey: createForm.mainStatKey,
        costClass: createForm.costClass,
        status: createForm.status,
      });

      if (createForm.presetId) {
        const preset = expectationPresets.find((x) => x.presetId === createForm.presetId);
        if (preset && preset.items.length > 0) {
          await setExpectations(created.echoId, preset.items);
        }
      }

      await refreshEchoes();
      setSelectedEchoId(created.echoId);
      setCreateForm((prev) => ({ ...prev, nickname: "" }));
      setMessage("声骸创建成功，可直接继续强化。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const handleRecordEvent = async (event: React.FormEvent) => {
    event.preventDefault();

    if (!selectedEchoId) {
      setMessage("请先选择声骸。");
      return;
    }
    if (!availableSlots.includes(slotNo)) {
      setMessage("当前槽位不可用，请选择未占用槽位。");
      return;
    }
    if (!selectedStat) {
      setMessage("当前声骸没有可出的副词条。请更换声骸或检查状态。");
      return;
    }

    setSaving(true);
    setMessage("");

    try {
      const result = await appendOrderedEvent({
        echoId: selectedEchoId,
        slotNo,
        statKey: selectedStat.statKey,
        tierIndex,
        eventTime: normalizeLocalTime(eventTimeLocal),
      });

      await Promise.all([refreshEchoes(), loadHistory(), loadDistribution()]);
      await loadEchoProbRows();
      setMessage(`录入成功，eventId: ${result.eventId}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const savePreset = async () => {
    const normalizedItems = presetDrafts
      .filter((x) => x.statKey)
      .map((x) => ({ statKey: x.statKey, rank: Number(x.rank) || 1 }));

    if (!presetName.trim()) {
      setMessage("请填写预设名称。");
      return;
    }
    if (normalizedItems.length === 0) {
      setMessage("预设至少需要一个词条。");
      return;
    }

    setSaving(true);
    setMessage("");
    try {
      const result = await saveExpectationPreset({
        presetId: editingPresetId ?? undefined,
        name: presetName.trim(),
        items: normalizedItems,
      });
      await refreshExpectationPresets();
      setEditingPresetId(result.presetId);
      setMessage("期望词条预设已保存。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const applyPresetToSelectedEcho = async (preset: ExpectationPreset) => {
    if (!selectedEchoId) {
      setMessage("请先选择要应用预设的声骸。");
      return;
    }
    setSaving(true);
    setMessage("");
    try {
      await setExpectations(selectedEchoId, preset.items);
      await refreshEchoes();
      setMessage(`预设「${preset.name}」已应用到当前声骸。`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const loadPresetToEditor = (preset: ExpectationPreset) => {
    setEditingPresetId(preset.presetId);
    setPresetName(preset.name);
    setPresetDrafts(
      preset.items.map((x) => ({
        statKey: x.statKey,
        rank: x.rank,
      })),
    );
  };

  const removePreset = async (presetId: string) => {
    if (!window.confirm("确认删除该预设？")) {
      return;
    }
    setSaving(true);
    setMessage("");
    try {
      await deleteExpectationPreset(presetId);
      await refreshExpectationPresets();
      if (editingPresetId === presetId) {
        setEditingPresetId(null);
        setPresetName("");
        setPresetDrafts([{ statKey: statDefs[0]?.statKey ?? "crit_rate", rank: 1 }]);
      }
      setMessage("预设已删除。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="page">
      <div className="dashboard-grid">
        <div className="dashboard-column">
          <form className="card form-grid" onSubmit={handleCreateEcho}>
            <h3>新建声骸</h3>
            <label>
              昵称
              <input
                value={createForm.nickname}
                onChange={(e) => setCreateForm((prev) => ({ ...prev, nickname: e.target.value }))}
                placeholder="可选"
              />
            </label>

            <label>
              主词条
              <select
                value={createForm.mainStatKey}
                onChange={(e) => setCreateForm((prev) => ({ ...prev, mainStatKey: e.target.value }))}
              >
                {statDefs.map((stat) => (
                  <option key={stat.statKey} value={stat.statKey}>
                    {stat.displayName}
                  </option>
                ))}
              </select>
            </label>

            <label>
              Cost
              <select
                value={createForm.costClass}
                onChange={(e) => setCreateForm((prev) => ({ ...prev, costClass: Number(e.target.value) }))}
              >
                <option value={1}>1</option>
                <option value={3}>3</option>
                <option value={4}>4</option>
              </select>
            </label>

            <label>
              状态
              <select
                value={createForm.status}
                onChange={(e) => setCreateForm((prev) => ({ ...prev, status: e.target.value as EchoStatus }))}
              >
                <option value="tracking">tracking</option>
                <option value="paused">paused</option>
                <option value="abandoned">abandoned</option>
                <option value="completed">completed</option>
              </select>
            </label>

            <label>
              期望预设（可选）
              <select
                value={createForm.presetId}
                onChange={(e) => setCreateForm((prev) => ({ ...prev, presetId: e.target.value }))}
              >
                <option value="">不应用预设</option>
                {expectationPresets.map((preset) => (
                  <option key={preset.presetId} value={preset.presetId}>
                    {preset.name}
                  </option>
                ))}
              </select>
            </label>

            <button type="submit" disabled={saving}>
              创建并开始强化
            </button>
          </form>

          <form className="card form-grid" onSubmit={handleRecordEvent}>
            <h3>强化录入</h3>
            <label>
              声骸
              <select value={selectedEchoId} onChange={(e) => setSelectedEchoId(e.target.value)}>
                <option value="">请选择</option>
                {echoes.map((echo) => (
                  <option key={echo.echoId} value={echo.echoId}>
                    {(echo.nickname ?? echo.echoId.slice(0, 8)) + ` (${echo.openedSlotsCount}/5)`}
                  </option>
                ))}
              </select>
            </label>

            <label>
              槽位（仅可用）
              <select value={slotNo} onChange={(e) => setSlotNo(Number(e.target.value))}>
                {availableSlots.map((x) => (
                  <option key={x} value={x}>
                    {x}
                  </option>
                ))}
              </select>
            </label>

            <label>
              词条（自动排除已存在）
              <select value={selectedStat?.statKey ?? ""} onChange={(e) => setStatKey(e.target.value)}>
                {availableStatDefs.map((stat) => (
                  <option key={stat.statKey} value={stat.statKey}>
                    {stat.displayName}
                  </option>
                ))}
              </select>
            </label>

            <label>
              档位
              <select value={tierIndex} onChange={(e) => setTierIndex(Number(e.target.value))}>
                {(selectedStat?.tiers ?? []).map((tier) => (
                  <option key={tier.tierIndex} value={tier.tierIndex}>
                    档位 {tier.tierIndex} ({formatScaledValue(selectedStat?.unit ?? "flat", tier.valueScaled)})
                  </option>
                ))}
              </select>
            </label>

            <label>
              事件时间
              <input
                type="datetime-local"
                value={eventTimeLocal}
                onChange={(e) => setEventTimeLocal(e.target.value)}
              />
            </label>

            <div className="inline-row">
              <span>当前值预览：{formatScaledValue(selectedStat?.unit ?? "flat", selectedTierValue)}</span>
              <button
                type="submit"
                disabled={saving || !selectedEchoId || availableSlots.length === 0 || availableStatDefs.length === 0}
              >
                保存事件并刷新分析
              </button>
            </div>
          </form>

          <div className="card split-card">
            <div>
              <h3>期望词条预设编辑</h3>
              <label>
                预设名称
                <input
                  value={presetName}
                  onChange={(e) => setPresetName(e.target.value)}
                  placeholder="例如：双爆输出模板"
                />
              </label>

              {presetDrafts.map((item, idx) => (
                <div className="inline-row" key={`preset-draft-${idx}`}>
                  <select
                    value={item.statKey}
                    onChange={(e) => {
                      const next = e.target.value;
                      setPresetDrafts((prev) =>
                        prev.map((row, rowIdx) => (rowIdx === idx ? { ...row, statKey: next } : row)),
                      );
                    }}
                  >
                    {statDefs.map((stat) => (
                      <option key={stat.statKey} value={stat.statKey}>
                        {stat.displayName}
                      </option>
                    ))}
                  </select>
                  <input
                    type="number"
                    min={1}
                    value={item.rank}
                    onChange={(e) => {
                      const next = Number(e.target.value);
                      setPresetDrafts((prev) =>
                        prev.map((row, rowIdx) => (rowIdx === idx ? { ...row, rank: next } : row)),
                      );
                    }}
                  />
                  <button
                    type="button"
                    onClick={() => setPresetDrafts((prev) => prev.filter((_, rowIdx) => rowIdx !== idx))}
                  >
                    删除
                  </button>
                </div>
              ))}

              <div className="inline-row">
                <button
                  type="button"
                  onClick={() =>
                    setPresetDrafts((prev) => [
                      ...prev,
                      { statKey: statDefs[0]?.statKey ?? "crit_rate", rank: 1 },
                    ])
                  }
                >
                  添加词条
                </button>
                <button type="button" onClick={() => void savePreset()} disabled={saving}>
                  {editingPresetId ? "覆盖保存" : "保存预设"}
                </button>
              </div>
            </div>

            <div>
              <h3>已保存预设</h3>
              <table className="table compact-table">
                <thead>
                  <tr>
                    <th>名称</th>
                    <th>词条</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {expectationPresets.map((preset) => (
                    <tr key={preset.presetId}>
                      <td>{preset.name}</td>
                      <td>{formatPresetSummary(preset.items)}</td>
                      <td>
                        <div className="inline-row">
                          <button type="button" onClick={() => loadPresetToEditor(preset)}>
                            载入
                          </button>
                          <button type="button" onClick={() => void applyPresetToSelectedEcho(preset)}>
                            应用当前声骸
                          </button>
                          <button type="button" onClick={() => void removePreset(preset.presetId)}>
                            删除
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>

          <div className="card">
            <h3>最近事件</h3>
            <table className="table compact-table">
              <thead>
                <tr>
                  <th>seq</th>
                  <th>声骸</th>
                  <th>槽位</th>
                  <th>词条</th>
                  <th>档位</th>
                  <th>值</th>
                </tr>
              </thead>
              <tbody>
                {eventHistory.map((row) => {
                  const stat = statDefs.find((s) => s.statKey === row.statKey);
                  return (
                    <tr key={row.eventId}>
                      <td>{row.analysisSeq}</td>
                      <td>{row.echoNickname ?? row.echoId.slice(0, 8)}</td>
                      <td>{row.slotNo}</td>
                      <td>{stat?.displayName ?? row.statKey}</td>
                      <td>{row.tierIndex}</td>
                      <td>{formatScaledValue(stat?.unit ?? "flat", row.valueScaled)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>

        <div className="dashboard-column">
          <div className="card form-grid">
            <h3>实时分布筛选（自动刷新）</h3>
            <label>
              开始时间
              <input
                type="datetime-local"
                value={distributionFilter.startTime ? distributionFilter.startTime.slice(0, 16) : ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev,
                    startTime: e.target.value ? new Date(e.target.value).toISOString() : undefined,
                  }))
                }
              />
            </label>
            <label>
              结束时间
              <input
                type="datetime-local"
                value={distributionFilter.endTime ? distributionFilter.endTime.slice(0, 16) : ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev,
                    endTime: e.target.value ? new Date(e.target.value).toISOString() : undefined,
                  }))
                }
              />
            </label>
            <label>
              主词条
              <select
                value={distributionFilter.mainStatKey ?? ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev,
                    mainStatKey: e.target.value || undefined,
                  }))
                }
              >
                <option value="">全部</option>
                {statDefs.map((stat) => (
                  <option key={stat.statKey} value={stat.statKey}>
                    {stat.displayName}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Cost
              <select
                value={distributionFilter.costClass ?? ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev,
                    costClass: e.target.value ? Number(e.target.value) : undefined,
                  }))
                }
              >
                <option value="">全部</option>
                <option value="1">1</option>
                <option value="3">3</option>
                <option value="4">4</option>
              </select>
            </label>
            <label>
              状态
              <select
                value={distributionFilter.status ?? ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev,
                    status: e.target.value || undefined,
                  }))
                }
              >
                <option value="">全部</option>
                <option value="tracking">tracking</option>
                <option value="paused">paused</option>
                <option value="abandoned">abandoned</option>
                <option value="completed">completed</option>
              </select>
            </label>
          </div>

          <div className="card">
            <h3>全局概率图 {loadingDistribution ? "(更新中...)" : ""}</h3>
            <BarChart labels={distributionChartData.labels} values={distributionChartData.values} />
          </div>

          <div className="card">
            <h3>词条分布（点击联动命中列表）</h3>
            <table className="table compact-table">
              <thead>
                <tr>
                  <th>词条</th>
                  <th>P(global)</th>
                  <th>Wilson CI</th>
                  <th>Bayes</th>
                </tr>
              </thead>
              <tbody>
                {(distribution?.rows ?? []).map((row) => (
                  <tr
                    key={row.statKey}
                    className={row.statKey === selectedDistStatKey ? "active-row" : ""}
                    onClick={() => setSelectedDistStatKey(row.statKey)}
                  >
                    <td>{row.displayName}</td>
                    <td>{toPercent(row.pGlobal)}</td>
                    <td>
                      {toPercent(row.ciFreqLow)} ~ {toPercent(row.ciFreqHigh)}
                    </td>
                    <td>
                      {toPercent(row.bayesMean)} ({toPercent(row.bayesLow)} ~ {toPercent(row.bayesHigh)})
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="card">
            <div className="inline-row">
              <h3>词条命中声骸列表 {selectedDistStatKey ? `(${selectedDistStatKey})` : ""}</h3>
              <label>
                排序
                <select value={sortBy} onChange={(e) => setSortBy(e.target.value)}>
                  <option value="pFinal">P(final)</option>
                  <option value="pNext">P(next)</option>
                  <option value="rank">期望权重</option>
                  <option value="slots">槽位</option>
                </select>
              </label>
            </div>

            <table className="table compact-table">
              <thead>
                <tr>
                  <th>声骸</th>
                  <th>槽位</th>
                  <th>权重</th>
                  <th>P(next)</th>
                  <th>P(final)</th>
                </tr>
              </thead>
              <tbody>
                {echoProbRows.map((row) => (
                  <tr key={row.echoId}>
                    <td>{row.nickname ?? row.echoId.slice(0, 8)}</td>
                    <td>{row.openedSlotsCount}/5</td>
                    <td>{row.expectationRankMin}</td>
                    <td>{toPercent(row.pNext)}</td>
                    <td>{toPercent(row.pFinal)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {loadingEchoProbRows ? <p className="hint">命中列表更新中...</p> : null}
          </div>
        </div>
      </div>

      {message ? <p className="message">{message}</p> : null}
    </section>
  );
}
