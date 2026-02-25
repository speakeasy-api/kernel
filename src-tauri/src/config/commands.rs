use std::path::Path;

use tracing::{debug, error, info, instrument};

use super::loader::load_config;
use super::types::KernelConfig;
use crate::modes::builtin::builtin_modes;
use crate::modes::types::Mode;

#[tauri::command]
#[instrument(skip_all, fields(project_root = %project_root))]
pub fn load_project_config(project_root: String) -> Result<KernelConfig, String> {
    info!("loading project config");
    load_config(Path::new(&project_root)).map_err(|e| {
        error!(error = %e, "failed to load project config");
        e.to_string()
    })
}

#[tauri::command]
#[instrument]
pub fn get_builtin_modes() -> Vec<Mode> {
    debug!("returning builtin modes");
    builtin_modes()
}
