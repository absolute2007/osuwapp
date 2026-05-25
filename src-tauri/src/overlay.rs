use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

use crate::models::{AppSnapshot, OverlaySettings};

const OVERLAY_LABEL: &str = "overlay";
const OVERLAY_EDITOR_LABEL: &str = "overlay-editor";
const DEFAULT_OVERLAY_WIDTH: f64 = 420.0;
const DEFAULT_OVERLAY_HEIGHT: f64 = 248.0;
const BASE_EDITOR_PANEL_WIDTH: f64 = 760.0;
const BASE_EDITOR_PANEL_HEIGHT: f64 = 520.0;
const COMPACT_EDITOR_PANEL_WIDTH: u32 = 680;
const COMPACT_EDITOR_PANEL_HEIGHT: u32 = 76;
const OVERLAY_TICK_MS: u64 = 16;
const OSU_TARGET_REFRESH_MS: u64 = 50;
const OSU_TARGET_MISSING_REFRESH_MS: u64 = 100;
const INGAME_INJECT_TIMEOUT_MS: u64 = 1_500;
const INGAME_INJECT_RETRY_MS: u64 = 750;
#[cfg(target_os = "windows")]
const WEB_OVERLAY_FAILOVER_MS: u64 = 8_000;
#[cfg(target_os = "windows")]
const WEB_OVERLAY_RETRY_MS: u64 = 15_000;
#[cfg(target_os = "windows")]
const OPEN_OVERLAY_SETTINGS_EVENT: &str = "open-overlay-settings";

pub fn spawn_overlay_manager(
    app: AppHandle,
    settings: Arc<Mutex<OverlaySettings>>,
    latest_snapshot: Arc<Mutex<Option<AppSnapshot>>>,
    running: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        #[cfg(target_os = "windows")]
        let mut cached_osu_target: Option<windows::OsuWindowTarget> = None;
        #[cfg(target_os = "windows")]
        let mut next_osu_target_refresh = Instant::now();

        loop {
            if !running.load(Ordering::SeqCst) {
                close_overlay(&app);
                break;
            }

            let current_settings = settings
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default()
                .normalized();

            #[cfg(target_os = "windows")]
            {
                if Instant::now() >= next_osu_target_refresh {
                    cached_osu_target = windows::find_osu_target();
                    next_osu_target_refresh = Instant::now()
                        + Duration::from_millis(if cached_osu_target.is_some() {
                            OSU_TARGET_REFRESH_MS
                        } else {
                            OSU_TARGET_MISSING_REFRESH_MS
                        });
                }

                let osu_target = cached_osu_target;
                let settings_hotkey_pressed =
                    windows::poll_overlay_settings_hotkey(&current_settings, osu_target.as_ref());
                if settings_hotkey_pressed {
                    let _ = app.emit(OPEN_OVERLAY_SETTINGS_EVENT, ());
                }

                sync_overlay_window(
                    &app,
                    &settings,
                    &current_settings,
                    &latest_snapshot,
                    osu_target,
                    settings_hotkey_pressed,
                );
            }

            #[cfg(not(target_os = "windows"))]
            {
                sync_overlay_window(
                    &app,
                    &settings,
                    &current_settings,
                    &latest_snapshot,
                    None,
                    false,
                );
            }
            thread::sleep(Duration::from_millis(OVERLAY_TICK_MS));
        }
    });
}

fn sync_overlay_window(
    app: &AppHandle,
    settings_store: &Arc<Mutex<OverlaySettings>>,
    settings: &OverlaySettings,
    latest_snapshot: &Arc<Mutex<Option<AppSnapshot>>>,
    #[cfg(target_os = "windows")] osu_target: Option<windows::OsuWindowTarget>,
    #[cfg(target_os = "windows")] settings_hotkey_pressed: bool,
) {
    #[cfg(target_os = "windows")]
    {
        if !settings.enabled {
            hide_overlay(app);
            windows::stop_ingame_overlay();
            windows::stop_web_overlay();
            return;
        }

        if let Some(main_window) = app.get_webview_window("main") {
            let is_focused = main_window.is_focused().unwrap_or(false);
            let is_minimized = main_window.is_minimized().unwrap_or(false);

            if is_focused && !is_minimized {
                hide_overlay(app);
                windows::stop_ingame_overlay();
                windows::stop_web_overlay();
                return;
            }
        }

        let Some(target) = osu_target else {
            hide_overlay(app);
            windows::stop_ingame_overlay();
            windows::stop_web_overlay();
            if settings_hotkey_pressed {
                open_overlay_settings_window(app);
            }
            return;
        };

        if target.is_minimized {
            hide_overlay(app);
            windows::stop_ingame_overlay();
            windows::stop_web_overlay();
            return;
        }

        hide_overlay(app);

        let Some(dll_dir) = resolve_asdf_overlay_dir(app) else {
            overlay_debug("failed: asdf overlay dll dir not found");
            return;
        };

        if target.is_fullscreen_surface {
            windows::stop_web_overlay();
            sync_legacy_overlay_window(
                app,
                settings_store,
                settings,
                latest_snapshot,
                target,
                settings_hotkey_pressed,
                &dll_dir,
            );
            return;
        }

        let Some((electron_exe, overlay_script)) = resolve_web_overlay_runtime(app) else {
            overlay_debug("failed: web overlay runtime not found");
            sync_legacy_overlay_window(
                app,
                settings_store,
                settings,
                latest_snapshot,
                target,
                settings_hotkey_pressed,
                &dll_dir,
            );
            return;
        };

        let snapshot = latest_snapshot.lock().ok().and_then(|guard| guard.clone());
        if let Some(next_settings) = windows::sync_web_overlay(
            &electron_exe,
            &overlay_script,
            &dll_dir,
            &target,
            settings,
            snapshot.as_ref(),
            settings_hotkey_pressed,
        ) {
            persist_overlay_settings(app, settings_store, next_settings);
        }

        if windows::web_overlay_failed() {
            sync_legacy_overlay_window(
                app,
                settings_store,
                settings,
                latest_snapshot,
                target,
                settings_hotkey_pressed,
                &dll_dir,
            );
        } else if windows::web_overlay_is_running() {
            windows::stop_ingame_overlay();
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        let _ = settings_store;
        let _ = settings;
        let _ = latest_snapshot;
    }
}

#[cfg(target_os = "windows")]
fn sync_legacy_overlay_window(
    app: &AppHandle,
    settings_store: &Arc<Mutex<OverlaySettings>>,
    settings: &OverlaySettings,
    latest_snapshot: &Arc<Mutex<Option<AppSnapshot>>>,
    target: windows::OsuWindowTarget,
    settings_hotkey_pressed: bool,
    dll_dir: &std::path::Path,
) {
    if target.is_fullscreen_surface {
        if !target.is_foreground {
            hide_overlay(app);
            windows::stop_ingame_overlay();
            return;
        }

        let snapshot = latest_snapshot.lock().ok().and_then(|guard| guard.clone());
        if let Some(next_settings) = windows::sync_ingame_overlay(
            dll_dir,
            &target,
            settings,
            snapshot.as_ref(),
            settings_hotkey_pressed,
        ) {
            persist_overlay_settings(app, settings_store, next_settings);
        }
        return;
    }

    windows::stop_ingame_overlay();

    if settings_hotkey_pressed {
        open_overlay_editor_window(app, Some(&target));
    }

    if overlay_editor_is_visible(app) {
        hide_overlay(app);
        return;
    }

    let overlay_width = settings.width;
    let overlay_height = settings.height;
    let base_rect = target.rect;
    let x = base_rect.left.saturating_add(settings.offset_x);
    let y = base_rect.top.saturating_add(settings.offset_y);

    let Ok(window) = ensure_overlay_window(app) else {
        overlay_debug("failed: could not ensure overlay window");
        return;
    };

    let _ = window.set_size(Size::Physical(PhysicalSize::new(
        overlay_width,
        overlay_height,
    )));
    let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
    let _ = windows::position_capture_overlay_window(&window, x, y, overlay_width, overlay_height);
    if let Err(error) = window.show() {
        overlay_debug(&format!("failed: show overlay window: {error}"));
    }
}

#[cfg(target_os = "windows")]
fn overlay_debug(line: &str) {
    use std::{fs::OpenOptions, io::Write};

    let path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("overlay-debug.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(target_os = "windows")]
fn persist_overlay_settings(
    app: &AppHandle,
    settings_store: &Arc<Mutex<OverlaySettings>>,
    settings: OverlaySettings,
) {
    let normalized = settings.normalized();

    if let Err(error) = crate::storage::save_overlay_settings(app, &normalized) {
        log::warn!("Failed to save in-game overlay settings: {error}");
        return;
    }

    if let Ok(mut guard) = settings_store.lock() {
        *guard = normalized.clone();
    }

    let _ = app.emit("overlay-settings-updated", &normalized);
}

fn hide_overlay(app: &AppHandle) {
    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
            let _ = windows::hide_overlay_window(&window);
        }
    }

    #[cfg(not(target_os = "windows"))]
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
    }
}

#[cfg(target_os = "windows")]
fn overlay_editor_is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(OVERLAY_EDITOR_LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn open_overlay_editor_window(app: &AppHandle, osu_target: Option<&windows::OsuWindowTarget>) {
    let Some(target) = osu_target else {
        open_overlay_settings_window(app);
        return;
    };

    let no_activate = target.is_fullscreen_surface;
    let editor_rect = if no_activate {
        target.monitor_rect
    } else {
        target.rect
    };
    let width = editor_rect.width().max(640) as f64;
    let height = editor_rect.height().max(420) as f64;
    hide_overlay(app);

    if let Some(existing) = app.get_webview_window(OVERLAY_EDITOR_LABEL) {
        let _ = existing.set_always_on_top(true);
        let _ = existing.set_focusable(!no_activate);
        let _ = existing.set_ignore_cursor_events(false);
        let _ = existing.set_size(Size::Physical(PhysicalSize::new(
            width as u32,
            height as u32,
        )));
        let _ = existing.set_position(Position::Physical(PhysicalPosition::new(
            editor_rect.left,
            editor_rect.top,
        )));
        let _ = windows::configure_editor_overlay(&existing, no_activate);
        let _ = windows::position_editor_overlay_window(
            &existing,
            editor_rect.left,
            editor_rect.top,
            width as u32,
            height as u32,
            no_activate,
        );
        if !no_activate {
            let _ = existing.unminimize();
            let _ = existing.set_focus();
            windows::bring_window_to_front(&existing);
        }
        return;
    }

    let window = match WebviewWindowBuilder::new(
        app,
        OVERLAY_EDITOR_LABEL,
        WebviewUrl::App("index.html?overlayEditor=1".into()),
    )
    .title("osu! Companion Overlay Editor")
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .focused(!no_activate)
    .visible(false)
    .shadow(false)
    .inner_size(width, height)
    .build()
    {
        Ok(window) => window,
        Err(error) => {
            log::warn!("Failed to open overlay editor: {error}");
            open_overlay_settings_window(app);
            return;
        }
    };

    let _ = window.set_always_on_top(true);
    let _ = window.set_focusable(!no_activate);
    let _ = window.set_ignore_cursor_events(false);
    let _ = window.set_size(Size::Physical(PhysicalSize::new(
        width as u32,
        height as u32,
    )));
    let _ = window.set_position(Position::Physical(PhysicalPosition::new(
        editor_rect.left,
        editor_rect.top,
    )));
    let _ = windows::configure_editor_overlay(&window, no_activate);
    let _ = windows::position_editor_overlay_window(
        &window,
        editor_rect.left,
        editor_rect.top,
        width as u32,
        height as u32,
        no_activate,
    );
    if !no_activate {
        let _ = window.show();
        let _ = window.set_focus();
        windows::bring_window_to_front(&window);
    }
}

fn open_overlay_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();

        #[cfg(target_os = "windows")]
        windows::bring_window_to_front(&window);
    }
}

pub fn close_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.close();
    }
    if let Some(window) = app.get_webview_window(OVERLAY_EDITOR_LABEL) {
        let _ = window.close();
    }
}

pub fn hide_overlay_windows(app: &AppHandle) {
    hide_overlay(app);
    if let Some(window) = app.get_webview_window(OVERLAY_EDITOR_LABEL) {
        let _ = window.hide();
    }
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn resolve_asdf_overlay_dir(app: &AppHandle) -> Option<PathBuf> {
    static ASDF_OVERLAY_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    ASDF_OVERLAY_DIR
        .get_or_init(|| {
            let resource_dir = app.path().resource_dir().ok();
            let current_dir = std::env::current_dir().ok();
            let candidates = [
                resource_dir
                    .as_ref()
                    .map(|dir| dir.join("resources").join("asdf-overlay")),
                current_dir
                    .as_ref()
                    .map(|dir| dir.join("src-tauri").join("resources").join("asdf-overlay")),
                current_dir
                    .as_ref()
                    .map(|dir| dir.join("resources").join("asdf-overlay")),
            ];

            candidates
                .into_iter()
                .flatten()
                .find(|dir| dir.join("asdf_overlay-x64.dll").exists())
        })
        .clone()
}

#[cfg(target_os = "windows")]
fn resolve_web_overlay_runtime(app: &AppHandle) -> Option<(PathBuf, PathBuf)> {
    static WEB_OVERLAY_RUNTIME: OnceLock<Option<(PathBuf, PathBuf)>> = OnceLock::new();

    WEB_OVERLAY_RUNTIME
        .get_or_init(|| {
            let current_dir = std::env::current_dir().ok();
            let resource_dir = app.path().resource_dir().ok();
            let project_candidates = [
                current_dir.clone(),
                current_dir.as_ref().and_then(|dir| dir.parent().map(PathBuf::from)),
                resource_dir,
            ];

            project_candidates
                .into_iter()
                .flatten()
                .find_map(|project_dir| {
                    let script = project_dir.join("scripts").join("tosu-overlay").join("main.mjs");
                    if !script.exists() {
                        return None;
                    }

                    [
                        project_dir
                            .join("node_modules")
                            .join(".bin")
                            .join("electron.cmd"),
                        project_dir
                            .join("node_modules")
                            .join("electron")
                            .join("dist")
                            .join("electron.exe"),
                    ]
                    .into_iter()
                    .find(|electron| electron.exists())
                    .map(|electron| (electron, script))
                })
        })
        .clone()
}

fn ensure_overlay_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        configure_overlay_window(&window)?;
        return Ok(window);
    }

    let window = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("index.html?overlay=1".into()),
    )
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .focused(false)
    .visible(false)
    .shadow(false)
    .inner_size(DEFAULT_OVERLAY_WIDTH, DEFAULT_OVERLAY_HEIGHT)
    .build()
    .map_err(|error| error.to_string())?;

    configure_overlay_window(&window)?;

    Ok(window)
}

