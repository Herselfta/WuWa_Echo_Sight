import { invoke } from "@tauri-apps/api/core";
import type {
  DistributionFilter,
  DistributionPayload,
  EchoProbRow,
  EchoSummary,
  ExpectationPreset,
  EventRow,
  ExpectationItem,
  StatDef,
} from "../types/domain";

export async function listStatDefs(): Promise<StatDef[]> {
  return invoke("list_stat_defs");
}

export async function createEcho(input: {
  nickname?: string;
  mainStatKey: string;
  costClass: number;
  status?: string;
}): Promise<{ echoId: string }> {
  return invoke("create_echo", { input });
}

export async function updateEcho(input: {
  echoId: string;
  nickname?: string;
  mainStatKey?: string;
  costClass?: number;
  status?: string;
}): Promise<{ ok: boolean }> {
  return invoke("update_echo", { input });
}

export async function deleteEcho(echoId: string): Promise<{ ok: boolean }> {
  return invoke("delete_echo", { input: { echoId } });
}

export async function listEchoes(filter?: {
  status?: string;
  mainStatKey?: string;
  costClass?: number;
}): Promise<EchoSummary[]> {
  return invoke("list_echoes", { filter });
}

export async function setExpectations(echoId: string, items: ExpectationItem[]): Promise<{ ok: boolean }> {
  return invoke("set_expectations", { input: { echoId, items } });
}

export async function listExpectationPresets(): Promise<ExpectationPreset[]> {
  return invoke("list_expectation_presets");
}

export async function saveExpectationPreset(input: {
  presetId?: string;
  name: string;
  items: ExpectationItem[];
}): Promise<{ presetId: string }> {
  return invoke("save_expectation_preset", { input });
}

export async function deleteExpectationPreset(presetId: string): Promise<{ ok: boolean }> {
  return invoke("delete_expectation_preset", { input: { presetId } });
}

export async function upsertBackfillState(input: {
  echoId: string;
  slots: Array<{ slotNo: number; statKey: string; tierIndex: number }>;
}): Promise<{ ok: boolean }> {
  return invoke("upsert_backfill_state", { input });
}

export async function appendOrderedEvent(input: {
  echoId: string;
  slotNo: number;
  statKey: string;
  tierIndex: number;
  eventTime: string;
}): Promise<{ eventId: string }> {
  return invoke("append_ordered_event", { input });
}

export async function editOrderedEvent(input: {
  eventId: string;
  slotNo?: number;
  statKey?: string;
  tierIndex?: number;
  eventTime?: string;
  reorderMode?: "none" | "time_assist";
}): Promise<{ ok: boolean; affectedRange: string }> {
  return invoke("edit_ordered_event", { input });
}

export async function getEventHistory(filter?: {
  echoId?: string;
  limit?: number;
}): Promise<EventRow[]> {
  return invoke("get_event_history", { filter });
}

export async function getGlobalDistribution(filter?: DistributionFilter): Promise<DistributionPayload> {
  return invoke("get_global_distribution", { filter });
}

export async function getEchoesForStat(input: {
  statKey: string;
  sortBy?: string;
  startTime?: string;
  endTime?: string;
  mainStatKey?: string;
  costClass?: number;
  status?: string;
}): Promise<EchoProbRow[]> {
  return invoke("get_echoes_for_stat", { filter: input });
}

export async function createProbabilitySnapshot(input: {
  scope: DistributionFilter;
  statKey?: string;
}): Promise<{ snapshotId: string }> {
  return invoke("create_probability_snapshot", { input });
}

export async function exportCsv(input: {
  scope: DistributionFilter;
  includeSnapshots: boolean;
}): Promise<{ zipPath: string }> {
  return invoke("export_csv", { input });
}

export async function importData(zipPath: string): Promise<{ ok: boolean; importedTables: string[] }> {
  return invoke("import_data", { input: { zipPath } });
}
