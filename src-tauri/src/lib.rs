pub mod agent;
pub mod anthropic;
mod compaction;
pub mod config;
pub mod db;
mod events;
mod git;
pub mod modes;
pub mod prompt_router;
mod tasks;
pub mod tools;
mod ux_agent;

use std::sync::Arc;

use tauri::Manager;

use crate::prompt_router::model_registry::ModelRegistry;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Structured logging via tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kernel_lib=info".into()),
        )
        .with_target(true)
        .init();

    tracing::info!("kernel backend starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let pool = tauri::async_runtime::block_on(async { db::open_pool().await })
                .expect("failed to create database pool");
            app.manage(pool);

            // Model registry: cached OpenRouter model lists
            let registry = ModelRegistry::new();
            app.manage(Arc::clone(&registry));

            // Per-session cancellation flags
            app.manage(Arc::new(
                crate::prompt_router::commands::CancellationFlags::new(),
            ));

            // Spawn background task: warm cache eagerly + 24h periodic refresh
            tauri::async_runtime::spawn(async move {
                registry.ensure_warm().await;

                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
                // The first tick completes immediately — skip it since we just warmed
                interval.tick().await;

                loop {
                    interval.tick().await;
                    registry.refresh().await;
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Task commands
            tasks::commands::create_task,
            tasks::commands::persist_task_plan,
            tasks::commands::list_tasks,
            tasks::commands::get_task,
            tasks::commands::update_task_status,
            tasks::commands::get_task_tree,
            tasks::commands::next_unblocked,
            tasks::commands::set_task_engagement,
            tasks::commands::get_task_cost,
            tasks::commands::get_session_cost,
            // DB commands (sessions, events, modes)
            db::commands::create_session,
            db::commands::get_session,
            db::commands::list_sessions,
            db::commands::delete_session,
            db::commands::list_db_modes,
            db::commands::get_db_mode,
            db::commands::insert_event,
            db::commands::events_since,
            db::commands::get_attached_plan,
            db::commands::read_plan_content,
            // Config commands
            config::commands::load_project_config,
            config::commands::get_builtin_modes,
            // Prompt router commands
            prompt_router::commands::submit_prompt,
            prompt_router::commands::cancel_prompt,
            prompt_router::commands::revert_file,
            prompt_router::commands::get_conversation_context,
            prompt_router::commands::get_conversation_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
