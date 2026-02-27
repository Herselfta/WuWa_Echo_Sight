PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS stat_defs (
  stat_key TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  unit TEXT NOT NULL CHECK(unit IN ('percent', 'flat')),
  enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS stat_tiers (
  stat_key TEXT NOT NULL,
  tier_index INTEGER NOT NULL,
  value_scaled INTEGER NOT NULL,
  PRIMARY KEY (stat_key, tier_index),
  FOREIGN KEY (stat_key) REFERENCES stat_defs(stat_key) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS echoes (
  echo_id TEXT PRIMARY KEY,
  nickname TEXT,
  main_stat_key TEXT NOT NULL,
  cost_class INTEGER NOT NULL CHECK(cost_class IN (1, 3, 4)),
  status TEXT NOT NULL CHECK(status IN ('tracking', 'paused', 'abandoned', 'completed')),
  opened_slots_count INTEGER NOT NULL CHECK(opened_slots_count BETWEEN 0 AND 5),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS echo_expectations (
  echo_id TEXT NOT NULL,
  stat_key TEXT NOT NULL,
  rank INTEGER NOT NULL,
  PRIMARY KEY (echo_id, stat_key),
  FOREIGN KEY (echo_id) REFERENCES echoes(echo_id) ON DELETE CASCADE,
  FOREIGN KEY (stat_key) REFERENCES stat_defs(stat_key)
);

CREATE TABLE IF NOT EXISTS echo_current_substats (
  echo_id TEXT NOT NULL,
  slot_no INTEGER NOT NULL CHECK(slot_no BETWEEN 1 AND 5),
  stat_key TEXT NOT NULL,
  tier_index INTEGER NOT NULL,
  value_scaled INTEGER NOT NULL,
  source TEXT NOT NULL CHECK(source IN ('ordered_event', 'backfill')),
  event_id TEXT,
  PRIMARY KEY (echo_id, slot_no),
  UNIQUE (echo_id, stat_key),
  FOREIGN KEY (echo_id) REFERENCES echoes(echo_id) ON DELETE CASCADE,
  FOREIGN KEY (stat_key, tier_index) REFERENCES stat_tiers(stat_key, tier_index)
);

CREATE TABLE IF NOT EXISTS ordered_events (
  event_id TEXT PRIMARY KEY,
  echo_id TEXT NOT NULL,
  slot_no INTEGER NOT NULL CHECK(slot_no BETWEEN 1 AND 5),
  stat_key TEXT NOT NULL,
  tier_index INTEGER NOT NULL,
  value_scaled INTEGER NOT NULL,
  event_time TEXT NOT NULL,
  created_seq INTEGER NOT NULL UNIQUE,
  analysis_seq INTEGER NOT NULL UNIQUE,
  game_day TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(echo_id, slot_no),
  FOREIGN KEY (echo_id) REFERENCES echoes(echo_id) ON DELETE CASCADE,
  FOREIGN KEY (stat_key, tier_index) REFERENCES stat_tiers(stat_key, tier_index)
);

CREATE TABLE IF NOT EXISTS event_edit_logs (
  log_id TEXT PRIMARY KEY,
  event_id TEXT NOT NULL,
  before_json TEXT NOT NULL,
  after_json TEXT NOT NULL,
  reorder_mode TEXT NOT NULL CHECK(reorder_mode IN ('none', 'time_assist')),
  edited_at TEXT NOT NULL,
  FOREIGN KEY (event_id) REFERENCES ordered_events(event_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS probability_snapshots (
  snapshot_id TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  scope_json TEXT NOT NULL,
  distribution_json TEXT NOT NULL,
  echo_probs_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ordered_events_event_time ON ordered_events(event_time);
CREATE INDEX IF NOT EXISTS idx_ordered_events_analysis_seq ON ordered_events(analysis_seq);
CREATE INDEX IF NOT EXISTS idx_ordered_events_echo_id ON ordered_events(echo_id);

INSERT OR IGNORE INTO stat_defs(stat_key, display_name, unit, enabled) VALUES
  ('crit_rate', '暴击率', 'percent', 1),
  ('crit_dmg', '暴击伤害', 'percent', 1),
  ('energy_regen', '共鸣效率', 'percent', 1),
  ('atk_pct', '攻击力%', 'percent', 1),
  ('hp_pct', '生命值%', 'percent', 1),
  ('basic_dmg', '普攻伤害加成', 'percent', 1),
  ('heavy_dmg', '重击伤害加成', 'percent', 1),
  ('skill_dmg', '共鸣技能伤害加成', 'percent', 1),
  ('liberation_dmg', '共鸣解放伤害加成', 'percent', 1),
  ('def_pct', '防御力%', 'percent', 1),
  ('hp_flat', '固定生命', 'flat', 1),
  ('atk_flat', '固定攻击', 'flat', 1),
  ('def_flat', '固定防御', 'flat', 1);

INSERT OR IGNORE INTO stat_tiers(stat_key, tier_index, value_scaled) VALUES
  ('crit_rate', 1, 105), ('crit_rate', 2, 99), ('crit_rate', 3, 93), ('crit_rate', 4, 87), ('crit_rate', 5, 81), ('crit_rate', 6, 75), ('crit_rate', 7, 69), ('crit_rate', 8, 63),
  ('crit_dmg', 1, 210), ('crit_dmg', 2, 198), ('crit_dmg', 3, 186), ('crit_dmg', 4, 174), ('crit_dmg', 5, 162), ('crit_dmg', 6, 150), ('crit_dmg', 7, 138), ('crit_dmg', 8, 126),
  ('energy_regen', 1, 124), ('energy_regen', 2, 116), ('energy_regen', 3, 108), ('energy_regen', 4, 100), ('energy_regen', 5, 92), ('energy_regen', 6, 84), ('energy_regen', 7, 76), ('energy_regen', 8, 68),
  ('atk_pct', 1, 116), ('atk_pct', 2, 109), ('atk_pct', 3, 101), ('atk_pct', 4, 94), ('atk_pct', 5, 86), ('atk_pct', 6, 79), ('atk_pct', 7, 71), ('atk_pct', 8, 64),
  ('hp_pct', 1, 116), ('hp_pct', 2, 109), ('hp_pct', 3, 101), ('hp_pct', 4, 94), ('hp_pct', 5, 86), ('hp_pct', 6, 79), ('hp_pct', 7, 71), ('hp_pct', 8, 64),
  ('basic_dmg', 1, 116), ('basic_dmg', 2, 109), ('basic_dmg', 3, 101), ('basic_dmg', 4, 94), ('basic_dmg', 5, 86), ('basic_dmg', 6, 79), ('basic_dmg', 7, 71), ('basic_dmg', 8, 64),
  ('heavy_dmg', 1, 116), ('heavy_dmg', 2, 109), ('heavy_dmg', 3, 101), ('heavy_dmg', 4, 94), ('heavy_dmg', 5, 86), ('heavy_dmg', 6, 79), ('heavy_dmg', 7, 71), ('heavy_dmg', 8, 64),
  ('skill_dmg', 1, 116), ('skill_dmg', 2, 109), ('skill_dmg', 3, 101), ('skill_dmg', 4, 94), ('skill_dmg', 5, 86), ('skill_dmg', 6, 79), ('skill_dmg', 7, 71), ('skill_dmg', 8, 64),
  ('liberation_dmg', 1, 116), ('liberation_dmg', 2, 109), ('liberation_dmg', 3, 101), ('liberation_dmg', 4, 94), ('liberation_dmg', 5, 86), ('liberation_dmg', 6, 79), ('liberation_dmg', 7, 71), ('liberation_dmg', 8, 64),
  ('def_pct', 1, 147), ('def_pct', 2, 138), ('def_pct', 3, 128), ('def_pct', 4, 118), ('def_pct', 5, 109), ('def_pct', 6, 100), ('def_pct', 7, 90), ('def_pct', 8, 81),
  ('hp_flat', 1, 580), ('hp_flat', 2, 540), ('hp_flat', 3, 510), ('hp_flat', 4, 470), ('hp_flat', 5, 430), ('hp_flat', 6, 390), ('hp_flat', 7, 360), ('hp_flat', 8, 320),
  ('atk_flat', 1, 60), ('atk_flat', 2, 50), ('atk_flat', 3, 40), ('atk_flat', 4, 30),
  ('def_flat', 1, 70), ('def_flat', 2, 60), ('def_flat', 3, 50), ('def_flat', 4, 40);

INSERT OR IGNORE INTO app_settings(key, value) VALUES
  ('day_boundary_hour', '4'),
  ('analysis_window', '200'),
  ('mc_iterations', '20000'),
  ('smoothing_alpha', '1.0'),
  ('confidence_level', '0.95');
