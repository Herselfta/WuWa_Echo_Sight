ALTER TABLE pattern_prediction_runs ADD COLUMN actual_tier_index INTEGER;

INSERT OR IGNORE INTO app_settings(key, value) VALUES ('pattern_model_mode', 'adaptive_v2');
