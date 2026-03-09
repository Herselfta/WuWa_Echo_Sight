CREATE TABLE IF NOT EXISTS pattern_prediction_runs (
  run_id TEXT PRIMARY KEY,
  context_hash TEXT NOT NULL UNIQUE,
  game_day TEXT NOT NULL,
  seq_len INTEGER NOT NULL,
  mode TEXT NOT NULL,
  weights_json TEXT NOT NULL,
  predictions_json TEXT NOT NULL,
  context_json TEXT NOT NULL,
  actual_stat_key TEXT,
  actual_event_id TEXT,
  top1_hit INTEGER,
  top3_hit INTEGER,
  log_loss REAL,
  resolved_at TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pattern_prediction_runs_game_day_seq
  ON pattern_prediction_runs(game_day, seq_len);
CREATE INDEX IF NOT EXISTS idx_pattern_prediction_runs_resolved_at
  ON pattern_prediction_runs(resolved_at);

INSERT OR IGNORE INTO app_settings(key, value) VALUES
  ('pattern_backtest_samples', '96'),
  ('pattern_bucket_min_samples', '12'),
  ('pattern_model_mode', 'adaptive_v2'),
  ('pattern_online_learning', '1'),
  ('pattern_online_ewma_alpha', '0.12'),
  ('pattern_online_adjust_cap', '0.15');