fn configure_overlay_window(window: &WebviewWindow) -> Result<(), String> {
    window
        .set_always_on_top(true)
        .map_err(|error| error.to_string())?;
    window
        .set_focusable(false)
        .map_err(|error| error.to_string())?;
    window
        .set_ignore_cursor_events(true)
        .map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    windows::configure_capture_overlay(window)?;

    Ok(())
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
mod windows {
    use std::{
        fs,
        fs::OpenOptions,
        io::{BufRead, BufReader, Write},
        os::windows::process::CommandExt,
        path::{Path, PathBuf},
        process::{Child, ChildStdin, Command, Stdio},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex, OnceLock,
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use crate::models::{OverlayElementSettings, OverlaySettings, SessionPhase, SessionSnapshot};
    use asdf_overlay_client::{
        common::{
            cursor::Cursor,
            request::{BlockInput, ListenInput, SetAnchor, SetBlockingCursor, SetPosition},
            size::PercentLength,
        },
        event::{
            input::{
                CursorAction, CursorEvent, CursorInput, CursorInputState, InputEvent,
                KeyInputState, KeyboardInput,
            },
            OverlayEvent, WindowEvent,
        },
        inject,
        surface::OverlaySurface,
        OverlayDll,
    };
    use rosu_mem::process::{Process, ProcessTraits};
    use tauri::WebviewWindow;
    use tokio::runtime::Runtime;
    use windows::{
        core::BOOL,
        Win32::{
            Foundation::{HWND, LPARAM, RECT},
            Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            },
            UI::{
                Input::KeyboardAndMouse::{
                    GetAsyncKeyState, VK_DELETE, VK_DOWN, VK_END, VK_F1, VK_F10, VK_F11, VK_F12,
                    VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_INSERT,
                    VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
                },
                WindowsAndMessaging::{
                    EnumWindows, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
                    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow,
                    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow,
                    GWLP_HWNDPARENT, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE,
                    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSENDCHANGING, SWP_NOSIZE, SW_HIDE,
                    SW_SHOW, SW_SHOWNOACTIVATE, WDA_NONE, WS_EX_APPWINDOW, WS_EX_LAYERED,
                    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
                },
            },
        },
    };

    const EXCLUDE_WORDS: [&str; 2] = ["umu-run", "waitforexitandrun"];
    static INGAME_STATE: OnceLock<Mutex<InjectedOverlayState>> = OnceLock::new();
    static WEB_OVERLAY_STATE: OnceLock<Mutex<ElectronWebOverlayState>> = OnceLock::new();

    #[derive(Clone, Copy)]
    pub struct OsuWindowTarget {
        pub pid: u32,
        pub rect: Rect,
        pub monitor_rect: Rect,
        pub is_minimized: bool,
        pub is_foreground: bool,
        pub is_fullscreen_surface: bool,
    }

    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[allow(dead_code)]
    impl Rect {
        pub fn width(&self) -> i32 {
            self.right - self.left
        }

        pub fn height(&self) -> i32 {
            self.bottom - self.top
        }

        pub fn is_valid(&self) -> bool {
            self.width() > 0 && self.height() > 0
        }
    }

    impl From<RECT> for Rect {
        fn from(rect: RECT) -> Self {
            Self {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            }
        }
    }

    struct EnumState {
        pid: u32,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        let mut pid = 0;

        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }

        if pid != state.pid {
            return true.into();
        }

        if unsafe { !IsWindowVisible(hwnd).as_bool() } {
            return true.into();
        }

        state.found = Some(hwnd);
        false.into()
    }

    pub fn find_osu_target() -> Option<OsuWindowTarget> {
        let process = Process::find_process("osu!.exe", &EXCLUDE_WORDS).ok()?;
        let mut state = EnumState {
            pid: process.pid,
            found: None,
        };

        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM((&mut state as *mut EnumState) as isize),
            );
        }

        let hwnd = state.found?;
        let mut rect = RECT::default();

        unsafe {
            GetWindowRect(hwnd, &mut rect).ok()?;
        }

        let monitor_rect = monitor_rect_for_window(hwnd).unwrap_or(rect);
        let is_fullscreen_surface = window_covers_monitor_rect(&rect, &monitor_rect);

        Some(OsuWindowTarget {
            pid: process.pid,
            rect: rect.into(),
            monitor_rect: monitor_rect.into(),
            is_minimized: unsafe { IsIconic(hwnd).as_bool() },
            is_foreground: unsafe { GetForegroundWindow() == hwnd },
            is_fullscreen_surface,
        })
    }

    pub fn poll_overlay_settings_hotkey(
        settings: &OverlaySettings,
        osu_target: Option<&OsuWindowTarget>,
    ) -> bool {
        let Some(target) = osu_target else {
            return false;
        };

        if !target.is_foreground {
            return false;
        }

        let Some(virtual_key) = virtual_key_from_binding(&settings.toggle_key) else {
            return false;
        };

        let key_is_down = unsafe { (GetAsyncKeyState(virtual_key) & (0x8000u16 as i16)) != 0 };
        static TOGGLE_WAS_DOWN: AtomicBool = AtomicBool::new(false);

        let key_was_down = TOGGLE_WAS_DOWN.swap(key_is_down, Ordering::SeqCst);

        if key_is_down && !key_was_down {
            return true;
        }

        false
    }

    pub fn bring_window_to_front(window: &WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
            );
            let _ = SetForegroundWindow(hwnd);
        }
    }

    pub fn configure_editor_overlay(
        window: &WebviewWindow,
        no_activate: bool,
    ) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let current_ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let mut next_ex_style = (current_ex_style & !WS_EX_TOOLWINDOW.0 & !WS_EX_TRANSPARENT.0)
                | WS_EX_APPWINDOW.0
                | WS_EX_LAYERED.0;

            if no_activate {
                next_ex_style |= WS_EX_NOACTIVATE.0;
            } else {
                next_ex_style &= !WS_EX_NOACTIVATE.0;
            }

            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next_ex_style as isize);
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            let _ = SetWindowDisplayAffinity(hwnd, WDA_NONE);
        }

        Ok(())
    }

    pub fn configure_capture_overlay(window: &WebviewWindow) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let current_ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let next_ex_style = (current_ex_style
                & !WS_EX_APPWINDOW.0
                & !WS_EX_TOOLWINDOW.0
                & !WS_EX_LAYERED.0
                & !WS_EX_TRANSPARENT.0)
                | WS_EX_NOACTIVATE.0;

            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next_ex_style as isize);
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            let _ = SetWindowDisplayAffinity(hwnd, WDA_NONE);
        }

        Ok(())
    }

    pub fn position_capture_overlay_window(
        window: &WebviewWindow,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING,
            );
        }

        Ok(())
    }

    pub fn position_editor_overlay_window(
        window: &WebviewWindow,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        no_activate: bool,
    ) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            let _ = ShowWindow(
                hwnd,
                if no_activate {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_SHOW
                },
            );
            let flags = if no_activate {
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING
            } else {
                SWP_NOOWNERZORDER | SWP_NOSENDCHANGING
            };
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width as i32,
                height as i32,
                flags,
            );
            if !no_activate {
                let _ = SetForegroundWindow(hwnd);
            }
        }

        Ok(())
    }

    pub fn hide_overlay_window(window: &WebviewWindow) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING,
            );
            let _ = ShowWindow(hwnd, SW_HIDE);
        }

        Ok(())
    }

    pub fn sync_ingame_overlay(
        dll_dir: &Path,
        target: &OsuWindowTarget,
        settings: &OverlaySettings,
        snapshot: Option<&super::AppSnapshot>,
        toggle_editor: bool,
    ) -> Option<OverlaySettings> {
        let state = INGAME_STATE.get_or_init(|| Mutex::new(InjectedOverlayState::new()));

        if let Ok(mut guard) = state.lock() {
            return guard.sync(dll_dir, target, settings, snapshot, toggle_editor);
        }

        None
    }

    pub fn stop_ingame_overlay() {
        if let Some(state) = INGAME_STATE.get() {
            if let Ok(mut guard) = state.lock() {
                guard.reset();
            }
        }
    }

    pub fn sync_web_overlay(
        electron_exe: &Path,
        overlay_script: &Path,
        dll_dir: &Path,
        target: &OsuWindowTarget,
        settings: &OverlaySettings,
        snapshot: Option<&super::AppSnapshot>,
        toggle_editor: bool,
    ) -> Option<OverlaySettings> {
        let state = WEB_OVERLAY_STATE.get_or_init(|| Mutex::new(ElectronWebOverlayState::new()));

        if let Ok(mut guard) = state.lock() {
            return guard.sync(
                electron_exe,
                overlay_script,
                dll_dir,
                target,
                settings,
                snapshot,
                toggle_editor,
            );
        }

        None
    }

    pub fn stop_web_overlay() {
        if let Some(state) = WEB_OVERLAY_STATE.get() {
            if let Ok(mut guard) = state.lock() {
                guard.reset();
            }
        }
    }

    pub fn web_overlay_is_running() -> bool {
        WEB_OVERLAY_STATE
            .get()
            .and_then(|state| state.lock().ok())
            .is_some_and(|guard| guard.child.is_some())
    }

    pub fn web_overlay_failed() -> bool {
        WEB_OVERLAY_STATE
            .get()
            .and_then(|state| state.lock().ok())
            .is_some_and(|guard| guard.failed_or_in_cooldown())
    }

    struct ElectronWebOverlayState {
        pid: Option<u32>,
        child: Option<Child>,
        stdin: Option<ChildStdin>,
        editor_active: bool,
        started_at: Option<Instant>,
        has_window: Arc<AtomicBool>,
        rendered: Arc<AtomicBool>,
        failed: Arc<AtomicBool>,
        pending_settings: Arc<Mutex<Option<OverlaySettings>>>,
        failed_until: Option<Instant>,
        disabled_pid: Option<u32>,
    }

    impl ElectronWebOverlayState {
        fn new() -> Self {
            Self {
                pid: None,
                child: None,
                stdin: None,
                editor_active: false,
                started_at: None,
                has_window: Arc::new(AtomicBool::new(false)),
                rendered: Arc::new(AtomicBool::new(false)),
                failed: Arc::new(AtomicBool::new(false)),
                pending_settings: Arc::new(Mutex::new(None)),
                failed_until: None,
                disabled_pid: None,
            }
        }

        fn reset(&mut self) {
            self.reset_process();
            self.disabled_pid = None;
        }

        fn reset_process(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
            }
            self.stdin = None;
            self.pid = None;
            self.editor_active = false;
            self.started_at = None;
            self.has_window.store(false, Ordering::SeqCst);
            self.rendered.store(false, Ordering::SeqCst);
            self.failed.store(false, Ordering::SeqCst);
        }

        fn disable_for_pid(&mut self, pid: u32, reason: &str) {
            web_overlay_debug(&format!("disabled web overlay for pid={pid}: {reason}"));
            self.disabled_pid = Some(pid);
            self.failed_until = None;
            self.reset_process();
        }

        fn retry_later(&mut self, pid: u32, reason: &str) {
            web_overlay_debug(&format!(
                "retry web overlay later for pid={pid}: {reason}"
            ));
            self.failed_until =
                Some(Instant::now() + Duration::from_millis(super::WEB_OVERLAY_RETRY_MS));
            self.reset_process();
        }

        fn sync(
            &mut self,
            electron_exe: &Path,
            overlay_script: &Path,
            dll_dir: &Path,
            target: &OsuWindowTarget,
            settings: &OverlaySettings,
            snapshot: Option<&super::AppSnapshot>,
            toggle_editor: bool,
        ) -> Option<OverlaySettings> {
            if self.disabled_pid == Some(target.pid) {
                return self.take_pending_settings();
            }

            if self.disabled_pid.is_some() && self.disabled_pid != Some(target.pid) {
                self.disabled_pid = None;
            }

            if self
                .failed_until
                .is_some_and(|retry_after| Instant::now() < retry_after)
            {
                return self.take_pending_settings();
            }

            if self.failed() {
                self.retry_later(target.pid, "runtime reported failure");
                return self.take_pending_settings();
            }

            let child_exited = self
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
                .is_some();

            if child_exited {
                self.retry_later(target.pid, "child exited");
                return self.take_pending_settings();
            }

            if self.pid != Some(target.pid) {
                self.reset_process();
                if !self.spawn(electron_exe, overlay_script, dll_dir, target) {
                    self.disable_for_pid(target.pid, "spawn failed");
                    return self.take_pending_settings();
                }
            }

            if toggle_editor {
                self.editor_active = !self.editor_active;
                self.send_json(&serde_json::json!({
                    "type": "editor",
                    "active": self.editor_active,
                }));
            }

            self.send_json(&serde_json::json!({
                "type": "state",
                "payload": {
                    "settings": settings,
                    "snapshot": snapshot,
                }
            }));

            self.take_pending_settings()
        }

        fn failed_or_in_cooldown(&self) -> bool {
            if self.disabled_pid.is_some() {
                return true;
            }
            self.failed_until
                .is_some_and(|retry_after| Instant::now() < retry_after)
                || self.failed()
        }

        fn failed(&self) -> bool {
            self.failed.load(Ordering::SeqCst)
                || self.started_at.is_some_and(|started_at| {
                    started_at.elapsed() > Duration::from_millis(super::WEB_OVERLAY_FAILOVER_MS)
                        && !self.rendered.load(Ordering::SeqCst)
                })
        }

        fn spawn(
            &mut self,
            electron_exe: &Path,
            overlay_script: &Path,
            dll_dir: &Path,
            target: &OsuWindowTarget,
        ) -> bool {
            let pid = target.pid;
            let Some(project_dir) = overlay_script
                .parent()
                .and_then(|dir| dir.parent())
                .and_then(|dir| dir.parent())
            else {
                log::warn!("Failed to resolve web overlay project directory");
                return false;
            };
            web_overlay_debug(&format!(
                "spawn electron={} script={} dll_dir={} pid={pid}",
                electron_exe.display(),
                overlay_script.display(),
                dll_dir.display()
            ));

            let mut child = match Command::new(electron_exe)
                .arg(overlay_script)
                .arg("--pid")
                .arg(pid.to_string())
                .arg("--dll-dir")
                .arg(dll_dir)
                .arg("--width")
                .arg(target.rect.width().max(1).to_string())
                .arg("--height")
                .arg(target.rect.height().max(1).to_string())
                .current_dir(project_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(0x08000000)
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    log::warn!("Failed to start web overlay runtime: {error}");
                    web_overlay_debug(&format!("spawn failed: {error}"));
                    return false;
                }
            };

            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                return false;
            };

            if let Some(stderr) = child.stderr.take() {
                thread::spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        log::warn!("web overlay runtime: {line}");
                        web_overlay_debug(&format!("stderr: {line}"));
                    }
                });
            }

            let pending_settings = self.pending_settings.clone();
            let has_window = self.has_window.clone();
            let rendered = self.rendered.clone();
            let failed = self.failed.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    let message_type = value.get("type").and_then(|item| item.as_str());
                    if message_type == Some("added") {
                        has_window.store(true, Ordering::SeqCst);
                    }
                    if message_type == Some("paint") {
                        rendered.store(true, Ordering::SeqCst);
                    }
                    if matches!(message_type, Some("error") | Some("disconnected")) {
                        failed.store(true, Ordering::SeqCst);
                    }
                    if message_type != Some("settings") {
                        if matches!(
                            message_type,
                            Some("error") | Some("ready") | Some("added") | Some("paint")
                        ) {
                            log::warn!("web overlay runtime: {line}");
                            web_overlay_debug(&format!("stdout: {line}"));
                        }
                        continue;
                    }
                    let Some(settings_value) = value.get("settings") else {
                        continue;
                    };
                    if let Ok(settings) =
                        serde_json::from_value::<OverlaySettings>(settings_value.clone())
                    {
                        if let Ok(mut guard) = pending_settings.lock() {
                            *guard = Some(settings.normalized());
                        }
                    }
                }
            });

            self.stdin = child.stdin.take();
            self.pid = Some(pid);
            self.started_at = Some(Instant::now());
            self.has_window.store(false, Ordering::SeqCst);
            self.rendered.store(false, Ordering::SeqCst);
            self.failed.store(false, Ordering::SeqCst);
            self.child = Some(child);
            web_overlay_debug("spawn ok");
            true
        }

        fn send_json(&mut self, value: &serde_json::Value) {
            let Some(stdin) = self.stdin.as_mut() else {
                web_overlay_debug("stdin missing");
                return;
            };
            match writeln!(stdin, "{value}") {
                Ok(()) => {
                    let _ = stdin.flush();
                }
                Err(error) => {
                    web_overlay_debug(&format!("stdin error: {error}"));
                }
            }
        }

        fn take_pending_settings(&self) -> Option<OverlaySettings> {
            self.pending_settings
                .lock()
                .ok()
                .and_then(|mut guard| guard.take())
        }
    }

    fn web_overlay_debug(line: &str) {
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("web-overlay-debug.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }

    struct InjectedOverlayState {
        runtime: Option<Runtime>,
        pid: Option<u32>,
        conn: Option<asdf_overlay_client::client::IpcClientConn>,
        events: Option<asdf_overlay_client::client::IpcClientEventStream>,
        surface: Option<OverlaySurface>,
        window_id: Option<u32>,
        window_size: Option<(u32, u32)>,
        input_blocking: bool,
        surface_mode: SurfaceMode,
        editor_surface_origin: (i32, i32),
        last_frame_key: Option<String>,
        editor_active: bool,
        editor_settings: Option<OverlaySettings>,
        last_session: Option<SessionSnapshot>,
        selected_element: OverlayElement,
        editing_field: Option<EditorField>,
        edit_buffer: String,
        drag_origin: Option<DragOrigin>,
        pending_settings: Option<OverlaySettings>,
        failed_until: Option<Instant>,
    }

    #[derive(Clone, Copy)]
    struct DragOrigin {
        element: OverlayElement,
        mode: DragMode,
        cursor_x: i32,
        cursor_y: i32,
        offset_x: i32,
        offset_y: i32,
        width: u32,
        height: u32,
    }

    #[derive(Clone, Copy)]
    enum DragMode {
        Move,
        Resize,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum OverlayElement {
        Whole,
        Pp,
        Stats,
        Hits,
        Map,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum EditorField {
        Opacity,
        Scale,
        FontScale,
        X,
        Y,
        Width,
        Height,
    }

    impl InjectedOverlayState {
        fn new() -> Self {
            Self {
                runtime: Runtime::new().ok(),
                pid: None,
                conn: None,
                events: None,
                surface: None,
                window_id: None,
                window_size: None,
                input_blocking: false,
                surface_mode: SurfaceMode::Unknown,
                editor_surface_origin: (0, 0),
                last_frame_key: None,
                editor_active: false,
                editor_settings: None,
                last_session: None,
                selected_element: OverlayElement::Pp,
                editing_field: None,
                edit_buffer: String::new(),
                drag_origin: None,
                pending_settings: None,
                failed_until: None,
            }
        }

        fn reset(&mut self) {
            self.conn = None;
            self.events = None;
            self.surface = None;
            self.window_id = None;
            self.window_size = None;
            self.pid = None;
            self.input_blocking = false;
            self.surface_mode = SurfaceMode::Unknown;
            self.editor_surface_origin = (0, 0);
            self.last_frame_key = None;
            self.editor_active = false;
            self.editor_settings = None;
            self.last_session = None;
            self.selected_element = OverlayElement::Pp;
            self.editing_field = None;
            self.edit_buffer.clear();
            self.drag_origin = None;
            self.pending_settings = None;
        }

        fn sync(
            &mut self,
            dll_dir: &Path,
            target: &OsuWindowTarget,
            settings: &OverlaySettings,
            snapshot: Option<&super::AppSnapshot>,
            toggle_editor: bool,
        ) -> Option<OverlaySettings> {
            if !settings.enabled {
                self.reset();
                return None;
            }

            if self
                .failed_until
                .is_some_and(|retry_after| Instant::now() < retry_after)
            {
                return None;
            }

            if self.pid != Some(target.pid) && !self.attach(dll_dir, target.pid) {
                self.failed_until =
                    Some(Instant::now() + Duration::from_millis(super::INGAME_INJECT_RETRY_MS));
                return None;
            }

            if toggle_editor {
                if self.editor_active {
                    self.editor_active = false;
                    self.editor_settings = None;
                    self.drag_origin = None;
                } else {
                    self.editor_active = true;
                    self.editor_settings = Some(settings.clone());
                    self.selected_element = OverlayElement::Pp;
                    self.editing_field = None;
                    self.edit_buffer.clear();
                    self.drag_origin = None;
                }
                self.surface_mode = SurfaceMode::Unknown;
                self.last_frame_key = None;
            }

            self.drain_events(settings);

            let Some(window_id) = self.window_id else {
                return self.pending_settings.take();
            };
            let Some(runtime) = self.runtime.as_ref() else {
                return self.pending_settings.take();
            };
            let Some(surface) = self.surface.as_mut() else {
                return self.pending_settings.take();
            };
            let Some(conn) = self.conn.as_mut() else {
                return self.pending_settings.take();
            };

            if self.editor_active && !self.input_blocking {
                let _ = runtime.block_on(async {
                    conn.window(window_id)
                        .request(ListenInput {
                            cursor: true,
                            keyboard: true,
                        })
                        .await
                });
                let _ = runtime.block_on(async {
                    conn.window(window_id)
                        .request(BlockInput { block: true })
                        .await
                });
                let _ = runtime.block_on(async {
                    conn.window(window_id)
                        .request(SetBlockingCursor {
                            cursor: Some(Cursor::Default),
                        })
                        .await
                });
                self.input_blocking = true;
                self.last_frame_key = None;
            } else if !self.editor_active && self.input_blocking {
                let _ = runtime.block_on(async {
                    conn.window(window_id)
                        .request(ListenInput {
                            cursor: false,
                            keyboard: false,
                        })
                        .await
                });
                let _ = runtime.block_on(async {
                    conn.window(window_id)
                        .request(BlockInput { block: false })
                        .await
                });
                let _ = runtime.block_on(async {
                    conn.window(window_id)
                        .request(SetBlockingCursor { cursor: None })
                        .await
                });
                self.input_blocking = false;
                self.last_frame_key = None;
            }

            let active_settings = self.editor_settings.as_ref().unwrap_or(settings);
            let desired_mode = if self.editor_active {
                let layout = editor_surface_layout(active_settings, self.window_size);
                self.editor_surface_origin = (layout.x, layout.y);
                SurfaceMode::Editor {
                    x: layout.x,
                    y: layout.y,
                }
            } else {
                let bounds = overlay_bounds(active_settings);
                self.editor_surface_origin = (0, 0);
                SurfaceMode::Hud {
                    x: bounds.0,
                    y: bounds.1,
                }
            };

            if self.surface_mode != desired_mode {
                apply_surface_mode(runtime, conn, window_id, desired_mode);
                self.surface_mode = desired_mode;
            }

            if let Some(session) = snapshot.and_then(|item| item.session.as_ref()) {
                self.last_session = Some(session.clone());
            }
            let session = snapshot
                .and_then(|item| item.session.as_ref())
                .or(self.last_session.as_ref());
            let frame_key = if self.editor_active {
                ingame_editor_frame_key(
                    active_settings,
                    session,
                    self.window_size,
                    self.editor_surface_origin,
                    self.selected_element,
                    self.editing_field,
                    &self.edit_buffer,
                )
            } else {
                ingame_frame_key(active_settings, session)
            };
            if self.last_frame_key.as_deref() == Some(frame_key.as_str()) {
                return self.pending_settings.take();
            }

            let bitmap = if self.editor_active {
                render_ingame_editor_bitmap(
                    active_settings,
                    session,
                    self.window_size,
                    self.editor_surface_origin,
                    self.selected_element,
                    self.editing_field,
                    &self.edit_buffer,
                )
            } else {
                render_ingame_bitmap(active_settings, session)
            };

            if let Ok(Some(handle)) = surface.update_bitmap(bitmap.width, &bitmap.data) {
                let _ = runtime.block_on(async { conn.window(window_id).request(handle).await });
            }
            self.last_frame_key = Some(frame_key);
            self.pending_settings.take()
        }

        fn attach(&mut self, dll_dir: &Path, pid: u32) -> bool {
            self.reset();

            let Some(runtime) = self.runtime.as_ref() else {
                return false;
            };

            let injection_dll_dir =
                prepare_injection_dll_dir(dll_dir, pid).unwrap_or_else(|| dll_dir.to_path_buf());
            let x64_dll = injection_dll_dir.join("asdf_overlay-x64.dll");
            let x86_dll = injection_dll_dir.join("asdf_overlay-x86.dll");
            let dll = OverlayDll {
                x64: Some(&x64_dll),
                x86: Some(&x86_dll),
                arm64: None,
            };

            match runtime.block_on(inject(
                pid,
                dll,
                Some(Duration::from_millis(super::INGAME_INJECT_TIMEOUT_MS)),
            )) {
                Ok((conn, events)) => {
                    self.conn = Some(conn);
                    self.events = Some(events);
                    self.surface = OverlaySurface::new(None).ok();
                    self.pid = Some(pid);
                    true
                }
                Err(error) => {
                    log::warn!("Failed to attach in-game overlay: {error:?}");
                    false
                }
            }
        }

        fn drain_events(&mut self, settings: &OverlaySettings) {
            let Some(runtime) = self.runtime.as_ref() else {
                return;
            };
            let Some(events) = self.events.as_mut() else {
                return;
            };
            let Some(conn) = self.conn.as_mut() else {
                return;
            };

            let mut input_events = Vec::new();
            let mut latest_drag_move = None;
            let event_limit = if self.editor_active { 128 } else { 16 };
            for _ in 0..event_limit {
                let event = runtime.block_on(async {
                    tokio::time::timeout(Duration::from_micros(50), events.recv()).await
                });

                let Ok(Some(event)) = event else {
                    break;
                };

                match event {
                    OverlayEvent::Window {
                        id,
                        event:
                            WindowEvent::Added { width, height, .. }
                            | WindowEvent::Resized { width, height },
                    } => {
                        self.window_id = Some(id);
                        self.window_size = Some((width, height));
                        let desired_mode = if self.editor_active {
                            let layout = editor_surface_layout(settings, self.window_size);
                            self.editor_surface_origin = (layout.x, layout.y);
                            SurfaceMode::Editor {
                                x: layout.x,
                                y: layout.y,
                            }
                        } else {
                            let bounds = overlay_bounds(settings);
                            self.editor_surface_origin = (0, 0);
                            SurfaceMode::Hud {
                                x: bounds.0,
                                y: bounds.1,
                            }
                        };
                        apply_surface_mode(runtime, conn, id, desired_mode);
                        self.surface_mode = desired_mode;
                    }
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::Input(input),
                    } if Some(id) == self.window_id => {
                        if self.drag_origin.is_some() && is_cursor_move(&input) {
                            latest_drag_move = Some(input);
                        } else {
                            input_events.push(input);
                        }
                    }
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::InputBlockingEnded,
                    } if Some(id) == self.window_id => {
                        self.input_blocking = false;
                        self.editor_active = false;
                        self.editor_settings = None;
                        self.surface_mode = SurfaceMode::Unknown;
                        self.last_frame_key = None;
                    }
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::Destroyed,
                    } if Some(id) == self.window_id => {
                        self.window_id = None;
                    }
                    _ => {}
                }
            }

            if let Some(input) = latest_drag_move {
                input_events.push(input);
            }

            for input in input_events {
                self.handle_editor_input(input);
            }
        }

        fn handle_editor_input(&mut self, input: InputEvent) {
            if !self.editor_active {
                return;
            }

            match input {
                InputEvent::Keyboard(KeyboardInput::Key { key, state }) => {
                    if state == KeyInputState::Pressed {
                        let code = key.code.get();
                        if self.editing_field.is_some() {
                            if code == VK_RETURN.0 as u8 {
                                self.commit_edit_buffer();
                                return;
                            }
                            if code == 0x08 {
                                self.edit_buffer.pop();
                                self.last_frame_key = None;
                                return;
                            }
                        }
                        if code == VK_END.0 as u8 || code == 0x1B {
                            if self.editing_field.is_some() {
                                self.editing_field = None;
                                self.edit_buffer.clear();
                                self.last_frame_key = None;
                                return;
                            }
                            self.editor_active = false;
                            self.editor_settings = None;
                            self.drag_origin = None;
                            self.surface_mode = SurfaceMode::Unknown;
                            self.last_frame_key = None;
                        }
                    }
                }
                InputEvent::Keyboard(KeyboardInput::Char(character)) => {
                    if self.editing_field.is_some()
                        && (character.is_ascii_digit()
                            || character == '-'
                            || character == '.'
                            || character == ',')
                        && self.edit_buffer.len() < 12
                    {
                        self.edit_buffer.push(character);
                        self.last_frame_key = None;
                    }
                }
                InputEvent::Cursor(cursor) => {
                    let Some(settings) = self.editor_settings.as_mut() else {
                        return;
                    };

                    let screen_x = cursor.client.x.saturating_add(self.editor_surface_origin.0);
                    let screen_y = cursor.client.y.saturating_add(self.editor_surface_origin.1);
                    let (panel_x, panel_y) = editor_panel_origin(self.window_size, settings);
                    let scale = EditorPanelScale::from_settings(settings);
                    let (local_x, local_y) =
                        scale.base_point(screen_x - panel_x, screen_y - panel_y);

                    match cursor.event {
                        CursorEvent::Action {
                            state: CursorInputState::Pressed { .. },
                            action: CursorAction::Left,
                        } => {
                            if let Some((element, mode, offset_x, offset_y, width, height)) =
                                hit_test_overlay_element(settings, screen_x, screen_y)
                            {
                                self.drag_origin = Some(DragOrigin {
                                    element,
                                    mode,
                                    cursor_x: screen_x,
                                    cursor_y: screen_y,
                                    offset_x,
                                    offset_y,
                                    width,
                                    height,
                                });
                                self.selected_element = element;
                                self.last_frame_key = None;
                                return;
                            }
                        }
                        CursorEvent::Move => {
                            let Some(drag) = self.drag_origin else {
                                return;
                            };

                            let next_x = drag
                                .offset_x
                                .saturating_add(screen_x.saturating_sub(drag.cursor_x));
                            let next_y = drag
                                .offset_y
                                .saturating_add(screen_y.saturating_sub(drag.cursor_y));
                            apply_overlay_drag(
                                settings,
                                drag,
                                screen_x - drag.cursor_x,
                                screen_y - drag.cursor_y,
                                next_x,
                                next_y,
                            );
                            let normalized = settings.clone().normalized();
                            *settings = normalized.clone();
                            self.last_frame_key = None;
                            return;
                        }
                        CursorEvent::Action {
                            state: CursorInputState::Released,
                            action: CursorAction::Left,
                        } => {
                            if let Some(drag) = self.drag_origin {
                                let next_x = drag
                                    .offset_x
                                    .saturating_add(screen_x.saturating_sub(drag.cursor_x));
                                let next_y = drag
                                    .offset_y
                                    .saturating_add(screen_y.saturating_sub(drag.cursor_y));
                                apply_overlay_drag(
                                    settings,
                                    drag,
                                    screen_x - drag.cursor_x,
                                    screen_y - drag.cursor_y,
                                    next_x,
                                    next_y,
                                );
                                let normalized = settings.clone().normalized();
                                *settings = normalized.clone();
                                self.pending_settings = Some(normalized);
                                self.last_frame_key = None;
                            }
                            self.drag_origin = None;

                            if in_rect(local_x, local_y, 612, 14, 52, 28) {
                                self.editor_active = false;
                                self.editor_settings = None;
                                self.surface_mode = SurfaceMode::Unknown;
                                self.last_frame_key = None;
                                return;
                            }

                            if let Some(field) = editor_value_field_at(local_x, local_y) {
                                self.editing_field = Some(field);
                                self.edit_buffer = current_editor_field_value(
                                    settings,
                                    self.selected_element,
                                    field,
                                );
                                self.last_frame_key = None;
                                return;
                            }

                            if apply_editor_click(
                                settings,
                                &mut self.selected_element,
                                local_x,
                                local_y,
                            ) {
                                let normalized = settings.clone().normalized();
                                *settings = normalized.clone();
                                self.pending_settings = Some(normalized);
                                self.last_frame_key = None;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        fn commit_edit_buffer(&mut self) {
            let Some(field) = self.editing_field.take() else {
                return;
            };
            let Some(settings) = self.editor_settings.as_mut() else {
                self.edit_buffer.clear();
                return;
            };

            let raw_value = self.edit_buffer.replace(',', ".");
            if let Ok(number) = raw_value.parse::<f64>() {
                apply_editor_field_value(settings, self.selected_element, field, number);
                let normalized = settings.clone().normalized();
                *settings = normalized.clone();
                self.pending_settings = Some(normalized);
            }

            self.edit_buffer.clear();
            self.last_frame_key = None;
        }
    }

    fn prepare_injection_dll_dir(source_dir: &Path, pid: u32) -> Option<PathBuf> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let target_dir = std::env::temp_dir()
            .join("osuwapp-asdf-overlay")
            .join(format!("{pid}-{stamp}"));

        fs::create_dir_all(&target_dir).ok()?;
        fs::copy(
            source_dir.join("asdf_overlay-x64.dll"),
            target_dir.join("asdf_overlay-x64.dll"),
        )
        .ok()?;
        fs::copy(
            source_dir.join("asdf_overlay-x86.dll"),
            target_dir.join("asdf_overlay-x86.dll"),
        )
        .ok()?;

        Some(target_dir)
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum SurfaceMode {
        Unknown,
        Hud { x: i32, y: i32 },
        Editor { x: i32, y: i32 },
    }

    fn apply_surface_mode(
        runtime: &Runtime,
        conn: &mut asdf_overlay_client::client::IpcClientConn,
        window_id: u32,
        mode: SurfaceMode,
    ) {
        let (position_x, position_y, anchor_x, anchor_y) = match mode {
            SurfaceMode::Unknown => return,
            SurfaceMode::Hud { x, y } => (
                PercentLength::Length(x as f32),
                PercentLength::Length(y as f32),
                PercentLength::Length(0.0),
                PercentLength::Length(0.0),
            ),
            SurfaceMode::Editor { x, y } => (
                PercentLength::Length(x as f32),
                PercentLength::Length(y as f32),
                PercentLength::Length(0.0),
                PercentLength::Length(0.0),
            ),
        };

        let _ = runtime.block_on(async {
            conn.window(window_id)
                .request(SetAnchor {
                    x: anchor_x,
                    y: anchor_y,
                })
                .await
        });
        let _ = runtime.block_on(async {
            conn.window(window_id)
                .request(SetPosition {
                    x: position_x,
                    y: position_y,
                })
                .await
        });
    }

    struct Bitmap {
        width: u32,
        data: Vec<u8>,
    }

    #[derive(Clone, Copy)]
    struct EditorPanelScale {
        x: f64,
        y: f64,
    }

    impl EditorPanelScale {
        fn from_settings(settings: &OverlaySettings) -> Self {
            Self {
                x: settings.editor_panel_width as f64 / super::BASE_EDITOR_PANEL_WIDTH,
                y: settings.editor_panel_height as f64 / super::BASE_EDITOR_PANEL_HEIGHT,
            }
        }

        fn x(&self, value: i32) -> i32 {
            (value as f64 * self.x).round() as i32
        }

        fn y(&self, value: i32) -> i32 {
            (value as f64 * self.y).round() as i32
        }

        fn w(&self, value: i32) -> i32 {
            self.x(value).max(1)
        }

        fn h(&self, value: i32) -> i32 {
            self.y(value).max(1)
        }

        fn font(&self, value: i32) -> i32 {
            (value as f64 * self.x.min(self.y)).round().max(8.0) as i32
        }

        fn base_point(&self, x: i32, y: i32) -> (i32, i32) {
            (
                (x as f64 / self.x).round() as i32,
                (y as f64 / self.y).round() as i32,
            )
        }
    }

    #[allow(dead_code)]
    struct HudLayout {
        width: u32,
        height: u32,
        hero_font: i32,
        body_font: i32,
        small_font: i32,
        padding: i32,
        gap: i32,
    }

    #[allow(dead_code)]
    impl HudLayout {
        fn from_settings(settings: &OverlaySettings) -> Self {
            let width = settings.width.max(80);
            let height = settings.height.max(40);
            let ui_scale = (settings.scale * settings.font_scale).clamp(0.36, 1.5);
            Self {
                width,
                height,
                hero_font: (24.0 * ui_scale).round().clamp(9.0, 28.0) as i32,
                body_font: (13.5 * ui_scale).round().clamp(7.0, 17.0) as i32,
                small_font: (10.5 * ui_scale).round().clamp(6.0, 13.0) as i32,
                padding: (settings.padding as i32).clamp(2, 18),
                gap: (5.0 * ui_scale).round().clamp(2.0, 7.0) as i32,
            }
        }
    }

    fn ingame_frame_key(settings: &OverlaySettings, session: Option<&SessionSnapshot>) -> String {
        let mut key = format!(
            "{}:{}:{}:{}:{}:{}:{}:{:.3}:{:.3}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{:?}:{:?}:{:?}:{:?}",
            settings.enabled,
            settings.width,
            settings.height,
            settings.offset_x,
            settings.offset_y,
            settings.padding,
            settings.corner_radius,
            settings.scale,
            settings.font_scale,
            settings.opacity,
            settings.show_background,
            settings.show_pp,
            settings.show_if_fc,
            settings.show_accuracy,
            settings.show_combo,
            settings.show_mods,
            settings.show_map,
            settings.show_hits,
            settings.editor_panel_width,
            settings.editor_panel_height,
            element_key(&settings.pp_panel),
            element_key(&settings.stats_panel),
            element_key(&settings.hits_panel),
            element_key(&settings.map_panel),
        );

        if let Some(session) = session {
            key.push_str(&format!(
                ":{}:{:.2}:{:.2}:{:?}:{}:{}:{}:{}:{}:{}:{}:{}",
                session_phase_label(session.phase),
                session.pp.current,
                session.pp.if_fc,
                session.live.accuracy,
                session.live.combo,
                session.live.mods_text,
                session.live.hits.n100,
                session.live.hits.n50,
                session.live.hits.misses,
                session.live.hits.slider_breaks,
                session.beatmap.artist,
                session.beatmap.title,
            ));
        }

        key
    }

    fn element_key(element: &OverlayElementSettings) -> (bool, bool, i32, i32, u32, u32, i32, i32) {
        (
            element.enabled,
            element.show_background,
            element.x,
            element.y,
            element.width,
            element.height,
            (element.scale * 1000.0).round() as i32,
            (element.font_scale * 1000.0).round() as i32,
        )
    }

    fn render_ingame_bitmap(
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
    ) -> Bitmap {
        let (x, y, width, height) = overlay_bounds(settings);
        let mut canvas = BitmapCanvas::new(width, height);
        draw_overlay_elements(&mut canvas, -x, -y, settings, session, false);
        canvas.into_bitmap()
    }

    fn ingame_editor_frame_key(
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
        window_size: Option<(u32, u32)>,
        surface_origin: (i32, i32),
        selected_element: OverlayElement,
        editing_field: Option<EditorField>,
        edit_buffer: &str,
    ) -> String {
        format!(
            "editor:{window_size:?}:{surface_origin:?}:{}:{:?}:{}:{}",
            selected_element_label(selected_element),
            editing_field_key(editing_field),
            edit_buffer,
            ingame_frame_key(settings, session)
        )
    }

    fn render_ingame_editor_bitmap(
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
        window_size: Option<(u32, u32)>,
        surface_origin: (i32, i32),
        selected_element: OverlayElement,
        _editing_field: Option<EditorField>,
        _edit_buffer: &str,
    ) -> Bitmap {
        let layout = editor_surface_layout(settings, window_size);
        let mut canvas = BitmapCanvas::new(layout.width, layout.height);
        let origin_x = surface_origin.0;
        let origin_y = surface_origin.1;
        draw_overlay_elements(&mut canvas, -origin_x, -origin_y, settings, session, true);
        draw_selected_element_outline(
            &mut canvas,
            settings,
            selected_element,
            -origin_x,
            -origin_y,
        );
        let (panel_screen_x, panel_screen_y) = editor_panel_origin(window_size, settings);
        draw_compact_editor_bar(
            &mut canvas,
            panel_screen_x - origin_x,
            panel_screen_y - origin_y,
            settings,
            selected_element,
        );

        canvas.into_bitmap()
    }

    fn editor_panel_origin(
        window_size: Option<(u32, u32)>,
        _settings: &OverlaySettings,
    ) -> (i32, i32) {
        let (width, height) = window_size.unwrap_or((1280, 720));
        (
            ((width as i32 - super::COMPACT_EDITOR_PANEL_WIDTH as i32) / 2).max(0),
            (height as i32 - super::COMPACT_EDITOR_PANEL_HEIGHT as i32 - 24).max(0),
        )
    }

    struct EditorSurfaceLayout {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    }

    fn editor_surface_layout(
        settings: &OverlaySettings,
        window_size: Option<(u32, u32)>,
    ) -> EditorSurfaceLayout {
        let (panel_x, panel_y) = editor_panel_origin(window_size, settings);
        let (hud_x, hud_y, hud_width, hud_height) = overlay_bounds(settings);
        let padding = 16;
        let left = hud_x.min(panel_x).saturating_sub(padding).max(0);
        let top = hud_y.min(panel_y).saturating_sub(padding).max(0);
        let right = (hud_x + hud_width as i32)
            .max(panel_x + super::COMPACT_EDITOR_PANEL_WIDTH as i32)
            .saturating_add(padding);
        let bottom = (hud_y + hud_height as i32)
            .max(panel_y + super::COMPACT_EDITOR_PANEL_HEIGHT as i32)
            .saturating_add(padding);

        EditorSurfaceLayout {
            x: left,
            y: top,
            width: right.saturating_sub(left).max(1) as u32,
            height: bottom.saturating_sub(top).max(1) as u32,
        }
    }

    fn draw_compact_editor_bar(
        canvas: &mut BitmapCanvas,
        x: i32,
        y: i32,
        settings: &OverlaySettings,
        selected_element: OverlayElement,
    ) {
        canvas.fill_rounded_rect(
            x,
            y,
            super::COMPACT_EDITOR_PANEL_WIDTH as i32,
            super::COMPACT_EDITOR_PANEL_HEIGHT as i32,
            10,
            [10, 14, 20, 236],
        );
        canvas.stroke_rounded_rect(
            x,
            y,
            super::COMPACT_EDITOR_PANEL_WIDTH as i32,
            super::COMPACT_EDITOR_PANEL_HEIGHT as i32,
            10,
            [120, 138, 166, 120],
        );
        canvas.draw_text_clipped(
            x + 16,
            y + 14,
            112,
            "Overlay",
            14,
            FontWeight::Semibold,
            [243, 247, 252, 255],
        );
        canvas.draw_text_clipped(
            x + 16,
            y + 38,
            122,
            selected_element_label(selected_element),
            11,
            FontWeight::Semibold,
            [164, 181, 205, 245],
        );

        for (element, bx, label, enabled) in [
            (OverlayElement::Pp, 148, "PP", settings.show_pp),
            (
                OverlayElement::Stats,
                216,
                "Stats",
                settings.show_if_fc
                    || settings.show_accuracy
                    || settings.show_combo
                    || settings.show_mods,
            ),
            (OverlayElement::Hits, 296, "Hits", settings.show_hits),
            (OverlayElement::Map, 372, "Map", settings.show_map),
        ] {
            let active = selected_element == element;
            let color = if active {
                [126, 190, 255, 245]
            } else if enabled {
                [38, 112, 78, 232]
            } else {
                [30, 38, 50, 232]
            };
            canvas.fill_rounded_rect(x + bx, y + 14, 62, 34, 7, color);
            canvas.stroke_rounded_rect(x + bx, y + 14, 62, 34, 7, [190, 210, 238, 70]);
            canvas.draw_text_clipped(
                x + bx + 10,
                y + 24,
                44,
                label,
                12,
                FontWeight::Semibold,
                if active {
                    [8, 17, 28, 255]
                } else {
                    [236, 242, 250, 255]
                },
            );
        }

        draw_compact_toggle(
            canvas,
            x + 462,
            y + 14,
            "BG",
            selected_element_settings(settings, selected_element).show_background,
        );
        draw_compact_toggle(canvas, x + 518, y + 14, "HUD", settings.enabled);
        canvas.fill_rounded_rect(x + 612, y + 14, 52, 28, 7, [44, 52, 66, 238]);
        canvas.draw_text_clipped(
            x + 624,
            y + 22,
            32,
            "Done",
            11,
            FontWeight::Semibold,
            [240, 244, 250, 255],
        );
    }

    fn draw_compact_toggle(canvas: &mut BitmapCanvas, x: i32, y: i32, label: &str, enabled: bool) {
        canvas.fill_rounded_rect(
            x,
            y,
            44,
            28,
            7,
            if enabled {
                [38, 112, 78, 238]
            } else {
                [30, 38, 50, 238]
            },
        );
        canvas.stroke_rounded_rect(x, y, 44, 28, 7, [190, 210, 238, 62]);
        canvas.draw_text_clipped(
            x + 8,
            y + 8,
            28,
            label,
            11,
            FontWeight::Semibold,
            [238, 243, 250, 255],
        );
    }

    fn draw_editor_button(
        canvas: &mut BitmapCanvas,
        scale: EditorPanelScale,
        panel_x: i32,
        panel_y: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        label: &str,
    ) {
        let x = panel_x + scale.x(x);
        let y = panel_y + scale.y(y);
        let w = scale.w(w);
        let h = scale.h(h);
        canvas.fill_rounded_rect(x, y, w, h, scale.font(9), [34, 42, 55, 238]);
        canvas.stroke_rounded_rect(x, y, w, h, scale.font(9), [116, 132, 156, 120]);
        canvas.draw_text_clipped(
            x + scale.x(14),
            y + scale.y(8),
            w - scale.w(28),
            label,
            scale.font(13),
            FontWeight::Semibold,
            [242, 246, 252, 255],
        );
    }

    fn draw_status_chip(
        canvas: &mut BitmapCanvas,
        scale: EditorPanelScale,
        panel_x: i32,
        panel_y: i32,
        x: i32,
        y: i32,
        w: i32,
        label: &str,
        enabled: bool,
    ) {
        let x = panel_x + scale.x(x);
        let y = panel_y + scale.y(y);
        let w = scale.w(w);
        let h = scale.h(32);
        canvas.fill_rounded_rect(
            x,
            y,
            w,
            h,
            scale.font(9),
            if enabled {
                [34, 95, 68, 230]
            } else {
                [70, 44, 48, 230]
            },
        );
        canvas.stroke_rounded_rect(x, y, w, h, scale.font(9), [132, 154, 176, 88]);
        canvas.draw_text_clipped(
            x + scale.x(14),
            y + scale.y(8),
            w - scale.w(28),
            label,
            scale.font(12),
            FontWeight::Semibold,
            [238, 245, 250, 255],
        );
    }

    fn draw_editor_row(
        canvas: &mut BitmapCanvas,
        scale: EditorPanelScale,
        panel_x: i32,
        panel_y: i32,
        x: i32,
        y: i32,
        label: &str,
        enabled: bool,
    ) {
        let x = panel_x + scale.x(x);
        let y = panel_y + scale.y(y);
        canvas.draw_text_clipped(
            x,
            y + scale.y(7),
            scale.w(180),
            label,
            scale.font(14),
            FontWeight::Semibold,
            [235, 240, 248, 255],
        );
        let toggle_x = x + scale.x(238);
        canvas.fill_rounded_rect(
            toggle_x,
            y,
            scale.w(44),
            scale.h(24),
            scale.font(12),
            if enabled {
                [36, 124, 82, 235]
            } else {
                [38, 45, 58, 235]
            },
        );
        canvas.stroke_rounded_rect(
            toggle_x,
            y,
            scale.w(44),
            scale.h(24),
            scale.font(12),
            [120, 138, 164, 92],
        );
        canvas.fill_rounded_rect(
            if enabled {
                toggle_x + scale.x(22)
            } else {
                toggle_x + scale.x(3)
            },
            y + scale.y(3),
            scale.w(18),
            scale.h(18),
            scale.font(9),
            [241, 246, 251, 248],
        );
    }

    fn draw_editor_stepper(
        canvas: &mut BitmapCanvas,
        scale: EditorPanelScale,
        panel_x: i32,
        panel_y: i32,
        x: i32,
        y: i32,
        label: &str,
        value: &str,
    ) {
        let actual_x = panel_x + scale.x(x);
        let actual_y = panel_y + scale.y(y);
        canvas.draw_text_clipped(
            actual_x,
            actual_y,
            scale.w(120),
            label,
            scale.font(12),
            FontWeight::Semibold,
            [168, 181, 202, 245],
        );
        draw_editor_button(canvas, scale, panel_x, panel_y, x, y + 20, 34, 30, "-");
        canvas.fill_rounded_rect(
            actual_x + scale.x(42),
            actual_y + scale.y(20),
            scale.w(76),
            scale.h(30),
            scale.font(9),
            [22, 29, 39, 238],
        );
        canvas.stroke_rounded_rect(
            actual_x + scale.x(42),
            actual_y + scale.y(20),
            scale.w(76),
            scale.h(30),
            scale.font(9),
            [105, 122, 148, 112],
        );
        canvas.draw_text_clipped(
            actual_x + scale.x(50),
            actual_y + scale.y(28),
            scale.w(60),
            value,
            scale.font(12),
            FontWeight::Semibold,
            [240, 244, 250, 255],
        );
        draw_editor_button(
            canvas,
            scale,
            panel_x,
            panel_y,
            x + 126,
            y + 20,
            34,
            30,
            "+",
        );
    }

    fn draw_metric_toggle(
        canvas: &mut BitmapCanvas,
        scale: EditorPanelScale,
        panel_x: i32,
        panel_y: i32,
        x: i32,
        y: i32,
        label: &str,
        enabled: bool,
    ) {
        let x = panel_x + scale.x(x);
        let y = panel_y + scale.y(y);
        canvas.fill_rounded_rect(
            x,
            y,
            scale.w(72),
            scale.h(34),
            scale.font(9),
            if enabled {
                [37, 112, 79, 238]
            } else {
                [31, 38, 50, 238]
            },
        );
        canvas.stroke_rounded_rect(
            x,
            y,
            scale.w(72),
            scale.h(34),
            scale.font(10),
            if enabled {
                [95, 180, 136, 132]
            } else {
                [94, 112, 140, 105]
            },
        );
        canvas.draw_text_clipped(
            x + scale.x(12),
            y + scale.y(9),
            scale.w(48),
            label,
            scale.font(12),
            FontWeight::Semibold,
            [238, 243, 250, 255],
        );
    }

    fn editor_value_text(
        editing_field: Option<EditorField>,
        edit_buffer: &str,
        field: EditorField,
        fallback: &str,
    ) -> String {
        if editing_field == Some(field) {
            if edit_buffer.is_empty() {
                "|".to_string()
            } else {
                format!("{edit_buffer}|")
            }
        } else {
            fallback.to_string()
        }
    }

    fn editor_value_field_at(x: i32, y: i32) -> Option<EditorField> {
        let _ = (x, y);
        None
    }

    fn current_editor_field_value(
        settings: &OverlaySettings,
        selected_element: OverlayElement,
        field: EditorField,
    ) -> String {
        let element = selected_element_settings(settings, selected_element);
        match field {
            EditorField::Opacity => format!("{:.0}", settings.opacity * 100.0),
            EditorField::Scale => format!("{:.0}", element.scale * 100.0),
            EditorField::FontScale => format!("{:.0}", element.font_scale * 100.0),
            EditorField::X => element.x.to_string(),
            EditorField::Y => element.y.to_string(),
            EditorField::Width => element.width.to_string(),
            EditorField::Height => element.height.to_string(),
        }
    }

    fn apply_editor_field_value(
        settings: &mut OverlaySettings,
        selected_element: OverlayElement,
        field: EditorField,
        number: f64,
    ) {
        match field {
            EditorField::Opacity => settings.opacity = number / 100.0,
            EditorField::Scale => {
                selected_element_settings_mut(settings, selected_element).scale = number / 100.0
            }
            EditorField::FontScale => {
                selected_element_settings_mut(settings, selected_element).font_scale =
                    number / 100.0
            }
            EditorField::X => {
                selected_element_settings_mut(settings, selected_element).x = number.round() as i32
            }
            EditorField::Y => {
                selected_element_settings_mut(settings, selected_element).y = number.round() as i32
            }
            EditorField::Width => {
                selected_element_settings_mut(settings, selected_element).width =
                    number.round().max(1.0) as u32
            }
            EditorField::Height => {
                selected_element_settings_mut(settings, selected_element).height =
                    number.round().max(1.0) as u32
            }
        }
    }

    fn editing_field_key(field: Option<EditorField>) -> &'static str {
        match field {
            Some(EditorField::Opacity) => "opacity",
            Some(EditorField::Scale) => "scale",
            Some(EditorField::FontScale) => "font",
            Some(EditorField::X) => "x",
            Some(EditorField::Y) => "y",
            Some(EditorField::Width) => "width",
            Some(EditorField::Height) => "height",
            None => "none",
        }
    }

    fn apply_editor_click(
        settings: &mut OverlaySettings,
        selected_element: &mut OverlayElement,
        x: i32,
        y: i32,
    ) -> bool {
        for (element, bx) in [
            (OverlayElement::Pp, 148),
            (OverlayElement::Stats, 216),
            (OverlayElement::Hits, 296),
            (OverlayElement::Map, 372),
        ] {
            if in_rect(x, y, bx, 14, 62, 34) {
                if *selected_element == element {
                    match element {
                        OverlayElement::Pp => {
                            let next = !(settings.show_pp && settings.pp_panel.enabled);
                            settings.show_pp = next;
                            settings.pp_panel.enabled = next;
                        }
                        OverlayElement::Stats => {
                            let next = !(settings.stats_panel.enabled
                                && (settings.show_if_fc
                                    || settings.show_accuracy
                                    || settings.show_combo
                                    || settings.show_mods));
                            settings.stats_panel.enabled = next;
                            settings.show_if_fc = next;
                            settings.show_accuracy = next;
                            settings.show_combo = next;
                            settings.show_mods = next;
                        }
                        OverlayElement::Hits => {
                            let next = !(settings.show_hits && settings.hits_panel.enabled);
                            settings.show_hits = next;
                            settings.hits_panel.enabled = next;
                        }
                        OverlayElement::Map => {
                            let next = !(settings.show_map && settings.map_panel.enabled);
                            settings.show_map = next;
                            settings.map_panel.enabled = next;
                        }
                        OverlayElement::Whole => {}
                    }
                } else {
                    *selected_element = element;
                }
                return true;
            }
        }

        if in_rect(x, y, 462, 14, 44, 28) {
            let element = selected_element_settings_mut(settings, *selected_element);
            element.show_background = !element.show_background;
            return true;
        }
        if in_rect(x, y, 518, 14, 44, 28) {
            settings.enabled = !settings.enabled;
            return true;
        }
        false
    }

    fn stepper_click(x: i32, y: i32, sx: i32, sy: i32) -> Option<i32> {
        if in_rect(x, y, sx, sy + 20, 34, 30) {
            Some(-1)
        } else if in_rect(x, y, sx + 126, sy + 20, 34, 30) {
            Some(1)
        } else {
            None
        }
    }

    fn is_cursor_move(input: &InputEvent) -> bool {
        matches!(
            input,
            InputEvent::Cursor(CursorInput {
                event: CursorEvent::Move,
                ..
            })
        )
    }

    fn in_rect(x: i32, y: i32, rx: i32, ry: i32, rw: i32, rh: i32) -> bool {
        x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    fn session_phase_label(phase: SessionPhase) -> &'static str {
        match phase {
            SessionPhase::Preview => "preview",
            SessionPhase::Playing => "playing",
            SessionPhase::Result => "result",
        }
    }

    fn overlay_bounds(settings: &OverlaySettings) -> (i32, i32, u32, u32) {
        let mut left = settings.offset_x;
        let mut top = settings.offset_y;
        let mut right = settings.offset_x + settings.width as i32;
        let mut bottom = settings.offset_y + settings.height as i32;

        for (visible, element) in [
            (settings.show_pp, &settings.pp_panel),
            (
                settings.show_if_fc
                    || settings.show_accuracy
                    || settings.show_combo
                    || settings.show_mods,
                &settings.stats_panel,
            ),
            (settings.show_hits, &settings.hits_panel),
            (settings.show_map, &settings.map_panel),
        ] {
            if !visible || !element.enabled {
                continue;
            }

            let element_left = settings.offset_x.saturating_add(element.x);
            let element_top = settings.offset_y.saturating_add(element.y);
            left = left.min(element_left);
            top = top.min(element_top);
            right = right.max(element_left + element.width as i32);
            bottom = bottom.max(element_top + element.height as i32);
        }

        (
            left,
            top,
            (right - left).max(1) as u32,
            (bottom - top).max(1) as u32,
        )
    }

    fn hit_test_overlay_element(
        settings: &OverlaySettings,
        x: i32,
        y: i32,
    ) -> Option<(OverlayElement, DragMode, i32, i32, u32, u32)> {
        for (element, item) in [
            (OverlayElement::Map, &settings.map_panel),
            (OverlayElement::Hits, &settings.hits_panel),
            (OverlayElement::Stats, &settings.stats_panel),
            (OverlayElement::Pp, &settings.pp_panel),
        ] {
            let visible = match element {
                OverlayElement::Pp => settings.show_pp,
                OverlayElement::Stats => {
                    settings.show_if_fc
                        || settings.show_accuracy
                        || settings.show_combo
                        || settings.show_mods
                }
                OverlayElement::Hits => settings.show_hits,
                OverlayElement::Map => settings.show_map,
                OverlayElement::Whole => true,
            };
            let element_x = settings.offset_x.saturating_add(item.x);
            let element_y = settings.offset_y.saturating_add(item.y);
            if visible
                && item.enabled
                && in_rect(
                    x,
                    y,
                    element_x,
                    element_y,
                    item.width as i32,
                    item.height as i32,
                )
            {
                let edge = 18;
                let near_right = x >= element_x + item.width as i32 - edge;
                let near_bottom = y >= element_y + item.height as i32 - edge;
                let mode = if near_right || near_bottom {
                    DragMode::Resize
                } else {
                    DragMode::Move
                };
                return Some((element, mode, item.x, item.y, item.width, item.height));
            }
        }

        if in_rect(
            x,
            y,
            settings.offset_x,
            settings.offset_y,
            settings.width as i32,
            settings.height as i32,
        ) {
            return Some((
                OverlayElement::Whole,
                DragMode::Move,
                settings.offset_x,
                settings.offset_y,
                settings.width,
                settings.height,
            ));
        }

        None
    }

    fn apply_overlay_drag(
        settings: &mut OverlaySettings,
        drag: DragOrigin,
        delta_x: i32,
        delta_y: i32,
        next_x: i32,
        next_y: i32,
    ) {
        match drag.mode {
            DragMode::Move => set_overlay_element_position(settings, drag.element, next_x, next_y),
            DragMode::Resize => {
                let next_width = (drag.width as i32 + delta_x).max(24) as u32;
                let next_height = (drag.height as i32 + delta_y).max(14) as u32;
                let element = selected_element_settings_mut(settings, drag.element);
                element.width = next_width;
                element.height = next_height;
            }
        }
    }

    fn set_overlay_element_position(
        settings: &mut OverlaySettings,
        element: OverlayElement,
        x: i32,
        y: i32,
    ) {
        match element {
            OverlayElement::Whole => {
                settings.offset_x = x;
                settings.offset_y = y;
            }
            OverlayElement::Pp => {
                settings.pp_panel.x = x;
                settings.pp_panel.y = y;
            }
            OverlayElement::Stats => {
                settings.stats_panel.x = x;
                settings.stats_panel.y = y;
            }
            OverlayElement::Hits => {
                settings.hits_panel.x = x;
                settings.hits_panel.y = y;
            }
            OverlayElement::Map => {
                settings.map_panel.x = x;
                settings.map_panel.y = y;
            }
        }
    }

    fn selected_element_settings(
        settings: &OverlaySettings,
        element: OverlayElement,
    ) -> &OverlayElementSettings {
        match element {
            OverlayElement::Whole | OverlayElement::Pp => &settings.pp_panel,
            OverlayElement::Stats => &settings.stats_panel,
            OverlayElement::Hits => &settings.hits_panel,
            OverlayElement::Map => &settings.map_panel,
        }
    }

    fn selected_element_settings_mut(
        settings: &mut OverlaySettings,
        element: OverlayElement,
    ) -> &mut OverlayElementSettings {
        match element {
            OverlayElement::Whole | OverlayElement::Pp => &mut settings.pp_panel,
            OverlayElement::Stats => &mut settings.stats_panel,
            OverlayElement::Hits => &mut settings.hits_panel,
            OverlayElement::Map => &mut settings.map_panel,
        }
    }

    fn selected_element_label(element: OverlayElement) -> &'static str {
        match element {
            OverlayElement::Whole => "Overlay",
            OverlayElement::Pp => "PP",
            OverlayElement::Stats => "Stats",
            OverlayElement::Hits => "Hits",
            OverlayElement::Map => "Map",
        }
    }

    fn draw_selected_element_outline(
        canvas: &mut BitmapCanvas,
        settings: &OverlaySettings,
        element: OverlayElement,
        offset_x: i32,
        offset_y: i32,
    ) {
        let item = selected_element_settings(settings, element);
        canvas.stroke_rounded_rect(
            settings.offset_x + item.x + offset_x - 2,
            settings.offset_y + item.y + offset_y - 2,
            item.width as i32 + 4,
            item.height as i32 + 4,
            8,
            [112, 183, 255, 238],
        );
        let left = settings.offset_x + item.x + offset_x;
        let top = settings.offset_y + item.y + offset_y;
        let right = left + item.width as i32;
        let bottom = top + item.height as i32;
        for (x, y) in [
            (right - 5, top + item.height as i32 / 2 - 5),
            (left + item.width as i32 / 2 - 5, bottom - 5),
            (right - 5, bottom - 5),
        ] {
            canvas.fill_rounded_rect(x, y, 10, 10, 4, [112, 183, 255, 245]);
            canvas.stroke_rounded_rect(x, y, 10, 10, 4, [10, 15, 24, 180]);
        }
    }

    fn draw_overlay_elements(
        canvas: &mut BitmapCanvas,
        offset_x: i32,
        offset_y: i32,
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
        editor: bool,
    ) {
        let _ = editor;

        let Some(session) = session else {
            draw_single_panel_text(
                canvas,
                settings.offset_x + offset_x,
                settings.offset_y + offset_y,
                settings.width,
                settings.height,
                settings.scale,
                settings.font_scale,
                settings,
                settings.show_background,
                "Waiting for osu!",
                [230, 238, 249, 255],
            );
            return;
        };

        if settings.show_pp && settings.pp_panel.enabled {
            draw_single_panel_text(
                canvas,
                settings.offset_x + settings.pp_panel.x + offset_x,
                settings.offset_y + settings.pp_panel.y + offset_y,
                settings.pp_panel.width,
                settings.pp_panel.height,
                settings.pp_panel.scale,
                settings.pp_panel.font_scale,
                settings,
                settings.pp_panel.show_background,
                &format!("{:.2} PP", session.pp.current),
                [250, 252, 255, 255],
            );
        }

        if settings.stats_panel.enabled
            && (settings.show_if_fc
                || settings.show_accuracy
                || settings.show_combo
                || settings.show_mods)
        {
            let mut cells = Vec::new();
            if settings.show_if_fc {
                cells.push(("IF FC".to_string(), format!("{:.2}", session.pp.if_fc)));
            }
            if settings.show_accuracy {
                cells.push((
                    "ACC".to_string(),
                    session
                        .live
                        .accuracy
                        .map(|value| format!("{value:.2}%"))
                        .unwrap_or_else(|| "--".to_string()),
                ));
            }
            if settings.show_combo {
                cells.push(("COMBO".to_string(), format!("{}x", session.live.combo)));
            }
            if settings.show_mods {
                cells.push(("MODS".to_string(), session.live.mods_text.clone()));
            }
            draw_separate_metric_panel(
                canvas,
                &settings.stats_panel,
                offset_x,
                offset_y,
                settings,
                &cells,
            );
        }

        if settings.show_hits && settings.hits_panel.enabled {
            draw_separate_hit_panel(
                canvas,
                &settings.hits_panel,
                offset_x,
                offset_y,
                settings,
                &session.live.hits,
            );
        }

        if settings.show_map && settings.map_panel.enabled {
            draw_single_panel_text(
                canvas,
                settings.offset_x + settings.map_panel.x + offset_x,
                settings.offset_y + settings.map_panel.y + offset_y,
                settings.map_panel.width,
                settings.map_panel.height,
                settings.map_panel.scale,
                settings.map_panel.font_scale,
                settings,
                settings.map_panel.show_background,
                &format!(
                    "{} - {} [{}]",
                    session.beatmap.artist, session.beatmap.title, session.beatmap.difficulty_name
                ),
                [220, 231, 244, 248],
            );
        }
    }

    fn draw_panel_shell(
        canvas: &mut BitmapCanvas,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        settings: &OverlaySettings,
        show_background: bool,
    ) {
        if show_background {
            canvas.fill_rounded_rect(
                x + 1,
                y + 2,
                width as i32,
                height as i32,
                8,
                [0, 0, 0, (settings.opacity * 58.0).clamp(0.0, 78.0) as u8],
            );
            canvas.fill_rounded_rect(
                x,
                y,
                width as i32,
                height as i32,
                8,
                [
                    12,
                    16,
                    24,
                    (settings.opacity * 172.0).clamp(0.0, 202.0) as u8,
                ],
            );
            canvas.stroke_rounded_rect(
                x,
                y,
                width as i32,
                height as i32,
                8,
                [
                    225,
                    235,
                    248,
                    (settings.opacity * 82.0).clamp(0.0, 118.0) as u8,
                ],
            );
        }
    }

    fn draw_single_panel_text(
        canvas: &mut BitmapCanvas,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale: f64,
        font_scale: f64,
        settings: &OverlaySettings,
        show_background: bool,
        text: &str,
        color: [u8; 4],
    ) {
        draw_panel_shell(canvas, x, y, width, height, settings, show_background);
        if show_background {
            canvas.fill_rounded_rect(
                x + 2,
                y + 2,
                3,
                height as i32 - 4,
                5,
                [
                    91,
                    177,
                    255,
                    (settings.opacity * 210.0).clamp(0.0, 235.0) as u8,
                ],
            );
            canvas.fill_rounded_rect(
                x + 6,
                y + 2,
                (width as i32 - 8).max(1),
                (height as i32 / 2).max(1),
                7,
                [
                    255,
                    255,
                    255,
                    (settings.opacity * 18.0).clamp(0.0, 34.0) as u8,
                ],
            );
        }
        let inset = 8;
        let max_font_by_height = ((height as i32 - inset * 2) as f64 / 1.35).floor() as i32;
        let pp_text = text.strip_suffix(" PP");
        let main_text = pp_text.unwrap_or(text);
        let max_font_by_width =
            ((width as f64 - inset as f64 * 2.0 - if pp_text.is_some() { 17.0 } else { 0.0 })
                / main_text.len().max(1) as f64
                * 1.78)
                .floor() as i32;
        let font = ((20.0 * scale * font_scale).round() as i32)
            .min(max_font_by_height)
            .min(max_font_by_width)
            .clamp(6, 28);
        let text_y = y + ((height as i32 - font) / 2).max(1) - 2;
        canvas.draw_text_clipped(
            x + inset,
            text_y,
            width as i32 - inset * 2,
            main_text,
            font,
            FontWeight::Bold,
            color,
        );
        if pp_text.is_some() {
            canvas.draw_text_clipped(
                x + width as i32 - 22,
                text_y + (font / 3).max(2),
                18,
                "PP",
                (font / 2).clamp(6, 10),
                FontWeight::Semibold,
                [176, 207, 246, 245],
            );
        }
    }

    fn draw_separate_metric_panel(
        canvas: &mut BitmapCanvas,
        element: &OverlayElementSettings,
        offset_x: i32,
        offset_y: i32,
        settings: &OverlaySettings,
        cells: &[(String, String)],
    ) {
        if cells.is_empty() {
            return;
        }

        let x = settings.offset_x + element.x + offset_x;
        let y = settings.offset_y + element.y + offset_y;
        draw_panel_shell(
            canvas,
            x,
            y,
            element.width,
            element.height,
            settings,
            element.show_background,
        );
        let columns = cells.len().max(1) as i32;
        let gap = 2;
        let cell_width = ((element.width as i32 - 6 - gap * (columns - 1)) / columns).max(12);
        let available_height = element.height as i32 - 5;
        let label_font =
            ((available_height as f64 * 0.3 * element.font_scale).round() as i32).clamp(5, 10);
        let value_font =
            ((available_height as f64 * 0.5 * element.font_scale).round() as i32).clamp(6, 15);

        for (index, (label, value)) in cells.iter().enumerate() {
            let cell_x = x + 3 + index as i32 * (cell_width + gap);
            if element.show_background {
                canvas.fill_rounded_rect(
                    cell_x,
                    y + 3,
                    cell_width,
                    element.height as i32 - 6,
                    6,
                    [
                        18,
                        26,
                        38,
                        (settings.opacity * 108.0).clamp(0.0, 145.0) as u8,
                    ],
                );
                canvas.stroke_rounded_rect(
                    cell_x,
                    y + 3,
                    cell_width,
                    element.height as i32 - 6,
                    6,
                    [
                        210,
                        225,
                        245,
                        (settings.opacity * 34.0).clamp(0.0, 54.0) as u8,
                    ],
                );
            }
            canvas.draw_text_clipped(
                cell_x + 3,
                y + 4,
                cell_width - 6,
                label,
                label_font.min((element.height as i32 / 3).max(5)),
                FontWeight::Semibold,
                [160, 181, 210, 245],
            );
            canvas.draw_text_clipped(
                cell_x + 3,
                y + 5 + label_font,
                cell_width - 6,
                value,
                value_font.min((element.height as i32 - label_font - 9).max(6)),
                FontWeight::Bold,
                [250, 252, 255, 255],
            );
        }
    }

    fn draw_separate_hit_panel(
        canvas: &mut BitmapCanvas,
        element: &OverlayElementSettings,
        offset_x: i32,
        offset_y: i32,
        settings: &OverlaySettings,
        hits: &crate::models::HitSnapshot,
    ) {
        let x = settings.offset_x + element.x + offset_x;
        let y = settings.offset_y + element.y + offset_y;
        draw_panel_shell(
            canvas,
            x,
            y,
            element.width,
            element.height,
            settings,
            element.show_background,
        );
        let parts = [
            ("100", hits.n100, [178, 222, 255, 255], [31, 96, 164, 170]),
            ("50", hits.n50, [255, 214, 132, 255], [174, 103, 22, 166]),
            (
                "MISS",
                hits.misses,
                [255, 154, 166, 255],
                [178, 45, 67, 170],
            ),
            (
                "SB",
                hits.slider_breaks,
                [213, 174, 255, 255],
                [102, 66, 160, 168],
            ),
        ];
        let gap = 2;
        let cell_width = ((element.width as i32 - 6 - gap * 3) / 4).max(10);
        let cell_height = (element.height as i32 - 6).max(10);
        let font = ((cell_height as f64 * 0.58 * element.font_scale).round() as i32).clamp(5, 12);

        for (index, (label, value, color, bg)) in parts.into_iter().enumerate() {
            let cell_x = x + 3 + index as i32 * (cell_width + gap);
            if element.show_background {
                canvas.fill_rounded_rect(cell_x, y + 3, cell_width, cell_height, 6, bg);
                canvas.stroke_rounded_rect(
                    cell_x,
                    y + 3,
                    cell_width,
                    cell_height,
                    6,
                    [
                        255,
                        255,
                        255,
                        (settings.opacity * 42.0).clamp(0.0, 68.0) as u8,
                    ],
                );
            }
            canvas.draw_text_clipped(
                cell_x + 2,
                y + ((element.height as i32 - font) / 2).max(1) - 2,
                cell_width - 4,
                &format!("{label} {value}"),
                font.min((element.height as i32 - 8).max(5)),
                FontWeight::Bold,
                color,
            );
        }
    }

    #[allow(dead_code)]
    fn draw_hud_card(
        canvas: &mut BitmapCanvas,
        origin_x: i32,
        origin_y: i32,
        settings: &OverlaySettings,
        layout: &HudLayout,
        session: Option<&SessionSnapshot>,
    ) {
        if settings.show_background {
            let alpha = (settings.opacity * 220.0).clamp(0.0, 235.0) as u8;
            canvas.fill_rounded_rect(
                origin_x,
                origin_y,
                layout.width as i32,
                layout.height as i32,
                settings.corner_radius as i32,
                [18, 22, 30, alpha],
            );
            canvas.stroke_rounded_rect(
                origin_x,
                origin_y,
                layout.width as i32,
                layout.height as i32,
                settings.corner_radius as i32,
                [
                    80,
                    91,
                    112,
                    (settings.opacity * 120.0).clamp(0.0, 140.0) as u8,
                ],
            );
        }

        let padding = layout.padding;
        let content_width = layout.width as i32 - padding * 2;
        if content_width <= 8 {
            return;
        }

        if let Some(session) = session {
            let mut y = origin_y + padding;
            if settings.show_pp {
                let pp_label = format!("{:.2} PP", session.pp.current);
                let hero_height = (layout.hero_font as f32 * 1.08).round() as i32;
                let hero_width =
                    content_width.min((pp_label.len() as i32 * layout.hero_font / 2).max(72));
                let panel_width = content_width.min(hero_width + layout.gap * 4);
                if settings.show_background {
                    canvas.fill_rounded_rect(
                        origin_x + padding,
                        y,
                        panel_width,
                        hero_height + layout.gap,
                        7,
                        [
                            20,
                            25,
                            34,
                            (settings.opacity * 154.0).clamp(0.0, 190.0) as u8,
                        ],
                    );
                }
                canvas.draw_text_clipped(
                    origin_x + padding + layout.gap,
                    y + 1,
                    panel_width - layout.gap * 2,
                    &pp_label,
                    layout.hero_font,
                    FontWeight::Bold,
                    [246, 249, 255, 255],
                );
                y += hero_height + layout.gap * 2;
            }

            let mut metric_cells = Vec::new();
            if settings.show_if_fc {
                metric_cells.push(("IF FC".to_string(), format!("{:.2}", session.pp.if_fc)));
            }
            if settings.show_accuracy {
                let acc = session
                    .live
                    .accuracy
                    .map(|value| format!("{value:.2}%"))
                    .unwrap_or_else(|| "--".to_string());
                metric_cells.push(("ACC".to_string(), acc));
            }
            if settings.show_combo {
                metric_cells.push(("COMBO".to_string(), format!("{}x", session.live.combo)));
            }
            if settings.show_mods {
                metric_cells.push(("MODS".to_string(), session.live.mods_text.clone()));
            }

            if !metric_cells.is_empty() {
                y = draw_metric_cells(
                    canvas,
                    origin_x + padding,
                    y,
                    content_width,
                    layout,
                    settings,
                    &metric_cells,
                );
            }

            if settings.show_hits {
                let hits = &session.live.hits;
                y = draw_hit_counts(
                    canvas,
                    origin_x + padding,
                    y,
                    content_width,
                    layout.small_font,
                    hits,
                );
            }

            if settings.show_map
                && y < origin_y + layout.height as i32 - padding - layout.small_font
            {
                let map = format!(
                    "{} - {} [{}]",
                    session.beatmap.artist, session.beatmap.title, session.beatmap.difficulty_name
                );
                let map_height = (layout.small_font as f32 * 1.8).round().max(14.0) as i32;
                canvas.fill_rounded_rect(
                    origin_x + padding,
                    y,
                    content_width,
                    map_height,
                    7,
                    [
                        25,
                        30,
                        39,
                        (settings.opacity * 142.0).clamp(0.0, 175.0) as u8,
                    ],
                );
                canvas.draw_text_clipped(
                    origin_x + padding + layout.gap,
                    y + ((map_height - layout.small_font) / 2).max(1),
                    content_width - layout.gap * 2,
                    &map,
                    layout.small_font,
                    FontWeight::Semibold,
                    [216, 226, 240, 246],
                );
            }
        } else {
            canvas.draw_text_clipped(
                origin_x + padding,
                origin_y + padding,
                content_width,
                "Waiting for osu!",
                layout.body_font,
                FontWeight::Semibold,
                [210, 220, 235, 255],
            );
        }
    }

    #[allow(dead_code)]
    fn draw_metric_cells(
        canvas: &mut BitmapCanvas,
        x: i32,
        y: i32,
        max_width: i32,
        layout: &HudLayout,
        settings: &OverlaySettings,
        cells: &[(String, String)],
    ) -> i32 {
        let gap = layout.gap;
        let columns = if max_width < 128 {
            1
        } else if max_width < 220 || cells.len() <= 2 {
            2.min(cells.len().max(1) as i32)
        } else {
            cells.len().min(4) as i32
        };
        let cell_width = ((max_width - gap * (columns - 1)) / columns).max(26);
        let cell_height = ((layout.body_font as f32 * 2.05).round() as i32).max(19);

        for (index, (label, value)) in cells.iter().enumerate() {
            let row = index as i32 / columns;
            let column = index as i32 % columns;
            let cell_x = x + column * (cell_width + gap);
            let cell_y = y + row * (cell_height + gap);
            canvas.fill_rounded_rect(
                cell_x,
                cell_y,
                cell_width,
                cell_height,
                7,
                [
                    27,
                    32,
                    42,
                    (settings.opacity * 188.0).clamp(0.0, 210.0) as u8,
                ],
            );
            canvas.stroke_rounded_rect(
                cell_x,
                cell_y,
                cell_width,
                cell_height,
                7,
                [
                    85,
                    96,
                    116,
                    (settings.opacity * 96.0).clamp(0.0, 130.0) as u8,
                ],
            );
            canvas.draw_text_clipped(
                cell_x + 5,
                cell_y + 2,
                cell_width - 10,
                label,
                layout.small_font,
                FontWeight::Semibold,
                [165, 180, 202, 245],
            );
            canvas.draw_text_clipped(
                cell_x + 5,
                cell_y + 4 + layout.small_font,
                cell_width - 10,
                value,
                layout.body_font,
                FontWeight::Bold,
                [240, 245, 252, 255],
            );
        }

        let rows = ((cells.len() as i32 + columns - 1) / columns).max(1);
        y + rows * cell_height + (rows - 1) * gap + gap
    }

    #[allow(dead_code)]
    fn draw_hit_counts(
        canvas: &mut BitmapCanvas,
        x: i32,
        y: i32,
        max_width: i32,
        font_px: i32,
        hits: &crate::models::HitSnapshot,
    ) -> i32 {
        let parts = [
            ("100", hits.n100, [92, 170, 255, 255], [37, 82, 148, 150]),
            ("50", hits.n50, [255, 190, 86, 255], [145, 83, 31, 150]),
            ("MISS", hits.misses, [255, 91, 112, 255], [152, 39, 55, 150]),
            (
                "SB",
                hits.slider_breaks,
                [190, 132, 255, 255],
                [102, 66, 160, 150],
            ),
        ];
        let gap = 4.max(font_px / 3);
        let columns = if max_width < 132 { 2 } else { 4 };
        let cell_width = ((max_width - gap * (columns - 1)) / columns).max(22);
        let cell_height = (font_px as f32 * 1.75).round().max(16.0) as i32;

        for (index, (label, value, color, bg)) in parts.into_iter().enumerate() {
            let row = index as i32 / columns;
            let column = index as i32 % columns;
            let cell_x = x + column * (cell_width + gap);
            let cell_y = y + row * (cell_height + gap);
            canvas.fill_rounded_rect(cell_x, cell_y, cell_width, cell_height, 6, bg);
            canvas.stroke_rounded_rect(
                cell_x,
                cell_y,
                cell_width,
                cell_height,
                6,
                [100, 111, 132, 92],
            );
            canvas.draw_text_clipped(
                cell_x + 4,
                cell_y + ((cell_height - font_px) / 2).max(1),
                cell_width - 8,
                &format!("{label} {value}"),
                font_px,
                FontWeight::Bold,
                color,
            );
        }

        let rows = (4 + columns - 1) / columns;
        y + rows * cell_height + (rows - 1) * gap + gap
    }

    struct BitmapCanvas {
        width: u32,
        height: u32,
        data: Vec<u8>,
    }

    impl BitmapCanvas {
        fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                data: vec![0; width as usize * height as usize * 4],
            }
        }

        fn into_bitmap(self) -> Bitmap {
            Bitmap {
                width: self.width,
                data: self.data,
            }
        }

        fn blend_pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
            if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                return;
            }

            let index = ((y as u32 * self.width + x as u32) * 4) as usize;
            let alpha = color[3] as f32 / 255.0;
            let inv_alpha = 1.0 - alpha;
            self.data[index] =
                (color[2] as f32 * alpha + self.data[index] as f32 * inv_alpha) as u8;
            self.data[index + 1] =
                (color[1] as f32 * alpha + self.data[index + 1] as f32 * inv_alpha) as u8;
            self.data[index + 2] =
                (color[0] as f32 * alpha + self.data[index + 2] as f32 * inv_alpha) as u8;
            self.data[index + 3] =
                (color[3] as f32 + self.data[index + 3] as f32 * inv_alpha).min(255.0) as u8;
        }

        fn fill_rounded_rect(
            &mut self,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            radius: i32,
            color: [u8; 4],
        ) {
            let radius = radius.max(0);
            for py in y..(y + height) {
                for px in x..(x + width) {
                    let coverage = rounded_rect_coverage(px - x, py - y, width, height, radius);
                    if coverage > 0 {
                        let mut covered_color = color;
                        covered_color[3] = ((color[3] as u16 * coverage as u16) / 4) as u8;
                        self.blend_pixel(px, py, covered_color);
                    }
                }
            }
        }

        fn stroke_rounded_rect(
            &mut self,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            radius: i32,
            color: [u8; 4],
        ) {
            let radius = radius.max(0);
            for py in y..(y + height) {
                for px in x..(x + width) {
                    let lx = px - x;
                    let ly = py - y;
                    let outer = rounded_rect_coverage(lx, ly, width, height, radius);
                    if outer == 0 {
                        continue;
                    }

                    let inner = rounded_rect_coverage(
                        lx - 1,
                        ly - 1,
                        width - 2,
                        height - 2,
                        (radius - 1).max(0),
                    );
                    let coverage = outer.saturating_sub(inner);
                    if coverage > 0 {
                        let mut covered_color = color;
                        covered_color[3] = ((color[3] as u16 * coverage as u16) / 4) as u8;
                        self.blend_pixel(px, py, covered_color);
                    }
                }
            }
        }

        fn draw_text_clipped(
            &mut self,
            x: i32,
            y: i32,
            max_width: i32,
            text: &str,
            font_px: i32,
            weight: FontWeight,
            color: [u8; 4],
        ) {
            if max_width <= 0 {
                return;
            }

            let font_px = font_px.max(8);
            let Some(mask) = render_text_mask(text, font_px, weight) else {
                return;
            };

            let draw_width = mask.width.min(max_width as u32);
            for py in 0..mask.height {
                for px in 0..draw_width {
                    let alpha = mask.data[(py * mask.width + px) as usize];
                    if alpha == 0 {
                        continue;
                    }

                    let scaled_alpha = ((alpha as u16 * color[3] as u16) / 255) as u8;
                    self.blend_pixel(
                        x + px as i32,
                        y + py as i32,
                        [color[0], color[1], color[2], scaled_alpha],
                    );
                }
            }
        }
    }

    #[derive(Clone, Copy)]
    enum FontWeight {
        Normal,
        Semibold,
        Bold,
    }

    struct TextMask {
        width: u32,
        height: u32,
        data: Vec<u8>,
    }

    fn render_text_mask(text: &str, font_px: i32, weight: FontWeight) -> Option<TextMask> {
        if text.is_empty() {
            return None;
        }

        let font = overlay_font(weight)?;
        let px = font_px.max(8) as f32;
        let baseline = (px * 1.15).round() as i32;
        let text_height = (px * 1.55).round().clamp(14.0, 160.0) as i32;
        let mut glyphs = Vec::new();
        let mut pen_x = 0.0f32;
        let mut text_width = 1i32;

        for character in text.chars() {
            let (metrics, bitmap) = font.rasterize(character, px);
            let glyph_x = pen_x.round() as i32 + metrics.xmin;
            let glyph_y = baseline - metrics.ymin - metrics.height as i32;
            text_width = text_width.max(glyph_x + metrics.width as i32 + 1);
            pen_x += metrics.advance_width;
            glyphs.push((glyph_x, glyph_y, metrics.width, metrics.height, bitmap));
        }

        let text_width = text_width.clamp(1, 2400) as u32;
        let text_height = text_height.max(1) as u32;
        let mut alpha = vec![0u8; text_width as usize * text_height as usize];

        for (glyph_x, glyph_y, glyph_width, glyph_height, bitmap) in glyphs {
            for gy in 0..glyph_height {
                let target_y = glyph_y + gy as i32;
                if target_y < 0 || target_y >= text_height as i32 {
                    continue;
                }

                for gx in 0..glyph_width {
                    let target_x = glyph_x + gx as i32;
                    if target_x < 0 || target_x >= text_width as i32 {
                        continue;
                    }

                    let src_index = gy * glyph_width + gx;
                    let dst_index = target_y as usize * text_width as usize + target_x as usize;
                    alpha[dst_index] = alpha[dst_index].max(bitmap[src_index]);
                }
            }
        }

        Some(TextMask {
            width: text_width,
            height: text_height,
            data: alpha,
        })
    }

    fn overlay_font(weight: FontWeight) -> Option<&'static fontdue::Font> {
        static REGULAR: OnceLock<Option<fontdue::Font>> = OnceLock::new();
        static SEMIBOLD: OnceLock<Option<fontdue::Font>> = OnceLock::new();
        static BOLD: OnceLock<Option<fontdue::Font>> = OnceLock::new();

        let slot = match weight {
            FontWeight::Normal => &REGULAR,
            FontWeight::Semibold => &SEMIBOLD,
            FontWeight::Bold => &BOLD,
        };

        slot.get_or_init(|| {
            let path = match weight {
                FontWeight::Normal => "C:\\Windows\\Fonts\\segoeui.ttf",
                FontWeight::Semibold => "C:\\Windows\\Fonts\\seguisb.ttf",
                FontWeight::Bold => "C:\\Windows\\Fonts\\segoeuib.ttf",
            };
            let bytes = fs::read(path).ok()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
        })
        .as_ref()
    }

    fn rounded_rect_coverage(x: i32, y: i32, width: i32, height: i32, radius: i32) -> u8 {
        if radius <= 0 {
            return 4;
        }

        [(0.25f32, 0.25f32), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
            .into_iter()
            .filter(|(sx, sy)| {
                inside_rounded_rect_sample(
                    x as f32 + sx,
                    y as f32 + sy,
                    width as f32,
                    height as f32,
                    radius as f32,
                )
            })
            .count() as u8
    }

    fn inside_rounded_rect_sample(x: f32, y: f32, width: f32, height: f32, radius: f32) -> bool {
        if x < 0.0 || y < 0.0 || x > width || y > height {
            return false;
        }

        let inner_left = radius;
        let inner_right = width - radius;
        let inner_top = radius;
        let inner_bottom = height - radius;

        if (x >= inner_left && x <= inner_right) || (y >= inner_top && y <= inner_bottom) {
            return true;
        }

        let cx = if x < inner_left {
            inner_left
        } else {
            inner_right
        };
        let cy = if y < inner_top {
            inner_top
        } else {
            inner_bottom
        };
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= radius * radius
    }

    fn window_covers_monitor_rect(window_rect: &RECT, monitor_rect: &RECT) -> bool {
        let tolerance = 24;

        rect_covers_monitor(window_rect, &monitor_rect, tolerance)
            || rect_area_overlap_ratio(window_rect, monitor_rect) >= 0.92
    }

    fn rect_covers_monitor(rect: &RECT, monitor_rect: &RECT, tolerance: i32) -> bool {
        (rect.left - monitor_rect.left).abs() <= tolerance
            && (rect.top - monitor_rect.top).abs() <= tolerance
            && (rect.right - monitor_rect.right).abs() <= tolerance
            && (rect.bottom - monitor_rect.bottom).abs() <= tolerance
    }

    fn rect_area_overlap_ratio(rect: &RECT, monitor_rect: &RECT) -> f64 {
        let left = rect.left.max(monitor_rect.left);
        let top = rect.top.max(monitor_rect.top);
        let right = rect.right.min(monitor_rect.right);
        let bottom = rect.bottom.min(monitor_rect.bottom);
        let intersection_width = right.saturating_sub(left).max(0) as f64;
        let intersection_height = bottom.saturating_sub(top).max(0) as f64;
        let monitor_width = monitor_rect.right.saturating_sub(monitor_rect.left).max(1) as f64;
        let monitor_height = monitor_rect.bottom.saturating_sub(monitor_rect.top).max(1) as f64;

        (intersection_width * intersection_height) / (monitor_width * monitor_height)
    }

    fn monitor_rect_for_window(hwnd: HWND) -> Option<RECT> {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };

        if monitor.is_invalid() {
            return None;
        }

        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };

        let ok = unsafe { GetMonitorInfoW(monitor, &mut monitor_info).as_bool() };

        if ok {
            Some(monitor_info.rcMonitor)
        } else {
            None
        }
    }

    fn virtual_key_from_binding(binding: &str) -> Option<i32> {
        let normalized = binding.trim().to_ascii_uppercase();

        match normalized.as_str() {
            "INSERT" | "INS" => Some(i32::from(VK_INSERT.0)),
            "HOME" => Some(i32::from(VK_HOME.0)),
            "DELETE" | "DEL" => Some(i32::from(VK_DELETE.0)),
            "END" => Some(i32::from(VK_END.0)),
            "PAGEUP" | "PAGE UP" | "PGUP" => Some(i32::from(VK_PRIOR.0)),
            "PAGEDOWN" | "PAGE DOWN" | "PGDN" => Some(i32::from(VK_NEXT.0)),
            "TAB" => Some(i32::from(VK_TAB.0)),
            "SPACE" => Some(i32::from(VK_SPACE.0)),
            "ENTER" | "RETURN" => Some(i32::from(VK_RETURN.0)),
            "LEFT" => Some(i32::from(VK_LEFT.0)),
            "RIGHT" => Some(i32::from(VK_RIGHT.0)),
            "UP" => Some(i32::from(VK_UP.0)),
            "DOWN" => Some(i32::from(VK_DOWN.0)),
            "F1" => Some(i32::from(VK_F1.0)),
            "F2" => Some(i32::from(VK_F2.0)),
            "F3" => Some(i32::from(VK_F3.0)),
            "F4" => Some(i32::from(VK_F4.0)),
            "F5" => Some(i32::from(VK_F5.0)),
            "F6" => Some(i32::from(VK_F6.0)),
            "F7" => Some(i32::from(VK_F7.0)),
            "F8" => Some(i32::from(VK_F8.0)),
            "F9" => Some(i32::from(VK_F9.0)),
            "F10" => Some(i32::from(VK_F10.0)),
            "F11" => Some(i32::from(VK_F11.0)),
            "F12" => Some(i32::from(VK_F12.0)),
            _ if normalized.len() == 1 => normalized.chars().next().and_then(|char| match char {
                '0'..='9' | 'A'..='Z' => Some(char as i32),
                _ => None,
            }),
            _ => None,
        }
    }
}
