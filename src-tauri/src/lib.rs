mod analysis;
mod commands;
mod db;
mod domain;

use tauri::Manager;

use commands::analysis::{
    create_probability_snapshot, get_echoes_for_stat, get_global_distribution,
};
use commands::echo::{
    create_echo, delete_echo, delete_expectation_preset, list_echoes, list_expectation_presets,
    list_stat_defs, save_expectation_preset, set_expectations, update_echo, upsert_backfill_state,
};
use commands::event::{
    append_ordered_event, delete_ordered_event, edit_ordered_event, get_event_history,
};
use commands::export::{export_csv, import_data};
use commands::hypothesis::{
    get_category_streak_analysis, get_reversion_analysis, get_slot_stat_distribution,
    get_transition_matrix,
};
use commands::pattern::get_daily_pattern_decision;
use db::{init_database, AppState};

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

            app.manage(AppState { db_path });
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
            get_echoes_for_stat,
            create_probability_snapshot,
            export_csv,
            import_data,
            get_transition_matrix,
            get_slot_stat_distribution,
            get_category_streak_analysis,
            get_reversion_analysis,
            get_daily_pattern_decision,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
