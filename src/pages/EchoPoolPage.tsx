import { Fragment, useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import {
  deleteEcho,
  deleteExpectationPreset,
  saveExpectationPreset,
  setExpectations,
  updateEcho,
  upsertBackfillState,
} from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type { EchoStatus, ExpectationItem, ExpectationPreset } from "../types/domain";

type RelOp = "gt" | "eq";
type ToastKind = "info" | "success" | "error";

interface SlotDraft {
  statKey: string;
  tierIndex: number;
}

interface BasicDraft {
  nickname: string;
  mainStatKey: string;
  costClass: number;
  status: EchoStatus;
}

const STAT_ABBR_MAP: Record<string, string> = {
  crit_rate: "b",
  crit_dmg: "B",
  energy_regen: "e",
  atk_flat: "a",
  hp_flat: "h",
  def_flat: "d",
  basic_dmg: "n",
  heavy_dmg: "c",
  skill_dmg: "s",
  liberation_dmg: "u",
  atk_pct: "A",
  hp_pct: "H",
  def_pct: "D",
};

function statKeyToAbbr(statKey: string): string {
  return STAT_ABBR_MAP[statKey] ?? statKey;
}

function buildExpectationChain(items: ExpectationItem[]) {
  const sorted = [...items].sort((a, b) => a.rank - b.rank || a.statKey.localeCompare(b.statKey));
  const stats = sorted.map((x) => x.statKey);
  const ops: RelOp[] = [];
  for (let i = 0; i < sorted.length - 1; i += 1) {
    ops.push(sorted[i + 1].rank === sorted[i].rank ? "eq" : "gt");
  }
  return { stats, ops };
}

function chainToExpectationItems(stats: string[], ops: RelOp[]): ExpectationItem[] {
  if (stats.length === 0) {
    return [];
  }
  const result: ExpectationItem[] = [];
  let rank = 1;
  result.push({ statKey: stats[0], rank });
  for (let i = 1; i < stats.length; i += 1) {
    if ((ops[i - 1] ?? "gt") === "gt") {
      rank += 1;
    }
    result.push({ statKey: stats[i], rank });
  }
  return result;
}

function moveArrayItem<T>(arr: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= arr.length || to >= arr.length) {
    return arr;
  }
  const next = [...arr];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}

function formatScaledValue(unit: string, valueScaled: number) {
  return unit === "percent" ? `${(valueScaled / 10).toFixed(1)}%` : String(valueScaled);
}

