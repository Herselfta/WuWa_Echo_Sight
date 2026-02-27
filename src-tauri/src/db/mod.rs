use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use rusqlite::Connection;

pub const DEFAULT_DAY_BOUNDARY_HOUR: i64 = 4;

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
}

pub fn init_database(db_path: &Path) -> Result<(), String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create app data dir: {e}"))?;
    }

    let conn = Connection::open(db_path).map_err(|e| format!("failed to open db: {e}"))?;
    conn.execute_batch(include_str!("migrations/001_init.sql"))
        .map_err(|e| format!("failed to run migrations: {e}"))?;
    Ok(())
}

pub fn open_connection(state: &AppState) -> Result<Connection, String> {
    let conn = Connection::open(&state.db_path).map_err(|e| format!("failed to open db: {e}"))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("failed to set pragmas: {e}"))?;
    Ok(conn)
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn parse_event_time(input: &str) -> Result<DateTime<chrono::FixedOffset>, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt);
    }

    let naive_patterns = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ];

    for pattern in naive_patterns {
        if let Ok(naive) = NaiveDateTime::parse_from_str(input, pattern) {
            let local_dt = Local
                .from_local_datetime(&naive)
                .single()
                .or_else(|| Local.from_local_datetime(&naive).earliest())
                .ok_or_else(|| "failed to resolve local datetime".to_string())?;
            return Ok(local_dt.fixed_offset());
        }
    }

    Err(format!("unsupported event_time format: {input}"))
}

pub fn compute_game_day(event_time: &str, boundary_hour: i64) -> Result<String, String> {
    let dt = parse_event_time(event_time)?;
    let adjusted = dt - chrono::Duration::hours(boundary_hour);
    Ok(adjusted.format("%Y-%m-%d").to_string())
}

pub fn get_setting_i64(conn: &Connection, key: &str, default: i64) -> i64 {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse::<i64>().ok())
    .unwrap_or(default)
}

pub fn get_setting_f64(conn: &Connection, key: &str, default: f64) -> f64 {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse::<f64>().ok())
    .unwrap_or(default)
}

pub fn get_tier_value(conn: &Connection, stat_key: &str, tier_index: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT value_scaled FROM stat_tiers WHERE stat_key = ?1 AND tier_index = ?2",
        rusqlite::params![stat_key, tier_index],
        |row| row.get(0),
    )
    .map_err(|_| format!("invalid tier for stat_key={stat_key} tier_index={tier_index}"))
}

#[cfg(test)]
mod tests {
    use super::compute_game_day;

    #[test]
    fn game_day_respects_4am_boundary() {
        let before_boundary =
            compute_game_day("2026-02-27T03:59:00+08:00", 4).expect("compute_game_day should work");
        let after_boundary =
            compute_game_day("2026-02-27T04:01:00+08:00", 4).expect("compute_game_day should work");

        assert_eq!(before_boundary, "2026-02-26");
        assert_eq!(after_boundary, "2026-02-27");
    }
}
