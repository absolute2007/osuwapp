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
const OVERLAY_POLL_MS: u64 = 30;
const DEFAULT_OVERLAY_WIDTH: f64 = 420.0;
const DEFAULT_OVERLAY_HEIGHT: f64 = 248.0;
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

        #[cfg(target_os = "windows")]
        {
            let current_settings = settings
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default()
                .normalized();

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
            let current_settings = settings
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default()
                .normalized();

            sync_overlay_window(
                &app,
                &settings,
                &current_settings,
                &latest_snapshot,
                None,
                false,
            );
        }
        thread::sleep(Duration::from_millis(OVERLAY_POLL_MS));
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
        use std::sync::{Mutex, OnceLock};

        static LAST_TARGET_RECT: OnceLock<Mutex<Option<windows::Rect>>> = OnceLock::new();

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
            return;
        }

        windows::stop_ingame_overlay();

        if !target.is_foreground {
            hide_overlay(app);
            return;
        }

        let last_target_rect = LAST_TARGET_RECT.get_or_init(|| Mutex::new(None));
        let mut last_target_rect_guard = match last_target_rect.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        let target_for_layout = if target.rect.is_valid() {
            *last_target_rect_guard = Some(target.rect);
            target.rect
        } else if let Some(previous_rect) = *last_target_rect_guard {
            previous_rect
        } else {
            hide_overlay(app);
            return;
        };

        let overlay_width = settings.width;
        let overlay_height = settings.height;
        let x = target_for_layout.left.saturating_add(settings.offset_x);
        let y = target_for_layout.top.saturating_add(settings.offset_y);

        let Ok(window) = ensure_overlay_window(app) else {
            return;
        };

        let _ = window.set_size(Size::Physical(PhysicalSize::new(
            overlay_width,
            overlay_height,
        )));
        let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
        let _ = windows::position_overlay_window(
            &window,
            target.hwnd,
            x,
            y,
            overlay_width,
            overlay_height,
            false,
        );
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        let _ = settings_store;
        let _ = settings;
    }
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

fn ensure_overlay_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        configure_overlay_window(&window, false)?;
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
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .shadow(false)
    .inner_size(DEFAULT_OVERLAY_WIDTH, DEFAULT_OVERLAY_HEIGHT)
    .build()
    .map_err(|error| error.to_string())?;

    configure_overlay_window(&window, false)?;

    Ok(window)
}

fn configure_overlay_window(window: &WebviewWindow, interactive: bool) -> Result<(), String> {
    window
        .set_always_on_top(false)
        .map_err(|error| error.to_string())?;
    window
        .set_focusable(interactive)
        .map_err(|error| error.to_string())?;
    window
        .set_ignore_cursor_events(!interactive)
        .map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    windows::configure_native_overlay(window, interactive)?;

    Ok(())
}

