import { create } from "zustand";
import {
  getEchoesForStat,
  getGlobalDistribution,
  listEchoes,
  listExpectationPresets,
  listStatDefs,
} from "../api/tauri";
import type {
  DistributionFilter,
  DistributionPayload,
  EchoProbRow,
  EchoSummary,
  ExpectationPreset,
  StatDef,
} from "../types/domain";

interface AppState {
  statDefs: StatDef[];
  echoes: EchoSummary[];
  expectationPresets: ExpectationPreset[];
  distribution: DistributionPayload | null;
  selectedStatKey: string | null;
  selectedEchoId: string;
  echoProbRows: EchoProbRow[];
  distributionFilter: DistributionFilter;
  loading: boolean;
  error: string | null;
  setDistributionFilter: (patch: Partial<DistributionFilter>) => void;
  setSelectedStatKey: (key: string | null) => void;
  setSelectedEchoId: (id: string) => void;
  loadBootData: () => Promise<void>;
  refreshEchoes: () => Promise<void>;
  refreshDistribution: () => Promise<void>;
  refreshEchoProbRows: (sortBy?: string) => Promise<void>;
  refreshExpectationPresets: () => Promise<void>;
}

export const useAppStore = create<AppState>((set, get) => ({
  statDefs: [],
  echoes: [],
  expectationPresets: [],
  distribution: null,
  selectedStatKey: null,
  selectedEchoId: "",
  echoProbRows: [],
  distributionFilter: {},
  loading: false,
  error: null,
  setDistributionFilter: (patch) => {
    set((state) => ({
      distributionFilter: {
        ...state.distributionFilter,
        ...patch,
      },
    }));
  },
  setSelectedStatKey: (key) => set({ selectedStatKey: key }),
  setSelectedEchoId: (id) => set({ selectedEchoId: id }),
  loadBootData: async () => {
    set({ loading: true, error: null });
    try {
      const [statDefs, echoes, distribution, expectationPresets] = await Promise.all([
        listStatDefs(),
        listEchoes(),
        getGlobalDistribution({}),
        listExpectationPresets(),
      ]);
      set({
        statDefs,
        echoes,
        distribution,
        expectationPresets,
      });
    } catch (error) {
      set({ error: String(error) });
    } finally {
      set({ loading: false });
    }
  },
  refreshEchoes: async () => {
    try {
      const echoes = await listEchoes();
      set({ echoes, error: null });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  refreshDistribution: async () => {
    try {
      const distribution = await getGlobalDistribution(get().distributionFilter);
      set({ distribution, error: null });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  refreshEchoProbRows: async (sortBy) => {
    const statKey = get().selectedStatKey;
    if (!statKey) {
      set({ echoProbRows: [] });
      return;
    }

    try {
      const rows = await getEchoesForStat({
        statKey,
        sortBy,
        ...get().distributionFilter,
      });
      set({ echoProbRows: rows, error: null });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  refreshExpectationPresets: async () => {
    try {
      const expectationPresets = await listExpectationPresets();
      set({ expectationPresets, error: null });
    } catch (error) {
      set({ error: String(error) });
    }
  },
}));
