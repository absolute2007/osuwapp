mod mock;
mod models;
mod overlay;
mod reader;
mod storage;

use base64::{engine::general_purpose, Engine as _};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};

use models::{AppSnapshot, OverlaySettings};

struct AppRuntimeState {
    live_reader_started: AtomicBool,
    shutting_down: AtomicBool,
    overlay_manager_running: Arc<AtomicBool>,
    overlay_settings: Arc<Mutex<OverlaySettings>>,
    latest_snapshot: Arc<Mutex<Option<AppSnapshot>>>,
}

impl Default for AppRuntimeState {
    fn default() -> Self {
        Self {
            live_reader_started: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            overlay_manager_running: Arc::new(AtomicBool::new(true)),
            overlay_settings: Arc::new(Mutex::new(OverlaySettings::default())),
            latest_snapshot: Arc::new(Mutex::new(None)),
        }
    }
}

#[tauri::command]
fn get_initial_snapshot(app: AppHandle) -> AppSnapshot {
    mock::searching_snapshot_with_recent(
        "Looking for osu!.exe (stable) and preparing live PP.",
        storage::load_recent_plays(&app),
    )
}

#[tauri::command]
fn start_live_updates(
    app: AppHandle,
    runtime_state: State<'_, AppRuntimeState>,
) -> Result<(), String> {
    if runtime_state
        .live_reader_started
        .swap(true, Ordering::SeqCst)
    {
        return Ok(());
    }

    reader::spawn_live_reader(app, runtime_state.latest_snapshot.clone());

    Ok(())
}

#[tauri::command]
fn get_overlay_settings(runtime_state: State<'_, AppRuntimeState>) -> OverlaySettings {
    runtime_state
        .overlay_settings
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn save_overlay_settings(
    app: AppHandle,
    runtime_state: State<'_, AppRuntimeState>,
    settings: OverlaySettings,
) -> Result<OverlaySettings, String> {
    let normalized = settings.normalized();

    storage::save_overlay_settings(&app, &normalized)?;

    {
        let mut guard = runtime_state
            .overlay_settings
            .lock()
            .map_err(|_| "Failed to lock overlay settings".to_string())?;

        *guard = normalized.clone();
    }

    let _ = app.emit("overlay-settings-updated", &normalized);

    Ok(normalized)
}

#[tauri::command]
fn quit_application(app: AppHandle) -> Result<(), String> {
    request_app_exit(&app);
    Ok(())
}

#[tauri::command]
fn hide_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[tauri::command]
fn load_image_data_uri(path: String) -> Result<String, String> {
    let path = Path::new(&path);
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;

    if !metadata.is_file() {
        return Err("Image path is not a file".to_string());
    }

    if metadata.len() > 20 * 1024 * 1024 {
        return Err("Image file is too large".to_string());
    }

    let mime = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        _ => return Err("Unsupported image format".to_string()),
    };

    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    Ok(format!(
        "data:{mime};base64,{}",
        general_purpose::STANDARD.encode(bytes)
    ))
}

fn show_main_window(app: &AppHandle) {
    overlay::hide_overlay_windows(app);

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();

        #[cfg(target_os = "windows")]
        {
            use windows::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
            };

            if let Ok(hwnd) = window.hwnd() {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
        }
    }
}

fn request_app_exit(app: &AppHandle) {
    let runtime_state = app.state::<AppRuntimeState>();

    if runtime_state.shutting_down.swap(true, Ordering::SeqCst) {
        return;
    }

    runtime_state
        .overlay_manager_running
        .store(false, Ordering::SeqCst);

    overlay::close_overlay(app);
    app.exit(0);
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .text("show", "Open Osuwapp")
        .separator()
        .text("quit", "Quit")
        .build()?;

    let mut tray_builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("Osuwapp")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => request_app_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => show_main_window(tray.app_handle()),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }

    let tray = tray_builder.build(app)?;
    app.manage(tray);

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppRuntimeState::default())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let overlay_settings = storage::load_overlay_settings(&app.handle());
            let runtime_state = app.state::<AppRuntimeState>();

            if let Ok(mut guard) = runtime_state.overlay_settings.lock() {
                *guard = overlay_settings;
            }

            build_tray(app)?;

            overlay::spawn_overlay_manager(
                app.handle().clone(),
                runtime_state.overlay_settings.clone(),
                runtime_state.latest_snapshot.clone(),
                runtime_state.overlay_manager_running.clone(),
            );

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                request_app_exit(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_initial_snapshot,
            start_live_updates,
            get_overlay_settings,
            save_overlay_settings,
            load_image_data_uri,
            hide_main_window,
            quit_application
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
