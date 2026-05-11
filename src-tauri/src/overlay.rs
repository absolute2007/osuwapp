use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

use crate::models::{AppSnapshot, OverlaySettings};

const OVERLAY_LABEL: &str = "overlay";
const DEFAULT_OVERLAY_WIDTH: f64 = 420.0;
const DEFAULT_OVERLAY_HEIGHT: f64 = 248.0;
const BASE_EDITOR_PANEL_WIDTH: f64 = 760.0;
const BASE_EDITOR_PANEL_HEIGHT: f64 = 520.0;
#[cfg(target_os = "windows")]
const OPEN_OVERLAY_SETTINGS_EVENT: &str = "open-overlay-settings";

pub fn spawn_overlay_manager(
    app: AppHandle,
    settings: Arc<Mutex<OverlaySettings>>,
    latest_snapshot: Arc<Mutex<Option<AppSnapshot>>>,
    running: Arc<AtomicBool>,
) {
    thread::spawn(move || loop {
        if !running.load(Ordering::SeqCst) {
            close_overlay(&app);
            break;
        }

        let current_settings = settings
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
            .normalized();
        let poll_interval_ms = current_settings.data_update_interval_ms;

        #[cfg(target_os = "windows")]
        {
            let osu_target = windows::find_osu_target();
            let settings_hotkey_pressed =
                windows::poll_overlay_settings_hotkey(&current_settings, osu_target.as_ref());
            if settings_hotkey_pressed
                && !osu_target
                    .as_ref()
                    .is_some_and(|target| target.is_fullscreen_surface)
            {
                let _ = app.emit(OPEN_OVERLAY_SETTINGS_EVENT, ());
                open_overlay_settings_window(&app);
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
        thread::sleep(Duration::from_millis(poll_interval_ms));
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
            return;
        }

        if let Some(main_window) = app.get_webview_window("main") {
            let is_focused = main_window.is_focused().unwrap_or(false);
            let is_minimized = main_window.is_minimized().unwrap_or(false);

            if is_focused && !is_minimized {
                hide_overlay(app);
                return;
            }
        }

        let Some(target) = osu_target else {
            hide_overlay(app);
            return;
        };

        if target.is_fullscreen_surface {
            hide_overlay(app);

            let snapshot = latest_snapshot.lock().ok().and_then(|guard| guard.clone());
            if let Some(dll_dir) = resolve_asdf_overlay_dir(app) {
                if let Some(next_settings) = windows::sync_ingame_overlay(
                    &dll_dir,
                    &target,
                    settings,
                    snapshot.as_ref(),
                    settings_hotkey_pressed,
                ) {
                    if let Ok(mut guard) = settings_store.lock() {
                        *guard = next_settings.clone();
                    }
                    let _ = crate::storage::save_overlay_settings(app, &next_settings);
                    let _ = app.emit("overlay-settings-updated", &next_settings);
                }
            }
            return;
        }

        if target.is_minimized {
            hide_overlay(app);
            windows::stop_ingame_overlay();
            return;
        }

        if !target.is_foreground {
            hide_overlay(app);
            windows::stop_ingame_overlay();
            return;
        }

        windows::stop_ingame_overlay();

        let overlay_width = settings.width;
        let overlay_height = settings.height;
        let x = target.rect.left.saturating_add(settings.offset_x);
        let y = target.rect.top.saturating_add(settings.offset_y);

        let Ok(window) = ensure_overlay_window(app) else {
            return;
        };

        let _ = window.set_size(Size::Physical(PhysicalSize::new(
            overlay_width,
            overlay_height,
        )));
        let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
        let _ =
            windows::position_capture_overlay_window(&window, x, y, overlay_width, overlay_height);
        let _ = window.show();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        let _ = settings_store;
        let _ = settings;
    }
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
}

pub fn hide_overlay_windows(app: &AppHandle) {
    hide_overlay(app);
}

#[cfg(target_os = "windows")]
fn resolve_asdf_overlay_dir(app: &AppHandle) -> Option<PathBuf> {
    let resource_dir = app.path().resource_dir().ok();
    let candidates = [
        resource_dir
            .as_ref()
            .map(|dir| dir.join("resources").join("asdf-overlay")),
        std::env::current_dir()
            .ok()
            .map(|dir| dir.join("src-tauri").join("resources").join("asdf-overlay")),
        std::env::current_dir()
            .ok()
            .map(|dir| dir.join("resources").join("asdf-overlay")),
    ];

    candidates
        .into_iter()
        .flatten()
        .find(|dir| dir.join("asdf_overlay-x64.dll").exists())
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
    .transparent(false)
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
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex, OnceLock,
        },
        time::{Duration, Instant},
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

    #[derive(Clone, Copy)]
    pub struct OsuWindowTarget {
        pub pid: u32,
        pub rect: Rect,
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

        Some(OsuWindowTarget {
            pid: process.pid,
            rect: Rect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
            is_minimized: unsafe { IsIconic(hwnd).as_bool() },
            is_foreground: unsafe { GetForegroundWindow() == hwnd },
            is_fullscreen_surface: window_covers_monitor(hwnd, &rect),
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
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
            );
            let _ = SetForegroundWindow(hwnd);
        }
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
        last_frame_key: Option<String>,
        editor_active: bool,
        editor_settings: Option<OverlaySettings>,
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
        cursor_x: i32,
        cursor_y: i32,
        offset_x: i32,
        offset_y: i32,
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
                last_frame_key: None,
                editor_active: false,
                editor_settings: None,
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
            self.last_frame_key = None;
            self.editor_active = false;
            self.editor_settings = None;
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
                self.failed_until = Some(Instant::now() + Duration::from_secs(3));
                return None;
            }

            if toggle_editor {
                self.editor_active = !self.editor_active;
                self.editor_settings = self.editor_active.then(|| settings.clone());
                self.selected_element = OverlayElement::Pp;
                self.editing_field = None;
                self.edit_buffer.clear();
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
                SurfaceMode::Editor
            } else {
                let bounds = overlay_bounds(active_settings);
                SurfaceMode::Hud {
                    x: bounds.0,
                    y: bounds.1,
                }
            };

            if self.surface_mode != desired_mode {
                apply_surface_mode(runtime, conn, window_id, desired_mode);
                self.surface_mode = desired_mode;
            }

            let session = snapshot.and_then(|item| item.session.as_ref());
            let frame_key = if self.editor_active {
                ingame_editor_frame_key(
                    active_settings,
                    session,
                    self.window_size,
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

            let x64_dll = dll_dir.join("asdf_overlay-x64.dll");
            let x86_dll = dll_dir.join("asdf_overlay-x86.dll");
            let dll = OverlayDll {
                x64: Some(&x64_dll),
                x86: Some(&x86_dll),
                arm64: None,
            };

            match runtime.block_on(inject(pid, dll, Some(Duration::from_secs(4)))) {
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
            let event_limit = if self.editor_active { 64 } else { 8 };
            for _ in 0..event_limit {
                let event = runtime.block_on(async {
                    tokio::time::timeout(Duration::from_millis(1), events.recv()).await
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
                            SurfaceMode::Editor
                        } else {
                            let bounds = overlay_bounds(settings);
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

                    let (panel_x, panel_y) = editor_panel_origin(self.window_size, settings);
                    let scale = EditorPanelScale::from_settings(settings);
                    let (local_x, local_y) =
                        scale.base_point(cursor.client.x - panel_x, cursor.client.y - panel_y);

                    match cursor.event {
                        CursorEvent::Action {
                            state: CursorInputState::Pressed { .. },
                            action: CursorAction::Left,
                        } => {
                            if let Some((element, offset_x, offset_y)) =
                                hit_test_overlay_element(settings, cursor.client.x, cursor.client.y)
                            {
                                self.drag_origin = Some(DragOrigin {
                                    element,
                                    cursor_x: cursor.client.x,
                                    cursor_y: cursor.client.y,
                                    offset_x,
                                    offset_y,
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
                                .saturating_add(cursor.client.x.saturating_sub(drag.cursor_x));
                            let next_y = drag
                                .offset_y
                                .saturating_add(cursor.client.y.saturating_sub(drag.cursor_y));
                            set_overlay_element_position(settings, drag.element, next_x, next_y);
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
                                    .saturating_add(cursor.client.x.saturating_sub(drag.cursor_x));
                                let next_y = drag
                                    .offset_y
                                    .saturating_add(cursor.client.y.saturating_sub(drag.cursor_y));
                                set_overlay_element_position(
                                    settings,
                                    drag.element,
                                    next_x,
                                    next_y,
                                );
                                let normalized = settings.clone().normalized();
                                *settings = normalized.clone();
                                self.pending_settings = Some(normalized);
                                self.last_frame_key = None;
                            }
                            self.drag_origin = None;

                            if in_rect(local_x, local_y, 648, 20, 84, 32) {
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

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum SurfaceMode {
        Unknown,
        Hud { x: i32, y: i32 },
        Editor,
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
            SurfaceMode::Editor => (
                PercentLength::Length(0.0),
                PercentLength::Length(0.0),
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
        selected_element: OverlayElement,
        editing_field: Option<EditorField>,
        edit_buffer: &str,
    ) -> String {
        format!(
            "editor:{window_size:?}:{}:{:?}:{}:{}",
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
        selected_element: OverlayElement,
        editing_field: Option<EditorField>,
        edit_buffer: &str,
    ) -> Bitmap {
        let panel_width = settings.editor_panel_width;
        let panel_height = settings.editor_panel_height;
        let (surface_width, surface_height) = window_size.unwrap_or((1280, 720));
        let mut canvas = BitmapCanvas::new(
            surface_width.max(panel_width),
            surface_height.max(panel_height),
        );
        draw_overlay_elements(&mut canvas, 0, 0, settings, session, true);
        draw_selected_element_outline(&mut canvas, settings, selected_element);

        let (panel_x, panel_y) = editor_panel_origin(Some((canvas.width, canvas.height)), settings);
        let scale = EditorPanelScale::from_settings(settings);
        canvas.fill_rounded_rect(
            panel_x,
            panel_y,
            panel_width as i32,
            panel_height as i32,
            18,
            [14, 18, 25, 226],
        );
        canvas.stroke_rounded_rect(
            panel_x,
            panel_y,
            panel_width as i32,
            panel_height as i32,
            18,
            [96, 112, 138, 120],
        );
        canvas.draw_text_clipped(
            panel_x + scale.x(26),
            panel_y + scale.y(22),
            scale.w(360),
            "Overlay editor",
            scale.font(20),
            FontWeight::Semibold,
            [245, 248, 253, 255],
        );
        canvas.draw_text_clipped(
            panel_x + scale.x(26),
            panel_y + scale.y(52),
            scale.w(360),
            "End",
            scale.font(12),
            FontWeight::Normal,
            [150, 163, 184, 235],
        );
        draw_editor_button(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            648,
            20,
            84,
            32,
            "Close",
        );

        canvas.draw_text_clipped(
            panel_x + scale.x(26),
            panel_y + scale.y(94),
            scale.w(300),
            "Live HUD",
            scale.font(15),
            FontWeight::Semibold,
            [234, 239, 247, 255],
        );
        canvas.draw_text_clipped(
            panel_x + scale.x(26),
            panel_y + scale.y(124),
            scale.w(300),
            "Drag a block. Size and scale apply to the selected block.",
            scale.font(12),
            FontWeight::Normal,
            [151, 164, 184, 245],
        );
        canvas.draw_text_clipped(
            panel_x + scale.x(26),
            panel_y + scale.y(154),
            scale.w(300),
            &format!("Selected: {}", selected_element_label(selected_element)),
            scale.font(13),
            FontWeight::Semibold,
            [220, 231, 246, 255],
        );

        let active = selected_element_settings(settings, selected_element);

        draw_editor_row(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            88,
            "Enabled",
            settings.enabled,
        );
        draw_editor_row(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            132,
            "Background",
            active.show_background,
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            190,
            "Opacity",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::Opacity,
                &format!("{:.0}%", settings.opacity * 100.0),
            ),
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            244,
            "Scale",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::Scale,
                &format!("{:.0}%", active.scale * 100.0),
            ),
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            298,
            "Text",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::FontScale,
                &format!("{:.0}%", active.font_scale * 100.0),
            ),
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            26,
            318,
            "X",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::X,
                &active.x.to_string(),
            ),
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            196,
            318,
            "Y",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::Y,
                &active.y.to_string(),
            ),
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            26,
            372,
            "Width",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::Width,
                &active.width.to_string(),
            ),
        );
        draw_editor_stepper(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            196,
            372,
            "Height",
            &editor_value_text(
                editing_field,
                edit_buffer,
                EditorField::Height,
                &active.height.to_string(),
            ),
        );

        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            372,
            "PP",
            settings.show_pp,
        );
        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            472,
            372,
            "IF FC",
            settings.show_if_fc,
        );
        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            554,
            372,
            "ACC",
            settings.show_accuracy,
        );
        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            636,
            372,
            "Hits",
            settings.show_hits,
        );
        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            390,
            420,
            "Combo",
            settings.show_combo,
        );
        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            472,
            420,
            "Mods",
            settings.show_mods,
        );
        draw_metric_toggle(
            &mut canvas,
            scale,
            panel_x,
            panel_y,
            554,
            420,
            "Map",
            settings.show_map,
        );

        canvas.into_bitmap()
    }

    fn editor_panel_origin(
        window_size: Option<(u32, u32)>,
        settings: &OverlaySettings,
    ) -> (i32, i32) {
        let (width, height) = window_size.unwrap_or((1280, 720));
        (
            ((width as i32 - settings.editor_panel_width as i32) / 2).max(0),
            ((height as i32 - settings.editor_panel_height as i32) / 2).max(0),
        )
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
        canvas.fill_rounded_rect(x, y, w, h, scale.font(10), [35, 41, 52, 235]);
        canvas.stroke_rounded_rect(x, y, w, h, scale.font(10), [94, 107, 130, 130]);
        canvas.draw_text_clipped(
            x + scale.x(14),
            y + scale.y(8),
            w - scale.w(28),
            label,
            scale.font(13),
            FontWeight::Semibold,
            [234, 239, 247, 255],
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
                [42, 160, 88, 230]
            } else {
                [40, 46, 58, 230]
            },
        );
        canvas.stroke_rounded_rect(
            toggle_x,
            y,
            scale.w(44),
            scale.h(24),
            scale.font(12),
            [100, 116, 145, 95],
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
            [238, 243, 250, 245],
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
            [151, 164, 184, 245],
        );
        draw_editor_button(canvas, scale, panel_x, panel_y, x, y + 20, 34, 30, "-");
        canvas.fill_rounded_rect(
            actual_x + scale.x(42),
            actual_y + scale.y(20),
            scale.w(76),
            scale.h(30),
            scale.font(9),
            [28, 34, 44, 230],
        );
        canvas.stroke_rounded_rect(
            actual_x + scale.x(42),
            actual_y + scale.y(20),
            scale.w(76),
            scale.h(30),
            scale.font(9),
            [82, 96, 120, 115],
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
            scale.font(10),
            if enabled {
                [42, 160, 88, 230]
            } else {
                [32, 38, 48, 230]
            },
        );
        canvas.stroke_rounded_rect(
            x,
            y,
            scale.w(72),
            scale.h(34),
            scale.font(10),
            [95, 113, 142, 125],
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
        for (field, sx, sy) in [
            (EditorField::Opacity, 390, 190),
            (EditorField::Scale, 390, 244),
            (EditorField::FontScale, 390, 298),
            (EditorField::X, 26, 318),
            (EditorField::Y, 196, 318),
            (EditorField::Width, 26, 372),
            (EditorField::Height, 196, 372),
        ] {
            if in_rect(x, y, sx + 42, sy + 20, 76, 30) {
                return Some(field);
            }
        }

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
        for (element, bx, by) in [
            (OverlayElement::Pp, 390, 372),
            (OverlayElement::Stats, 472, 372),
            (OverlayElement::Hits, 636, 372),
            (OverlayElement::Map, 554, 420),
        ] {
            if in_rect(x, y, bx, by, 72, 34) {
                if *selected_element == element {
                    match element {
                        OverlayElement::Pp => settings.show_pp = !settings.show_pp,
                        OverlayElement::Stats => {
                            let next = !(settings.show_if_fc
                                || settings.show_accuracy
                                || settings.show_combo
                                || settings.show_mods);
                            settings.show_if_fc = next;
                            settings.show_accuracy = next;
                            settings.show_combo = next;
                            settings.show_mods = next;
                        }
                        OverlayElement::Hits => settings.show_hits = !settings.show_hits,
                        OverlayElement::Map => settings.show_map = !settings.show_map,
                        OverlayElement::Whole => {}
                    }
                } else {
                    *selected_element = element;
                }
                return true;
            }
        }

        if in_rect(x, y, 628, 88, 44, 24) {
            settings.enabled = !settings.enabled;
            return true;
        }
        if in_rect(x, y, 628, 132, 44, 24) {
            let element = selected_element_settings_mut(settings, *selected_element);
            element.show_background = !element.show_background;
            return true;
        }
        if stepper_click(x, y, 390, 190) == Some(-1) {
            settings.opacity -= 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 190) == Some(1) {
            settings.opacity += 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 244) == Some(-1) {
            selected_element_settings_mut(settings, *selected_element).scale -= 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 244) == Some(1) {
            selected_element_settings_mut(settings, *selected_element).scale += 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 298) == Some(-1) {
            selected_element_settings_mut(settings, *selected_element).font_scale -= 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 298) == Some(1) {
            selected_element_settings_mut(settings, *selected_element).font_scale += 0.05;
            return true;
        }
        if stepper_click(x, y, 26, 318) == Some(-1) {
            selected_element_settings_mut(settings, *selected_element).x -= 5;
            return true;
        }
        if stepper_click(x, y, 26, 318) == Some(1) {
            selected_element_settings_mut(settings, *selected_element).x += 5;
            return true;
        }
        if stepper_click(x, y, 196, 318) == Some(-1) {
            selected_element_settings_mut(settings, *selected_element).y -= 5;
            return true;
        }
        if stepper_click(x, y, 196, 318) == Some(1) {
            selected_element_settings_mut(settings, *selected_element).y += 5;
            return true;
        }
        if stepper_click(x, y, 26, 372) == Some(-1) {
            let element = selected_element_settings_mut(settings, *selected_element);
            element.width = element.width.saturating_sub(10);
            return true;
        }
        if stepper_click(x, y, 26, 372) == Some(1) {
            selected_element_settings_mut(settings, *selected_element).width += 10;
            return true;
        }
        if stepper_click(x, y, 196, 372) == Some(-1) {
            let element = selected_element_settings_mut(settings, *selected_element);
            element.height = element.height.saturating_sub(10);
            return true;
        }
        if stepper_click(x, y, 196, 372) == Some(1) {
            selected_element_settings_mut(settings, *selected_element).height += 10;
            return true;
        }
        if in_rect(x, y, 390, 372, 72, 34) {
            settings.show_pp = !settings.show_pp;
            return true;
        }
        if in_rect(x, y, 472, 372, 72, 34) {
            settings.show_if_fc = !settings.show_if_fc;
            return true;
        }
        if in_rect(x, y, 554, 372, 72, 34) {
            settings.show_accuracy = !settings.show_accuracy;
            return true;
        }
        if in_rect(x, y, 636, 372, 72, 34) {
            settings.show_hits = !settings.show_hits;
            return true;
        }
        if in_rect(x, y, 390, 420, 72, 34) {
            settings.show_combo = !settings.show_combo;
            return true;
        }
        if in_rect(x, y, 472, 420, 72, 34) {
            settings.show_mods = !settings.show_mods;
            return true;
        }
        if in_rect(x, y, 554, 420, 72, 34) {
            settings.show_map = !settings.show_map;
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

            left = left.min(element.x);
            top = top.min(element.y);
            right = right.max(element.x + element.width as i32);
            bottom = bottom.max(element.y + element.height as i32);
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
    ) -> Option<(OverlayElement, i32, i32)> {
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
            if visible
                && item.enabled
                && in_rect(x, y, item.x, item.y, item.width as i32, item.height as i32)
            {
                return Some((element, item.x, item.y));
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
            return Some((OverlayElement::Whole, settings.offset_x, settings.offset_y));
        }

        None
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
    ) {
        let item = selected_element_settings(settings, element);
        canvas.stroke_rounded_rect(
            item.x - 2,
            item.y - 2,
            item.width as i32 + 4,
            item.height as i32 + 4,
            8,
            [255, 224, 92, 230],
        );
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
                settings.pp_panel.x + offset_x,
                settings.pp_panel.y + offset_y,
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
                settings.map_panel.x + offset_x,
                settings.map_panel.y + offset_y,
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
                x,
                y,
                width as i32,
                height as i32,
                7,
                [
                    18,
                    22,
                    30,
                    (settings.opacity * 224.0).clamp(0.0, 238.0) as u8,
                ],
            );
            canvas.stroke_rounded_rect(
                x,
                y,
                width as i32,
                height as i32,
                7,
                [
                    92,
                    108,
                    134,
                    (settings.opacity * 110.0).clamp(0.0, 150.0) as u8,
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
        let inset = 5;
        let max_font_by_height = ((height as i32 - inset * 2) as f64 / 1.35).floor() as i32;
        let max_font_by_width =
            ((width as f64 - inset as f64 * 2.0) / text.len().max(1) as f64 * 1.75).floor() as i32;
        let font = ((20.0 * scale * font_scale).round() as i32)
            .min(max_font_by_height)
            .min(max_font_by_width)
            .clamp(6, 28);
        canvas.draw_text_clipped(
            x + inset,
            y + ((height as i32 - font) / 2).max(1) - 2,
            width as i32 - inset * 2,
            text,
            font,
            FontWeight::Bold,
            color,
        );
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

        let x = element.x + offset_x;
        let y = element.y + offset_y;
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
        let gap = 3;
        let cell_width = ((element.width as i32 - 8 - gap * (columns - 1)) / columns).max(12);
        let available_height = element.height as i32 - 6;
        let label_font =
            ((available_height as f64 * 0.32 * element.font_scale).round() as i32).clamp(5, 11);
        let value_font =
            ((available_height as f64 * 0.46 * element.font_scale).round() as i32).clamp(6, 14);

        for (index, (label, value)) in cells.iter().enumerate() {
            let cell_x = x + 4 + index as i32 * (cell_width + gap);
            if element.show_background {
                canvas.fill_rounded_rect(
                    cell_x,
                    y + 4,
                    cell_width,
                    element.height as i32 - 8,
                    5,
                    [26, 32, 43, 120],
                );
            }
            canvas.draw_text_clipped(
                cell_x + 3,
                y + 5,
                cell_width - 6,
                label,
                label_font.min((element.height as i32 / 3).max(5)),
                FontWeight::Semibold,
                [178, 193, 216, 245],
            );
            canvas.draw_text_clipped(
                cell_x + 3,
                y + 6 + label_font,
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
        let x = element.x + offset_x;
        let y = element.y + offset_y;
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
            ("100", hits.n100, [92, 170, 255, 255], [37, 82, 148, 180]),
            ("50", hits.n50, [255, 190, 86, 255], [145, 83, 31, 180]),
            ("MISS", hits.misses, [255, 91, 112, 255], [152, 39, 55, 180]),
            (
                "SB",
                hits.slider_breaks,
                [190, 132, 255, 255],
                [102, 66, 160, 180],
            ),
        ];
        let gap = 3;
        let cell_width = ((element.width as i32 - 8 - gap * 3) / 4).max(10);
        let cell_height = (element.height as i32 - 8).max(10);
        let font = ((cell_height as f64 * 0.62 * element.font_scale).round() as i32).clamp(5, 12);

        for (index, (label, value, color, bg)) in parts.into_iter().enumerate() {
            let cell_x = x + 4 + index as i32 * (cell_width + gap);
            if element.show_background {
                canvas.fill_rounded_rect(cell_x, y + 4, cell_width, cell_height, 5, bg);
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

    fn window_covers_monitor(hwnd: HWND, window_rect: &RECT) -> bool {
        let Some(monitor_rect) = monitor_rect_for_window(hwnd) else {
            return false;
        };
        let tolerance = 2;

        (window_rect.left - monitor_rect.left).abs() <= tolerance
            && (window_rect.top - monitor_rect.top).abs() <= tolerance
            && (window_rect.right - monitor_rect.right).abs() <= tolerance
            && (window_rect.bottom - monitor_rect.bottom).abs() <= tolerance
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
