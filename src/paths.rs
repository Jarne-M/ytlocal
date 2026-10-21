//! Well-known locations. Everything lives under `%APPDATA%\YtLocal` except
//! in-progress downloads, which go to `%LOCALAPPDATA%\YtLocal\tmp` so large
//! partial files never end up in the roaming profile.

use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "YtLocal";

pub fn app_data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
}

pub fn bin_dir() -> PathBuf {
    app_data_dir().join("bin")
}

/// Where config.json and history.json live. `YTLOCAL_DATA_DIR` overrides it
/// (used by tests so they never touch the real settings).
fn settings_dir() -> PathBuf {
    std::env::var_os("YTLOCAL_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(app_data_dir)
}

pub fn config_path() -> PathBuf {
    settings_dir().join("config.json")
}

pub fn history_path() -> PathBuf {
    settings_dir().join("history.json")
}

pub fn temp_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
        .join("tmp")
}

/// Directory containing the running executable (first place we look for tools).
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
}