#[cfg(target_os = "windows")]
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

    use crate::models::{OverlaySettings, SessionPhase, SessionSnapshot};
    use asdf_overlay_client::{
        common::{
            cursor::Cursor,
            request::{BlockInput, ListenInput, SetAnchor, SetBlockingCursor, SetPosition},
            size::PercentLength,
        },
        event::{
            input::{
                CursorAction, CursorEvent, CursorInputState, InputEvent, KeyInputState,
                KeyboardInput,
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
                    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE,
                    HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
                    SWP_NOSENDCHANGING, SWP_NOSIZE, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
                    WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
                    WS_EX_TRANSPARENT,
                },
            },
        },
    };

    const EXCLUDE_WORDS: [&str; 2] = ["umu-run", "waitforexitandrun"];
    static INGAME_STATE: OnceLock<Mutex<InjectedOverlayState>> = OnceLock::new();

    #[derive(Clone, Copy)]
    pub struct OsuWindowTarget {
        pub pid: u32,
        pub hwnd: HWND,
        pub rect: Rect,
        pub is_minimized: bool,
        pub is_foreground: bool,
        pub is_fullscreen_surface: bool,
    }

    #[derive(Clone, Copy)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

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
            hwnd,
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

    pub fn configure_native_overlay(
        window: &WebviewWindow,
        interactive: bool,
    ) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let current_ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let cleared_ex_style =
                current_ex_style & !WS_EX_APPWINDOW.0 & !WS_EX_TRANSPARENT.0 & !WS_EX_NOACTIVATE.0;

            let next_ex_style = if interactive {
                cleared_ex_style | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0
            } else {
                cleared_ex_style
                    | WS_EX_LAYERED.0
                    | WS_EX_TOOLWINDOW.0
                    | WS_EX_TRANSPARENT.0
                    | WS_EX_NOACTIVATE.0
            };

            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next_ex_style as isize);
        }

        Ok(())
    }

    pub fn position_overlay_window(
        window: &WebviewWindow,
        target_hwnd: HWND,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        interactive: bool,
    ) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        unsafe {
            let _ = SetWindowLongPtrW(
                hwnd,
                GWLP_HWNDPARENT,
                if interactive {
                    0
                } else {
                    target_hwnd.0 as isize
                },
            );
            let _ = ShowWindow(
                hwnd,
                if interactive {
                    SW_SHOW
                } else {
                    SW_SHOWNOACTIVATE
                },
            );
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width as i32,
                height as i32,
                if interactive {
                    SWP_NOOWNERZORDER | SWP_NOSENDCHANGING
                } else {
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING
                },
            );

            if interactive {
                let _ = SetForegroundWindow(hwnd);
            }
        }

        Ok(())
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
        pending_settings: Option<OverlaySettings>,
        failed_until: Option<Instant>,
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
                SurfaceMode::Hud {
                    x: active_settings.offset_x,
                    y: active_settings.offset_y,
                }
            };

            if self.surface_mode != desired_mode {
                apply_surface_mode(runtime, conn, window_id, desired_mode);
                self.surface_mode = desired_mode;
            }

            let session = snapshot.and_then(|item| item.session.as_ref());
            let frame_key = if self.editor_active {
                ingame_editor_frame_key(active_settings, session, self.window_size)
            } else {
                ingame_frame_key(active_settings, session)
            };
            if self.last_frame_key.as_deref() == Some(frame_key.as_str()) {
                return self.pending_settings.take();
            }

            let bitmap = if self.editor_active {
                render_ingame_editor_bitmap(active_settings, session, self.window_size)
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
            for _ in 0..8 {
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
                            SurfaceMode::Hud {
                                x: settings.offset_x,
                                y: settings.offset_y,
                            }
                        };
                        apply_surface_mode(runtime, conn, id, desired_mode);
                        self.surface_mode = desired_mode;
                    }
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::Input(input),
                    } if Some(id) == self.window_id => {
                        input_events.push(input);
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
                        if code == VK_END.0 as u8 || code == 0x1B {
                            self.editor_active = false;
                            self.editor_settings = None;
                            self.surface_mode = SurfaceMode::Unknown;
                            self.last_frame_key = None;
                        }
                    }
                }
                InputEvent::Cursor(cursor) => {
                    let CursorEvent::Action {
                        state: CursorInputState::Released,
                        action: CursorAction::Left,
                    } = cursor.event
                    else {
                        return;
                    };

                    let Some(settings) = self.editor_settings.as_mut() else {
                        return;
                    };

                    let (panel_x, panel_y) = editor_panel_origin(self.window_size);
                    let local_x = cursor.client.x - panel_x;
                    let local_y = cursor.client.y - panel_y;

                    if in_rect(local_x, local_y, 648, 20, 84, 32) {
                        self.editor_active = false;
                        self.editor_settings = None;
                        self.surface_mode = SurfaceMode::Unknown;
                        self.last_frame_key = None;
                        return;
                    }

                    if apply_editor_click(settings, local_x, local_y) {
                        let normalized = settings.clone().normalized();
                        *settings = normalized.clone();
                        self.pending_settings = Some(normalized);
                        self.last_frame_key = None;
                    }
                }
                _ => {}
            }
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

    struct HudLayout {
        width: u32,
        height: u32,
        hero_font: i32,
        body_font: i32,
        small_font: i32,
        padding: i32,
    }

    impl HudLayout {
        fn from_settings(settings: &OverlaySettings) -> Self {
            let ui_scale = (settings.scale * settings.font_scale).clamp(0.45, 1.85);
            Self {
                width: settings.width.max(80),
                height: settings.height.max(40),
                hero_font: (24.0 * ui_scale).round().max(9.0) as i32,
                body_font: (13.0 * ui_scale).round().max(7.0) as i32,
                small_font: (11.0 * ui_scale).round().max(6.0) as i32,
                padding: (settings.padding as i32).clamp(0, 32),
            }
        }
    }

    fn ingame_frame_key(settings: &OverlaySettings, session: Option<&SessionSnapshot>) -> String {
        let mut key = format!(
            "{}:{}:{}:{}:{}:{:.3}:{:.3}:{}:{}:{}:{}:{}:{}:{}:{}",
            settings.enabled,
            settings.width,
            settings.height,
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
            settings.show_hits,
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

    fn render_ingame_bitmap(
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
    ) -> Bitmap {
        let layout = HudLayout::from_settings(settings);
        let mut canvas = BitmapCanvas::new(layout.width, layout.height);
        draw_hud_card(&mut canvas, 0, 0, settings, &layout, session);
        canvas.into_bitmap()
    }

    fn ingame_editor_frame_key(
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
        window_size: Option<(u32, u32)>,
    ) -> String {
        format!(
            "editor:{window_size:?}:{}",
            ingame_frame_key(settings, session)
        )
    }

    fn render_ingame_editor_bitmap(
        settings: &OverlaySettings,
        session: Option<&SessionSnapshot>,
        window_size: Option<(u32, u32)>,
    ) -> Bitmap {
        let (surface_width, surface_height) = window_size.unwrap_or((1280, 720));
        let mut canvas = BitmapCanvas::new(surface_width.max(760), surface_height.max(520));
        let hud_layout = HudLayout::from_settings(settings);
        draw_hud_card(
            &mut canvas,
            settings.offset_x,
            settings.offset_y,
            settings,
            &hud_layout,
            session,
        );

        let (panel_x, panel_y) = editor_panel_origin(Some((canvas.width, canvas.height)));
        canvas.fill_rounded_rect(panel_x, panel_y, 760, 520, 18, [14, 18, 25, 226]);
        canvas.stroke_rounded_rect(panel_x, panel_y, 760, 520, 18, [96, 112, 138, 120]);
        canvas.draw_text_clipped(
            panel_x + 26,
            panel_y + 22,
            360,
            "Overlay editor",
            20,
            FontWeight::Semibold,
            [245, 248, 253, 255],
        );
        canvas.draw_text_clipped(
            panel_x + 26,
            panel_y + 52,
            360,
            "End",
            12,
            FontWeight::Normal,
            [150, 163, 184, 235],
        );
        draw_editor_button(&mut canvas, panel_x + 648, panel_y + 20, 84, 32, "Close");

        canvas.draw_text_clipped(
            panel_x + 26,
            panel_y + 94,
            300,
            "Live HUD",
            15,
            FontWeight::Semibold,
            [234, 239, 247, 255],
        );
        canvas.draw_text_clipped(
            panel_x + 26,
            panel_y + 124,
            300,
            "HUD is shown at its real in-game position.",
            12,
            FontWeight::Normal,
            [151, 164, 184, 245],
        );

        draw_editor_row(
            &mut canvas,
            panel_x + 390,
            panel_y + 88,
            "Enabled",
            settings.enabled,
        );
        draw_editor_row(
            &mut canvas,
            panel_x + 390,
            panel_y + 132,
            "Background",
            settings.show_background,
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 390,
            panel_y + 190,
            "Opacity",
            &format!("{:.0}%", settings.opacity * 100.0),
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 390,
            panel_y + 244,
            "Scale",
            &format!("{:.0}%", settings.scale * 100.0),
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 390,
            panel_y + 298,
            "Text",
            &format!("{:.0}%", settings.font_scale * 100.0),
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 26,
            panel_y + 318,
            "X",
            &settings.offset_x.to_string(),
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 196,
            panel_y + 318,
            "Y",
            &settings.offset_y.to_string(),
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 26,
            panel_y + 372,
            "Width",
            &settings.width.to_string(),
        );
        draw_editor_stepper(
            &mut canvas,
            panel_x + 196,
            panel_y + 372,
            "Height",
            &settings.height.to_string(),
        );

        draw_metric_toggle(
            &mut canvas,
            panel_x + 390,
            panel_y + 372,
            "PP",
            settings.show_pp,
        );
        draw_metric_toggle(
            &mut canvas,
            panel_x + 472,
            panel_y + 372,
            "IF FC",
            settings.show_if_fc,
        );
        draw_metric_toggle(
            &mut canvas,
            panel_x + 554,
            panel_y + 372,
            "ACC",
            settings.show_accuracy,
        );
        draw_metric_toggle(
            &mut canvas,
            panel_x + 636,
            panel_y + 372,
            "Hits",
            settings.show_hits,
        );
        draw_metric_toggle(
            &mut canvas,
            panel_x + 390,
            panel_y + 420,
            "Combo",
            settings.show_combo,
        );
        draw_metric_toggle(
            &mut canvas,
            panel_x + 472,
            panel_y + 420,
            "Mods",
            settings.show_mods,
        );
        draw_metric_toggle(
            &mut canvas,
            panel_x + 554,
            panel_y + 420,
            "Map",
            settings.show_map,
        );

        canvas.into_bitmap()
    }

    fn editor_panel_origin(window_size: Option<(u32, u32)>) -> (i32, i32) {
        let (width, height) = window_size.unwrap_or((1280, 720));
        (
            ((width as i32 - 760) / 2).max(0),
            ((height as i32 - 520) / 2).max(0),
        )
    }

    fn draw_editor_button(canvas: &mut BitmapCanvas, x: i32, y: i32, w: i32, h: i32, label: &str) {
        canvas.fill_rounded_rect(x, y, w, h, 10, [35, 41, 52, 235]);
        canvas.stroke_rounded_rect(x, y, w, h, 10, [94, 107, 130, 130]);
        canvas.draw_text_clipped(
            x + 14,
            y + 8,
            w - 28,
            label,
            13,
            FontWeight::Semibold,
            [234, 239, 247, 255],
        );
    }

    fn draw_editor_row(canvas: &mut BitmapCanvas, x: i32, y: i32, label: &str, enabled: bool) {
        canvas.draw_text_clipped(
            x,
            y + 7,
            180,
            label,
            14,
            FontWeight::Semibold,
            [235, 240, 248, 255],
        );
        let toggle_x = x + 238;
        canvas.fill_rounded_rect(
            toggle_x,
            y,
            44,
            24,
            12,
            if enabled {
                [50, 101, 190, 230]
            } else {
                [40, 46, 58, 230]
            },
        );
        canvas.stroke_rounded_rect(toggle_x, y, 44, 24, 12, [100, 116, 145, 95]);
        canvas.fill_rounded_rect(
            if enabled { toggle_x + 22 } else { toggle_x + 3 },
            y + 3,
            18,
            18,
            9,
            [238, 243, 250, 245],
        );
    }

    fn draw_editor_stepper(canvas: &mut BitmapCanvas, x: i32, y: i32, label: &str, value: &str) {
        canvas.draw_text_clipped(
            x,
            y,
            120,
            label,
            12,
            FontWeight::Semibold,
            [151, 164, 184, 245],
        );
        draw_editor_button(canvas, x, y + 20, 34, 30, "-");
        canvas.fill_rounded_rect(x + 42, y + 20, 76, 30, 9, [28, 34, 44, 230]);
        canvas.stroke_rounded_rect(x + 42, y + 20, 76, 30, 9, [82, 96, 120, 115]);
        canvas.draw_text_clipped(
            x + 50,
            y + 28,
            60,
            value,
            12,
            FontWeight::Semibold,
            [240, 244, 250, 255],
        );
        draw_editor_button(canvas, x + 126, y + 20, 34, 30, "+");
    }

    fn draw_metric_toggle(canvas: &mut BitmapCanvas, x: i32, y: i32, label: &str, enabled: bool) {
        canvas.fill_rounded_rect(
            x,
            y,
            72,
            34,
            10,
            if enabled {
                [50, 101, 190, 230]
            } else {
                [32, 38, 48, 230]
            },
        );
        canvas.stroke_rounded_rect(x, y, 72, 34, 10, [95, 113, 142, 125]);
        canvas.draw_text_clipped(
            x + 12,
            y + 9,
            48,
            label,
            12,
            FontWeight::Semibold,
            [238, 243, 250, 255],
        );
    }

    fn apply_editor_click(settings: &mut OverlaySettings, x: i32, y: i32) -> bool {
        if in_rect(x, y, 628, 88, 44, 24) {
            settings.enabled = !settings.enabled;
            return true;
        }
        if in_rect(x, y, 628, 132, 44, 24) {
            settings.show_background = !settings.show_background;
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
            settings.scale -= 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 244) == Some(1) {
            settings.scale += 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 298) == Some(-1) {
            settings.font_scale -= 0.05;
            return true;
        }
        if stepper_click(x, y, 390, 298) == Some(1) {
            settings.font_scale += 0.05;
            return true;
        }
        if stepper_click(x, y, 26, 318) == Some(-1) {
            settings.offset_x -= 5;
            return true;
        }
        if stepper_click(x, y, 26, 318) == Some(1) {
            settings.offset_x += 5;
            return true;
        }
        if stepper_click(x, y, 196, 318) == Some(-1) {
            settings.offset_y -= 5;
            return true;
        }
        if stepper_click(x, y, 196, 318) == Some(1) {
            settings.offset_y += 5;
            return true;
        }
        if stepper_click(x, y, 26, 372) == Some(-1) {
            settings.width = settings.width.saturating_sub(10);
            return true;
        }
        if stepper_click(x, y, 26, 372) == Some(1) {
            settings.width += 10;
            return true;
        }
        if stepper_click(x, y, 196, 372) == Some(-1) {
            settings.height = settings.height.saturating_sub(10);
            return true;
        }
        if stepper_click(x, y, 196, 372) == Some(1) {
            settings.height += 10;
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
                canvas.draw_text_clipped(
                    origin_x + padding,
                    y,
                    content_width,
                    &pp_label,
                    layout.hero_font,
                    FontWeight::Semibold,
                    [246, 249, 255, 255],
                );
                y += (layout.hero_font as f32 * 1.25).round() as i32;
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
                canvas.draw_text_clipped(
                    origin_x + padding,
                    y + 2,
                    content_width,
                    &map,
                    layout.small_font,
                    FontWeight::Normal,
                    [160, 173, 194, 230],
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

    fn draw_metric_cells(
        canvas: &mut BitmapCanvas,
        x: i32,
        y: i32,
        max_width: i32,
        layout: &HudLayout,
        settings: &OverlaySettings,
        cells: &[(String, String)],
    ) -> i32 {
        let gap = 6;
        let visible_count = cells.len().max(1) as i32;
        let cell_width = ((max_width - gap * (visible_count - 1)) / visible_count).max(24);
        let cell_height = ((layout.body_font as f32 * 2.2).round() as i32).max(22);

        for (index, (label, value)) in cells.iter().enumerate() {
            let cell_x = x + index as i32 * (cell_width + gap);
            canvas.fill_rounded_rect(
                cell_x,
                y,
                cell_width,
                cell_height,
                8,
                [
                    27,
                    32,
                    42,
                    (settings.opacity * 178.0).clamp(0.0, 200.0) as u8,
                ],
            );
            canvas.stroke_rounded_rect(
                cell_x,
                y,
                cell_width,
                cell_height,
                8,
                [
                    85,
                    96,
                    116,
                    (settings.opacity * 86.0).clamp(0.0, 120.0) as u8,
                ],
            );
            canvas.draw_text_clipped(
                cell_x + 6,
                y + 3,
                cell_width - 12,
                label,
                layout.small_font,
                FontWeight::Semibold,
                [134, 149, 172, 230],
            );
            canvas.draw_text_clipped(
                cell_x + 6,
                y + 5 + layout.small_font,
                cell_width - 12,
                value,
                layout.body_font,
                FontWeight::Semibold,
                [240, 245, 252, 255],
            );
        }

        y + cell_height + 6
    }

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
        let gap = 5;
        let cell_width = ((max_width - gap * 3) / 4).max(18);
        let cell_height = (font_px as f32 * 1.95).round().max(18.0) as i32;

        for (index, (label, value, color, bg)) in parts.into_iter().enumerate() {
            let cell_x = x + index as i32 * (cell_width + gap);
            canvas.fill_rounded_rect(cell_x, y, cell_width, cell_height, 7, bg);
            canvas.stroke_rounded_rect(cell_x, y, cell_width, cell_height, 7, [100, 111, 132, 78]);
            canvas.draw_text_clipped(
                cell_x + 5,
                y + 3,
                cell_width - 10,
                &format!("{label} {value}"),
                font_px,
                FontWeight::Semibold,
                color,
            );
        }

        y + cell_height + 5
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

        let slot = match weight {
            FontWeight::Normal => &REGULAR,
            FontWeight::Semibold => &SEMIBOLD,
        };

        slot.get_or_init(|| {
            let path = match weight {
                FontWeight::Normal => "C:\\Windows\\Fonts\\segoeui.ttf",
                FontWeight::Semibold => "C:\\Windows\\Fonts\\seguisb.ttf",
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
