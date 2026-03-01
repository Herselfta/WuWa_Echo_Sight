import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import {
  deleteEcho,
  deleteExpectationPreset,
  saveExpectationPreset,
  setExpectations,
  updateEcho,
  upsertBackfillState,
} from "../api/tauri";
import {
  beginLongPressDrag,
  cancelLongPressDragCandidate,
  CHAIN_LONG_PRESS_MS,
  clearNativeTextSelection,
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
import type { EchoStatus, ExpectationItem, ExpectationPreset } from "../types/domain";

type RelOp = "gt" | "eq";
type ToastKind = "info" | "success" | "error";
type DragKind = "expectation" | "slot" | "preset";

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

type DragState = ChainDragState<DragKind>;

type PresetDraftSource = Pick<ExpectationPreset, "presetId" | "name" | "items">;
type PresetCreateSource = "selector" | "manager";
type SubstatFilterMode = "or" | "and" | "not";
type EchoSortBy =
  | "created_desc"
  | "created_asc"
  | "updated_desc"
  | "updated_asc"
  | "opened_desc"
  | "opened_asc"
  | "cost_desc"
  | "cost_asc"
  | "main_asc"
  | "main_desc"
  | "status_asc"
  | "status_desc"
  | "nickname_asc"
  | "nickname_desc";

interface EchoFilters {
  costClass: "all" | "1" | "3" | "4";
  mainStatKey: string;
  status: "all" | EchoStatus;
  openedSlots: "all" | "0" | "1" | "2" | "3" | "4" | "5";
  presetId: string;
  searchText: string;
  substatMode: SubstatFilterMode;
  substatStatKeys: string[];
  substatTiers: Record<string, number[]>;
  sortBy: EchoSortBy;
}

const ECHO_FILTER_STORAGE_KEY = "wuwa.echo-pool.filters.v2";
const DEFAULT_ECHO_FILTERS: EchoFilters = {
  costClass: "all",
  mainStatKey: "all",
  status: "all",
  openedSlots: "all",
  presetId: "all",
  searchText: "",
  substatMode: "or",
  substatStatKeys: [],
  substatTiers: {},
  sortBy: "created_desc",
};
const ECHO_STATUS_OPTIONS: EchoStatus[] = ["tracking", "paused", "abandoned", "completed"];
const SUBSTAT_FILTER_MODE_OPTIONS: Array<{ value: SubstatFilterMode; label: string }> = [
  { value: "or", label: "或（任一匹配）" },
  { value: "and", label: "与（全部匹配）" },
  { value: "not", label: "非（全部排除）" },
];
const ECHO_SORT_OPTIONS: Array<{ value: EchoSortBy; label: string }> = [
  { value: "created_desc", label: "创建时间（新到旧）" },
  { value: "created_asc", label: "创建时间（旧到新）" },
  { value: "updated_desc", label: "更新时间（新到旧）" },
  { value: "updated_asc", label: "更新时间（旧到新）" },
  { value: "opened_desc", label: "已开槽位（多到少）" },
  { value: "opened_asc", label: "已开槽位（少到多）" },
  { value: "cost_desc", label: "Cost（高到低）" },
  { value: "cost_asc", label: "Cost（低到高）" },
  { value: "main_asc", label: "主词条（A-Z）" },
  { value: "main_desc", label: "主词条（Z-A）" },
  { value: "status_asc", label: "状态（A-Z）" },
  { value: "status_desc", label: "状态（Z-A）" },
  { value: "nickname_asc", label: "名称（A-Z）" },
  { value: "nickname_desc", label: "名称（Z-A）" },
];

function sanitizeEchoSortBy(input: unknown): EchoSortBy {
  return ECHO_SORT_OPTIONS.some((option) => option.value === input) ? (input as EchoSortBy) : "created_desc";
}

function sanitizeSubstatFilterMode(input: unknown): SubstatFilterMode {
  return input === "or" || input === "and" || input === "not" ? input : "or";
}

function isDefaultEchoFilters(filters: EchoFilters): boolean {
  return (
    filters.costClass === DEFAULT_ECHO_FILTERS.costClass &&
    filters.mainStatKey === DEFAULT_ECHO_FILTERS.mainStatKey &&
    filters.status === DEFAULT_ECHO_FILTERS.status &&
    filters.openedSlots === DEFAULT_ECHO_FILTERS.openedSlots &&
    filters.presetId === DEFAULT_ECHO_FILTERS.presetId &&
    filters.searchText.trim() === DEFAULT_ECHO_FILTERS.searchText &&
    filters.substatMode === DEFAULT_ECHO_FILTERS.substatMode &&
    filters.substatStatKeys.length === 0 &&
    Object.keys(filters.substatTiers).length === 0 &&
    filters.sortBy === DEFAULT_ECHO_FILTERS.sortBy
  );
}

function parseEchoFilters(raw: string | null): EchoFilters {
  if (!raw) {
    return { ...DEFAULT_ECHO_FILTERS };
  }
  try {
    const parsed = JSON.parse(raw) as Partial<EchoFilters>;
    const costClass =
      parsed.costClass === "1" || parsed.costClass === "3" || parsed.costClass === "4"
        ? parsed.costClass
        : "all";
    const mainStatKey = typeof parsed.mainStatKey === "string" && parsed.mainStatKey.trim()
      ? parsed.mainStatKey
      : "all";
    const status =
      typeof parsed.status === "string" && (parsed.status === "all" || ECHO_STATUS_OPTIONS.includes(parsed.status as EchoStatus))
        ? parsed.status
        : "all";
    const openedSlots =
      parsed.openedSlots === "0" ||
      parsed.openedSlots === "1" ||
      parsed.openedSlots === "2" ||
      parsed.openedSlots === "3" ||
      parsed.openedSlots === "4" ||
      parsed.openedSlots === "5"
        ? parsed.openedSlots
        : "all";
    const presetId = typeof parsed.presetId === "string" ? parsed.presetId : "all";
    const searchText = typeof parsed.searchText === "string" ? parsed.searchText : "";
    const substatMode = sanitizeSubstatFilterMode(parsed.substatMode);
    const substatStatKeys = Array.isArray(parsed.substatStatKeys)
      ? Array.from(
          new Set(
            parsed.substatStatKeys.filter(
              (value): value is string => typeof value === "string" && value.trim().length > 0,
            ),
          ),
        )
      : [];
    const substatTiers: Record<string, number[]> = {};
    if (typeof parsed.substatTiers === "object" && parsed.substatTiers !== null) {
      for (const [key, val] of Object.entries(parsed.substatTiers)) {
        if (Array.isArray(val)) {
          substatTiers[key] = val.filter((v): v is number => typeof v === "number");
        } else if (typeof val === "number") {
          substatTiers[key] = [val];
        }
      }
    }
    const sortBy = sanitizeEchoSortBy(parsed.sortBy);
    return {
      costClass,
      mainStatKey,
      status,
      openedSlots,
      presetId,
      searchText,
      substatMode,
      substatStatKeys,
      substatTiers,
      sortBy,
    };
  } catch {
    return { ...DEFAULT_ECHO_FILTERS };
  }
}

function loadEchoFilters(): EchoFilters {
  if (typeof window === "undefined") {
    return { ...DEFAULT_ECHO_FILTERS };
  }
  return parseEchoFilters(window.localStorage.getItem(ECHO_FILTER_STORAGE_KEY));
}

function buildDefaultPresetName(date = new Date()) {
  return `新预设 ${date.getFullYear()}${String(date.getMonth() + 1).padStart(2, "0")}${String(
    date.getDate(),
  ).padStart(2, "0")}-${String(date.getHours()).padStart(2, "0")}${String(date.getMinutes()).padStart(
    2,
    "0",
  )}${String(date.getSeconds()).padStart(2, "0")}`;
}

function areChainsEqual(aStats: string[], aOps: RelOp[], bStats: string[], bOps: RelOp[]) {
  if (aStats.length !== bStats.length || aOps.length !== bOps.length) {
    return false;
  }
  for (let i = 0; i < aStats.length; i += 1) {
    if (aStats[i] !== bStats[i]) {
      return false;
    }
  }
  for (let i = 0; i < aOps.length; i += 1) {
    if (aOps[i] !== bOps[i]) {
      return false;
    }
  }
  return true;
}

function findPresetByChain(
  presets: ExpectationPreset[],
  stats: string[],
  ops: RelOp[],
  excludePresetId?: string,
): ExpectationPreset | null {
  // canonicalize input chain for comparison
  const inputItems = chainToExpectationItems(stats, ops);
  const inputCanonical = buildExpectationChain(inputItems);
  return (
    presets.find((preset) => {
      if (excludePresetId && preset.presetId === excludePresetId) {
        return false;
      }
      const chain = buildExpectationChain(preset.items);
      return areChainsEqual(inputCanonical.stats, inputCanonical.ops, chain.stats, chain.ops);
    }) ?? null
  );
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

function formatScaledValue(unit: string, valueScaled: number) {
  return unit === "percent" ? `${(valueScaled / 10).toFixed(1)}%` : String(valueScaled);
}

export function EchoPoolPage() {
  const { statDefs, echoes, expectationPresets, refreshEchoes, refreshExpectationPresets, selectedEchoId, setSelectedEchoId } = useAppStore();
  const tableWrapRef = useRef<HTMLDivElement | null>(null);
  const presetSelectorRef = useRef<HTMLDivElement | null>(null);
  const presetSelectorButtonRef = useRef<HTMLButtonElement | null>(null);
  const presetManagerAddButtonRef = useRef<HTMLButtonElement | null>(null);
  const presetSelectorMenuRef = useRef<HTMLDivElement | null>(null);
  const presetNamingPopRef = useRef<HTMLFormElement | null>(null);
  const presetNamingInputRef = useRef<HTMLInputElement | null>(null);
  const substatSelectorRef = useRef<HTMLDivElement | null>(null);
  const selectAllCheckboxRef = useRef<HTMLInputElement | null>(null);
  const expectationRowRef = useRef<HTMLDivElement | null>(null);
  const slotRowRef = useRef<HTMLDivElement | null>(null);
  const presetRowRef = useRef<HTMLDivElement | null>(null);
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

  const [editingEchoId, setEditingEchoId] = useState<string | null>(null);
  const [basicDraft, setBasicDraft] = useState<BasicDraft | null>(null);
  const [expectationStats, setExpectationStats] = useState<string[]>([]);
  const [expectationOps, setExpectationOps] = useState<RelOp[]>([]);
  const [slotsDraft, setSlotsDraft] = useState<SlotDraft[]>([]);
  const [activeExpectationIndex, setActiveExpectationIndex] = useState<number | null>(null);
  const [activeSlotIndex, setActiveSlotIndex] = useState<number | null>(null);
  const [activePresetIndex, setActivePresetIndex] = useState<number | null>(null);
  const [dragState, setDragState] = useState<DragState | null>(null);
  const [pendingDeleteEchoId, setPendingDeleteEchoId] = useState<string | null>(null);
  const [pendingDeletePresetId, setPendingDeletePresetId] = useState<string | null>(null);
  const [presetSelectorOpen, setPresetSelectorOpen] = useState(false);
  const [presetNamingOpen, setPresetNamingOpen] = useState(false);
  const [presetNamingValue, setPresetNamingValue] = useState("");
  const [presetConflictId, setPresetConflictId] = useState<string | null>(null);
  const [presetCreateSource, setPresetCreateSource] = useState<PresetCreateSource>("selector");
  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(null);
  const [presetPopoverPos, setPresetPopoverPos] = useState({ left: 8, top: 8, width: 240 });
  const [presetManagerExpanded, setPresetManagerExpanded] = useState(false);
  const [presetDraftId, setPresetDraftId] = useState<string | null>(null);
  const [presetDraftName, setPresetDraftName] = useState("");
  const [presetDraftStats, setPresetDraftStats] = useState<string[]>([]);
  const [presetDraftOps, setPresetDraftOps] = useState<RelOp[]>([]);
  const [echoFilters, setEchoFilters] = useState<EchoFilters>(() => loadEchoFilters());
  const [substatSelectorOpen, setSubstatSelectorOpen] = useState(false);
  const [batchPanelExpanded, setBatchPanelExpanded] = useState(false);
  const [selectedEchoIds, setSelectedEchoIds] = useState<string[]>([]);
  const [selectionAnchorId, setSelectionAnchorId] = useState<string | null>(null);
  const [batchPresetId, setBatchPresetId] = useState("");
  const [pendingBatchDelete, setPendingBatchDelete] = useState(false);
  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startPosRef = useRef({ x: 0, y: 0 });
  const [activeTierSelectStat, setActiveTierSelectStat] = useState<string | null>(null);
  const [tierSelectorPos, setTierSelectorPos] = useState({ top: 0, left: 0 });

  const [saving, setSaving] = useState(false);
  const [toast, setToast] = useState<{ id: number; text: string; kind: ToastKind } | null>(null);
  const dragStateRef = useRef<DragState | null>(null);
  const filteredEchoes = useMemo(
    () => {
      const normalizedSearch = echoFilters.searchText.trim().toLocaleLowerCase();
      const requiredSubstats = echoFilters.substatStatKeys;

      // build preset chain for preset filter
      let filterPresetChain: { stats: string[]; ops: RelOp[] } | null = null;
      if (echoFilters.presetId !== "all") {
        if (echoFilters.presetId === "__none__") {
          filterPresetChain = { stats: [], ops: [] }; // sentinel for "no preset"
        } else {
          const filterPreset = expectationPresets.find((p) => p.presetId === echoFilters.presetId);
          if (filterPreset) {
            filterPresetChain = buildExpectationChain(filterPreset.items);
          }
        }
      }

      const filtered = echoes.filter((echo) => {
        if (echoFilters.costClass !== "all" && String(echo.costClass) !== echoFilters.costClass) {
          return false;
        }
        if (echoFilters.mainStatKey !== "all" && echo.mainStatKey !== echoFilters.mainStatKey) {
          return false;
        }
        if (echoFilters.status !== "all" && echo.status !== echoFilters.status) {
          return false;
        }
        if (echoFilters.openedSlots !== "all" && String(echo.openedSlotsCount) !== echoFilters.openedSlots) {
          return false;
        }
        if (echoFilters.presetId !== "all") {
          if (echoFilters.presetId === "__none__") {
            // "无预设" = echoes with no expectations, or expectations that don't match any preset
            const matchedPreset = echo.expectations.length > 0
              ? findPresetByChain(expectationPresets, buildExpectationChain(echo.expectations).stats, buildExpectationChain(echo.expectations).ops)
              : null;
            if (echo.expectations.length > 0 && matchedPreset) return false;
          } else if (filterPresetChain) {
            const echoChain = buildExpectationChain(echo.expectations);
            if (!areChainsEqual(echoChain.stats, echoChain.ops, filterPresetChain.stats, filterPresetChain.ops)) {
              return false;
            }
          }
        }
        if (normalizedSearch) {
          const searchBlob = [echo.nickname ?? "", echo.echoId].join(" ").toLocaleLowerCase();
          if (!searchBlob.includes(normalizedSearch)) {
            return false;
          }
        }
        if (requiredSubstats.length > 0) {
          const checkSubstat = (statKey: string) => {
            const slot = echo.currentSubstats.find((s) => s.statKey === statKey);
            if (!slot) return false;
            const tiers = echoFilters.substatTiers[statKey];
            if (tiers && tiers.length > 0 && !tiers.includes(slot.tierIndex)) return false;
            return true;
          };

          if (echoFilters.substatMode === "and") {
            if (!requiredSubstats.every(checkSubstat)) {
              return false;
            }
          } else if (echoFilters.substatMode === "or") {
            if (!requiredSubstats.some(checkSubstat)) {
              return false;
            }
          } else if (requiredSubstats.some(checkSubstat)) {
            return false;
          }
        }
        return true;
      });

      const readMainStat = (echo: (typeof echoes)[number]) =>
        statMap.get(echo.mainStatKey)?.displayName ?? echo.mainStatKey;
      const readNickname = (echo: (typeof echoes)[number]) => (echo.nickname?.trim() || echo.echoId).toLocaleLowerCase();
      const parseTimestamp = (value: string) => {
        const parsed = Date.parse(value);
        return Number.isFinite(parsed) ? parsed : 0;
      };

      const compareText = (a: string, b: string) => a.localeCompare(b, "zh-Hans-CN");
      const sorted = [...filtered].sort((a, b) => {
        let diff = 0;
        switch (echoFilters.sortBy) {
          case "created_desc":
            diff = parseTimestamp(b.createdAt) - parseTimestamp(a.createdAt);
            break;
          case "created_asc":
            diff = parseTimestamp(a.createdAt) - parseTimestamp(b.createdAt);
            break;
          case "updated_desc":
            diff = parseTimestamp(b.updatedAt) - parseTimestamp(a.updatedAt);
            break;
          case "updated_asc":
            diff = parseTimestamp(a.updatedAt) - parseTimestamp(b.updatedAt);
            break;
          case "opened_desc":
            diff = b.openedSlotsCount - a.openedSlotsCount;
            break;
          case "opened_asc":
            diff = a.openedSlotsCount - b.openedSlotsCount;
            break;
          case "cost_desc":
            diff = b.costClass - a.costClass;
            break;
          case "cost_asc":
            diff = a.costClass - b.costClass;
            break;
          case "main_asc":
            diff = compareText(readMainStat(a), readMainStat(b));
            break;
          case "main_desc":
            diff = compareText(readMainStat(b), readMainStat(a));
            break;
          case "status_asc":
            diff = compareText(a.status, b.status);
            break;
          case "status_desc":
            diff = compareText(b.status, a.status);
            break;
          case "nickname_asc":
            diff = compareText(readNickname(a), readNickname(b));
            break;
          case "nickname_desc":
            diff = compareText(readNickname(b), readNickname(a));
            break;
          default:
            diff = parseTimestamp(b.createdAt) - parseTimestamp(a.createdAt);
            break;
        }

        if (diff !== 0) {
          return diff;
        }
        return compareText(a.echoId, b.echoId);
      });

      return sorted;
    },
    [echoes, echoFilters, statMap],
  );
  const filteredEchoIds = useMemo(() => filteredEchoes.map((echo) => echo.echoId), [filteredEchoes]);
  const filteredEchoIdsKey = useMemo(() => filteredEchoIds.join("|"), [filteredEchoIds]);
  const selectedEchoIdSet = useMemo(() => new Set(selectedEchoIds), [selectedEchoIds]);
  const allFilteredSelected = useMemo(
    () => filteredEchoIds.length > 0 && filteredEchoIds.every((echoId) => selectedEchoIdSet.has(echoId)),
    [filteredEchoIds, selectedEchoIdSet],
  );
  const someFilteredSelected = useMemo(
    () => filteredEchoIds.some((echoId) => selectedEchoIdSet.has(echoId)),
    [filteredEchoIds, selectedEchoIdSet],
  );
  const selectedBatchPreset = useMemo(
    () => expectationPresets.find((preset) => preset.presetId === batchPresetId) ?? null,
    [expectationPresets, batchPresetId],
  );

  const syncPresetPopoverPosition = () => {
    const trigger =
      presetCreateSource === "manager"
        ? presetManagerAddButtonRef.current ?? presetSelectorButtonRef.current
        : presetSelectorButtonRef.current ?? presetManagerAddButtonRef.current;
    if (!trigger) {
      return;
    }
    const rect = trigger.getBoundingClientRect();
    const width = Math.max(220, Math.min(360, Math.max(rect.width, 220)));
    const maxLeft = Math.max(8, window.innerWidth - width - 8);
    const left = Math.min(maxLeft, Math.max(8, rect.left));
    const top = Math.max(8, Math.min(window.innerHeight - 8, rect.bottom + 4));
    setPresetPopoverPos({ left, top, width });
  };
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
    if (typeof window === "undefined") {
      return;
    }
    window.localStorage.setItem(ECHO_FILTER_STORAGE_KEY, JSON.stringify(echoFilters));
  }, [echoFilters]);

  useEffect(() => {
    if (echoFilters.mainStatKey === "all") {
      return;
    }
    if (!statDefs.some((stat) => stat.statKey === echoFilters.mainStatKey)) {
      setEchoFilters((prev) => ({ ...prev, mainStatKey: "all" }));
    }
  }, [echoFilters.mainStatKey, statDefs]);

  useEffect(() => {
    if (statDefs.length === 0) {
      return;
    }
    const validStatKeys = new Set(statDefs.map((stat) => stat.statKey));
    setEchoFilters((prev) => {
      if (prev.substatStatKeys.length === 0) {
        return prev;
      }
      const sanitized = prev.substatStatKeys.filter((statKey) => validStatKeys.has(statKey));
      if (sanitized.length === prev.substatStatKeys.length) {
        return prev;
      }
      return { ...prev, substatStatKeys: sanitized };
    });
  }, [statDefs]);

  useEffect(() => {
    const validIds = new Set(echoes.map((echo) => echo.echoId));
    setSelectedEchoIds((prev) => prev.filter((echoId) => validIds.has(echoId)));
    setSelectionAnchorId((prev) => (prev && validIds.has(prev) ? prev : null));
  }, [echoes]);

  useEffect(() => {
    const visibleIds = new Set(filteredEchoIds);
    setSelectedEchoIds((prev) => prev.filter((echoId) => visibleIds.has(echoId)));
    setSelectionAnchorId((prev) => (prev && visibleIds.has(prev) ? prev : null));
  }, [filteredEchoIdsKey, filteredEchoIds]);

  useEffect(() => {
    const checkbox = selectAllCheckboxRef.current;
    if (!checkbox) {
      return;
    }
    checkbox.indeterminate = !allFilteredSelected && someFilteredSelected;
  }, [allFilteredSelected, someFilteredSelected]);

  useEffect(() => {
    setPendingBatchDelete(false);
  }, [selectedEchoIds]);

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

  const dismissChainSelection = useCallback(() => {
    setActiveExpectationIndex(null);
    setActiveSlotIndex(null);
  }, []);

  const handleChainPointerDown = useCallback((target: Element) => {
    if (!target.closest(".echo-action-btn")) {
      setPendingDeleteEchoId(null);
    }
    if (!target.closest(".preset-action-btn")) {
      setPendingDeletePresetId(null);
    }
  }, []);

  useChainSelectionDismiss({
    chainScopeSelector: ".chain-item, .chain-op",
    onDismiss: dismissChainSelection,
    onPointerDown: handleChainPointerDown,
  });

  useEffect(() => {
    if (!presetSelectorOpen && !presetNamingOpen) {
      return;
    }
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        return;
      }
      const inTrigger = presetSelectorRef.current?.contains(target) ?? false;
      const inMenu = presetSelectorMenuRef.current?.contains(target) ?? false;
      const inNaming = presetNamingPopRef.current?.contains(target) ?? false;
      const inManagerAdd = presetManagerAddButtonRef.current?.contains(target) ?? false;
      if (!inTrigger && !inMenu && !inNaming && !inManagerAdd) {
        setPresetSelectorOpen(false);
        setPresetNamingOpen(false);
      }
    };
    window.addEventListener("pointerdown", handlePointerDown);
    return () => window.removeEventListener("pointerdown", handlePointerDown);
  }, [presetSelectorOpen, presetNamingOpen]);

  useEffect(() => {
    if (!substatSelectorOpen) {
      return;
    }
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        return;
      }
      const isElement = target instanceof Element;
      if (!(substatSelectorRef.current?.contains(target) ?? false) && 
          (!isElement || !target.closest(".echo-tier-selector-container"))) {
        setSubstatSelectorOpen(false);
      }
    };
    window.addEventListener("pointerdown", handlePointerDown);
    return () => window.removeEventListener("pointerdown", handlePointerDown);
  }, [substatSelectorOpen]);

  useEffect(() => {
    if (!activeTierSelectStat) {
      return;
    }
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        return;
      }
      if (!(target instanceof Element) || !target.closest(".echo-tier-selector-container")) {
        setActiveTierSelectStat(null);
      }
    };
    window.addEventListener("pointerdown", handlePointerDown);
    return () => window.removeEventListener("pointerdown", handlePointerDown);
  }, [activeTierSelectStat]);

  useEffect(() => {
    if (!presetSelectorOpen && !presetNamingOpen) {
      return;
    }
    const updatePosition = () => syncPresetPopoverPosition();
    updatePosition();
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [presetSelectorOpen, presetNamingOpen]);

  useEffect(() => {
    if (!presetNamingOpen) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      const input = presetNamingInputRef.current;
      if (!input) {
        return;
      }
      input.focus();
      input.select();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [presetNamingOpen]);

  useEffect(() => {
    if (!editingEchoId) {
      setSelectedPresetId(null);
      return;
    }

    const matchedPreset = findPresetByChain(expectationPresets, expectationStats, expectationOps);
    setSelectedPresetId(matchedPreset?.presetId ?? null);
  }, [editingEchoId, expectationStats, expectationOps, expectationPresets]);

  useChainDragSession<DragKind, DragState>({
    dragState,
    dragStateRef,
    setDragState,
    getRowElement: (kind) =>
      kind === "expectation" ? expectationRowRef.current : kind === "slot" ? slotRowRef.current : presetRowRef.current,
    onApplyDrag: (current) => {
      if (current.kind === "expectation") {
        setExpectationStats((prev) => moveArrayToInsertion(prev, current.fromIndex, current.dropIndex));
        setActiveExpectationIndex((prev) =>
          remapIndexAfterInsertion(prev, expectationStats.length, current.fromIndex, current.dropIndex),
        );
      } else if (current.kind === "slot") {
        setSlotsDraft((prev) => moveArrayToInsertion(prev, current.fromIndex, current.dropIndex));
        setActiveSlotIndex((prev) =>
          remapIndexAfterInsertion(prev, slotsDraft.length, current.fromIndex, current.dropIndex),
        );
      } else {
        setPresetDraftStats((prev) => moveArrayToInsertion(prev, current.fromIndex, current.dropIndex));
        setActivePresetIndex((prev) =>
          remapIndexAfterInsertion(prev, presetDraftStats.length, current.fromIndex, current.dropIndex),
        );
      }
    },
  });

  const columnWidths = useMemo(() => {
    const total = Math.max(tableClientWidth, 1);
    const select = 40;
    const actions = total >= 760 ? 300 : Math.max(210, Math.floor(total * 0.38));
    const remaining = Math.max(total - actions - select, 0);
    const nickname = Math.floor((remaining * 2) / 10);
    const main = Math.floor(remaining / 10);
    const cost = Math.floor(remaining / 10);
    const status = Math.floor(remaining / 10);
    const slots = remaining - nickname - main - cost - status;
    return {
      select,
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
  const draggingExpectationFromIndex = dragState?.kind === "expectation" ? dragState.fromIndex : null;
  const draggingSlotFromIndex = dragState?.kind === "slot" ? dragState.fromIndex : null;
  const draggingPresetFromIndex = dragState?.kind === "preset" ? dragState.fromIndex : null;
  const expectationInsertBeforeIndex =
    dragState?.kind === "expectation"
      ? resolveInsertBeforeIndex(expectationStats.length, dragState.fromIndex, dragState.dropIndex)
      : null;
  const slotInsertBeforeIndex =
    dragState?.kind === "slot"
      ? resolveInsertBeforeIndex(slotsDraft.length, dragState.fromIndex, dragState.dropIndex)
      : null;
  const presetInsertBeforeIndex =
    dragState?.kind === "preset"
      ? resolveInsertBeforeIndex(presetDraftStats.length, dragState.fromIndex, dragState.dropIndex)
      : null;
  const selectedPresetName = useMemo(
    () => expectationPresets.find((preset) => preset.presetId === selectedPresetId)?.name ?? "未选择",
    [expectationPresets, selectedPresetId],
  );
  const tableColumnCount = 7;

  const openBatchPanel = () => setBatchPanelExpanded(true);
  const collapseBatchPanel = () => {
    setBatchPanelExpanded(false);
    setSelectedEchoIds([]);
    setSelectionAnchorId(null);
    setPendingBatchDelete(false);
  };

  const applyEchoSelection = (echoId: string, options: { toggle: boolean; range: boolean }) => {
    const targetIndex = filteredEchoIds.indexOf(echoId);
    if (targetIndex < 0) {
      return;
    }

    setSelectedEchoIds((prev) => {
      if (options.range && selectionAnchorId) {
        const anchorIndex = filteredEchoIds.indexOf(selectionAnchorId);
        if (anchorIndex >= 0) {
          const start = Math.min(anchorIndex, targetIndex);
          const end = Math.max(anchorIndex, targetIndex);
          const rangeIds = filteredEchoIds.slice(start, end + 1);
          if (options.toggle) {
            const merged = new Set(prev);
            rangeIds.forEach((id) => merged.add(id));
            return Array.from(merged);
          }
          return rangeIds;
        }
      }

      if (options.toggle) {
        if (prev.includes(echoId)) {
          return prev.filter((id) => id !== echoId);
        }
        return [...prev, echoId];
      }

      return [echoId];
    });

    setSelectionAnchorId(echoId);
  };

  const onEchoRowClick = (event: ReactMouseEvent<HTMLTableRowElement>, echoId: string) => {
    const target = event.target;
    if (target instanceof HTMLElement && target.closest("button, input, select, textarea, a, label")) {
      return;
    }
    const isModifierToggle = event.metaKey || event.ctrlKey;
    const isRange = event.shiftKey;
    if (!batchPanelExpanded && !isModifierToggle && !isRange) {
      return;
    }
    if (!batchPanelExpanded && (isModifierToggle || isRange)) {
      setBatchPanelExpanded(true);
    }
    applyEchoSelection(echoId, {
      toggle: batchPanelExpanded ? (isRange ? isModifierToggle : true) : isModifierToggle,
      range: isRange,
    });
    clearNativeTextSelection();
  };

  const onEchoRowMouseDown = (event: ReactMouseEvent<HTMLTableRowElement>) => {
    const target = event.target;
    if (target instanceof HTMLElement && target.closest("button, input, select, textarea, a, label")) {
      return;
    }
    const hasModifier = event.shiftKey || event.metaKey || event.ctrlKey;
    if (batchPanelExpanded || hasModifier) {
      event.preventDefault();
    }
  };

  const onEchoCheckboxClick = (event: ReactMouseEvent<HTMLInputElement>, echoId: string) => {
    event.stopPropagation();
    applyEchoSelection(echoId, {
      toggle: event.metaKey || event.ctrlKey || !event.shiftKey,
      range: event.shiftKey,
    });
    clearNativeTextSelection();
  };

  const onToggleSelectAllVisible = (checked: boolean) => {
    if (!batchPanelExpanded) {
      return;
    }
    if (!checked) {
      setSelectedEchoIds([]);
      setSelectionAnchorId(null);
      return;
    }
    setSelectedEchoIds(filteredEchoIds);
    setSelectionAnchorId(filteredEchoIds[0] ?? null);
  };

  const applyPresetToSelectedEchoes = async () => {
    if (selectedEchoIds.length === 0) {
      setMessage("请先选择要批量操作的声骸。");
      return;
    }
    if (!selectedBatchPreset) {
      setMessage("请先选择要应用的预设。");
      return;
    }

    const targetEchoIds = [...selectedEchoIds];
    setSaving(true);
    setMessage("");
    try {
      const results = await Promise.allSettled(
        targetEchoIds.map((echoId) => setExpectations(echoId, selectedBatchPreset.items)),
      );
      await refreshEchoes();

      if (editingEchoId && targetEchoIds.includes(editingEchoId)) {
        cancelEdit();
      }

      const failed = results.filter((result) => result.status === "rejected");
      const successCount = targetEchoIds.length - failed.length;
      if (failed.length === 0) {
        setMessage(
          `已对 ${successCount} 条声骸应用预设「${selectedBatchPreset.name}」。`,
          "success",
        );
      } else {
        const firstError = failed[0];
        const detail = firstError?.status === "rejected" ? ` 首个错误：${String(firstError.reason)}` : "";
        setMessage(
          `已应用 ${successCount}/${targetEchoIds.length} 条，${failed.length} 条失败。${detail}`,
          "error",
        );
      }
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setSaving(false);
    }
  };

  const removeSelectedEchoes = async () => {
    if (selectedEchoIds.length === 0) {
      setMessage("请先选择要删除的声骸。");
      return;
    }
    if (!pendingBatchDelete) {
      setPendingBatchDelete(true);
      setMessage(`再次点击“批量删除”以确认删除 ${selectedEchoIds.length} 条声骸。`);
      return;
    }

    const targetEchoIds = [...selectedEchoIds];
    setSaving(true);
    setMessage("");
    try {
      const results = await Promise.allSettled(targetEchoIds.map((echoId) => deleteEcho(echoId)));
      await refreshEchoes();

      const targetSet = new Set(targetEchoIds);
      setSelectedEchoIds((prev) => prev.filter((echoId) => !targetSet.has(echoId)));
      setSelectionAnchorId((prev) => (prev && targetSet.has(prev) ? null : prev));

      if (editingEchoId && targetSet.has(editingEchoId)) {
        cancelEdit();
      }

      const failed = results.filter((result) => result.status === "rejected");
      const successCount = targetEchoIds.length - failed.length;
      if (failed.length === 0) {
        setMessage(`已删除 ${successCount} 条声骸。`, "success");
      } else {
        const firstError = failed[0];
        const detail = firstError?.status === "rejected" ? ` 首个错误：${String(firstError.reason)}` : "";
        setMessage(
          `已删除 ${successCount}/${targetEchoIds.length} 条，${failed.length} 条删除失败。${detail}`,
          "error",
        );
      }
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingBatchDelete(false);
      setSaving(false);
    }
  };

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
    setActivePresetIndex(null);
    setDragState(null);
    setPendingDeleteEchoId(null);
    setPendingDeletePresetId(null);
    setPresetSelectorOpen(false);
    setPresetNamingOpen(false);
    setPresetNamingValue("");
    // selectedPresetId is auto-synced by the effect based on expectation chain
    setPresetDraftId(null);
    setPresetDraftName("");
    setPresetDraftStats([]);
    setPresetDraftOps([]);
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
    setActivePresetIndex(null);
    setDragState(null);
    setPendingDeleteEchoId(null);
    setPendingDeletePresetId(null);
    setPresetSelectorOpen(false);
    setPresetNamingOpen(false);
    setPresetNamingValue("");
    setSelectedPresetId(null);
    setPresetDraftId(null);
    setPresetDraftName("");
    setPresetDraftStats([]);
    setPresetDraftOps([]);
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

  const usePresetToCurrentEditor = (preset: ExpectationPreset) => {
    if (!editingEcho) {
      setMessage("请先点击某条声骸的“管理”，再使用预设。");
      return;
    }
    const chain = buildExpectationChain(preset.items);
    setExpectationStats(chain.stats);
    setExpectationOps(chain.ops);
    setActiveExpectationIndex(null);
    setSelectedPresetId(preset.presetId);
    setPresetSelectorOpen(false);
    setMessage(`已载入预设「${preset.name}」。`);
  };

  const openPresetEditor = (preset: PresetDraftSource) => {
    const chain = buildExpectationChain(preset.items);
    setPresetDraftId(preset.presetId);
    setPresetDraftName(preset.name);
    setPresetDraftStats(chain.stats);
    setPresetDraftOps(chain.ops);
    setActivePresetIndex(null);
    setPendingDeletePresetId(null);
    setPresetManagerExpanded(true);
  };

  const cancelPresetEdit = () => {
    setPresetDraftId(null);
    setPresetDraftName("");
    setPresetDraftStats([]);
    setPresetDraftOps([]);
    setActivePresetIndex(null);
    setPendingDeletePresetId(null);
  };

  const openCreatePresetNaming = (source: PresetCreateSource = "selector") => {
    if (source === "selector" && !editingEcho) {
      setMessage("请先点击某条声骸的“管理”，再设为预设。");
      return;
    }
    setPresetSelectorOpen(false);
    setPresetCreateSource(source);
    setPresetNamingValue(buildDefaultPresetName());
    setPresetNamingOpen(true);
    window.requestAnimationFrame(() => syncPresetPopoverPosition());
  };

  const createPresetFromCurrent = async (forceOverwriteId?: string) => {
    if (presetCreateSource === "selector" && !editingEcho) {
      setMessage("请先点击某条声骸的“管理”，再设为预设。");
      return;
    }
    const isFromSelector = presetCreateSource === "selector";
    let items: ExpectationItem[] = [];

    if (isFromSelector) {
      const uniqueExpectationStats = Array.from(new Set(expectationStats));
      if (uniqueExpectationStats.length !== expectationStats.length) {
        setMessage("期望词条存在重复，请先调整。");
        return;
      }
      items = chainToExpectationItems(expectationStats, expectationOps);
      if (items.length === 0) {
        setMessage("当前没有可保存的期望词条。");
        return;
      }

      if (!forceOverwriteId) {
        const existingPreset = findPresetByChain(expectationPresets, expectationStats, expectationOps);
        if (existingPreset) {
          setSelectedPresetId(existingPreset.presetId);
          setPresetNamingOpen(false);
          setPresetSelectorOpen(false);
          setMessage(`已存在相同内容的预设「${existingPreset.name}」，已自动选中。`);
          return;
        }
      }
    }

    const generatedName = presetNamingValue.trim() || buildDefaultPresetName();

    if (!forceOverwriteId) {
      const conflict = expectationPresets.find((p) => p.name === generatedName);
      if (conflict) {
        setPresetConflictId(conflict.presetId);
        return;
      }
    }

    setSaving(true);
    setMessage("");
    try {
      const result = await saveExpectationPreset({
        presetId: forceOverwriteId,
        name: generatedName,
        items,
      });
      await refreshExpectationPresets();
      setSelectedPresetId(result.presetId);
      setPresetNamingOpen(false);
      setPresetSelectorOpen(false);
      setPresetConflictId(null);
      if (!isFromSelector) {
        openPresetEditor({
          presetId: result.presetId,
          name: generatedName,
          items,
        });
      }
      setMessage(isFromSelector ? `已设为预设「${generatedName}」。` : `已新建空预设「${generatedName}」。`, "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      setPendingDeletePresetId(null);
      setSaving(false);
    }
  };

  const addPresetStat = () => {
    setPresetDraftStats((prev) => {
      const nextStat = pickAvailableStat(prev);
      if (prev.includes(nextStat)) {
        setMessage("没有可添加的预设词条。");
        return prev;
      }
      if (prev.length > 0) {
        setPresetDraftOps((ops) => [...ops, "gt"]);
      }
      setActivePresetIndex(prev.length);
      return [...prev, nextStat];
    });
  };

  const removePresetStatAt = (idx: number) => {
    setPresetDraftStats((prevStats) => {
      const next = prevStats.filter((_, i) => i !== idx);
      setPresetDraftOps((prevOps) => {
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
    setActivePresetIndex((prev) => {
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

  const savePresetDraft = async () => {
    const name = presetDraftName.trim();
    if (!name) {
      setMessage("请填写预设名称。");
      return;
    }
    if (presetDraftStats.length === 0) {
      setMessage("预设词条为空，无法保存。");
      return;
    }
    const uniquePresetStats = Array.from(new Set(presetDraftStats));
    if (uniquePresetStats.length !== presetDraftStats.length) {
      setMessage("预设词条存在重复，请先调整。");
      return;
    }
    const duplicatePreset = findPresetByChain(
      expectationPresets,
      presetDraftStats,
      presetDraftOps,
      presetDraftId ?? undefined,
    );
    if (duplicatePreset) {
      setSelectedPresetId(duplicatePreset.presetId);
      setMessage(`已存在相同内容的预设「${duplicatePreset.name}」，请调整后再保存。`);
      return;
    }

    setSaving(true);
    setMessage("");
    try {
      await saveExpectationPreset({
        presetId: presetDraftId ?? undefined,
        name,
        items: chainToExpectationItems(presetDraftStats, presetDraftOps),
      });
      await refreshExpectationPresets();
      cancelPresetEdit();
      setMessage("预设已保存。", "success");
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
      setMessage("再次点击“删除”以确认。");
      return;
    }

    setSaving(true);
    setMessage("");
    try {
      await deleteExpectationPreset(presetId);
      await refreshExpectationPresets();
      if (presetDraftId === presetId) {
        setPresetDraftId(null);
        setPresetDraftName("");
        setPresetDraftStats([]);
        setPresetDraftOps([]);
        setActivePresetIndex(null);
      }
      if (selectedPresetId === presetId) {
        setSelectedPresetId(null);
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

  const renderPresetManager = () => (
    <div className="chain-block">
      <span className="chain-label">预设列表</span>
      <table className="table compact-table preset-table">
        <colgroup>
          <col className="preset-col-name" />
          <col className="preset-col-items" />
          <col className="preset-col-actions" />
        </colgroup>
        <thead>
          <tr>
            <th>名称</th>
            <th>词条</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {expectationPresets.map((preset) => {
            const editing = presetDraftId === preset.presetId;

            return (
              <Fragment key={preset.presetId}>
                <tr className={editing ? "active-row" : ""}>
                  <td className="preset-col-name">
                    <div className="preset-cell">
                      {editing ? (
                        <input
                          className="preset-row-name-input preset-cell-control"
                          value={presetDraftName}
                          onChange={(e) => setPresetDraftName(e.target.value)}
                          placeholder="预设名称"
                        />
                      ) : (
                        <span className="preset-cell-display" title={preset.name}>
                          {preset.name}
                        </span>
                      )}
                    </div>
                  </td>
                  <td className="preset-col-items">
                    <div className="preset-cell">
                      {editing ? (
                        <span className="preset-cell-display hint">在下方编辑词条链</span>
                      ) : (
                        <span className="preset-cell-display" title={formatPresetSummary(preset.items)}>
                          {formatPresetSummary(preset.items)}
                        </span>
                      )}
                    </div>
                  </td>
                  <td className="preset-col-actions">
                    <div className="inline-row preset-actions-inline">
                      <button
                        type="button"
                        className={editing ? "preset-action-btn manage-btn-active" : "preset-action-btn"}
                        onClick={() => {
                          if (editing) {
                            return;
                          }
                          openPresetEditor(preset);
                        }}
                        disabled={saving}
                      >
                        管理
                      </button>
                      {editing ? (
                        <>
                          <button type="button" className="preset-action-btn" onClick={cancelPresetEdit} disabled={saving}>
                            取消
                          </button>
                          <button
                            type="button"
                            className="preset-action-btn"
                            onClick={() => void savePresetDraft()}
                            disabled={saving}
                          >
                            保存
                          </button>
                        </>
                      ) : null}
                      <button
                        type="button"
                        className="preset-action-btn"
                        onClick={() => void removePreset(preset.presetId)}
                        disabled={saving}
                      >
                        {pendingDeletePresetId === preset.presetId ? "确认删除" : "删除"}
                      </button>
                    </div>
                  </td>
                </tr>
                {editing ? (
                  <tr className="active-row">
                    <td colSpan={3}>
                      <div className="chain-block">
                        <span className="chain-label">预设词条</span>
                        <div className="chain-row" ref={presetRowRef}>
                          {presetDraftStats.length === 0 ? <span className="chain-empty">无</span> : null}
                          {presetDraftStats.map((statKey, idx) => {
                            const stat = statMap.get(statKey);
                            const selected = activePresetIndex === idx;
                            const isDraggingThis = draggingPresetFromIndex === idx;
                            const classNames = ["chain-item"];
                            if (selected) classNames.push("active");
                            if (isDraggingThis) classNames.push("dragging");
                            const availableStats = statDefs.filter(
                              (x) => x.statKey === statKey || !presetDraftStats.includes(x.statKey),
                            );
                            const hideOperator =
                              draggingPresetFromIndex !== null && idx === draggingPresetFromIndex;

                            return (
                              <Fragment key={`preset-${idx}-${statKey}`}>
                                {presetInsertBeforeIndex === idx ? (
                                  <span className="drag-insert-line" aria-hidden="true" />
                                ) : null}
                                <div className="chain-fragment">
                                  <div
                                    className={classNames.join(" ")}
                                    data-drag-kind="preset"
                                    data-drag-index={idx}
                                    onPointerDown={(e) => {
                                      beginLongPressDrag<DragKind, DragState>({
                                        event: e,
                                        kind: "preset",
                                        fromIndex: idx,
                                        label: stat?.displayName ?? statKey,
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
                                        onTap: () => setActivePresetIndex(idx),
                                      });
                                    }}
                                    onContextMenu={(e) => {
                                      e.preventDefault();
                                      removePresetStatAt(idx);
                                    }}
                                    title="长按拖动，点击编辑，右键删除"
                                  >
                                    {selected ? (
                                      <select
                                        value={statKey}
                                        onChange={(e) => {
                                          const next = e.target.value;
                                          setPresetDraftStats((prev) =>
                                            prev.map((item, itemIdx) => (itemIdx === idx ? next : item)),
                                          );
                                        }}
                                        onPointerDown={(e) => e.stopPropagation()}
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
                                  {idx < presetDraftOps.length && !hideOperator ? (
                                    <button
                                      type="button"
                                      className="chain-op"
                                      onClick={() =>
                                        setPresetDraftOps((prev) =>
                                          prev.map((x, opIdx) => (opIdx === idx ? (x === "gt" ? "eq" : "gt") : x)),
                                        )
                                      }
                                      title="点击切换 > 或 ="
                                    >
                                      {presetDraftOps[idx] === "gt" ? ">" : "="}
                                    </button>
                                  ) : null}
                                </div>
                              </Fragment>
                            );
                          })}
                          {dragState?.kind === "preset" && presetInsertBeforeIndex === null ? (
                            <span className="drag-insert-line" aria-hidden="true" />
                          ) : null}
                          <button type="button" className="chain-add" onClick={addPresetStat}>
                            +
                          </button>
                        </div>
                      </div>
                    </td>
                  </tr>
                ) : null}
              </Fragment>
            );
          })}
          {expectationPresets.length === 0 ? (
            <tr>
              <td colSpan={3} className="chain-empty">
                暂无预设，可在声骸条目“预设”里设为预设。
              </td>
            </tr>
          ) : null}
        </tbody>
      </table>
    </div>
  );

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

  const closePresetManagerView = () => {
    setPresetManagerExpanded(false);
    setPresetNamingOpen(false);
    setPresetSelectorOpen(false);
  };

  return (
    <section className="page echo-pool-page">
      {toast ? <div className={`toast toast-${toast.kind}`}>{toast.text}</div> : null}
      {dragState ? (
        <div className="drag-ghost" style={{ left: dragState.x + 12, top: dragState.y + 12 }}>
          <span>{dragState.label}</span>
        </div>
      ) : null}

      <div className="preset-manager-dock">
        {!presetManagerExpanded ? (
          <button type="button" className="preset-manager-tab" onClick={() => setPresetManagerExpanded(true)}>
            <span>期望预设管理</span>
            <span className="preset-manager-tab-icon">◀</span>
          </button>
        ) : null}
      </div>

      {presetManagerExpanded ? (
        <div className="preset-manager-overlay" onClick={closePresetManagerView}>
          <div className="card preset-manager-panel" onClick={(e) => e.stopPropagation()}>
            <div className="preset-manager-head">
              <strong>期望预设管理</strong>
              <div className="preset-manager-head-actions">
                <button
                  type="button"
                  ref={presetManagerAddButtonRef}
                  className="preset-manager-add-btn"
                  onClick={() => openCreatePresetNaming("manager")}
                >
                  +
                </button>
                <button type="button" onClick={closePresetManagerView}>
                  收起
                </button>
              </div>
            </div>
            {renderPresetManager()}
          </div>
        </div>
      ) : null}

      {presetSelectorOpen ? (
        <div
          ref={presetSelectorMenuRef}
          className="preset-selector-menu preset-floating"
          style={{
            left: `${presetPopoverPos.left}px`,
            top: `${presetPopoverPos.top}px`,
            width: `${presetPopoverPos.width}px`,
          }}
        >
          {expectationPresets.map((preset) => (
            <button
              key={preset.presetId}
              type="button"
              className={selectedPresetId === preset.presetId ? "preset-option-active" : ""}
              onClick={() => usePresetToCurrentEditor(preset)}
              title={formatPresetSummary(preset.items)}
            >
              {preset.name}
            </button>
          ))}
          {expectationPresets.length === 0 ? <span className="chain-empty">暂无预设</span> : null}
          <span className="preset-option-divider" />
          <button
            type="button"
            className="preset-create-option"
            onClick={() => openCreatePresetNaming("selector")}
            disabled={saving}
          >
            设为预设
          </button>
        </div>
      ) : null}

      {presetNamingOpen ? (
        <form
          ref={presetNamingPopRef}
          className="preset-naming-pop preset-floating"
          style={{
            left: `${presetPopoverPos.left}px`,
            top: `${presetPopoverPos.top}px`,
            width: `${Math.max(280, presetPopoverPos.width)}px`,
          }}
          onSubmit={(e) => {
            e.preventDefault();
            void createPresetFromCurrent();
          }}
        >
          {presetConflictId ? (
            <div className="preset-conflict-alert">
              <span>已存在同名预设「{presetNamingValue.trim()}」，是否覆盖？</span>
              <div className="preset-conflict-actions">
                <button type="button" disabled={saving} onClick={() => {
                  setPresetNamingOpen(false);
                  setPresetConflictId(null);
                }}>取消</button>
                <button type="button" disabled={saving} onClick={() => {
                  setPresetConflictId(null);
                  requestAnimationFrame(() => {
                    const input = presetNamingInputRef.current;
                    if (input) {
                      input.focus();
                      input.select();
                    }
                  });
                }}>重命名</button>
                <button type="button" disabled={saving} className="btn-danger" onClick={() => void createPresetFromCurrent(presetConflictId)}>确定</button>
              </div>
            </div>
          ) : (
            <>
              <input
                ref={presetNamingInputRef}
                value={presetNamingValue}
                onChange={(e) => setPresetNamingValue(e.target.value)}
              />
              <button type="submit" disabled={saving}>
                确定
              </button>
            </>
          )}
        </form>
      ) : null}

      <div className="card echo-table-card" ref={tableWrapRef}>
        <div className="echo-toolbar">
          <div className="echo-toolbar-group echo-filter-group">
            <div className="echo-filter-head">
              <div className="echo-filter-head-main">
                <span className="echo-toolbar-title echo-filter-title">筛选</span>
                <button
                  type="button"
                  className="echo-toolbar-control echo-reset-btn"
                  onClick={() => setEchoFilters({ ...DEFAULT_ECHO_FILTERS })}
                  disabled={isDefaultEchoFilters(echoFilters)}
                  title="重置筛选"
                  aria-label="重置筛选"
                >
                  ↻
                </button>
                <input
                  className="echo-search-input"
                  value={echoFilters.searchText}
                  placeholder="搜索ID/昵称"
                  onChange={(e) => setEchoFilters((prev) => ({ ...prev, searchText: e.target.value }))}
                />
              </div>
              <div className={`echo-filter-batch ${batchPanelExpanded ? "is-expanded" : "is-collapsed"}`}>
                {batchPanelExpanded ? (
                  <>
                    <div className="echo-filter-batch-info">
                      <span className="echo-toolbar-title">批量</span>
                      <span className="echo-toolbar-meta">
                        已选 {selectedEchoIds.length} / 可见 {filteredEchoes.length}
                      </span>
                    </div>
                    <select
                      className="echo-toolbar-control echo-batch-preset-select"
                      value={batchPresetId}
                      onChange={(e) => setBatchPresetId(e.target.value)}
                    >
                      <option value="">选择预设</option>
                      {expectationPresets.map((preset) => (
                        <option key={preset.presetId} value={preset.presetId}>
                          {preset.name}
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      className="echo-toolbar-control"
                      onClick={() => void applyPresetToSelectedEchoes()}
                      disabled={saving}
                    >
                      批量应用预设
                    </button>
                    <button
                      type="button"
                      className="echo-toolbar-control"
                      onClick={() => void removeSelectedEchoes()}
                      disabled={saving}
                    >
                      {pendingBatchDelete ? "确认批量删除" : "批量删除"}
                    </button>
                    <div className="hover-tip">
                      <button
                        type="button"
                        className="hover-tip-trigger"
                        aria-label="多选说明"
                      >
                        ?
                      </button>
                      <span className="hover-tip-content">
                        展开批量后可直接单击多选；收起时 Ctrl/Cmd 或 Shift 可触发批量模式
                      </span>
                    </div>
                  </>
                ) : null}
                <button
                  type="button"
                  className={`echo-toolbar-control echo-batch-expand-btn ${batchPanelExpanded ? "manage-btn-active" : ""}`}
                  onClick={() => (batchPanelExpanded ? collapseBatchPanel() : openBatchPanel())}
                  disabled={saving}
                  title={batchPanelExpanded ? "收起批量" : "展开批量"}
                  aria-label={batchPanelExpanded ? "收起批量" : "展开批量"}
                >
                  ☑
                </button>
              </div>
            </div>
            <label className="echo-short-select">
              Cost
              <select
                className="echo-toolbar-control"
                value={echoFilters.costClass}
                onChange={(e) =>
                  setEchoFilters((prev) => ({ ...prev, costClass: e.target.value as EchoFilters["costClass"] }))
                }
              >
                <option value="all">全部</option>
                <option value="1">1</option>
                <option value="3">3</option>
                <option value="4">4</option>
              </select>
            </label>
            <label>
              主词条
              <select
                className="echo-toolbar-control"
                value={echoFilters.mainStatKey}
                onChange={(e) => setEchoFilters((prev) => ({ ...prev, mainStatKey: e.target.value }))}
              >
                <option value="all">全部</option>
                {statDefs.map((stat) => (
                  <option key={stat.statKey} value={stat.statKey}>
                    {stat.displayName}
                  </option>
                ))}
              </select>
            </label>
            <label>
              状态
              <select
                className="echo-toolbar-control"
                value={echoFilters.status}
                onChange={(e) =>
                  setEchoFilters((prev) => ({
                    ...prev,
                    status:
                      e.target.value === "all" || ECHO_STATUS_OPTIONS.includes(e.target.value as EchoStatus)
                        ? (e.target.value as EchoFilters["status"])
                        : "all",
                  }))
                }
              >
                <option value="all">全部</option>
                {ECHO_STATUS_OPTIONS.map((status) => (
                  <option key={status} value={status}>
                    {status}
                  </option>
                ))}
              </select>
            </label>
            <label className="echo-short-select">
              已开槽位
              <select
                className="echo-toolbar-control"
                value={echoFilters.openedSlots}
                onChange={(e) =>
                  setEchoFilters((prev) => ({ ...prev, openedSlots: e.target.value as EchoFilters["openedSlots"] }))
                }
              >
                <option value="all">全部</option>
                <option value="0">0</option>
                <option value="1">1</option>
                <option value="2">2</option>
                <option value="3">3</option>
                <option value="4">4</option>
                <option value="5">5</option>
              </select>
            </label>
            <label>
              预设
              <select
                className="echo-toolbar-control"
                value={echoFilters.presetId}
                onChange={(e) =>
                  setEchoFilters((prev) => ({ ...prev, presetId: e.target.value }))
                }
              >
                <option value="all">全部</option>
                <option value="__none__">无预设</option>
                {expectationPresets.map((preset) => (
                  <option key={preset.presetId} value={preset.presetId}>
                    {preset.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              副词条逻辑
              <select
                className="echo-toolbar-control"
                value={echoFilters.substatMode}
                onChange={(e) =>
                  setEchoFilters((prev) => ({
                    ...prev,
                    substatMode: sanitizeSubstatFilterMode(e.target.value),
                  }))
                }
              >
                {SUBSTAT_FILTER_MODE_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <div className="echo-substat-selector" ref={substatSelectorRef}>
              <button
                type="button"
                className={`${substatSelectorOpen ? "manage-btn-active " : ""}echo-toolbar-control echo-substat-trigger`}
                onClick={() => setSubstatSelectorOpen((prev) => !prev)}
              >
                副词条：{echoFilters.substatStatKeys.length === 0 ? "全部" : `${echoFilters.substatStatKeys.length}项`}
              </button>
              {substatSelectorOpen ? (
                <div className="echo-substat-popup">
                  <div className="echo-substat-popup-head">
                    <span className="hint">单击词条可选择/取消</span>
                    <button
                      type="button"
                      onClick={() => setEchoFilters((prev) => ({ ...prev, substatStatKeys: [], substatTiers: {} }))}
                      disabled={echoFilters.substatStatKeys.length === 0}
                    >
                      清空
                    </button>
                  </div>
                  <div className="echo-substat-chip-list">
                    {statDefs.map((stat) => {
                      const selected = echoFilters.substatStatKeys.includes(stat.statKey);
                      return (
                        <div
                          key={stat.statKey}
                          style={{ display: "inline-flex", alignItems: "center", gap: "2px" }}
                        >
                          <button
                            type="button"
                            className={selected ? "echo-substat-chip active" : "echo-substat-chip"}
                            onClick={() =>
                              setEchoFilters((prev) => {
                                const nextTiers = { ...prev.substatTiers };
                                if (selected) delete nextTiers[stat.statKey];
                                return {
                                  ...prev,
                                  substatStatKeys: selected
                                    ? prev.substatStatKeys.filter((statKey) => statKey !== stat.statKey)
                                    : [...prev.substatStatKeys, stat.statKey],
                                  substatTiers: nextTiers,
                                };
                              })
                            }
                          >
                            {stat.displayName}
                          </button>
                          {selected ? (
                            <div className="echo-tier-selector-container" style={{ position: "relative", display: "inline-flex", alignItems: "center" }}>
                              <button
                                type="button"
                                className={`echo-substat-lv-btn ${echoFilters.substatTiers[stat.statKey]?.length || activeTierSelectStat === stat.statKey ? "active" : ""}`}
                                style={{
                                  padding: "0 6px",
                                  minWidth: "24px",
                                  height: "24px",
                                  fontSize: "10px",
                                  marginLeft: "4px",
                                  display: "flex",
                                  alignItems: "center",
                                  justifyContent: "center",
                                  borderRadius: "12px",
                                  border: (echoFilters.substatTiers[stat.statKey]?.length || activeTierSelectStat === stat.statKey)
                                    ? "1px solid var(--accent)"
                                    : "1px dashed #64748b",
                                  background: (echoFilters.substatTiers[stat.statKey]?.length || activeTierSelectStat === stat.statKey)
                                    ? "#eff6ff"
                                    : "#f8fafc",
                                  color: (echoFilters.substatTiers[stat.statKey]?.length || activeTierSelectStat === stat.statKey)
                                    ? "var(--accent-ink)"
                                    : "#64748b",
                                  cursor: "pointer",
                                  transition: "all 0.2s cubic-bezier(0.4, 0, 0.2, 1)",
                                  boxSizing: "border-box",
                                  flexShrink: 0,
                                }}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  if (activeTierSelectStat === stat.statKey) {
                                    setActiveTierSelectStat(null);
                                  } else {
                                    const rect = e.currentTarget.getBoundingClientRect();
                                    setTierSelectorPos({ top: rect.bottom + 6, left: rect.left });
                                    setActiveTierSelectStat(stat.statKey);
                                  }
                                }}
                              >
                                {echoFilters.substatTiers[stat.statKey]?.length
                                  ? echoFilters.substatTiers[stat.statKey].join(",")
                                  : "Lv"}
                              </button>

                              {activeTierSelectStat === stat.statKey && (
                                <div
                                  className="echo-tier-selector"
                                  onClick={(e) => e.stopPropagation()}
                                  style={{
                                    position: "fixed",
                                    top: tierSelectorPos.top,
                                    left: tierSelectorPos.left,
                                    zIndex: 100000,
                                    display: "inline-flex",
                                    alignItems: "center",
                                    gap: "4px",
                                    background: "var(--bg-card, #fff)",
                                    padding: "4px 6px",
                                    borderRadius: "20px",
                                    border: "1px solid var(--border, #ccc)",
                                    boxShadow: "0 4px 15px rgba(0,0,0,0.15)",
                                  }}
                                >
                                  <div style={{ display: "inline-flex", gap: "2px" }}>
                                    {(() => {
                                      const maxTier = (stat.statKey === "atk_flat" || stat.statKey === "def_flat" || stat.statKey === "hp_flat") ? 4 : 8;
                                      return Array.from({ length: maxTier }, (_, i) => i + 1).map((t) => {
                                        const tiers = echoFilters.substatTiers[stat.statKey] || [];
                                        const isTSelected = tiers.includes(t);
                                        return (
                                          <button
                                            key={t}
                                            type="button"
                                            onClick={(e) => {
                                              e.stopPropagation();
                                              setEchoFilters((prev) => {
                                                const currentTiers = prev.substatTiers[stat.statKey] || [];
                                                const nextTiers = currentTiers.includes(t)
                                                  ? currentTiers.filter((v) => v !== t)
                                                  : [...currentTiers, t].sort();
                                                return {
                                                  ...prev,
                                                  substatTiers: {
                                                    ...prev.substatTiers,
                                                    [stat.statKey]: nextTiers,
                                                  },
                                                };
                                              });
                                            }}
                                            style={{
                                              width: "24px",
                                              height: "24px",
                                              fontSize: "12px",
                                              padding: 0,
                                              display: "flex",
                                              alignItems: "center",
                                              justifyContent: "center",
                                              border: "none",
                                              borderRadius: "50%",
                                              cursor: "pointer",
                                              backgroundColor: isTSelected ? "var(--btn-primary-bg, #3b82f6)" : "transparent",
                                              color: isTSelected ? "#fff" : "var(--text-main, #334155)",
                                              fontWeight: isTSelected ? "bold" : "normal",
                                              transition: "background 0.2s",
                                            }}
                                          >
                                            {t}
                                          </button>
                                        );
                                      });
                                    })()}
                                  </div>
                                  <button
                                    type="button"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      setEchoFilters((prev) => {
                                        const nextTiers = { ...prev.substatTiers };
                                        delete nextTiers[stat.statKey];
                                        return { ...prev, substatTiers: nextTiers };
                                      });
                                    }}
                                    title="重置档位"
                                    style={{
                                      display: "flex",
                                      alignItems: "center",
                                      justifyContent: "center",
                                      width: "20px",
                                      height: "20px",
                                      borderRadius: "50%",
                                      background: "var(--bg-app, #f1f5f9)",
                                      border: "none",
                                      cursor: "pointer",
                                      padding: 0,
                                      marginLeft: "2px",
                                    }}
                                  >
                                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                                      <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"></path>
                                      <path d="M3 3v5h5"></path>
                                    </svg>
                                  </button>
                                  <button
                                    type="button"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      setActiveTierSelectStat(null);
                                    }}
                                    style={{
                                      display: "flex",
                                      alignItems: "center",
                                      justifyContent: "center",
                                      width: "20px",
                                      height: "20px",
                                      borderRadius: "50%",
                                      background: "#569d79",
                                      border: "none",
                                      cursor: "pointer",
                                      padding: 0,
                                      marginLeft: "2px",
                                    }}
                                  >
                                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                                      <polyline points="20 6 9 17 4 12"></polyline>
                                    </svg>
                                  </button>
                                </div>
                              )}
                            </div>
                          ) : null}
                        </div>
                      );
                    })}
                  </div>
                </div>
              ) : null}
            </div>
            <label>
              排序
              <select
                className="echo-toolbar-control"
                value={echoFilters.sortBy}
                onChange={(e) =>
                  setEchoFilters((prev) => ({
                    ...prev,
                    sortBy: sanitizeEchoSortBy(e.target.value),
                  }))
                }
              >
                {ECHO_SORT_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>
        <div className="echo-table-scroll">
        <table className="table echo-table">
          <colgroup>
            <col className="echo-col-select" style={{ width: `${columnWidths.select}px` }} />
            <col className="echo-col-nickname" style={{ width: `${columnWidths.nickname}px` }} />
            <col className="echo-col-main" style={{ width: `${columnWidths.main}px` }} />
            <col className="echo-col-cost" style={{ width: `${columnWidths.cost}px` }} />
            <col className="echo-col-status" style={{ width: `${columnWidths.status}px` }} />
            <col className="echo-col-slots" style={{ width: `${columnWidths.slots}px` }} />
            <col className="echo-actions-col" style={{ width: `${columnWidths.actions}px` }} />
          </colgroup>
          <thead>
            <tr>
              <th className="echo-col-select">
                <div className="echo-cell echo-select-cell">
                  {batchPanelExpanded ? (
                    <input
                      ref={selectAllCheckboxRef}
                      type="checkbox"
                      checked={allFilteredSelected}
                      onChange={(e) => onToggleSelectAllVisible(e.target.checked)}
                      disabled={filteredEchoes.length === 0}
                      title="全选当前筛选结果"
                    />
                  ) : (
                    <span className="echo-select-placeholder" aria-hidden="true" />
                  )}
                </div>
              </th>
              <th className="echo-col-nickname">昵称</th>
              <th className="echo-col-main">主词条</th>
              <th className="echo-col-cost">Cost</th>
              <th className="echo-col-status">状态</th>
              <th className="echo-col-slots">槽位</th>
              <th className="echo-actions-col">操作</th>
            </tr>
          </thead>
          <tbody>
            {filteredEchoes.map((echo) => {
              const editing = editingEchoId === echo.echoId;
              const selected = selectedEchoIdSet.has(echo.echoId);
              const rowClassName = [editing ? "active-row" : "", batchPanelExpanded && selected ? "selected-row" : ""]
                .filter((x) => x)
                .join(" ");

              return (
                <Fragment key={echo.echoId}>
                  <tr
                    key={`${echo.echoId}-row`}
                    className={rowClassName || undefined}
                    onMouseDown={onEchoRowMouseDown}
                    onClick={(event) => onEchoRowClick(event, echo.echoId)}
                  >
                    <td className="echo-col-select">
                      <div className="echo-cell echo-select-cell">
                        {batchPanelExpanded ? (
                          <input
                            type="checkbox"
                            checked={selected}
                            onClick={(event) => onEchoCheckboxClick(event, echo.echoId)}
                            readOnly
                            title="选择该声骸"
                          />
                        ) : (
                          <span className="echo-select-placeholder" aria-hidden="true" />
                        )}
                      </div>
                    </td>
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
                      <div className="echo-cell">
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
                                  {slot.slotNo}: {statKeyToAbbr(slot.statKey)}{slot.tierIndex}={formatScaledValue(statMap.get(slot.statKey)?.unit ?? "flat", slot.valueScaled)}
                                </span>
                              ))
                          )}
                        </div>
                      </div>
                    </td>
                    <td className="echo-actions-col">
                      <div className="inline-row echo-actions-inline">
                        <button
                          type="button"
                          className={selectedEchoId === echo.echoId ? "echo-action-btn manage-btn-active" : "echo-action-btn"}
                          onClick={(e) => {
                            e.stopPropagation();
                            setSelectedEchoId(echo.echoId);
                          }}
                          disabled={saving}
                          title={selectedEchoId === echo.echoId ? "当前已选中此声骸" : "将此声骸设为记录板与分析目标"}
                        >
                          选中
                        </button>
                        <button
                          type="button"
                          className={editing ? "echo-action-btn manage-btn-active" : "echo-action-btn"}
                          onClick={(e) => {
                            e.stopPropagation();
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
                            <button type="button" className="echo-action-btn" onClick={cancelEdit} disabled={saving}>
                              取消
                            </button>
                            <button
                              type="button"
                              className="echo-action-btn"
                              onClick={() => void saveEdit()}
                              disabled={saving}
                            >
                              保存
                            </button>
                          </>
                        ) : null}
                        <button
                          type="button"
                          className="echo-action-btn"
                          onClick={() => void removeEcho(echo.echoId)}
                          disabled={saving}
                        >
                          {pendingDeleteEchoId === echo.echoId ? "确认删除" : "删除"}
                        </button>
                      </div>
                    </td>
                  </tr>
                  {editing && editingEcho && basicDraft ? (
                    <tr key={`${echo.echoId}-editor`} className="active-row">
                      <td colSpan={tableColumnCount}>
                        <div className="inline-edit-panel">
                          <div className="chain-block">
                            <span className="chain-label">期望词条</span>
                            <div className="chain-row" ref={expectationRowRef}>
                              {expectationStats.length === 0 ? <span className="chain-empty">无</span> : null}
                              {expectationStats.map((statKey, idx) => {
                                const stat = statMap.get(statKey);
                                const selected = activeExpectationIndex === idx;
                                const isDraggingThis = draggingExpectationFromIndex === idx;
                                const classNames = ["chain-item"];
                                if (selected) classNames.push("active");
                                if (isDraggingThis) classNames.push("dragging");

                                const availableStats = statDefs.filter(
                                  (x) => x.statKey === statKey || !expectationStats.includes(x.statKey),
                                );
                                const hideOperator =
                                  draggingExpectationFromIndex !== null && idx === draggingExpectationFromIndex;

                                return (
                                  <Fragment key={`exp-${idx}-${statKey}`}>
                                    {expectationInsertBeforeIndex === idx ? (
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
                                            label: stat?.displayName ?? statKey,
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
                                            onTap: () => setActiveExpectationIndex(idx),
                                          });
                                        }}
                                        onContextMenu={(e) => {
                                          e.preventDefault();
                                          removeExpectationAt(idx);
                                        }}
                                        title="长按拖动，点击编辑，右键删除"
                                      >
                                        {selected ? (
                                          <select
                                            value={statKey}
                                            onChange={(e) => {
                                              const next = e.target.value;
                                              setExpectationStats((prev) =>
                                                prev.map((item, itemIdx) => (itemIdx === idx ? next : item)),
                                              );
                                            }}
                                            onPointerDown={(e) => e.stopPropagation()}
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
                                      {idx < expectationOps.length && !hideOperator ? (
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
                                  </Fragment>
                                );
                              })}
                              {dragState?.kind === "expectation" && expectationInsertBeforeIndex === null ? (
                                <span className="drag-insert-line" aria-hidden="true" />
                              ) : null}
                              <button type="button" className="chain-add" onClick={addExpectation}>
                                +
                              </button>
                              <div className="preset-selector" ref={presetSelectorRef}>
                                <button
                                  type="button"
                                  ref={presetSelectorButtonRef}
                                  className={presetSelectorOpen ? "manage-btn-active" : ""}
                                  onClick={() => {
                                    setPresetSelectorOpen((prev) => {
                                      const next = !prev;
                                      if (next) {
                                        window.requestAnimationFrame(() => syncPresetPopoverPosition());
                                      }
                                      return next;
                                    });
                                    setPresetNamingOpen(false);
                                  }}
                                  disabled={saving}
                                >
                                  预设：{selectedPresetName}
                                </button>
                              </div>
                            </div>
                          </div>

                          <div className="chain-block">
                            <span className="chain-label">槽位状态</span>
                            <div className="chain-row" ref={slotRowRef}>
                              {editingOrderedSlots.length === 0 && slotsDraft.length === 0 ? (
                                <span className="chain-empty">无</span>
                              ) : null}
                              {[1, 2, 3, 4, 5].map((slotNo) => {
                                const lockedSlot = editingOrderedSlots.find((s) => s.slotNo === slotNo);
                                if (lockedSlot) {
                                  return (
                                    <div
                                      key={`ordered-slot-${lockedSlot.slotNo}`}
                                      className="chain-item locked"
                                      title="顺序事件槽位（仅显示，不可编辑）"
                                    >
                                      <span>
                                        {slotNo}: {statKeyToAbbr(lockedSlot.statKey)}{lockedSlot.tierIndex}={formatScaledValue(statMap.get(lockedSlot.statKey)?.unit ?? "flat", statMap.get(lockedSlot.statKey)?.tiers.find((t) => t.tierIndex === lockedSlot.tierIndex)?.valueScaled ?? 0)}
                                      </span>
                                    </div>
                                  );
                                }

                                const idx = editingEditableSlots.indexOf(slotNo);
                                if (idx === -1 || idx >= slotsDraft.length) return null;

                                const slot = slotsDraft[idx];
                                const stat = statMap.get(slot.statKey);
                                const selected = activeSlotIndex === idx;
                                const isDraggingThis = draggingSlotFromIndex === idx;
                                const classNames = ["chain-item"];
                                if (selected) classNames.push("active");
                                if (isDraggingThis) classNames.push("dragging");

                                const currentUsed = slotsDraft.map((x) => x.statKey);
                                const availableStats = statDefs.filter(
                                  (x) =>
                                    x.statKey === slot.statKey ||
                                    (!currentUsed.includes(x.statKey) && !editingOrderedStatSet.has(x.statKey)),
                                );
                                const tiers = statMap.get(slot.statKey)?.tiers ?? [];
                                const previewSlotNo = slotNo;

                                return (
                                  <Fragment key={`slot-${idx}-${slot.statKey}`}>
                                    {slotInsertBeforeIndex === idx ? (
                                      <span className="drag-insert-line" aria-hidden="true" />
                                    ) : null}
                                    <div
                                      className={classNames.join(" ")}
                                      data-drag-kind="slot"
                                      data-drag-index={idx}
                                      onPointerDown={(e) => {
                                        beginLongPressDrag<DragKind, DragState>({
                                          event: e,
                                          kind: "slot",
                                          fromIndex: idx,
                                          label: `S${previewSlotNo} ${statKeyToAbbr(slot.statKey)}${slot.tierIndex}`,
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
                                          onTap: () => setActiveSlotIndex(idx),
                                        });
                                      }}
                                      onContextMenu={(e) => {
                                        e.preventDefault();
                                        removeSlotAt(idx);
                                      }}
                                      title="长按拖动，点击编辑，右键删除"
                                    >
                                      {selected ? (
                                        <>
                                          <span className="slot-label">S{previewSlotNo}</span>
                                          <div className="inline-row" onPointerDown={(e) => e.stopPropagation()}>
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
                                        </>
                                      ) : (
                                        <span>
                                          {previewSlotNo}: {statKeyToAbbr(slot.statKey)}{slot.tierIndex}={formatScaledValue(stat?.unit ?? "flat", stat?.tiers.find((t) => t.tierIndex === slot.tierIndex)?.valueScaled ?? 0)}
                                        </span>
                                      )}
                                    </div>
                                  </Fragment>
                                );
                              })}
                              {dragState?.kind === "slot" && slotInsertBeforeIndex === null ? (
                                <span className="drag-insert-line" aria-hidden="true" />
                              ) : null}
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
            {filteredEchoes.length === 0 ? (
              <tr>
                <td colSpan={tableColumnCount} className="chain-empty">
                  当前筛选条件下无声骸。
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
        </div>
      </div>
    </section>
  );
}
