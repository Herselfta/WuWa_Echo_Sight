PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS expectation_presets (
  preset_id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS expectation_preset_items (
  preset_id TEXT NOT NULL,
  stat_key TEXT NOT NULL,
  rank INTEGER NOT NULL,
  PRIMARY KEY (preset_id, stat_key),
  FOREIGN KEY (preset_id) REFERENCES expectation_presets(preset_id) ON DELETE CASCADE,
  FOREIGN KEY (stat_key) REFERENCES stat_defs(stat_key)
);

CREATE INDEX IF NOT EXISTS idx_expectation_preset_items_preset_id
ON expectation_preset_items(preset_id);
