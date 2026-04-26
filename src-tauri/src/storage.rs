use std::{fs, io::ErrorKind, path::PathBuf};

use tauri::{AppHandle, Manager};

use crate::models::{OverlaySettings, RecentPlaySnapshot};

const RECENT_PLAYS_FILE: &str = "recent-plays.json";
const RECENT_PLAY_LIMIT: usize = 5;
const OVERLAY_SETTINGS_FILE: &str = "overlay-settings.json";

fn app_storage_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;

    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

    Ok(dir)
}

fn recent_plays_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_storage_dir(app)?.join(RECENT_PLAYS_FILE))
}

fn overlay_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_storage_dir(app)?.join(OVERLAY_SETTINGS_FILE))
}

pub fn load_recent_plays(app: &AppHandle) -> Vec<RecentPlaySnapshot> {
    let path = match recent_plays_path(app) {
        Ok(path) => path,
        Err(error) => {
            log::warn!("Failed to resolve recent plays path: {error}");
            return Vec::new();
        }
    };

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            log::warn!("Failed to read recent plays: {error}");
            return Vec::new();
        }
    };

    let mut plays: Vec<RecentPlaySnapshot> = match serde_json::from_slice(&bytes) {
        Ok(plays) => plays,
        Err(error) => {
            log::warn!("Failed to parse recent plays: {error}");
            return Vec::new();
        }
    };

    plays.truncate(RECENT_PLAY_LIMIT);
    plays
}

pub fn save_recent_plays(
    app: &AppHandle,
    recent_plays: &[RecentPlaySnapshot],
) -> Result<(), String> {
    let path = recent_plays_path(app)?;
    let payload = serde_json::to_vec_pretty(recent_plays).map_err(|error| error.to_string())?;

    fs::write(path, payload).map_err(|error| error.to_string())
}

pub fn load_overlay_settings(app: &AppHandle) -> OverlaySettings {
    let path = match overlay_settings_path(app) {
        Ok(path) => path,
        Err(error) => {
            log::warn!("Failed to resolve overlay settings path: {error}");
            return OverlaySettings::default();
        }
    };

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return OverlaySettings::default(),
        Err(error) => {
            log::warn!("Failed to read overlay settings: {error}");
            return OverlaySettings::default();
        }
    };

    match serde_json::from_slice::<OverlaySettings>(&bytes) {
        Ok(settings) => settings.normalized(),
        Err(error) => {
            log::warn!("Failed to parse overlay settings: {error}");
            OverlaySettings::default()
        }
    }
}

pub fn save_overlay_settings(app: &AppHandle, settings: &OverlaySettings) -> Result<(), String> {
    let path = overlay_settings_path(app)?;
    let payload = serde_json::to_vec_pretty(&settings.clone().normalized())
        .map_err(|error| error.to_string())?;

    fs::write(path, payload).map_err(|error| error.to_string())
}
