import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  appendOrderedEvent,
  createEcho,
  deleteOrderedEvent,
  getDailyPatternDecision,
  getEchoesForStat,
  getEventHistory,
  getGlobalDistribution,
  saveExpectationPreset,
  setExpectations,
  upsertBackfillState,
} from "../api/tauri";
import { BarChart } from "../components/BarChart";
import { HypothesisVerification } from "../components/HypothesisVerification";
import {
  beginLongPressDrag,
  cancelLongPressDragCandidate,
  CHAIN_LONG_PRESS_MS,
  completeLongPressTap,
  moveArrayToInsertion,
  remapIndexAfterInsertion,
  resolveInsertBeforeIndex,
  updateLongPressDragCandidate,
  useChainDragSession,
  type ChainDragState,
} from "../hooks/useChainDrag";
import { useChainSelectionDismiss } from "../hooks/useChainSelectionDismiss";
import { useAppStore } from "../store/useAppStore";
import type {
  DailyPatternDecisionReport,
  DistributionFilter,
  DistributionPayload,
  EchoProbRow,
  EchoStatus,
  EventRow,
  ExpectationItem,
  ExpectationPreset,
} from "../types/domain";

/* ── helpers ─────────────────────────────────────────── */

type RelOp = "gt" | "eq";
type DragKind = "expectation" | "slot";

interface SlotDraft {
  statKey: string;
  tierIndex: number;
}

type DragState = ChainDragState<DragKind>;

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

function toPercent(value: number): string {
  return `${(value * 100).toFixed(2)}%`;
}

function parseGuessShapes(raw: string): string[] {
  const uniq = new Set<string>();
  for (const token of raw.split(/[\\s,，;；|]+/)) {
    const normalized = token.toUpperCase().replace(/[^A-Z]/g, "");
    if (normalized.length >= 2) {
      uniq.add(normalized);
    }
  }
  return Array.from(uniq);
}

/* ── chain helpers (same logic as EchoPoolPage) ───── */

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
  if (stats.length === 0) return [];
  const result: ExpectationItem[] = [];
  let rank = 1;
  result.push({ statKey: stats[0], rank });
  for (let i = 1; i < stats.length; i += 1) {
    if ((ops[i - 1] ?? "gt") === "gt") rank += 1;
    result.push({ statKey: stats[i], rank });
  }
  return result;
}

function findPresetByChain(
  presets: ExpectationPreset[],
  stats: string[],
  ops: RelOp[],
): ExpectationPreset | null {
  return (
    presets.find((preset) => {
      const chain = buildExpectationChain(preset.items);
      if (chain.stats.length !== stats.length) return false;
      for (let i = 0; i < stats.length; i++) if (chain.stats[i] !== stats[i]) return false;
      for (let i = 0; i < ops.length; i++) if (chain.ops[i] !== ops[i]) return false;
      return true;
    }) ?? null
  );
}

const STATUS_LABELS: Record<EchoStatus, string> = {
  tracking: "追踪中",
  paused: "已暂停",
  abandoned: "已弃置",
  completed: "已完成",
};

const STATUS_BADGE_CLASS: Record<EchoStatus, string> = {
  tracking: "badge-tracking",
  paused: "badge-paused",
  abandoned: "badge-abandoned",
  completed: "badge-completed",
};

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

function getStatColorClass(statKey: string): string {
  if (["hp_pct", "hp_flat", "def_pct", "def_flat"].includes(statKey)) return "record-history-abbr-green";
  if (["crit_rate", "crit_dmg"].includes(statKey)) return "record-history-abbr-red";
  if (statKey === "energy_regen") return "record-history-abbr-regen";
  if (["atk_pct", "atk_flat"].includes(statKey)) return "record-history-abbr-atk";
  if (["basic_dmg", "heavy_dmg", "skill_dmg", "liberation_dmg"].includes(statKey)) return "record-history-abbr-blue";
  return "record-history-abbr-blue"; // default
}

/* ── component ───────────────────────────────────── */

