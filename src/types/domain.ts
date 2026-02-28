export type EchoStatus = "tracking" | "paused" | "abandoned" | "completed";

export interface StatTier {
  tierIndex: number;
  valueScaled: number;
}

export interface StatDef {
  statKey: string;
  displayName: string;
  unit: "percent" | "flat";
  tiers: StatTier[];
}

export interface ExpectationItem {
  statKey: string;
  rank: number;
}

export interface ExpectationPreset {
  presetId: string;
  name: string;
  createdAt: string;
  updatedAt: string;
  items: ExpectationItem[];
}

export interface EchoSubstatSlot {
  slotNo: number;
  statKey: string;
  tierIndex: number;
  valueScaled: number;
  source: "ordered_event" | "backfill";
}

export interface EchoSummary {
  echoId: string;
  nickname: string | null;
  mainStatKey: string;
  costClass: number;
  status: EchoStatus;
  openedSlotsCount: number;
  createdAt: string;
  updatedAt: string;
  expectations: ExpectationItem[];
  currentSubstats: EchoSubstatSlot[];
}

export interface DistributionFilter {
  startTime?: string;
  endTime?: string;
  mainStatKey?: string;
  costClass?: number;
  status?: string;
}

export interface DistributionRow {
  statKey: string;
  displayName: string;
  unit: string;
  count: number;
  pGlobal: number;
  ciFreqLow: number;
  ciFreqHigh: number;
  bayesMean: number;
  bayesLow: number;
  bayesHigh: number;
}

export interface DistributionPayload {
  totalEvents: number;
  rows: DistributionRow[];
}

export interface EchoProbRow {
  echoId: string;
  nickname: string | null;
  mainStatKey: string;
  costClass: number;
  status: EchoStatus;
  openedSlotsCount: number;
  expectationRankMin: number;
  pNext: number;
  pFinal: number;
}

export interface EventRow {
  eventId: string;
  echoId: string;
  echoNickname: string | null;
  slotNo: number;
  statKey: string;
  tierIndex: number;
  valueScaled: number;
  eventTime: string;
  createdSeq: number;
  analysisSeq: number;
  gameDay: string;
  createdAt: string;
}

/* ═══ Hypothesis verification types ═══ */

export interface HypothesisFilter {
  costClass?: number;
  mainStatKey?: string;
  status?: string;
}

export interface TransitionCell {
  fromStat: string;
  toStat: string;
  count: number;
  expected: number;
  residual: number;
}

export interface TransitionMatrix {
  statKeys: [string, string][];
  cells: TransitionCell[];
  totalTransitions: number;
  chiSquared: number;
  degreesOfFreedom: number;
  pValue: number;
}

export interface SlotStatCell {
  slotNo: number;
  statKey: string;
  displayName: string;
  category: string;
  count: number;
  probability: number;
}

export interface SlotStatDistribution {
  statKeys: [string, string][];
  cells: SlotStatCell[];
  slotTotals: number[];
  totalEvents: number;
  chiSquared: number;
  degreesOfFreedom: number;
  pValue: number;
}

export interface CategoryStreakRow {
  echoId: string;
  category: string;
  startSlot: number;
  endSlot: number;
  length: number;
  stats: string[];
  tiers: number[];
  possibleZones: string[];
}

export interface CategoryStreakReport {
  streaks: CategoryStreakRow[];
  zoneTransitions: [string, string, number][];
  zoneVisits: [string, number][];
  tierTotalPairs: number;
  tierStopCount: number;
  tierStepCount: number;
  tierJumpCount: number;
  tierStopRatio: number;
  tierStepRatio: number;
  tierJumpRatio: number;
}
