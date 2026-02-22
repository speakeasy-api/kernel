pub mod agent;
mod compaction;
mod config;
mod db;
mod events;
mod git;
mod modes;
mod prompt_router;
mod tasks;
mod ux_agent;

use std::sync::Mutex;

pub struct DbState(pub Mutex<rusqlite::Connection>);

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = ".kernel/kernel.db";
    std::fs::create_dir_all(".kernel").expect("failed to create .kernel dir");
    let conn = rusqlite::Connection::open(db_path).expect("failed to open database");
    tasks::db::init_schema(&conn).expect("failed to init task schema");

    tauri::Builder::default()
        .manage(DbState(Mutex::new(conn)))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            tasks::commands::create_task,
            tasks::commands::persist_task_plan,
            tasks::commands::list_tasks,
            tasks::commands::get_task,
            tasks::commands::update_task_status,
            tasks::commands::get_task_tree,
            tasks::commands::next_unblocked,
            tasks::commands::set_task_engagement,
            tasks::commands::get_task_cost,
            tasks::commands::get_session_cost
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
