CREATE INDEX IF NOT EXISTS idx_ordered_events_event_time_created_seq_event_id
  ON ordered_events(event_time, created_seq, event_id);

CREATE INDEX IF NOT EXISTS idx_ordered_events_game_day
  ON ordered_events(game_day);

CREATE INDEX IF NOT EXISTS idx_event_edit_logs_event_id
  ON event_edit_logs(event_id);
