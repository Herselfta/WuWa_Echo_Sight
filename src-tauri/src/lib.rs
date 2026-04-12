mod commands;
mod db;
mod domain;
mod pattern_state;
mod stats;

use rusqlite::OptionalExtension;
use tauri::{Emitter, Manager};

use commands::decision::get_daily_pattern_decision;
use commands::echo::{
    create_echo, delete_echo, delete_expectation_preset, list_echoes, list_expectation_presets,
    list_stat_defs, save_expectation_preset, set_expectations, update_echo, upsert_backfill_state,
};
use commands::event::{
    append_ordered_event, delete_ordered_event, edit_ordered_event, get_event_history,
};
use commands::export::{export_csv, import_data};
use commands::probability::{create_probability_snapshot, get_global_distribution};
use commands::verification::{
    get_category_streak_analysis, get_reversion_analysis, get_transition_matrix,
};
use db::{init_database, open_connection, AppState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
            std::fs::create_dir_all(&app_data_dir)
                .map_err(|e| format!("failed to create app data dir: {e}"))?;

            let db_path = app_data_dir.join("wuwa_echo_sight.sqlite3");
            init_database(&db_path)?;

            app.manage(AppState { db_path: db_path.clone() });

            let handle = app.handle().clone();
            let server_state = AppState { db_path: db_path.clone() };
            std::thread::spawn(move || {
                println!("[EchoSync-Server] Starting local server on 127.0.0.1:8192");
                
                // Maintain a persistent connection for the local server thread
                // Opening connection per request causes severe disk I/O overhead on Windows
                let mut conn = open_connection(&server_state).expect("HTTP server DB connection failed");
                
                if let Ok(server) = tiny_http::Server::http("127.0.0.1:8192") {
                    for mut request in server.incoming_requests() {
                        let mut content = String::new();
                        if request.as_reader().read_to_string(&mut content).is_ok() {
                            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&content) {
                                println!("[EchoSync-Server] Received sync payload");

                                // Directly insert into SQLite
                                if let Ok(tx) = conn.transaction() {
                                        // create an echo
                                        let echo_id = payload.get("echo_id").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("sync_{}", uuid::Uuid::new_v4()));
                                        let nickname = payload.get("nickname").and_then(|v| v.as_str());
                                        let main_stat_key = payload.get("main_stat_key").and_then(|v| v.as_str()).unwrap_or("crit_rate");
                                        let cost_class = payload.get("cost_class").and_then(|v| v.as_i64()).unwrap_or(4);
                                        let opened_slots = payload.get("opened_slots_count").and_then(|v| v.as_i64()).unwrap_or(0);
                                        let status_raw = payload.get("status").and_then(|v| v.as_str()).unwrap_or("tracking");
                                        let now = chrono::Utc::now().to_rfc3339();

                                        // Ensure status conforms to the database CHECK constraint
                                        let status_val = match status_raw {
                                            "tracking" | "paused" | "abandoned" | "completed" => status_raw,
                                            _ => "tracking"
                                        };

                                        let _ = tx.execute(
                                            "INSERT INTO echoes (echo_id, nickname, main_stat_key, cost_class, status, opened_slots_count, created_at, updated_at)
                                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                                             ON CONFLICT(echo_id) DO UPDATE SET
                                                nickname = excluded.nickname,
                                                status = CASE WHEN excluded.status = 'tracking' AND echoes.status IN ('completed', 'abandoned') THEN echoes.status ELSE excluded.status END,
                                                opened_slots_count = excluded.opened_slots_count,
                                                updated_at = ?7",
                                            rusqlite::params![echo_id, nickname, main_stat_key, cost_class, status_val, opened_slots, now],
                                        ).map_err(|e| eprintln!("[EchoSync-Server] Error inserting echo: {:?}", e));

                                        if let Some(substats) = payload.get("substats").and_then(|v| v.as_array()) {
                                            for sub in substats {
                                                let stat_key = sub.get("stat_key").and_then(|v| v.as_str()).unwrap_or("");
                                                if stat_key.is_empty() { continue; }
                                                let value_scaled = sub.get("value_scaled").and_then(|v| v.as_i64()).unwrap_or(0);
                                                let slot_no = sub.get("slot_no").and_then(|v| v.as_i64()).unwrap_or(1);

                                                // Check if this slot already exists (prevent overwriting)
                                                let exists_slot: i64 = tx.query_row(
                                                    "SELECT count(*) FROM echo_current_substats WHERE echo_id = ?1 AND slot_no = ?2",
                                                    rusqlite::params![echo_id, slot_no],
                                                    |row| row.get(0)
                                                ).unwrap_or(0);

                                                if exists_slot == 0 {
                                                    // Check duplicate stat key
                                                    let duplicate_stat: i64 = tx.query_row(
                                                        "SELECT count(*) FROM echo_current_substats WHERE echo_id = ?1 AND stat_key = ?2",
                                                        rusqlite::params![echo_id, stat_key],
                                                        |row| row.get(0)
                                                    ).unwrap_or(0);

                                                    if duplicate_stat == 0 {
                                                        // Map value_scaled to closest tier_index
                                                        let tier_match: Option<(i64, i64)> = tx.query_row(
                                                            "SELECT tier_index, value_scaled FROM stat_tiers WHERE stat_key = ? ORDER BY abs(value_scaled - ?) ASC LIMIT 1",
                                                            rusqlite::params![stat_key, value_scaled],
                                                            |row| Ok((row.get(0)?, row.get(1)?)),
                                                        ).optional().unwrap_or(None);
                                                        let Some((tier_index, true_value)) = tier_match else {
                                                            eprintln!("[EchoSync-Server] Skip unknown stat_key={stat_key} for echo {echo_id}");
                                                            continue;
                                                        };

                                                        let event_id = uuid::Uuid::new_v4().to_string();

                                                        let created_seq: i64 = tx.query_row("SELECT COALESCE(MAX(created_seq), 0) + 1 FROM ordered_events", [], |row| row.get(0)).unwrap_or(1);
                                                        let analysis_seq: i64 = tx.query_row("SELECT COALESCE(MAX(analysis_seq), 0) + 1 FROM ordered_events", [], |row| row.get(0)).unwrap_or(1);
                                                        let boundary: i64 = tx.query_row("SELECT CAST(value AS INTEGER) FROM app_settings WHERE key = 'day_boundary_hour'", [], |row| row.get(0)).unwrap_or(4);
                                                        let game_day = crate::db::compute_game_day(&now, boundary).unwrap_or_else(|_| "1970-01-01".to_string());

                                                        let _ = tx.execute(
                                                            "INSERT INTO ordered_events(event_id, echo_id, slot_no, stat_key, tier_index, value_scaled, event_time, created_seq, analysis_seq, game_day, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                                                            rusqlite::params![event_id, echo_id, slot_no, stat_key, tier_index, true_value, now, created_seq, analysis_seq, game_day, now]
                                                        ).map_err(|e| eprintln!("Event insert err: {:?}", e));

                                                        let _ = tx.execute(
                                                            "INSERT INTO echo_current_substats (echo_id, slot_no, stat_key, tier_index, value_scaled, source, event_id) VALUES (?1, ?2, ?3, ?4, ?5, 'ordered_event', ?6)",
                                                            rusqlite::params![echo_id, slot_no, stat_key, tier_index, true_value, event_id]
                                                        ).map_err(|e| eprintln!("Substats insert err: {:?}", e));
                                                    }
                                                }
                                            }
                                        }

                                        // Recompute opened slots and save status
                                        let max_slot: i64 = tx.query_row("SELECT COALESCE(MAX(slot_no), 0) FROM echo_current_substats WHERE echo_id = ?1", rusqlite::params![echo_id], |row| row.get(0)).unwrap_or(0);
                                        let _ = tx.execute(
                                            "UPDATE echoes SET opened_slots_count = ?2, status = CASE WHEN ?3 = 'tracking' AND status IN ('completed', 'abandoned') THEN status ELSE ?3 END, updated_at = ?4 WHERE echo_id = ?1",
                                            rusqlite::params![echo_id, max_slot, status_val, now]
                                        );

                                        if let Err(e) = tx.commit() {
                                            eprintln!("[EchoSync-Server] Error committing echo: {}", e);
                                        } else {
                                            println!("[EchoSync-Server] Successfully inserted echo {} to db", echo_id);
                                        }
                                    } else {
                                        eprintln!("[EchoSync-Server] Failed to begin transaction");
                                    }

                                if let Err(e) = handle.emit("echo_updated", payload) {
                                    eprintln!("[EchoSync-Server] Failed to emit: {}", e);
                                }
                                let _ = request.respond(tiny_http::Response::from_string("{\"status\":\"ok\"}").with_status_code(200));
                            } else {
                                let _ = request.respond(tiny_http::Response::from_string("{\"error\":\"Bad JSON\"}").with_status_code(400));
                            }
                        } else {
                            let _ = request.respond(tiny_http::Response::from_string("{\"error\":\"Bad Request\"}").with_status_code(400));
                        }
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_stat_defs,
            create_echo,
            update_echo,
            delete_echo,
            list_echoes,
            set_expectations,
            list_expectation_presets,
            save_expectation_preset,
            delete_expectation_preset,
            upsert_backfill_state,
            append_ordered_event,
            edit_ordered_event,
            delete_ordered_event,
            get_event_history,
            get_global_distribution,
            create_probability_snapshot,
            export_csv,
            import_data,
            get_transition_matrix,
            get_category_streak_analysis,
            get_reversion_analysis,
            get_daily_pattern_decision,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
