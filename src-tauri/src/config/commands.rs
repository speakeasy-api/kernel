use std::path::Path;

use super::loader::load_config;
use super::types::KernelConfig;
use crate::modes::builtin::builtin_modes;
use crate::modes::types::Mode;

#[tauri::command]
pub fn load_project_config(project_root: String) -> Result<KernelConfig, String> {
    load_config(Path::new(&project_root)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_builtin_modes() -> Vec<Mode> {
    builtin_modes()
}