export function EchoPoolPage() {
  const { statDefs, echoes, expectationPresets, refreshEchoes, refreshExpectationPresets } = useAppStore();
  const tableWrapRef = useRef<HTMLDivElement | null>(null);
  const [tableClientWidth, setTableClientWidth] = useState(0);
  const statMap = useMemo(() => new Map(statDefs.map((x) => [x.statKey, x])), [statDefs]);
  const formatTierLabel = (statKey: string, tierIndex: number) => {
    const stat = statMap.get(statKey);
    const valueScaled = stat?.tiers.find((x) => x.tierIndex === tierIndex)?.valueScaled ?? 0;
    return `档${tierIndex}：${formatScaledValue(stat?.unit ?? "flat", valueScaled)}`;
  };
  const formatPresetSummary = (items: ExpectationItem[]) =>
    [...items]
      .sort((a, b) => a.rank - b.rank || a.statKey.localeCompare(b.statKey))
      .map((item) => `${statMap.get(item.statKey)?.displayName ?? item.statKey}(r${item.rank})`)
      .join(" / ");
  const getDragIndex = (event: DragEvent<HTMLElement>) => {
    const raw = event.dataTransfer.getData("text/plain");
    if (!raw) {
      return null;
    }
    const parsed = Number(raw);
    return Number.isInteger(parsed) ? parsed : null;
  };

  const [editingEchoId, setEditingEchoId] = useState<string | null>(null);
  const [basicDraft, setBasicDraft] = useState<BasicDraft | null>(null);
  const [expectationStats, setExpectationStats] = useState<string[]>([]);
  const [expectationOps, setExpectationOps] = useState<RelOp[]>([]);
  const [slotsDraft, setSlotsDraft] = useState<SlotDraft[]>([]);
  const [activeExpectationIndex, setActiveExpectationIndex] = useState<number | null>(null);
  const [activeSlotIndex, setActiveSlotIndex] = useState<number | null>(null);
  const [dragExpectationIndex, setDragExpectationIndex] = useState<number | null>(null);
  const [dragSlotIndex, setDragSlotIndex] = useState<number | null>(null);
  const [pendingDeleteEchoId, setPendingDeleteEchoId] = useState<string | null>(null);
  const [pendingDeletePresetId, setPendingDeletePresetId] = useState<string | null>(null);
  const [presetName, setPresetName] = useState("");
  const [editingPresetId, setEditingPresetId] = useState<string | null>(null);

  const [saving, setSaving] = useState(false);
  const [toast, setToast] = useState<{ id: number; text: string; kind: ToastKind } | null>(null);
  const setMessage = (text: string, kind: ToastKind = "info") => {
    if (!text.trim()) {
      setToast(null);
      return;
    }
    setToast({ id: Date.now() + Math.floor(Math.random() * 1000), text, kind });
  };

  useEffect(() => {
    if (!toast) {
      return;
    }
    const timeoutId = window.setTimeout(() => {
      setToast((current) => (current?.id === toast.id ? null : current));
    }, 2600);
    return () => window.clearTimeout(timeoutId);
  }, [toast]);

  useEffect(() => {
    const element = tableWrapRef.current;
    if (!element || typeof ResizeObserver === "undefined") {
      return;
    }
    const updateWidth = () => {
      const style = window.getComputedStyle(element);
      const paddingLeft = Number.parseFloat(style.paddingLeft) || 0;
      const paddingRight = Number.parseFloat(style.paddingRight) || 0;
      setTableClientWidth(Math.max(0, element.clientWidth - paddingLeft - paddingRight));
    };
    updateWidth();
    const observer = new ResizeObserver(() => updateWidth());
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const columnWidths = useMemo(() => {
    const total = Math.max(tableClientWidth, 1);
    const actions = total >= 760 ? 220 : Math.max(160, Math.floor(total * 0.28));
    const remaining = Math.max(total - actions, 0);
    const nickname = Math.floor((remaining * 3) / 10);
    const main = Math.floor(remaining / 10);
    const cost = Math.floor(remaining / 10);
    const status = Math.floor(remaining / 10);
    const slots = remaining - nickname - main - cost - status;
    return {
      nickname,
      main,
      cost,
      status,
      slots,
      actions,
    };
  }, [tableClientWidth]);

  const editingEcho = useMemo(
    () => echoes.find((echo) => echo.echoId === editingEchoId) ?? null,
    [echoes, editingEchoId],
  );
  const editingOrderedSlots = useMemo(
    () =>
      editingEcho
        ? editingEcho.currentSubstats
            .filter((x) => x.source === "ordered_event")
            .sort((a, b) => a.slotNo - b.slotNo)
        : [],
    [editingEcho],
  );
  const editingLockedSlots = useMemo(
    () => editingOrderedSlots.map((x) => x.slotNo),
    [editingOrderedSlots],
  );
  const editingEditableSlots = useMemo(
    () => [1, 2, 3, 4, 5].filter((slotNo) => !editingLockedSlots.includes(slotNo)),
    [editingLockedSlots],
  );
  const editingOrderedStatSet = useMemo(
    () => new Set(editingOrderedSlots.map((x) => x.statKey)),
    [editingOrderedSlots],
  );

  const startEdit = (echoId: string) => {
    const echo = echoes.find((x) => x.echoId === echoId);
    if (!echo) {
      return;
    }

    const chain = buildExpectationChain(echo.expectations);
    setEditingEchoId(echo.echoId);
    setBasicDraft({
      nickname: echo.nickname ?? "",
      mainStatKey: echo.mainStatKey,
      costClass: echo.costClass,
      status: echo.status,
    });
    setExpectationStats(chain.stats);
    setExpectationOps(chain.ops);
    setSlotsDraft(
      echo.currentSubstats
        .filter((x) => x.source === "backfill")
        .sort((a, b) => a.slotNo - b.slotNo)
        .map((x) => ({ statKey: x.statKey, tierIndex: x.tierIndex })),
    );
    setActiveExpectationIndex(null);
    setActiveSlotIndex(null);
    setDragExpectationIndex(null);
    setDragSlotIndex(null);
    setPendingDeleteEchoId(null);
    setMessage("");
  };

  const cancelEdit = () => {
    setEditingEchoId(null);
    setBasicDraft(null);
    setExpectationStats([]);
    setExpectationOps([]);
    setSlotsDraft([]);
    setActiveExpectationIndex(null);
    setActiveSlotIndex(null);
    setDragExpectationIndex(null);
    setDragSlotIndex(null);
    setPendingDeleteEchoId(null);
  };

  const pickAvailableStat = (used: string[]) => {
    const found = statDefs.find((s) => !used.includes(s.statKey));
    return found?.statKey ?? statDefs[0]?.statKey ?? "crit_rate";
  };

  const addExpectation = () => {
    setExpectationStats((prev) => {
      const nextStat = pickAvailableStat(prev);
      if (prev.includes(nextStat)) {
        setMessage("没有可添加的期望词条。");
        return prev;
      }
      if (prev.length > 0) {
        setExpectationOps((ops) => [...ops, "gt"]);
      }
      setActiveExpectationIndex(prev.length);
      return [...prev, nextStat];
    });
  };

  const removeExpectationAt = (idx: number) => {
    setExpectationStats((prevStats) => {
      const next = prevStats.filter((_, i) => i !== idx);
      setExpectationOps((prevOps) => {
        if (prevStats.length <= 1) {
          return [];
        }
        const ops = [...prevOps];
        if (idx === 0) {
          ops.shift();
          return ops;
        }
        if (idx === prevStats.length - 1) {
          ops.pop();
          return ops;
        }
        ops[idx - 1] = "gt";
        ops.splice(idx, 1);
        return ops;
      });
      return next;
    });
    setActiveExpectationIndex((prev) => {
      if (prev === null) {
        return prev;
      }
      if (prev === idx) {
        return null;
      }
      if (prev > idx) {
        return prev - 1;
      }
      return prev;
    });
  };

  const addSlot = () => {
    setSlotsDraft((prev) => {
      if (prev.length >= editingEditableSlots.length) {
        setMessage(`当前仅可编辑 ${editingEditableSlots.length} 个槽位。`);
        return prev;
      }
      const used = [...prev.map((x) => x.statKey), ...Array.from(editingOrderedStatSet)];
      const statKey = pickAvailableStat(used);
      if (used.includes(statKey)) {
        setMessage("没有可添加的槽位词条。");
        return prev;
      }
      const tierIndex = statMap.get(statKey)?.tiers[0]?.tierIndex ?? 1;
      setActiveSlotIndex(prev.length);
      return [...prev, { statKey, tierIndex }];
    });
  };

  const removeSlotAt = (idx: number) => {
    setSlotsDraft((prev) => prev.filter((_, i) => i !== idx));
    setActiveSlotIndex((prev) => {
      if (prev === null) {
        return prev;
      }
      if (prev === idx) {
        return null;
      }
      if (prev > idx) {
        return prev - 1;
      }
      return prev;
    });
  };

  const saveEdit = async () => {
    if (!editingEcho || !basicDraft) {
      return;
    }

    const uniqueExpectationStats = Array.from(new Set(expectationStats));
    if (uniqueExpectationStats.length !== expectationStats.length) {
      setMessage("期望词条存在重复，请先调整。");
      return;
    }

    const slotStatKeys = slotsDraft.map((x) => x.statKey);
    const uniqueSlotStats = Array.from(new Set(slotStatKeys));
    if (uniqueSlotStats.length !== slotStatKeys.length) {
      setMessage("槽位词条存在重复，请先调整。");
      return;
    }
    if (slotStatKeys.some((x) => editingOrderedStatSet.has(x))) {
      setMessage("槽位词条与已锁定词条重复，请先调整。");
      return;
    }

    const lockedSlots = editingEcho.currentSubstats
      .filter((x) => x.source === "ordered_event")
      .map((x) => x.slotNo)
      .sort((a, b) => a - b);
    const editableSlots = [1, 2, 3, 4, 5].filter((slotNo) => !lockedSlots.includes(slotNo));

    if (slotsDraft.length > editableSlots.length) {
      setMessage(`当前声骸仅剩 ${editableSlots.length} 个可编辑槽位。`);
      return;
    }

    setSaving(true);
    setMessage("");
    try {
      await updateEcho({
        echoId: editingEcho.echoId,
        nickname: basicDraft.nickname || undefined,
        mainStatKey: basicDraft.mainStatKey,
        costClass: basicDraft.costClass,
        status: basicDraft.status,
      });

      await setExpectations(editingEcho.echoId, chainToExpectationItems(expectationStats, expectationOps));

      const mappedSlots = slotsDraft.map((slot, idx) => {
        const slotNo = editableSlots[idx];
        if (!slotNo) {
          throw new Error("槽位映射失败，请重新打开管理后再试。");
        }
        return {
          slotNo,
          statKey: slot.statKey,
          tierIndex: slot.tierIndex,
        };
      });

      await upsertBackfillState({
        echoId: editingEcho.echoId,
        slots: mappedSlots,
      });

      await refreshEchoes();
      cancelEdit();
      setPendingDeleteEchoId(null);
      setMessage("已保存。", "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingDeleteEchoId(null);
      setSaving(false);
    }
  };

  const loadPresetToEditor = (preset: ExpectationPreset) => {
    if (!editingEcho) {
      setMessage("请先点击某条声骸的“管理”，再载入预设。");
      return;
    }
    const chain = buildExpectationChain(preset.items);
    setEditingPresetId(preset.presetId);
    setPresetName(preset.name);
    setExpectationStats(chain.stats);
    setExpectationOps(chain.ops);
    setActiveExpectationIndex(null);
    setMessage(`预设「${preset.name}」已载入编辑区。`);
  };

  const savePresetFromCurrent = async () => {
    if (!editingEcho) {
      setMessage("请先点击某条声骸的“管理”，再保存预设。");
      return;
    }
    const items = chainToExpectationItems(expectationStats, expectationOps);
    if (!presetName.trim()) {
      setMessage("请填写预设名称。");
      return;
    }
    if (items.length === 0) {
      setMessage("当前没有可保存的期望词条。");
      return;
    }

    setSaving(true);
    setMessage("");
    try {
      const result = await saveExpectationPreset({
        presetId: editingPresetId ?? undefined,
        name: presetName.trim(),
        items,
      });
      await refreshExpectationPresets();
      setEditingPresetId(result.presetId);
      setPendingDeletePresetId(null);
      setMessage("期望预设已保存。", "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingDeletePresetId(null);
      setSaving(false);
    }
  };

  const applyPresetToEditingEcho = async (preset: ExpectationPreset) => {
    if (!editingEcho) {
      setMessage("请先点击某条声骸的“管理”，再应用预设。");
      return;
    }
    setSaving(true);
    setMessage("");
    try {
      const chain = buildExpectationChain(preset.items);
      setExpectationStats(chain.stats);
      setExpectationOps(chain.ops);
      setEditingPresetId(preset.presetId);
      setPresetName(preset.name);
      await setExpectations(editingEcho.echoId, preset.items);
      await refreshEchoes();
      setMessage(`预设「${preset.name}」已应用到当前声骸。`, "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingDeletePresetId(null);
      setSaving(false);
    }
  };

  const removePreset = async (presetId: string) => {
    if (pendingDeletePresetId !== presetId) {
      setPendingDeletePresetId(presetId);
      setMessage("再次点击“删除预设”以确认。");
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
      }
      setPendingDeletePresetId(null);
      setMessage("预设已删除。", "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingDeletePresetId(null);
      setSaving(false);
    }
  };

  const removeEcho = async (echoId: string) => {
    if (pendingDeleteEchoId !== echoId) {
      setPendingDeleteEchoId(echoId);
      setMessage("再次点击“删除”以确认删除该声骸。");
      return;
    }
    setSaving(true);
    setMessage("");
    try {
      await deleteEcho(echoId);
      await refreshEchoes();
      if (editingEchoId === echoId) {
        cancelEdit();
      }
      setPendingDeleteEchoId(null);
      setMessage("声骸已删除。", "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingDeleteEchoId(null);
      setSaving(false);
    }
  };

  return (
    <section className="page">
      {toast ? <div className={`toast toast-${toast.kind}`}>{toast.text}</div> : null}

      <div className="card split-card">
        <div>
          <h3>期望预设管理</h3>
          <label>
            预设名称
            <input value={presetName} onChange={(e) => setPresetName(e.target.value)} placeholder="例如：双爆模板" />
          </label>
          <div className="inline-row">
            <button type="button" onClick={() => void savePresetFromCurrent()} disabled={saving}>
              {editingPresetId ? "覆盖保存预设" : "保存为新预设"}
            </button>
            <button
              type="button"
              onClick={() => {
                setEditingPresetId(null);
                setPresetName("");
                setPendingDeletePresetId(null);
              }}
              disabled={saving}
            >
              清空选择
            </button>
          </div>
          <p className="hint">保存来源为当前“管理”中的期望词条链。</p>
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
                      <button type="button" onClick={() => loadPresetToEditor(preset)} disabled={saving}>
                        载入编辑区
                      </button>
                      <button type="button" onClick={() => void applyPresetToEditingEcho(preset)} disabled={saving}>
                        应用当前声骸
                      </button>
                      <button type="button" onClick={() => void removePreset(preset.presetId)} disabled={saving}>
                        {pendingDeletePresetId === preset.presetId ? "确认删除预设" : "删除预设"}
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
              {expectationPresets.length === 0 ? (
                <tr>
                  <td colSpan={3} className="chain-empty">
                    暂无预设
                  </td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </div>
      </div>

      <div className="card echo-table-card" ref={tableWrapRef}>
        <table className="table echo-table">
          <colgroup>
            <col className="echo-col-nickname" style={{ width: `${columnWidths.nickname}px` }} />
            <col className="echo-col-main" style={{ width: `${columnWidths.main}px` }} />
            <col className="echo-col-cost" style={{ width: `${columnWidths.cost}px` }} />
            <col className="echo-col-status" style={{ width: `${columnWidths.status}px` }} />
            <col className="echo-col-slots" style={{ width: `${columnWidths.slots}px` }} />
            <col className="echo-actions-col" style={{ width: `${columnWidths.actions}px` }} />
          </colgroup>
          <thead>
            <tr>
              <th className="echo-col-nickname">昵称</th>
              <th className="echo-col-main">主词条</th>
              <th className="echo-col-cost">Cost</th>
              <th className="echo-col-status">状态</th>
              <th className="echo-col-slots">槽位</th>
              <th className="echo-actions-col">操作</th>
            </tr>
          </thead>
          <tbody>
            {echoes.map((echo) => {
              const editing = editingEchoId === echo.echoId;

              return (
                <Fragment key={echo.echoId}>
                  <tr key={`${echo.echoId}-row`} className={editing ? "active-row" : ""}>
                    <td className="echo-col-nickname">
                      <div className="echo-cell">
                        {editing && basicDraft ? (
                          <input
                            className="echo-inline-nickname echo-cell-control"
                            value={basicDraft.nickname}
                            placeholder="可选"
                            onChange={(e) =>
                              setBasicDraft((prev) => (prev ? { ...prev, nickname: e.target.value } : prev))
                            }
                          />
                        ) : (
                          <span className="echo-cell-display">{echo.nickname ?? echo.echoId.slice(0, 8)}</span>
                        )}
                      </div>
                    </td>
                    <td className="echo-col-main">
                      <div className="echo-cell">
                        {editing && basicDraft ? (
                          <select
                            className="echo-cell-control"
                            value={basicDraft.mainStatKey}
                            onChange={(e) =>
                              setBasicDraft((prev) =>
                                prev ? { ...prev, mainStatKey: e.target.value } : prev,
                              )
                            }
                          >
                            {statDefs.map((stat) => (
                              <option key={stat.statKey} value={stat.statKey}>
                                {stat.displayName}
                              </option>
                            ))}
                          </select>
                        ) : (
                          <span className="echo-cell-display">
                            {statMap.get(echo.mainStatKey)?.displayName ?? echo.mainStatKey}
                          </span>
                        )}
                      </div>
                    </td>
                    <td className="echo-col-cost">
                      <div className="echo-cell">
                        {editing && basicDraft ? (
                          <select
                            className="echo-cell-control"
                            value={basicDraft.costClass}
                            onChange={(e) =>
                              setBasicDraft((prev) =>
                                prev ? { ...prev, costClass: Number(e.target.value) } : prev,
                              )
                            }
                          >
                            <option value={1}>1</option>
                            <option value={3}>3</option>
                            <option value={4}>4</option>
                          </select>
                        ) : (
                          <span className="echo-cell-display">{echo.costClass}</span>
                        )}
                      </div>
                    </td>
                    <td className="echo-col-status">
                      <div className="echo-cell">
                        {editing && basicDraft ? (
                          <select
                            className="echo-cell-control"
                            value={basicDraft.status}
                            onChange={(e) =>
                              setBasicDraft((prev) =>
                                prev ? { ...prev, status: e.target.value as EchoStatus } : prev,
                              )
                            }
                          >
                            <option value="tracking">tracking</option>
                            <option value="paused">paused</option>
                            <option value="abandoned">abandoned</option>
                            <option value="completed">completed</option>
                          </select>
                        ) : (
                          <span className="echo-cell-display">{echo.status}</span>
                        )}
                      </div>
                    </td>
                    <td className="echo-col-slots">
                      <div className="slot-summary-list">
                        <span className="slot-summary-head">{echo.openedSlotsCount}/5</span>
                        {echo.currentSubstats.length === 0 ? (
                          <span className="chain-empty">无</span>
                        ) : (
                          [...echo.currentSubstats]
                            .sort((a, b) => a.slotNo - b.slotNo)
                            .map((slot) => (
                              <span
                                key={`${echo.echoId}-slot-${slot.slotNo}`}
                                className={slot.source === "ordered_event" ? "slot-pill slot-pill-locked" : "slot-pill"}
                                title={`${slot.slotNo}: ${statMap.get(slot.statKey)?.displayName ?? slot.statKey} ${formatTierLabel(slot.statKey, slot.tierIndex)}`}
                              >
                                {slot.slotNo}: {statKeyToAbbr(slot.statKey)}
                                {slot.tierIndex}
                              </span>
                            ))
                        )}
                      </div>
                    </td>
                    <td className="echo-actions-col">
                      <div className="inline-row echo-actions-inline">
                        <button
                          type="button"
                          className={editing ? "manage-btn-active" : ""}
                          onClick={() => {
                            if (editing) {
                              return;
                            }
                            startEdit(echo.echoId);
                          }}
                        >
                          管理
                        </button>
                        {editing ? (
                          <>
                            <button type="button" onClick={cancelEdit} disabled={saving}>
                              取消
                            </button>
                            <button type="button" onClick={() => void saveEdit()} disabled={saving}>
                              保存
                            </button>
                          </>
                        ) : null}
                        <button type="button" onClick={() => void removeEcho(echo.echoId)} disabled={saving}>
                          {pendingDeleteEchoId === echo.echoId ? "确认删除" : "删除"}
                        </button>
                      </div>
                    </td>
                  </tr>
                  {editing && editingEcho && basicDraft ? (
                    <tr key={`${echo.echoId}-editor`} className="active-row">
                      <td colSpan={6}>
                        <div className="inline-edit-panel">
                          <div className="chain-block">
                            <span className="chain-label">期望词条</span>
                            <div className="chain-row">
                              {expectationStats.length === 0 ? <span className="chain-empty">无</span> : null}
                              {expectationStats.map((statKey, idx) => {
                                const stat = statMap.get(statKey);
                                const selected = activeExpectationIndex === idx;
                                const availableStats = statDefs.filter(
                                  (x) => x.statKey === statKey || !expectationStats.includes(x.statKey),
                                );

                                return (
                                  <div className="chain-fragment" key={`exp-${idx}-${statKey}`}>
                                    <div
                                      className={selected ? "chain-item active" : "chain-item"}
                                      onDragOver={(e) => e.preventDefault()}
                                      onDrop={(e) => {
                                        e.preventDefault();
                                        const fromIndex = getDragIndex(e) ?? dragExpectationIndex;
                                        if (fromIndex === null || fromIndex === idx) {
                                          setDragExpectationIndex(null);
                                          return;
                                        }
                                        setExpectationStats((prev) => moveArrayItem(prev, fromIndex, idx));
                                        setDragExpectationIndex(null);
                                      }}
                                      onClick={() => setActiveExpectationIndex(idx)}
                                      onContextMenu={(e) => {
                                        e.preventDefault();
                                        removeExpectationAt(idx);
                                      }}
                                      title="单击编辑，右键删除"
                                    >
                                      <button
                                        type="button"
                                        className="drag-handle"
                                        draggable
                                        onDragStart={(e) => {
                                          e.stopPropagation();
                                          e.dataTransfer.effectAllowed = "move";
                                          e.dataTransfer.setData("text/plain", String(idx));
                                          setDragExpectationIndex(idx);
                                        }}
                                        onDragEnd={(e) => {
                                          e.stopPropagation();
                                          setDragExpectationIndex(null);
                                        }}
                                        onClick={(e) => e.stopPropagation()}
                                        title="按住拖动排序"
                                      >
                                        ::
                                      </button>
                                      {selected ? (
                                        <select
                                          value={statKey}
                                          onChange={(e) => {
                                            const next = e.target.value;
                                            setExpectationStats((prev) =>
                                              prev.map((item, itemIdx) => (itemIdx === idx ? next : item)),
                                            );
                                          }}
                                        >
                                          {availableStats.map((s) => (
                                            <option key={s.statKey} value={s.statKey}>
                                              {s.displayName}
                                            </option>
                                          ))}
                                        </select>
                                      ) : (
                                        <span>{stat?.displayName ?? statKey}</span>
                                      )}
                                    </div>
                                    {idx < expectationOps.length ? (
                                      <button
                                        type="button"
                                        className="chain-op"
                                        onClick={() =>
                                          setExpectationOps((prev) =>
                                            prev.map((x, opIdx) =>
                                              opIdx === idx ? (x === "gt" ? "eq" : "gt") : x,
                                            ),
                                          )
                                        }
                                        title="点击切换 > 或 ="
                                      >
                                        {expectationOps[idx] === "gt" ? ">" : "="}
                                      </button>
                                    ) : null}
                                  </div>
                                );
                              })}
                              <button type="button" className="chain-add" onClick={addExpectation}>
                                +
                              </button>
                            </div>
                          </div>

                          <div className="chain-block">
                            <span className="chain-label">槽位状态</span>
                            <div className="chain-row">
                              {editingOrderedSlots.length === 0 && slotsDraft.length === 0 ? (
                                <span className="chain-empty">无</span>
                              ) : null}
                              {editingOrderedSlots.map((slot) => (
                                <div
                                  key={`ordered-slot-${slot.slotNo}`}
                                  className="chain-item locked"
                                  title="顺序事件槽位（仅显示，不可编辑）"
                                >
                                  <span className="slot-label">S{slot.slotNo}</span>
                                  <span>
                                    {statMap.get(slot.statKey)?.displayName ?? slot.statKey} /{" "}
                                    {formatTierLabel(slot.statKey, slot.tierIndex)}
                                  </span>
                                </div>
                              ))}
                              {slotsDraft.map((slot, idx) => {
                                const stat = statMap.get(slot.statKey);
                                const selected = activeSlotIndex === idx;
                                const currentUsed = slotsDraft.map((x) => x.statKey);
                                const availableStats = statDefs.filter(
                                  (x) =>
                                    x.statKey === slot.statKey ||
                                    (!currentUsed.includes(x.statKey) && !editingOrderedStatSet.has(x.statKey)),
                                );
                                const tiers = statMap.get(slot.statKey)?.tiers ?? [];
                                const previewSlotNo = editingEditableSlots[idx] ?? idx + 1;

                                return (
                                  <div
                                    key={`slot-${idx}-${slot.statKey}`}
                                    className={selected ? "chain-item active" : "chain-item"}
                                    onDragOver={(e) => e.preventDefault()}
                                    onDrop={(e) => {
                                      e.preventDefault();
                                      const fromIndex = getDragIndex(e) ?? dragSlotIndex;
                                      if (fromIndex === null || fromIndex === idx) {
                                        setDragSlotIndex(null);
                                        return;
                                      }
                                      setSlotsDraft((prev) => moveArrayItem(prev, fromIndex, idx));
                                      setDragSlotIndex(null);
                                    }}
                                    onClick={() => setActiveSlotIndex(idx)}
                                    onContextMenu={(e) => {
                                      e.preventDefault();
                                      removeSlotAt(idx);
                                    }}
                                    title="单击编辑，右键删除"
                                  >
                                    <span className="slot-label">S{previewSlotNo}</span>
                                    <button
                                      type="button"
                                      className="drag-handle"
                                      draggable
                                      onDragStart={(e) => {
                                        e.stopPropagation();
                                        e.dataTransfer.effectAllowed = "move";
                                        e.dataTransfer.setData("text/plain", String(idx));
                                        setDragSlotIndex(idx);
                                      }}
                                      onDragEnd={(e) => {
                                        e.stopPropagation();
                                        setDragSlotIndex(null);
                                      }}
                                      onClick={(e) => e.stopPropagation()}
                                      title="按住拖动排序"
                                    >
                                      ::
                                    </button>
                                    {selected ? (
                                      <div className="inline-row">
                                        <select
                                          value={slot.statKey}
                                          onChange={(e) => {
                                            const nextStatKey = e.target.value;
                                            const nextTier =
                                              statMap.get(nextStatKey)?.tiers[0]?.tierIndex ?? 1;
                                            setSlotsDraft((prev) =>
                                              prev.map((item, itemIdx) =>
                                                itemIdx === idx
                                                  ? { statKey: nextStatKey, tierIndex: nextTier }
                                                  : item,
                                              ),
                                            );
                                          }}
                                        >
                                          {availableStats.map((s) => (
                                            <option key={s.statKey} value={s.statKey}>
                                              {s.displayName}
                                            </option>
                                          ))}
                                        </select>
                                        <select
                                          value={slot.tierIndex}
                                          onChange={(e) => {
                                            const nextTier = Number(e.target.value);
                                            setSlotsDraft((prev) =>
                                              prev.map((item, itemIdx) =>
                                                itemIdx === idx ? { ...item, tierIndex: nextTier } : item,
                                              ),
                                            );
                                          }}
                                        >
                                          {tiers.map((tier) => (
                                            <option key={tier.tierIndex} value={tier.tierIndex}>
                                              {formatTierLabel(slot.statKey, tier.tierIndex)}
                                            </option>
                                          ))}
                                        </select>
                                      </div>
                                    ) : (
                                      <span>
                                        {stat?.displayName ?? slot.statKey} / {formatTierLabel(slot.statKey, slot.tierIndex)}
                                      </span>
                                    )}
                                  </div>
                                );
                              })}
                              <button
                                type="button"
                                className="chain-add"
                                onClick={addSlot}
                                disabled={slotsDraft.length >= editingEditableSlots.length}
                              >
                                +
                              </button>
                            </div>
                            {editingLockedSlots.length > 0 ? (
                              <p className="hint">
                                已锁定槽位（顺序事件）：{editingLockedSlots.map((x) => `S${x}`).join(" / ")}
                              </p>
                            ) : null}
                          </div>
                        </div>
                      </td>
                    </tr>
                  ) : null}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
