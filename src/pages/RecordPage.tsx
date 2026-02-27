import { useEffect, useMemo, useState } from "react";
import {
  appendOrderedEvent,
  createEcho,
  deleteExpectationPreset,
  getEventHistory,
  saveExpectationPreset,
  setExpectations,
} from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type { EchoStatus, EventRow, ExpectationItem, ExpectationPreset } from "../types/domain";

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

  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");

  const selectedEcho = useMemo(
    () => echoes.find((echo) => echo.echoId === selectedEchoId) ?? null,
    [echoes, selectedEchoId],
  );

  const selectedStat = useMemo(
    () => statDefs.find((stat) => stat.statKey === statKey) ?? null,
    [statDefs, statKey],
  );

  useEffect(() => {
    if (!selectedEchoId && echoes.length > 0) {
      const defaultEcho = echoes[0];
      setSelectedEchoId(defaultEcho.echoId);
      setSlotNo(Math.min(defaultEcho.openedSlotsCount + 1, 5));
    }
  }, [echoes, selectedEchoId]);

  useEffect(() => {
    if (!selectedEcho) {
      return;
    }
    setSlotNo(Math.min(selectedEcho.openedSlotsCount + 1, 5));
  }, [selectedEcho]);

  useEffect(() => {
    if (!selectedStat) {
      return;
    }
    setTierIndex(selectedStat.tiers[0]?.tierIndex ?? 1);
  }, [selectedStat]);

  useEffect(() => {
    if (!statDefs.some((s) => s.statKey === createForm.mainStatKey) && statDefs.length > 0) {
      setCreateForm((prev) => ({ ...prev, mainStatKey: statDefs[0].statKey }));
    }
  }, [createForm.mainStatKey, statDefs]);

  const loadHistory = async () => {
    const rows = await getEventHistory({ limit: 200 });
    setEventHistory(rows);
  };

  useEffect(() => {
    void loadHistory();
  }, []);

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
      setMessage("声骸创建成功，可直接继续录入强化事件。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const handleRecordEvent = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setMessage("");

    try {
      const result = await appendOrderedEvent({
        echoId: selectedEchoId,
        slotNo,
        statKey,
        tierIndex,
        eventTime: normalizeLocalTime(eventTimeLocal),
      });

      await Promise.all([refreshEchoes(), loadHistory()]);
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

  const selectedTierValue = selectedStat?.tiers.find((x) => x.tierIndex === tierIndex)?.valueScaled ?? 0;

  return (
    <section className="page">
      <h2>录入工作台</h2>
      <p className="hint">同页完成新增声骸、应用期望预设、录入强化事件。</p>

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
          槽位
          <select value={slotNo} onChange={(e) => setSlotNo(Number(e.target.value))}>
            {[1, 2, 3, 4, 5].map((x) => (
              <option key={x} value={x}>
                {x}
              </option>
            ))}
          </select>
        </label>

        <label>
          词条
          <select value={statKey} onChange={(e) => setStatKey(e.target.value)}>
            {statDefs.map((stat) => (
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
          <button type="submit" disabled={saving || !selectedEchoId}>
            保存事件
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
            <button
              type="button"
              onClick={() => {
                setEditingPresetId(null);
                setPresetName("");
                setPresetDrafts([{ statKey: statDefs[0]?.statKey ?? "crit_rate", rank: 1 }]);
              }}
            >
              清空编辑器
            </button>
          </div>
        </div>

        <div>
          <h3>已保存预设</h3>
          <table className="table">
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
                        应用到当前声骸
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

      {message ? <p className="message">{message}</p> : null}

      <div className="card">
        <h3>最近事件</h3>
        <table className="table">
          <thead>
            <tr>
              <th>analysisSeq</th>
              <th>声骸</th>
              <th>槽位</th>
              <th>词条</th>
              <th>档位</th>
              <th>值</th>
              <th>事件时间</th>
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
                  <td>{new Date(row.eventTime).toLocaleString()}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