export function RecordPage() {
  const { echoes, statDefs, expectationPresets, selectedEchoId, setSelectedEchoId, createFormDraft, patchCreateForm, refreshEchoes, refreshExpectationPresets } = useAppStore();
  const statMap = useMemo(() => new Map(statDefs.map((x) => [x.statKey, x])), [statDefs]);

  /* === create echo form — persisted in store === */
  // read aliases
  const createExpanded = createFormDraft.expanded;
  const createNickname = createFormDraft.nickname;
  const createMainStat = createFormDraft.mainStat;
  const createCost = createFormDraft.cost;
  const createStatus = createFormDraft.status;
  const createExpStats = createFormDraft.expStats;
  const createExpOps = createFormDraft.expOps as RelOp[];
  const createSlots = createFormDraft.slots as SlotDraft[];
  // write wrappers (same API as useState setters so callers need no change)
  const setCreateExpanded = (v: boolean | ((p: boolean) => boolean)) =>
    patchCreateForm({ expanded: typeof v === "function" ? v(createFormDraft.expanded) : v });
  const setCreateNickname = (v: string) => patchCreateForm({ nickname: v });
  const setCreateMainStat = (v: string) => patchCreateForm({ mainStat: v });
  const setCreateCost = (v: number) => patchCreateForm({ cost: v });
  const setCreateStatus = (v: EchoStatus) => patchCreateForm({ status: v });
  const setCreateExpStats: React.Dispatch<React.SetStateAction<string[]>> = (action) =>
    patchCreateForm({ expStats: typeof action === "function" ? action(createFormDraft.expStats) : action });
  const setCreateExpOps: React.Dispatch<React.SetStateAction<RelOp[]>> = (action) =>
    patchCreateForm({ expOps: typeof action === "function" ? action(createFormDraft.expOps as RelOp[]) : action });
  const setCreateSlots: React.Dispatch<React.SetStateAction<SlotDraft[]>> = (action) =>
    patchCreateForm({ slots: typeof action === "function" ? action(createFormDraft.slots as SlotDraft[]) : action });

  // local-only UI state (no need to persist)
  const [createActiveExpIdx, setCreateActiveExpIdx] = useState<number | null>(null);
  const [createPresetId, setCreatePresetId] = useState<string | null>(null);
  const [createPresetSelectorOpen, setCreatePresetSelectorOpen] = useState(false);
  const [createPresetNamingOpen, setCreatePresetNamingOpen] = useState(false);
  const [createPresetNamingValue, setCreatePresetNamingValue] = useState("");
  const [createPresetConflictId, setCreatePresetConflictId] = useState<string | null>(null);
  const createPresetBtnRef = useRef<HTMLButtonElement | null>(null);
  const createPresetMenuRef = useRef<HTMLDivElement | null>(null);
  const createPresetNamingInputRef = useRef<HTMLInputElement | null>(null);
  const createPresetNamingFormRef = useRef<HTMLFormElement | null>(null);
  const [createPresetNamingPos, setCreatePresetNamingPos] = useState({ left: 0, top: 0, width: 260 });

  const [createActiveSlotIdx, setCreateActiveSlotIdx] = useState<number | null>(null);

  /* === drag-to-reorder === */
  const [dragState, setDragState] = useState<DragState | null>(null);
  const dragStateRef = useRef<DragState | null>(null);
  const createExpRowRef = useRef<HTMLDivElement | null>(null);
  const createSlotRowRef = useRef<HTMLDivElement | null>(null);
  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startPosRef = useRef({ x: 0, y: 0 });

  /* === record event === */
  const [statKey, setStatKey] = useState<string>("crit_rate");
  const [tierIndex, setTierIndex] = useState<number>(1);
  const [eventTimeLocal, setEventTimeLocal] = useState<string>(toLocalInputValue(new Date()));
  const [eventHistory, setEventHistory] = useState<EventRow[]>([]);
  
  const { recordPageDraft, patchRecordPageDraft } = useAppStore();
  const { historyLimitStr, historyTodayOnly } = recordPageDraft;
  const setHistoryLimitStr = (val: string) => patchRecordPageDraft({ historyLimitStr: val });
  const setHistoryTodayOnly = (val: boolean) => patchRecordPageDraft({ historyTodayOnly: val });

  /* === distribution / analysis === */
  const [distributionFilter, setDistributionFilter] = useState<DistributionFilter>({});
  const [distribution, setDistribution] = useState<DistributionPayload | null>(null);
  const [selectedDistStatKey, setSelectedDistStatKey] = useState<string | null>(null);
  const [echoProbRows, setEchoProbRows] = useState<EchoProbRow[]>([]);
  const [patternDecision, setPatternDecision] = useState<DailyPatternDecisionReport | null>(null);
  const [patternManualStartStr, setPatternManualStartStr] = useState("0");
  const [patternManualCycleStr, setPatternManualCycleStr] = useState("5");
  const [patternManualGuessStr, setPatternManualGuessStr] = useState("AABCB, ABA");
  const [patternAutoMinLenStr, setPatternAutoMinLenStr] = useState("3");
  const [patternConfigVersion, setPatternConfigVersion] = useState(0);
  const [sortBy, setSortBy] = useState("pFinal");

  /* === misc === */
  const [saving, setSaving] = useState(false);
  const [undoConfirmId, setUndoConfirmId] = useState<string | null>(null);
  const [loadingDist, setLoadingDist] = useState(false);
  const [loadingProb, setLoadingProb] = useState(false);
  const [loadingPatternDecision, setLoadingPatternDecision] = useState(false);
  const [message, setMessage] = useState("");
  const [msgKind, setMsgKind] = useState<"info" | "success" | "error">("info");
  const [messageId, setMessageId] = useState(0);

  const showMsg = (text: string, kind: "info" | "success" | "error" = "info") => {
    setMessage(text);
    setMsgKind(kind);
    if (text) {
      setMessageId((id) => id + 1);
    }
  };

  useEffect(() => {
    if (!message) return;
    const currentId = messageId;
    const tid = setTimeout(() => {
      setMessage((m) => (m && messageId === currentId ? "" : m));
    }, 2600);
    return () => clearTimeout(tid);
  }, [message, messageId]);

  /* ── selected echo derivations ───── */

  const selectedEcho = useMemo(
    () => echoes.find((e) => e.echoId === selectedEchoId) ?? null,
    [echoes, selectedEchoId],
  );

  const occupiedSlots = useMemo(
    () => new Set((selectedEcho?.currentSubstats ?? []).map((s) => s.slotNo)),
    [selectedEcho],
  );

  const occupiedStats = useMemo(
    () => new Set((selectedEcho?.currentSubstats ?? []).map((s) => s.statKey)),
    [selectedEcho],
  );

  const availableSlots = useMemo(() => [1, 2, 3, 4, 5].filter((x) => !occupiedSlots.has(x)), [occupiedSlots]);
  const nextSlotNo = availableSlots[0] ?? 1;

  const availableStatDefs = useMemo(
    () => statDefs.filter((s) => !occupiedStats.has(s.statKey)),
    [statDefs, occupiedStats],
  );

  const selectedStat = useMemo(
    () => availableStatDefs.find((s) => s.statKey === statKey) ?? availableStatDefs[0] ?? null,
    [availableStatDefs, statKey],
  );

  const selectedTierValue = selectedStat?.tiers.find((x) => x.tierIndex === tierIndex)?.valueScaled ?? 0;

  const distributionChartData = useMemo(() => {
    const rows = distribution?.rows ?? [];
    return { labels: rows.map((r) => r.displayName), values: rows.map((r) => r.pGlobal) };
  }, [distribution]);

  const createPresetName = useMemo(
    () => expectationPresets.find((p) => p.presetId === createPresetId)?.name ?? null,
    [expectationPresets, createPresetId],
  );

  /* ── drag derivations ──────────── */
  const draggingExpFromIndex = dragState?.kind === "expectation" ? dragState.fromIndex : null;
  const draggingSlotFromIndex = dragState?.kind === "slot" ? dragState.fromIndex : null;
  const expInsertBeforeIndex =
    dragState?.kind === "expectation"
      ? resolveInsertBeforeIndex(createExpStats.length, dragState.fromIndex, dragState.dropIndex)
      : null;
  const slotInsertBeforeIndex =
    dragState?.kind === "slot"
      ? resolveInsertBeforeIndex(createSlots.length, dragState.fromIndex, dragState.dropIndex)
      : null;

  /* ── effects ───────────────────── */

  useEffect(() => {
    if (!selectedEchoId && echoes.length > 0) setSelectedEchoId(echoes[0].echoId);
  }, [echoes, selectedEchoId]);

  useEffect(() => {
    if (!selectedStat) return;
    if (statKey !== selectedStat.statKey) { setStatKey(selectedStat.statKey); return; }
    if (!selectedStat.tiers.some((t) => t.tierIndex === tierIndex)) setTierIndex(selectedStat.tiers[0]?.tierIndex ?? 1);
  }, [selectedStat, statKey, tierIndex]);

  useEffect(() => {
    if (!statDefs.some((s) => s.statKey === createMainStat) && statDefs.length > 0) setCreateMainStat(statDefs[0].statKey);
  }, [createMainStat, statDefs]);

  // auto-sync preset match
  useEffect(() => {
    const matched = findPresetByChain(expectationPresets, createExpStats, createExpOps);
    setCreatePresetId(matched?.presetId ?? null);
  }, [createExpStats, createExpOps, expectationPresets]);

  // close preset menu on outside click
  useEffect(() => {
    if (!createPresetSelectorOpen && !createPresetNamingOpen) return;
    const handler = (e: PointerEvent) => {
      if (!(e.target instanceof Node)) return;
      if (createPresetBtnRef.current?.contains(e.target)) return;
      if (createPresetMenuRef.current?.contains(e.target)) return;
      if (createPresetNamingFormRef.current?.contains(e.target)) return;
      setCreatePresetSelectorOpen(false);
      setCreatePresetNamingOpen(false);
      setCreatePresetConflictId(null);
    };
    window.addEventListener("pointerdown", handler);
    return () => window.removeEventListener("pointerdown", handler);
  }, [createPresetSelectorOpen, createPresetNamingOpen]);

  // auto-focus naming input & track position
  useEffect(() => {
    if (!createPresetNamingOpen) return;
    const frame = requestAnimationFrame(() => {
      syncCreatePresetNamingPos();
      const input = createPresetNamingInputRef.current;
      if (!input) return;
      input.focus();
      input.select();
    });
    const onResize = () => syncCreatePresetNamingPos();
    window.addEventListener("resize", onResize);
    window.addEventListener("scroll", onResize, true);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("scroll", onResize, true);
    };
  }, [createPresetNamingOpen]);

  const dismissChainSelection = useCallback(() => {
    setCreateActiveExpIdx(null);
    setCreateActiveSlotIdx(null);
  }, []);

  const handleChainPointerDown = useCallback((target: Element) => {
    if (!target.closest(".record-undo-btn")) {
      setUndoConfirmId(null);
    }
  }, []);

  useChainSelectionDismiss({
    chainScopeSelector: ".chain-item, .chain-op",
    onDismiss: dismissChainSelection,
    onPointerDown: handleChainPointerDown,
  });

  useChainDragSession<DragKind, DragState>({
    dragState,
    dragStateRef,
    setDragState,
    getRowElement: (kind) => (kind === "expectation" ? createExpRowRef.current : createSlotRowRef.current),
    onApplyDrag: (current) => {
      if (current.kind === "expectation") {
        setCreateExpStats((prev) => moveArrayToInsertion(prev, current.fromIndex, current.dropIndex));
        setCreateActiveExpIdx((prev) =>
          remapIndexAfterInsertion(prev, createExpStats.length, current.fromIndex, current.dropIndex),
        );
      } else {
        setCreateSlots((prev) => moveArrayToInsertion(prev, current.fromIndex, current.dropIndex));
        setCreateActiveSlotIdx((prev) =>
          remapIndexAfterInsertion(prev, createSlots.length, current.fromIndex, current.dropIndex),
        );
      }
    },
  });

  const parsedHistoryLimit = parseInt(historyLimitStr, 10) || 20;

  const loadHistory = async (limit = parsedHistoryLimit, todayOnly = historyTodayOnly) => {
    const fetchLimit = todayOnly ? Math.max(200, limit * 3) : Math.max(100, limit);
    const rows = await getEventHistory({ limit: fetchLimit });
    setEventHistory(rows);
  };

  const loadDistribution = async () => {
    setLoadingDist(true);
    try {
      const result = await getGlobalDistribution(distributionFilter);
      setDistribution(result);
      if (!selectedDistStatKey && result.rows.length > 0) setSelectedDistStatKey(result.rows[0].statKey);
    } finally { setLoadingDist(false); }
  };

  const loadEchoProbRows = async (statKey?: string) => {
    const key = statKey ?? selectedDistStatKey;
    if (!key) { setEchoProbRows([]); return; }
    setLoadingProb(true);
    try {
      const rows = await getEchoesForStat({ statKey: key, sortBy, ...distributionFilter });
      setEchoProbRows(rows);
    } finally { setLoadingProb(false); }
  };

  const loadPatternDecision = async () => {
    setLoadingPatternDecision(true);
    try {
      const manualStartIndex = Math.max(0, Number(patternManualStartStr) || 0);
      const manualCycleLen = Math.max(2, Number(patternManualCycleStr) || 5);
      const autoMinLen = Math.max(2, Number(patternAutoMinLenStr) || 3);
      const manualGuessShapes = parseGuessShapes(patternManualGuessStr);
      const report = await getDailyPatternDecision({
        manualStartIndex,
        manualCycleLen,
        manualGuessShapes,
        minLen: autoMinLen,
        maxLen: Math.max(autoMinLen, manualCycleLen + 1),
        minSupport: 2,
        maxOrder: 5,
        topK: 10,
      });
      setPatternDecision(report);
    } finally {
      setLoadingPatternDecision(false);
    }
  };

  // mount fetching
  useEffect(() => { void loadDistribution(); }, []);
  useEffect(() => { void loadPatternDecision(); }, [patternConfigVersion]);
  // history fetching
  useEffect(() => { void loadHistory(parsedHistoryLimit, historyTodayOnly); }, [parsedHistoryLimit, historyTodayOnly]);

  useEffect(() => { void loadDistribution(); setSelectedDistStatKey(null); setEchoProbRows([]); }, [
    distributionFilter.startTime, distributionFilter.endTime,
    distributionFilter.mainStatKey, distributionFilter.costClass, distributionFilter.status,
  ]);

  // hit list is loaded manually on click, not automatically
  const handleDistRowClick = (statKey: string) => {
    setSelectedDistStatKey(statKey);
    void loadEchoProbRows(statKey);
  };

  const handleSortByChange = (newSortBy: string) => {
    setSortBy(newSortBy);
    if (selectedDistStatKey) {
      void (async () => {
        setLoadingProb(true);
        try {
          const rows = await getEchoesForStat({ statKey: selectedDistStatKey, sortBy: newSortBy, ...distributionFilter });
          setEchoProbRows(rows);
        } finally { setLoadingProb(false); }
      })();
    }
  };

  /* ── chain helpers for create form ─── */

  const pickAvailableStat = (used: string[]) => {
    const found = statDefs.find((s) => !used.includes(s.statKey));
    return found?.statKey ?? statDefs[0]?.statKey ?? "crit_rate";
  };

  const addCreateExp = () => {
    setCreateExpStats((prev) => {
      const next = pickAvailableStat(prev);
      if (prev.includes(next)) { showMsg("没有可添加的期望词条。"); return prev; }
      if (prev.length > 0) setCreateExpOps((ops) => [...ops, "gt"]);
      setCreateActiveExpIdx(prev.length);
      return [...prev, next];
    });
  };

  const removeCreateExpAt = (idx: number) => {
    setCreateExpStats((prev) => {
      const next = prev.filter((_, i) => i !== idx);
      setCreateExpOps((prevOps) => {
        if (prev.length <= 1) return [];
        const ops = [...prevOps];
        if (idx === 0) { ops.shift(); return ops; }
        if (idx === prev.length - 1) { ops.pop(); return ops; }
        ops[idx - 1] = "gt";
        ops.splice(idx, 1);
        return ops;
      });
      return next;
    });
    setCreateActiveExpIdx((prev) => {
      if (prev === null) return null;
      if (prev === idx) return null;
      return prev > idx ? prev - 1 : prev;
    });
  };

  const addCreateSlot = () => {
    setCreateSlots((prev) => {
      if (prev.length >= 5) { showMsg("最多5个初始槽位。"); return prev; }
      const used = prev.map((x) => x.statKey);
      const sk = pickAvailableStat(used);
      if (used.includes(sk)) { showMsg("没有可添加的槽位词条。"); return prev; }
      const ti = statMap.get(sk)?.tiers[0]?.tierIndex ?? 1;
      setCreateActiveSlotIdx(prev.length);
      return [...prev, { statKey: sk, tierIndex: ti }];
    });
  };

  const removeCreateSlotAt = (idx: number) => {
    setCreateSlots((prev) => prev.filter((_, i) => i !== idx));
    setCreateActiveSlotIdx((prev) => {
      if (prev === null) return null;
      if (prev === idx) return null;
      return prev > idx ? prev - 1 : prev;
    });
  };

  const applyPresetToCreate = (preset: ExpectationPreset) => {
    const chain = buildExpectationChain(preset.items);
    setCreateExpStats(chain.stats);
    setCreateExpOps(chain.ops);
    setCreateActiveExpIdx(null);
    setCreatePresetId(preset.presetId);
    setCreatePresetSelectorOpen(false);
    setCreatePresetNamingOpen(false);
    showMsg(`已载入预设「${preset.name}」。`);
  };

  const syncCreatePresetNamingPos = () => {
    const btn = createPresetBtnRef.current;
    if (!btn) return;
    const rect = btn.getBoundingClientRect();
    setCreatePresetNamingPos({
      left: rect.left,
      top: rect.bottom + 4,
      width: Math.max(260, rect.width),
    });
  };

  const openSaveCreatePreset = () => {
    if (createExpStats.length === 0) { showMsg("请先添加期望词条。"); return; }
    const d = new Date();
    const defaultName = `新预设 ${d.getFullYear()}${String(d.getMonth() + 1).padStart(2, "0")}${String(d.getDate()).padStart(2, "0")}`;
    setCreatePresetNamingValue(defaultName);
    setCreatePresetSelectorOpen(false);
    setCreatePresetNamingOpen(true);
    requestAnimationFrame(() => syncCreatePresetNamingPos());
  };

  const handleSaveCreatePreset = async (e?: React.FormEvent, forceOverwriteId?: string) => {
    if (e) e.preventDefault();
    const name = createPresetNamingValue.trim();
    if (!name) { showMsg("请输入预设名称。"); return; }

    if (!forceOverwriteId) {
      const conflict = expectationPresets.find((p) => p.name === name);
      if (conflict) {
        setCreatePresetConflictId(conflict.presetId);
        return;
      }
    }

    setSaving(true);
    try {
      const items = chainToExpectationItems(createExpStats, createExpOps);
      const result = await saveExpectationPreset({ presetId: forceOverwriteId, name, items });
      await refreshExpectationPresets();
      setCreatePresetId(result.presetId);
      setCreatePresetNamingOpen(false);
      setCreatePresetSelectorOpen(false);
      setCreatePresetConflictId(null);
      showMsg(`已设为预设「${name}」。`, "success");
    } catch (err) {
      showMsg(String(err), "error");
    } finally {
      setSaving(false);
    }
  };

  const resetCreateForm = () => {
    setCreateNickname("");
    setCreateStatus("tracking");
    setCreateSlots([]);
    setCreateActiveSlotIdx(null);
    setCreateActiveExpIdx(null);
    setCreatePresetNamingOpen(false);
    setCreatePresetNamingValue("");
  };

  /* ── handlers ──────────────────── */

  const handleCreateEcho = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    showMsg("");

    try {
      const created = await createEcho({
        nickname: createNickname || undefined,
        mainStatKey: createMainStat,
        costClass: createCost,
        status: createStatus,
      });

      // apply expectations
      const items = chainToExpectationItems(createExpStats, createExpOps);
      if (items.length > 0) {
        await setExpectations(created.echoId, items);
      }

      // apply initial slots
      if (createSlots.length > 0) {
        const uniqueSlots = Array.from(new Set(createSlots.map((x) => x.statKey)));
        if (uniqueSlots.length !== createSlots.length) {
          showMsg("初始槽位词条存在重复，已忽略槽位设置。", "error");
        } else {
          const mapped = createSlots.map((slot, idx) => ({
            slotNo: idx + 1,
            statKey: slot.statKey,
            tierIndex: slot.tierIndex,
          }));
          await upsertBackfillState({ echoId: created.echoId, slots: mapped });
        }
      }

      await refreshEchoes();
      setSelectedEchoId(created.echoId);
      resetCreateForm();
      showMsg("声骸创建成功，可直接继续强化。", "success");
    } catch (error) {
      showMsg(String(error), "error");
    } finally {
      setSaving(false);
    }
  };

  const handleRecordEvent = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedEchoId) { showMsg("请先选择声骸。"); return; }
    if (availableSlots.length === 0) { showMsg("所有槽位已满。"); return; }
    if (!selectedStat) { showMsg("没有可出的副词条。"); return; }

    setSaving(true);
    showMsg("");

    try {
      const result = await appendOrderedEvent({
        echoId: selectedEchoId,
        slotNo: nextSlotNo,
        statKey: selectedStat.statKey,
        tierIndex,
        eventTime: normalizeLocalTime(eventTimeLocal),
      });
      await Promise.all([refreshEchoes(), loadHistory(), loadDistribution(), loadPatternDecision()]);
      await loadEchoProbRows();
      showMsg(`录入成功  ·  eventId: ${result.eventId.slice(0, 8)}`, "success");
    } catch (error) {
      showMsg(String(error), "error");
    } finally {
      setSaving(false);
    }
  };

  const handleUndoEvent = async (eventId: string) => {
    if (undoConfirmId !== eventId) {
      setUndoConfirmId(eventId);
      return;
    }
    setSaving(true);
    setUndoConfirmId(null);
    showMsg("");
    try {
      await deleteOrderedEvent({ eventId });
      await Promise.all([refreshEchoes(), loadHistory(), loadDistribution(), loadPatternDecision()]);
      await loadEchoProbRows();
      showMsg("已撤销最近一次录入。", "success");
    } catch (error) {
      showMsg(String(error), "error");
    } finally {
      setSaving(false);
    }
  };

  /* ── render helpers ────────────── */

  const renderExpChainEditor = (
    stats: string[],
    ops: RelOp[],
    activeIdx: number | null,
    setStats: React.Dispatch<React.SetStateAction<string[]>>,
    setOps: React.Dispatch<React.SetStateAction<RelOp[]>>,
    setActiveIdx: React.Dispatch<React.SetStateAction<number | null>>,
    addFn: () => void,
    removeFn: (idx: number) => void,
  ) => (
    <div className="chain-row" ref={createExpRowRef}>
      {stats.length === 0 ? <span className="chain-empty">点击 + 添加</span> : null}
      {stats.map((sk, idx) => {
        const stat = statMap.get(sk);
        const selected = activeIdx === idx;
        const availStats = statDefs.filter((x) => x.statKey === sk || !stats.includes(x.statKey));

        const hideOperator = draggingExpFromIndex !== null && idx === draggingExpFromIndex;
        const isDraggingThis = draggingExpFromIndex === idx;
        const classNames = ["chain-item"];
        if (selected) classNames.push("active");
        if (isDraggingThis) classNames.push("dragging");

        return (
          <Fragment key={`exp-${idx}-${sk}`}>
            {expInsertBeforeIndex === idx ? (
              <span className="drag-insert-line" aria-hidden="true" />
            ) : null}
            <div className="chain-fragment">
              <div
                className={classNames.join(" ")}
                data-drag-kind="expectation"
                data-drag-index={idx}
                onPointerDown={(e) => {
                  beginLongPressDrag<DragKind, DragState>({
                    event: e,
                    kind: "expectation",
                    fromIndex: idx,
                    label: stat?.displayName ?? sk,
                    longPressTimerRef,
                    startPosRef,
                    setDragState,
                    longPressMs: CHAIN_LONG_PRESS_MS,
                    ignoreTagNames: ["SELECT"],
                  });
                }}
                onPointerMove={(e) => {
                  updateLongPressDragCandidate({
                    event: e,
                    longPressTimerRef,
                    startPosRef,
                  });
                }}
                onPointerCancel={() => cancelLongPressDragCandidate(longPressTimerRef)}
                onPointerUp={() => {
                  completeLongPressTap({
                    longPressTimerRef,
                    onTap: () => setActiveIdx(idx),
                  });
                }}
                onContextMenu={(e) => { e.preventDefault(); removeFn(idx); }}
                title="长按拖动，点击编辑，右键删除"
              >
                {selected ? (
                  <select
                    value={sk}
                    onChange={(e) => {
                      const v = e.target.value;
                      setStats((prev) => prev.map((item, i) => (i === idx ? v : item)));
                    }}
                    onPointerDown={(e) => e.stopPropagation()}
                  >
                    {availStats.map((s) => (
                      <option key={s.statKey} value={s.statKey}>{s.displayName}</option>
                    ))}
                  </select>
                ) : (
                  <span>{stat?.displayName ?? sk}</span>
                )}
              </div>
              {idx < ops.length && !hideOperator ? (
                <button
                  type="button" className="chain-op"
                  onClick={() => setOps((prev) => prev.map((x, i) => (i === idx ? (x === "gt" ? "eq" : "gt") : x)))}
                  title="点击切换 > 或 ="
                >
                  {ops[idx] === "gt" ? ">" : "="}
                </button>
              ) : null}
            </div>
          </Fragment>
        );
      })}
      {dragState?.kind === "expectation" && expInsertBeforeIndex === null ? (
        <span className="drag-insert-line" aria-hidden="true" />
      ) : null}
      <button type="button" className="chain-add" onClick={addFn}>+</button>
    </div>
  );

  const renderSlotEditor = (
    slots: SlotDraft[],
    activeIdx: number | null,
    setSlots: React.Dispatch<React.SetStateAction<SlotDraft[]>>,
    setActiveIdx: React.Dispatch<React.SetStateAction<number | null>>,
    addFn: () => void,
    removeFn: (idx: number) => void,
  ) => (
    <div className="chain-row" ref={createSlotRowRef}>
      {slots.length === 0 ? <span className="chain-empty">点击 + 添加初始词条</span> : null}
      {slots.map((slot, idx) => {
        const stat = statMap.get(slot.statKey);
        const selected = activeIdx === idx;
        const isDraggingThis = draggingSlotFromIndex === idx;
        const classNames = ["chain-item"];
        if (selected) classNames.push("active");
        if (isDraggingThis) classNames.push("dragging");

        const currentUsed = slots.map((x) => x.statKey);
        const availStats = statDefs.filter(
          (x) => x.statKey === slot.statKey || !currentUsed.includes(x.statKey),
        );
        const tiers = statMap.get(slot.statKey)?.tiers ?? [];

        return (
          <Fragment key={`slot-${idx}-${slot.statKey}`}>
            {slotInsertBeforeIndex === idx ? (
              <span className="drag-insert-line" aria-hidden="true" />
            ) : null}
            <div className="chain-fragment">
              <div
                className={classNames.join(" ")}
                data-drag-kind="slot"
                data-drag-index={idx}
                onContextMenu={(e) => { e.preventDefault(); removeFn(idx); }}
                onPointerDown={(e) => {
                  beginLongPressDrag<DragKind, DragState>({
                    event: e,
                    kind: "slot",
                    fromIndex: idx,
                    label: `S${idx + 1} ${statKeyToAbbr(slot.statKey)}${slot.tierIndex}`,
                    longPressTimerRef,
                    startPosRef,
                    setDragState,
                    longPressMs: CHAIN_LONG_PRESS_MS,
                    ignoreTagNames: ["SELECT"],
                  });
                }}
                onPointerMove={(e) => {
                  updateLongPressDragCandidate({
                    event: e,
                    longPressTimerRef,
                    startPosRef,
                  });
                }}
                onPointerCancel={() => cancelLongPressDragCandidate(longPressTimerRef)}
                onPointerUp={() => {
                  completeLongPressTap({
                    longPressTimerRef,
                    onTap: () => setActiveIdx(idx),
                  });
                }}
                title="长按拖动，点击编辑，右键删除"
              >
                {selected ? (
                  <>
                    <span className="slot-label">S{idx + 1}</span>
                    <div className="inline-row" onPointerDown={e => e.stopPropagation()}>
                      <select
                        value={slot.statKey}
                        onChange={(e) => {
                          const nk = e.target.value;
                          const nt = statMap.get(nk)?.tiers[0]?.tierIndex ?? 1;
                          setSlots((prev) => prev.map((item, i) => (i === idx ? { statKey: nk, tierIndex: nt } : item)));
                        }}
                      >
                        {availStats.map((s) => (
                          <option key={s.statKey} value={s.statKey}>{s.displayName}</option>
                        ))}
                      </select>
                      <select
                        value={slot.tierIndex}
                        onChange={(e) => {
                          const nt = Number(e.target.value);
                          setSlots((prev) => prev.map((item, i) => (i === idx ? { ...item, tierIndex: nt } : item)));
                        }}
                      >
                        {tiers.map((t) => (
                          <option key={t.tierIndex} value={t.tierIndex}>
                            档{t.tierIndex}: {formatScaledValue(stat?.unit ?? "flat", t.valueScaled)}
                          </option>
                        ))}
                      </select>
                    </div>
                  </>
                ) : (
                  <span>
                    {idx + 1}: {statKeyToAbbr(slot.statKey)}{slot.tierIndex}={formatScaledValue(stat?.unit ?? "flat", stat?.tiers.find((t) => t.tierIndex === slot.tierIndex)?.valueScaled ?? 0)}
                  </span>
                )}
              </div>
            </div>
          </Fragment>
        );
      })}
      {dragState?.kind === "slot" && slotInsertBeforeIndex === null ? (
        <span className="drag-insert-line" aria-hidden="true" />
      ) : null}
      <button type="button" className="chain-add" onClick={addFn} disabled={slots.length >= 5}>+</button>
    </div>
  );

  /* ── main render ───────────────── */

  const displayedHistory = useMemo(() => {
    let list = eventHistory;
    if (historyTodayOnly) {
      const now = new Date();
      // "Today" starts at 4:00 AM local time
      const boundary = new Date(now);
      boundary.setHours(4, 0, 0, 0);
      // If it's currently before 4:00 AM, the boundary is yesterday's 4:00 AM
      if (now < boundary) {
        boundary.setDate(boundary.getDate() - 1);
      }
      list = list.filter((r) => new Date(r.eventTime) >= boundary);
    }
    return list.slice(0, parsedHistoryLimit);
  }, [eventHistory, historyTodayOnly, parsedHistoryLimit]);

  /* === history search === */
  const [searchStats, setSearchStats] = useState<string[]>([]);
  const [searchActiveIdx, setSearchActiveIdx] = useState<number | null>(null);
  const [matchIndices, setMatchIndices] = useState<number[]>([]);
  const [currentMatchIdx, setCurrentMatchIdx] = useState<number>(-1);
  const historyListRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (searchStats.length === 0) {
      setMatchIndices([]);
      setCurrentMatchIdx(-1);
      return;
    }
    const matches: number[] = [];
    // User: "在前方的词条对应先出的词条"
    // In searchStats [S1, S2, S3], S1 is oldest.
    // In displayedHistory [H0 (newest), H1, ..., Hn (oldest)]
    // Match logic: H[i] == SN, ..., H[i+N-1] == S1
    for (let i = 0; i <= displayedHistory.length - searchStats.length; i++) {
      let isMatch = true;
      for (let j = 0; j < searchStats.length; j++) {
        const historyItem = displayedHistory[i + (searchStats.length - 1 - j)];
        if (historyItem.statKey !== searchStats[j]) {
          isMatch = false;
          break;
        }
      }
      if (isMatch) matches.push(i);
    }
    setMatchIndices(matches);
    setCurrentMatchIdx(matches.length > 0 ? 0 : -1);
  }, [searchStats, displayedHistory]);

  const jumpToMatch = (dir: "prev" | "next") => {
    if (matchIndices.length === 0) return;
    let nextIdx = currentMatchIdx + (dir === "next" ? 1 : -1);
    // Note: Since list is newest-to-oldest, "prev" in search (older) means "next" in list index
    // But we'll just follow the list order for the jump buttons (up/down)
    if (nextIdx < 0) nextIdx = matchIndices.length - 1;
    if (nextIdx >= matchIndices.length) nextIdx = 0;
    setCurrentMatchIdx(nextIdx);

    const targetIdx = matchIndices[nextIdx];
    const listEl = historyListRef.current;
    if (listEl && listEl.children[targetIdx]) {
      listEl.children[targetIdx].scrollIntoView({ behavior: "smooth", block: "center" });
    }
  };

  return (
    <section className="page record-page">
      {/* Toast */}
      {message ? (
        <div className={`toast toast-${msgKind}`} onClick={() => showMsg("")}>{message}</div>
      ) : null}
      {/* Drag ghost */}
      {dragState ? (
        <div className="drag-ghost" style={{ left: dragState.x + 12, top: dragState.y + 12 }}>
          <span>{dragState.label}</span>
        </div>
      ) : null}
      {/* Preset naming popover — outside all forms to avoid nested-form issues */}
      {createPresetNamingOpen ? (
        <form
          ref={createPresetNamingFormRef}
          className="record-preset-naming-pop"
          style={{
            left: `${createPresetNamingPos.left}px`,
            top: `${createPresetNamingPos.top}px`,
            width: `${createPresetNamingPos.width}px`,
          }}
          onSubmit={(e) => { void handleSaveCreatePreset(e); }}
        >
          {createPresetConflictId ? (
            <div className="preset-conflict-alert">
              <span>已存在同名预设「{createPresetNamingValue.trim()}」，是否覆盖？</span>
              <div className="preset-conflict-actions">
                <button type="button" disabled={saving} onClick={() => {
                  setCreatePresetNamingOpen(false);
                  setCreatePresetConflictId(null);
                }}>取消</button>
                <button type="button" disabled={saving} onClick={() => {
                  setCreatePresetConflictId(null);
                  requestAnimationFrame(() => {
                    const input = createPresetNamingInputRef.current;
                    if (input) {
                      input.focus();
                      input.select();
                    }
                  });
                }}>重命名</button>
                <button type="button" disabled={saving} className="btn-danger" onClick={() => void handleSaveCreatePreset(undefined, createPresetConflictId)}>确定</button>
              </div>
            </div>
          ) : (
            <>
              <input
                ref={createPresetNamingInputRef}
                value={createPresetNamingValue}
                onChange={(e) => setCreatePresetNamingValue(e.target.value)}
                placeholder="输入预设名称"
              />
              <button type="submit" disabled={saving}>确定</button>
            </>
          )}
        </form>
      ) : null}

      {/* ═══ 工作区 ═══ */}
      <div className="record-workspace">
        {/* ── 左：新建/选择声骸 ── */}
        <div className="record-col-create">
          {/* 新建声骸 - 折叠式卡片 */}
          <div className="card record-card">
            <button
              type="button"
              className={`record-section-toggle ${createExpanded ? "is-open" : ""}`}
              onClick={() => setCreateExpanded((v) => !v)}
            >
              <span className="record-section-title">新建声骸</span>
              <span className="record-toggle-icon">{createExpanded ? "▾" : "▸"}</span>
            </button>

            {createExpanded ? (
              <form className="record-create-form" onSubmit={handleCreateEcho}>
                {/* 基本信息 */}
                <div className="record-create-basics">
                  <label className="record-field">
                    <span className="record-field-label">昵称</span>
                    <input
                      value={createNickname}
                      onChange={(e) => setCreateNickname(e.target.value)}
                      placeholder="可选"
                    />
                  </label>
                  <label className="record-field">
                    <span className="record-field-label">主词条</span>
                    <select value={createMainStat} onChange={(e) => setCreateMainStat(e.target.value)}>
                      {statDefs.map((s) => (
                        <option key={s.statKey} value={s.statKey}>{s.displayName}</option>
                      ))}
                    </select>
                  </label>
                  <label className="record-field record-field-short">
                    <span className="record-field-label">Cost</span>
                    <select value={createCost} onChange={(e) => setCreateCost(Number(e.target.value))}>
                      <option value={1}>1</option>
                      <option value={3}>3</option>
                      <option value={4}>4</option>
                    </select>
                  </label>
                  <label className="record-field record-field-short">
                    <span className="record-field-label">状态</span>
                    <select value={createStatus} onChange={(e) => setCreateStatus(e.target.value as EchoStatus)}>
                      {(Object.keys(STATUS_LABELS) as EchoStatus[]).map((s) => (
                        <option key={s} value={s}>{STATUS_LABELS[s]}</option>
                      ))}
                    </select>
                  </label>
                </div>

                {/* 期望词条链 */}
                <div className="record-chain-section">
                  <div className="record-chain-header">
                    <span className="record-field-label">期望词条</span>
                    <div className="record-preset-inline">
                      <button
                        type="button"
                        ref={createPresetBtnRef}
                        className={createPresetSelectorOpen ? "manage-btn-active" : ""}
                        onClick={() => {
                          setCreatePresetSelectorOpen((v) => !v);
                          setCreatePresetNamingOpen(false);
                        }}
                      >
                        预设：{createPresetName ?? "未选择"}
                      </button>
                    </div>
                  </div>
                  {renderExpChainEditor(
                    createExpStats, createExpOps, createActiveExpIdx,
                    setCreateExpStats, setCreateExpOps, setCreateActiveExpIdx,
                    addCreateExp, removeCreateExpAt,
                  )}
                  {/* Preset selector popup */}
                  {createPresetSelectorOpen ? (
                    <div ref={createPresetMenuRef} className="record-preset-menu">
                      {expectationPresets.map((p) => (
                        <button
                          key={p.presetId} type="button"
                          className={createPresetId === p.presetId ? "preset-option-active" : ""}
                          onClick={() => applyPresetToCreate(p)}
                        >
                          <span className="record-preset-menu-name">{p.name}</span>
                          <span className="record-preset-menu-detail">
                            {[...p.items]
                              .sort((a, b) => a.rank - b.rank)
                              .map((x) => statMap.get(x.statKey)?.displayName ?? x.statKey)
                              .join(" / ")}
                          </span>
                        </button>
                      ))}
                      {expectationPresets.length === 0 ? (
                        <span className="chain-empty" style={{ padding: "6px 8px" }}>暂无预设</span>
                      ) : null}
                      <span className="preset-option-divider" />
                      <button
                        type="button"
                        className="preset-create-option"
                        onClick={openSaveCreatePreset}
                        disabled={saving}
                      >
                        设为预设
                      </button>
                    </div>
                  ) : null}

                </div>

                {/* 初始词条位 */}
                <div className="record-chain-section">
                  <span className="record-field-label">初始词条（可选，用于补录已有词条）</span>
                  {renderSlotEditor(
                    createSlots, createActiveSlotIdx,
                    setCreateSlots, setCreateActiveSlotIdx,
                    addCreateSlot, removeCreateSlotAt,
                  )}
                </div>

                {/* 提交 */}
                <div className="record-create-actions">
                  <button type="submit" className="btn-primary" disabled={saving}>
                    创建声骸
                  </button>
                  <button type="button" onClick={resetCreateForm} disabled={saving}>
                    重置
                  </button>
                </div>
              </form>
            ) : null}
          </div>

          {/* 统一面板的强化录入被移动至此 */}
          <form className="card record-card record-event-form" onSubmit={handleRecordEvent}>
            <span className="record-section-title">强化录入</span>

            {/* 录入行：左侧预览 + 右侧选择和按钮 */}
            <div className="record-event-inline-row">
              <span className="record-preview-value">
                {selectedStat ? (
                  <>
                    S{nextSlotNo} {selectedStat.displayName} 档{tierIndex} = {formatScaledValue(selectedStat.unit, selectedTierValue)}
                  </>
                ) : "—"}
              </span>

              <div className="record-event-controls">
                <label className="record-field record-field-compact">
                  <span className="record-field-label">时间</span>
                  <input
                    type="datetime-local"
                    value={eventTimeLocal}
                    onChange={(e) => setEventTimeLocal(e.target.value)}
                  />
                </label>

                <label className="record-field record-field-compact">
                  <span className="record-field-label">词条</span>
                  <select value={selectedStat?.statKey ?? ""} onChange={(e) => setStatKey(e.target.value)}>
                    {availableStatDefs.map((s) => (
                      <option key={s.statKey} value={s.statKey}>{s.displayName}</option>
                    ))}
                  </select>
                </label>

                <label className="record-field record-field-compact">
                  <span className="record-field-label">档位</span>
                  <select value={tierIndex} onChange={(e) => setTierIndex(Number(e.target.value))}>
                    {(selectedStat?.tiers ?? []).map((t) => (
                      <option key={t.tierIndex} value={t.tierIndex}>
                        档{t.tierIndex} ({formatScaledValue(selectedStat?.unit ?? "flat", t.valueScaled)})
                      </option>
                    ))}
                  </select>
                </label>

                <button
                  type="submit" className="btn-primary"
                  disabled={saving || !selectedEchoId || availableSlots.length === 0 || availableStatDefs.length === 0}
                >
                  录入
                </button>
                <button
                  type="button"
                  className={`record-undo-btn ${undoConfirmId && eventHistory.length > 0 ? "btn-danger" : ""}`}
                  disabled={saving || eventHistory.length === 0}
                  onClick={() => eventHistory.length > 0 && handleUndoEvent(eventHistory[0].eventId)}
                  title="撤销最近一次录入"
                >
                  {undoConfirmId && eventHistory.length > 0 && undoConfirmId === eventHistory[0].eventId ? "确认撤销" : "撤销"}
                </button>
              </div>
            </div>

            {/* 声骸信息 — 选择器嵌入名称位置 */}
            <div
              className="record-echo-info"
              style={{
                padding: "8px 10px",
                background: "var(--bg-app, #f8fafc)",
                borderRadius: "6px",
              }}
            >
              <div className="record-echo-info-header" style={{ marginBottom: selectedEcho ? "4px" : "0" }}>
                <select
                  className="record-echo-name-select"
                  value={selectedEchoId}
                  onChange={(e) => setSelectedEchoId(e.target.value)}
                >
                  <option value="">请选择声骸</option>
                  {echoes
                    .filter((e) => e.status === "tracking" || e.status === "paused")
                    .map((echo) => (
                      <option key={echo.echoId} value={echo.echoId}>
                        {(echo.nickname ?? echo.echoId.slice(0, 8)) + ` · ${echo.openedSlotsCount}/5`}
                      </option>
                    ))}
                  {echoes.filter((e) => e.status !== "tracking" && e.status !== "paused").length > 0 ? (
                    <optgroup label="── 其他状态 ──">
                      {echoes
                        .filter((e) => e.status !== "tracking" && e.status !== "paused")
                        .map((echo) => (
                          <option key={echo.echoId} value={echo.echoId}>
                            {(echo.nickname ?? echo.echoId.slice(0, 8)) + ` · ${echo.openedSlotsCount}/5 [${STATUS_LABELS[echo.status]}]`}
                          </option>
                        ))}
                    </optgroup>
                  ) : null}
                </select>
                {selectedEcho ? (
                  <>
                    <span className={`record-badge ${STATUS_BADGE_CLASS[selectedEcho.status]}`}>
                      {STATUS_LABELS[selectedEcho.status]}
                    </span>
                    <span className="record-badge badge-neutral">
                      Cost {selectedEcho.costClass}
                    </span>
                    <span className="record-badge badge-neutral">
                      {statMap.get(selectedEcho.mainStatKey)?.displayName ?? selectedEcho.mainStatKey}
                    </span>
                  </>
                ) : null}
              </div>
              {selectedEcho ? (
                <>
                  <div className="record-echo-slots" style={{ marginBottom: selectedEcho.expectations.length > 0 ? "4px" : "0" }}>
                    <span className="record-echo-slots-label">
                      词条 {selectedEcho.openedSlotsCount}/5
                    </span>
                    {selectedEcho.currentSubstats.length === 0 && (!selectedStat || availableSlots.length === 0) ? (
                      <span className="chain-empty">暂无词条</span>
                    ) : (
                      <>
                        {[...selectedEcho.currentSubstats]
                          .sort((a, b) => a.slotNo - b.slotNo)
                          .map((slot) => {
                            const st = statMap.get(slot.statKey);
                            return (
                              <span
                                key={slot.slotNo}
                                className={`slot-pill ${slot.source === "ordered_event" ? "slot-pill-locked" : ""}`}
                                title={`${st?.displayName ?? slot.statKey} 档${slot.tierIndex}：${formatScaledValue(st?.unit ?? "flat", slot.valueScaled)}`}
                              >
                                {slot.slotNo}: {statKeyToAbbr(slot.statKey)}{slot.tierIndex}={formatScaledValue(st?.unit ?? "flat", slot.valueScaled)}
                              </span>
                            );
                          })}
                        {selectedStat && availableSlots.length > 0 ? (
                          <span
                            className="slot-pill"
                            style={{
                              borderColor: "#10b981",
                              color: "#059669",
                              backgroundColor: "#ecfdf5",
                              borderStyle: "dashed"
                            }}
                            title={`预览：${selectedStat.displayName ?? selectedStat.statKey} 档${tierIndex}：${formatScaledValue(selectedStat.unit ?? "flat", selectedTierValue)}`}
                          >
                            {nextSlotNo}: {statKeyToAbbr(selectedStat.statKey)}{tierIndex}={formatScaledValue(selectedStat.unit ?? "flat", selectedTierValue)}
                          </span>
                        ) : null}
                      </>
                    )}
                  </div>
                  {selectedEcho.expectations.length > 0 ? (
                    <div className="record-echo-exp">
                      <span className="chain-label">期望：</span>
                      {[...selectedEcho.expectations]
                        .sort((a, b) => a.rank - b.rank || a.statKey.localeCompare(b.statKey))
                        .map((exp, idx, arr) => (
                          <Fragment key={idx}>
                            {idx > 0 ? (
                              <span className="record-exp-op">
                                {arr[idx].rank === arr[idx - 1].rank ? "=" : ">"}
                              </span>
                            ) : null}
                            <span className="record-exp-tag">
                              {statMap.get(exp.statKey)?.displayName ?? exp.statKey}
                            </span>
                          </Fragment>
                        ))}
                    </div>
                  ) : null}
                </>
              ) : null}
            </div>
          </form>
        </div>

        {/* ── 右：最近录入 ── */}
        <div className="record-col-event">

        {/* ── 右：强化录入 ── */}


          {/* 最近事件 */}
          <div className="card record-card record-history-card">
            <div className="record-card-header" style={{ marginBottom: 12 }}>
              <span className="record-section-title" style={{ whiteSpace: "nowrap" }}>最近录入</span>

              {/* 查找工具栏 - 嵌入标题行 */}
              <div className="record-history-search-bar">
                <svg
                  width="14" height="14" viewBox="0 0 24 24" fill="none"
                  stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"
                  style={{ opacity: 0.5, flexShrink: 0 }}
                >
                  <circle cx="11" cy="11" r="8" />
                  <path d="m21 21-4.3-4.3" />
                </svg>
                <div className="chain-row" style={{ padding: 0, minHeight: 24 }}>
                  {searchStats.map((sk, idx) => {
                    const isActive = searchActiveIdx === idx;
                    return (
                      <div
                        key={idx}
                        className={`chain-item ${isActive ? "active" : ""}`}
                        style={{ padding: isActive ? "0 4px" : "0", border: isActive ? undefined : "none" }}
                        onContextMenu={(e) => { e.preventDefault(); setSearchStats(prev => prev.filter((_, i) => i !== idx)); }}
                        onClick={(e) => {
                          e.stopPropagation();
                          setSearchActiveIdx(idx);
                        }}
                        title={isActive ? "选择词条" : `${statMap.get(sk)?.displayName ?? sk} - 点击修改，右键删除`}
                      >
                        {isActive ? (
                          <select
                            autoFocus
                            value={sk}
                            onChange={(e) => {
                              const nextSk = e.target.value;
                              setSearchStats(prev => prev.map((item, i) => i === idx ? nextSk : item));
                              setSearchActiveIdx(null);
                            }}
                            onBlur={() => setSearchActiveIdx(null)}
                            onPointerDown={(e) => e.stopPropagation()}
                          >
                            {statDefs.map(s => (
                              <option key={s.statKey} value={s.statKey}>{s.displayName}</option>
                            ))}
                          </select>
                        ) : (
                          <span className="record-history-abbr-group">
                            <span className={`record-history-abbr-part ${getStatColorClass(sk)}`} style={{ borderRadius: 4 }}>
                              {statKeyToAbbr(sk)}
                            </span>
                          </span>
                        )}
                      </div>
                    );
                  })}
                  <button
                    type="button" className="chain-add"
                    style={{ height: 20, width: 20, minWidth: 20 }}
                    onClick={(e) => {
                      e.stopPropagation();
                      const firstAvail = statDefs[0].statKey;
                      const nextIdx = searchStats.length;
                      setSearchStats(prev => [...prev, firstAvail]);
                      setSearchActiveIdx(nextIdx);
                    }}
                  >+</button>
                </div>
                {searchStats.length > 0 && (
                  <div className="inline-row" style={{ gap: 4 }}>
                    <span className="record-history-match-info">
                      {matchIndices.length > 0 ? `${currentMatchIdx + 1}/${matchIndices.length}` : "0"}
                    </span>
                    <div className="inline-row" style={{ gap: 2 }}>
                      <button type="button" className="record-history-jump-btn" onClick={() => jumpToMatch("prev")}>↑</button>
                      <span className="record-history-dist-num">
                        {currentMatchIdx > 0 ? matchIndices[currentMatchIdx] - matchIndices[currentMatchIdx - 1] : "-"}
                      </span>
                    </div>
                    <div className="inline-row" style={{ gap: 2 }}>
                      <button type="button" className="record-history-jump-btn" onClick={() => jumpToMatch("next")}>↓</button>
                      <span className="record-history-dist-num">
                        {currentMatchIdx < matchIndices.length - 1 ? matchIndices[currentMatchIdx + 1] - matchIndices[currentMatchIdx] : "-"}
                      </span>
                    </div>
                    <button type="button" className="record-history-jump-btn" onClick={() => setSearchStats([])}>✕</button>
                  </div>
                )}
              </div>

              <div className="inline-row" style={{ gap: 8, fontSize: 13, fontWeight: "normal", flexShrink: 0 }}>
                <label className="inline-row" style={{ gap: 4 }}>
                  <input
                    type="checkbox"
                    checked={historyTodayOnly}
                    onChange={(e) => setHistoryTodayOnly(e.target.checked)}
                  />
                  仅今日
                </label>
                <label className="inline-row" style={{ gap: 2 }}>
                  限:
                  <input
                    type="number"
                    style={{ width: 55 }}
                    min={1}
                    value={historyLimitStr}
                    onChange={(e) => setHistoryLimitStr(e.target.value)}
                  />
                </label>
              </div>
            </div>

            <div className="record-history-list" ref={historyListRef}>
              {displayedHistory.map((row, idx) => {
                const st = statDefs.find((s) => s.statKey === row.statKey);
                // Compute preexisting stats
                const echo = echoes.find(e => e.echoId === row.echoId);
                const preStats = echo ? echo.currentSubstats.filter(s => s.slotNo < row.slotNo).sort((a, b) => a.slotNo - b.slotNo) : [];

                // Search Highlight logic
                let isPartOfMatch = false;
                let isMatchCurrent = false;

                if (matchIndices.length > 0) {
                  const activeMatchStart = matchIndices[currentMatchIdx];
                  // If current row is within the sequence starting at any match index
                  for (let mStart of matchIndices) {
                    if (idx >= mStart && idx < mStart + searchStats.length) {
                      isPartOfMatch = true;
                      if (mStart === activeMatchStart) isMatchCurrent = true;
                      break;
                    }
                  }
                }

                return (
                  <div
                    key={row.eventId}
                    className={`record-history-item ${row.echoId === selectedEchoId ? "record-history-active" : ""} ${isPartOfMatch ? "is-match" : ""} ${isMatchCurrent ? "is-match-current" : ""}`}
                    onClick={() => setSelectedEchoId(row.echoId)}
                    title={`点击切换到此声骸 · ${row.eventId}`}
                  >
                    <span className="record-history-abbr-group">
                      <span className={`record-history-abbr-part ${getStatColorClass(row.statKey)}`}>
                        {statKeyToAbbr(row.statKey)}
                      </span>
                      <span className="record-history-abbr-part abbr-tier">
                        {row.tierIndex}
                      </span>
                    </span>
                    <span className="record-history-echo">
                      {row.echoNickname ?? row.echoId.slice(0, 8)}
                    </span>
                    <span className="record-history-detail">
                      S{row.slotNo} · {st?.displayName ?? row.statKey}={formatScaledValue(st?.unit ?? "", row.valueScaled)}
                    </span>
                    <div className="record-history-prestats">
                      {preStats.map((pre) => {
                        const pst = statMap.get(pre.statKey);
                        return (
                          <span key={pre.slotNo} className="record-history-abbr-group" title={`${pst?.displayName ?? pre.statKey} 档${pre.tierIndex}：${formatScaledValue(pst?.unit ?? "flat", pre.valueScaled)}`}>
                            <span className={`record-history-abbr-part ${getStatColorClass(pre.statKey)}`}>
                              {statKeyToAbbr(pre.statKey)}
                            </span>
                            <span className="record-history-abbr-part abbr-tier">
                              {pre.tierIndex}
                            </span>
                          </span>
                        );
                      })}
                    </div>
                    <span className="record-history-time">
                      {new Date(row.eventTime).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })}
                    </span>
                  </div>
                );
              })}
              {eventHistory.length === 0 ? (
                <span className="chain-empty" style={{ padding: "12px" }}>暂无录入记录</span>
              ) : null}
            </div>
          </div>
        </div>
      </div>

      {/* ═══ 实时分析区 ═══ */}
      <div className="record-analysis">
        {/* 筛选条件 - 紧凑行 */}
        <div className="card record-card record-filter-bar">
          <span className="record-section-title">实时分析</span>
          <div className="record-filter-fields">
            <label className="record-filter-field">
              开始
              <input
                type="datetime-local"
                value={distributionFilter.startTime ? toLocalInputValue(new Date(distributionFilter.startTime)) : ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev, startTime: e.target.value ? new Date(e.target.value).toISOString() : undefined,
                  }))
                }
              />
            </label>
            <label className="record-filter-field">
              结束
              <input
                type="datetime-local"
                value={distributionFilter.endTime ? toLocalInputValue(new Date(distributionFilter.endTime)) : ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({
                    ...prev, endTime: e.target.value ? new Date(e.target.value).toISOString() : undefined,
                  }))
                }
              />
            </label>
            <label className="record-filter-field">
              主词条
              <select
                value={distributionFilter.mainStatKey ?? ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({ ...prev, mainStatKey: e.target.value || undefined }))
                }
              >
                <option value="">全部</option>
                {statDefs.map((s) => (
                  <option key={s.statKey} value={s.statKey}>{s.displayName}</option>
                ))}
              </select>
            </label>
            <label className="record-filter-field">
              Cost
              <select
                value={distributionFilter.costClass ?? ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({ ...prev, costClass: e.target.value ? Number(e.target.value) : undefined }))
                }
              >
                <option value="">全部</option>
                <option value="1">1</option>
                <option value="3">3</option>
                <option value="4">4</option>
              </select>
            </label>
            <label className="record-filter-field">
              状态
              <select
                value={distributionFilter.status ?? ""}
                onChange={(e) =>
                  setDistributionFilter((prev) => ({ ...prev, status: e.target.value || undefined }))
                }
              >
                <option value="">全部</option>
                {(Object.keys(STATUS_LABELS) as EchoStatus[]).map((s) => (
                  <option key={s} value={s}>{STATUS_LABELS[s]}</option>
                ))}
              </select>
            </label>
          </div>
        </div>

        {/* 图表 + 分布表 + 命中列表 */}
        <div className="record-analysis-grid">
          {/* 概率图 */}
          <div className="card record-card">
            <div className="record-card-header">
              <span className="record-section-title">全局概率图</span>
              <span className="record-card-meta">
                总事件 {distribution?.totalEvents ?? 0}
                {loadingDist ? " · 更新中..." : ""}
              </span>
            </div>
            <BarChart labels={distributionChartData.labels} values={distributionChartData.values} />
          </div>

          {/* 分布详情 */}
          <div className="card record-card">
            <span className="record-section-title">词条分布（点击联动）</span>
            <div className="record-dist-table-wrap">
              <table className="table compact-table">
                <thead>
                  <tr>
                    <th>词条</th>
                    <th>次数</th>
                    <th>P(gbl)</th>
                    <th>Wilson CI</th>
                    <th>Bayes</th>
                  </tr>
                </thead>
                <tbody>
                  {(distribution?.rows ?? []).map((row) => (
                    <tr
                      key={row.statKey}
                      className={row.statKey === selectedDistStatKey ? "active-row" : ""}
                      onClick={() => handleDistRowClick(row.statKey)}
                      style={{ cursor: "pointer" }}
                    >
                      <td>{row.displayName}</td>
                      <td>{row.count}</td>
                      <td>{toPercent(row.pGlobal)}</td>
                      <td className="record-ci-cell">
                        {toPercent(row.ciFreqLow)} ~ {toPercent(row.ciFreqHigh)}
                      </td>
                      <td className="record-ci-cell">
                        {toPercent(row.bayesMean)} ({toPercent(row.bayesLow)}~{toPercent(row.bayesHigh)})
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>

          {/* 命中声骸 */}
          <div className="card record-card">
            <div className="record-card-header">
              <span className="record-section-title">
                命中列表 {selectedDistStatKey ? `· ${statMap.get(selectedDistStatKey)?.displayName ?? selectedDistStatKey}` : ""}
              </span>
              <label className="record-sort-label">
                排序
                <select value={sortBy} onChange={(e) => handleSortByChange(e.target.value)}>
                  <option value="pFinal">P(final)</option>
                  <option value="pNext">P(next)</option>
                  <option value="rank">期望权重</option>
                  <option value="slots">槽位</option>
                </select>
              </label>
            </div>
            <div className="record-dist-table-wrap">
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
                    <tr key={row.echoId} style={{ cursor: "pointer" }} onClick={() => setSelectedEchoId(row.echoId)}>
                      <td>{row.nickname ?? row.echoId.slice(0, 8)}</td>
                      <td>{row.openedSlotsCount}/5</td>
                      <td>{row.expectationRankMin}</td>
                      <td>{toPercent(row.pNext)}</td>
                      <td>{toPercent(row.pFinal)}</td>
                    </tr>
                  ))}
                  {echoProbRows.length === 0 && !loadingProb ? (
                    <tr><td colSpan={5} className="chain-empty">
                      {selectedDistStatKey ? "无匹配声骸" : "请点击左侧词条行"}
                    </td></tr>
                  ) : null}
                </tbody>
              </table>
              {loadingProb ? <p className="hint" style={{ textAlign: "center", padding: "8px" }}>加载中...</p> : null}
            </div>
          </div>
        </div>

        <div className="card record-card" style={{ marginTop: 12 }}>
          <div className="record-card-header">
            <span className="record-section-title">今日模式决策 (MVP)</span>
            <span className="record-card-meta">
              {patternDecision?.gameDay ? `${patternDecision.gameDay} · 事件 ${patternDecision.totalEvents}` : "暂无"}
              {loadingPatternDecision ? " · 更新中..." : ""}
            </span>
          </div>
          <div className="inline-row" style={{ flexWrap: "wrap", gap: 8, marginBottom: 8 }}>
            <label style={{ fontSize: 12 }}>
              起点
              <input
                type="number"
                min={0}
                value={patternManualStartStr}
                onChange={(e) => setPatternManualStartStr(e.target.value)}
                style={{ width: 70, marginLeft: 4 }}
              />
            </label>
            <label style={{ fontSize: 12 }}>
              假设手数
              <input
                type="number"
                min={2}
                max={20}
                value={patternManualCycleStr}
                onChange={(e) => setPatternManualCycleStr(e.target.value)}
                style={{ width: 70, marginLeft: 4 }}
              />
            </label>
            <label style={{ fontSize: 12 }}>
              自动最短模式
              <input
                type="number"
                min={2}
                max={12}
                value={patternAutoMinLenStr}
                onChange={(e) => setPatternAutoMinLenStr(e.target.value)}
                style={{ width: 70, marginLeft: 4 }}
              />
            </label>
            <label style={{ fontSize: 12, minWidth: 280 }}>
              猜测形态
              <input
                value={patternManualGuessStr}
                onChange={(e) => setPatternManualGuessStr(e.target.value)}
                placeholder="例: AABCB, ABA"
                style={{ width: 240, marginLeft: 4 }}
              />
            </label>
            <button type="button" onClick={() => setPatternConfigVersion((v) => v + 1)} disabled={loadingPatternDecision}>
              应用手动分析
            </button>
          </div>
          {patternDecision ? (
            <>
              <p className="hint" style={{ marginBottom: 8 }}>
                置信度 {toPercent(patternDecision.modelConfidence)} · 模式长度 {patternDecision.minLen}-{patternDecision.maxLen} · 最小支持 {patternDecision.minSupport}
              </p>
              <div className="record-dist-table-wrap">
                <table className="table compact-table">
                  <thead>
                    <tr>
                      <th>建议词条</th>
                      <th>P(mix)</th>
                      <th>P(base)</th>
                      <th>P(markov)</th>
                      <th>P(cycle)</th>
                      <th>Boost</th>
                      <th>触发模式</th>
                    </tr>
                  </thead>
                  <tbody>
                    {patternDecision.suggestions.map((s) => (
                      <tr key={s.statKey}>
                        <td>{s.displayName}</td>
                        <td>{toPercent(s.probability)}</td>
                        <td>{toPercent(s.baseProbability)}</td>
                        <td>{toPercent(s.markovProbability)}</td>
                        <td>{toPercent(s.cycleProbability)}</td>
                        <td>{s.motifBoost.toFixed(2)}</td>
                        <td style={{ fontSize: 11, textAlign: "left" }}>
                          {s.matchedPatterns.length > 0 ? s.matchedPatterns.join(" | ") : "—"}
                        </td>
                      </tr>
                    ))}
                    {patternDecision.suggestions.length === 0 ? (
                      <tr><td colSpan={7} className="chain-empty">暂无可用建议</td></tr>
                    ) : null}
                  </tbody>
                </table>
              </div>

              {patternDecision.manualSummary ? (
                <div style={{ marginTop: 10 }}>
                  <strong style={{ fontSize: 13 }}>
                    手动验证 · start={patternDecision.manualSummary.startIndex}, L={patternDecision.manualSummary.cycleLen}, full={patternDecision.manualSummary.fullCycles}, nextPos={patternDecision.manualSummary.nextCyclePos}
                  </strong>
                  <div className="inline-row" style={{ flexWrap: "wrap", gap: 6, marginTop: 4 }}>
                    {patternDecision.manualSummary.topCycleShapes.map(([shape, cnt]) => (
                      <span
                        key={`${shape}-${cnt}`}
                        style={{
                          fontSize: 11,
                          padding: "2px 8px",
                          borderRadius: 4,
                          border: "1px solid var(--line)",
                          background: "var(--panel)",
                        }}
                      >
                        {shape}: {cnt}
                      </span>
                    ))}
                    {patternDecision.manualSummary.topCycleShapes.length === 0 ? (
                      <span className="chain-empty">暂无完整周期形态</span>
                    ) : null}
                  </div>
                  <div className="record-dist-table-wrap" style={{ marginTop: 6 }}>
                    <table className="table compact-table">
                      <thead>
                        <tr>
                          <th>猜测</th>
                          <th>hit/opp</th>
                          <th>HitRate</th>
                          <th>Baseline</th>
                          <th>Lift</th>
                          <th>下一个提示</th>
                        </tr>
                      </thead>
                      <tbody>
                        {patternDecision.manualSummary.guesses.map((g) => (
                          <tr key={g.guessShape}>
                            <td>{g.guessShape}</td>
                            <td>{g.support}/{g.opportunities}</td>
                            <td>{toPercent(g.hitRate)}</td>
                            <td>{toPercent(g.baselineRate)}</td>
                            <td>{g.lift > 0 ? g.lift.toFixed(2) : "—"}</td>
                            <td>{g.nextStatHint ?? "—"}</td>
                          </tr>
                        ))}
                        {patternDecision.manualSummary.guesses.length === 0 ? (
                          <tr><td colSpan={6} className="chain-empty">未配置有效猜测形态</td></tr>
                        ) : null}
                      </tbody>
                    </table>
                  </div>
                  {patternDecision.manualSummary.positionSuggestions.length > 0 ? (
                    <div className="inline-row" style={{ flexWrap: "wrap", gap: 6, marginTop: 6 }}>
                      {patternDecision.manualSummary.positionSuggestions.map((p) => (
                        <span
                          key={p.statKey}
                          style={{
                            fontSize: 11,
                            padding: "2px 8px",
                            borderRadius: 4,
                            border: "1px dashed var(--line)",
                            background: "var(--panel)",
                          }}
                        >
                          next@pos: {p.displayName} {toPercent(p.probability)} (n={p.count})
                        </span>
                      ))}
                    </div>
                  ) : null}
                </div>
              ) : null}

              <div style={{ marginTop: 8 }}>
                <strong style={{ fontSize: 13 }}>高分模式</strong>
                <div className="inline-row" style={{ flexWrap: "wrap", gap: 6, marginTop: 4 }}>
                  {patternDecision.exactPatterns.slice(0, 8).map((p, idx) => (
                    <span
                      key={`${p.shape}-${idx}`}
                      style={{
                        fontSize: 11,
                        padding: "2px 8px",
                        borderRadius: 4,
                        background: "var(--panel)",
                        border: "1px solid var(--line)",
                      }}
                      title={`期望 ${p.expectedCount.toFixed(2)} · score ${p.score.toFixed(2)}`}
                    >
                      L{p.length} {p.shape} · n={p.support} · lift={p.lift.toFixed(2)}
                    </span>
                  ))}
                  {patternDecision.exactPatterns.length === 0 ? (
                    <span className="chain-empty">暂无满足支持度的模式</span>
                  ) : null}
                </div>
              </div>

              <div style={{ marginTop: 8 }}>
                <strong style={{ fontSize: 13 }}>形态模式（组合视角）</strong>
                <div className="inline-row" style={{ flexWrap: "wrap", gap: 6, marginTop: 4 }}>
                  {patternDecision.shapePatterns.slice(0, 8).map((p, idx) => (
                    <span
                      key={`${p.shape}-${p.length}-${idx}`}
                      style={{
                        fontSize: 11,
                        padding: "2px 8px",
                        borderRadius: 4,
                        background: "var(--panel)",
                        border: "1px dashed var(--line)",
                      }}
                      title={p.examplePatterns.join(" | ")}
                    >
                      L{p.length} {p.shape} · n={p.support} · lift={p.lift.toFixed(2)}
                    </span>
                  ))}
                  {patternDecision.shapePatterns.length === 0 ? (
                    <span className="chain-empty">暂无稳定形态组合</span>
                  ) : null}
                </div>
              </div>

              {patternDecision.notes.length > 0 ? (
                <p className="hint" style={{ marginTop: 8 }}>
                  {patternDecision.notes.join(" ｜ ")}
                </p>
              ) : null}
            </>
          ) : (
            <p className="hint">暂无数据</p>
          )}
        </div>
        <HypothesisVerification />
      </div>
    </section>
  );
}
