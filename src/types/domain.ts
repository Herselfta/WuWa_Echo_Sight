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

/* ═══ Daily pattern decision (MVP) ═══ */

export interface DailyPatternDecisionFilter {
  gameDay?: string;
  manualStartIndex?: number;
  manualCycleLen?: number;
  manualGuessShapes?: string[];
  minLen?: number;
  maxLen?: number;
  minSupport?: number;
  maxOrder?: number;
  topK?: number;
}

export interface DailyExactPatternRow {
  length: number;
  pattern: string[];
  displayPattern: string[];
  shape: string;
  support: number;
  windowCount: number;
  expectedCount: number;
  lift: number;
  score: number;
}

export interface DailyShapePatternRow {
  length: number;
  shape: string;
  support: number;
  expectedCount: number;
  lift: number;
  score: number;
  examplePatterns: string[];
}

export interface AdaptiveNextSuggestion {
  statKey: string;
  displayName: string;
  probability: number;
  baseProbability: number;
  markovProbability: number;
  cycleProbability: number;
  motifBoost: number;
  matchedPatterns: string[];
}

export interface ManualGuessVerificationRow {
  guessShape: string;
  length: number;
  support: number;
  opportunities: number;
  hitRate: number;
  baselineRate: number;
  lift: number;
  matchedCycleIndices: number[];
  nextStatHint: string | null;
}

export interface ManualCycleSuggestion {
  statKey: string;
  displayName: string;
  count: number;
  probability: number;
}

export interface ManualPatternSummary {
  startIndex: number;
  cycleLen: number;
  fullCycles: number;
  nextCyclePos: number;
  topCycleShapes: [string, number][];
  guesses: ManualGuessVerificationRow[];
  positionSuggestions: ManualCycleSuggestion[];
}

export interface DailyPatternDecisionReport {
  gameDay: string;
  totalEvents: number;
  minLen: number;
  maxLen: number;
  minSupport: number;
  maxOrder: number;
  modelConfidence: number;
  exactPatterns: DailyExactPatternRow[];
  shapePatterns: DailyShapePatternRow[];
  suggestions: AdaptiveNextSuggestion[];
  manualSummary?: ManualPatternSummary | null;
  notes: string[];
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
  tierExpectedStopRatio: number | null;
  tierExpectedStepRatio: number | null;
  tierExpectedJumpRatio: number | null;
}

/* ═══ Reversion analysis ═══ */

export interface ReversionBucket {
  /** Count of this stat in the previous W events */
  prevWindowCount: number;
  /** How many samples fell into this bucket */
  sampleCount: number;
  /** Average occurrence rate in the NEXT W events: occurrences / W */
  meanNextFreq: number;
}

export interface StatReversionSeries {
  statKey: string;
  displayName: string;
  /** Global base frequency = total appearances / total events */
  baseFreq: number;
  totalCount: number;
  /** Cumulative frequency deviation at each global event position */
  deviations: number[];
  /** Inter-arrival gaps (in event count) between consecutive appearances */
  gaps: number[];
  meanGap: number;
  /** 1 / baseFreq (expected gap under i.i.d.) */
  expectedGap: number;
  gapVariance: number;
  /** Var/Mean; Geometric(p) baseline ≈ (1-p)/p */
  dispersionIndex: number;
  geometricDispersion: number;
  /** [lag, autocorrelation] pairs */
  lagAutocorrs: [number, number][];
  windowBuckets: ReversionBucket[];
}

export interface ReversionReport {
  totalEvents: number;
  /** All analysis_seq values in order (x-axis for deviation chart) */
  globalSeqs: number[];
  statSeries: StatReversionSeries[];
}
