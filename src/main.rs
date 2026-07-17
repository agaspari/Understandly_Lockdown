#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::{DeepLinkExt, OpenUrlEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_updater::UpdaterExt;
use url::Url;

// ============================================================================
// LockdownConfig - loaded from lockdown.config.json
// ============================================================================

#[derive(Deserialize)]
struct WindowConfig {
    title: String,
    fullscreen: bool,
    always_on_top: bool,
    skip_taskbar: bool,
}

#[derive(Deserialize, Serialize)]
struct LoadingRecoveryConfig {
    enabled: bool,
    button_label: String,
    confirmation_message: String,
}

#[derive(Deserialize)]
struct LockdownConfig {
    base_url: String,
    production_url: String,
    window: WindowConfig,
    loading_recovery: LoadingRecoveryConfig,
}

impl LockdownConfig {
    fn load() -> Self {
        let config_str = include_str!("../lockdown.config.json");
        serde_json::from_str(config_str).expect("Invalid lockdown.config.json")
    }
}

// ============================================================================
// Initialization script - injected into every page load (survives navigation)
// ============================================================================

const INIT_SCRIPT: &str = r#"
    // Disable right-click context menu
    document.addEventListener('contextmenu', function (e) {
        e.preventDefault();
    });

    // Block clipboard exfiltration
    document.addEventListener('copy', function (e) { e.preventDefault(); });
    document.addEventListener('cut', function (e) { e.preventDefault(); });
    document.addEventListener('paste', function (e) { e.preventDefault(); });

    // Disable keyboard shortcuts (Ctrl on Windows/Linux, Cmd on macOS)
    document.addEventListener('keydown', function (e) {
        var mod = e.ctrlKey || e.metaKey;
        var k = e.code;

        // F12 (DevTools)
        if (e.key === 'F12') {
            e.preventDefault();
            return;
        }

        // Ctrl/Cmd+Shift+I/J/C and Cmd+Option+I/J/C (DevTools, console, inspector)
        if (mod && e.shiftKey && (k === 'KeyI' || k === 'KeyJ' || k === 'KeyC')) {
            e.preventDefault();
            return;
        }
        if (e.metaKey && e.altKey && (k === 'KeyI' || k === 'KeyJ' || k === 'KeyC')) {
            e.preventDefault();
            return;
        }

        // View source, save, print, copy/cut/paste, select-all
        if (mod && ['KeyU', 'KeyS', 'KeyP', 'KeyC', 'KeyV', 'KeyX', 'KeyA'].indexOf(k) !== -1) {
            e.preventDefault();
            return;
        }

        // Cmd+W/M/H/Q/N/T (close, minimize, hide, quit, new window/tab)
        if (e.metaKey && ['KeyW', 'KeyM', 'KeyH', 'KeyQ', 'KeyN', 'KeyT'].indexOf(k) !== -1) {
            e.preventDefault();
        }
    });

    // Disable text selection (except in input fields)
    document.addEventListener('selectstart', function (e) {
        if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') {
            return;
        }
        e.preventDefault();
    });

    // Disable drag and drop
    document.addEventListener('dragstart', function (e) {
        e.preventDefault();
    });

    console.log('[Lockdown] Security features initialized');
"#;

fn loading_recovery_script(config: &LoadingRecoveryConfig) -> String {
    let config_json = serde_json::to_string(config)
        .expect("loading recovery configuration should serialize to JSON");
    format!(
        r#"
        window.__UNDERSTANDLY_LOADING_RECOVERY_CONFIG__ = {config_json};
        document.addEventListener('DOMContentLoaded', function () {{
            var config = window.__UNDERSTANDLY_LOADING_RECOVERY_CONFIG__;
            var button = document.getElementById('loading-exit');
            if (!button) return;

            button.textContent = config.button_label;
            button.setAttribute('aria-label', config.button_label);
            button.addEventListener('click', async function () {{
                if (config.confirmation_message && !window.confirm(config.confirmation_message)) return;

                button.disabled = true;
                try {{
                    await window.__TAURI_INTERNALS__.invoke('close_during_loading');
                }} catch (error) {{
                    button.disabled = false;
                    console.error('[Lockdown] loading exit rejected', error);
                }}
            }});
        }}, {{ once: true }});
        "#
    )
}

#[derive(Default)]
struct QuizSessionState {
    ready: AtomicBool,
}

// ============================================================================
// Windows Security Module - Low-level keyboard hook
// ============================================================================

#[cfg(target_os = "windows")]
mod windows_security {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
        WM_SYSKEYDOWN,
    };

    static HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);

    // Virtual key codes
    const VK_TAB: u32 = 0x09;
    const VK_ESCAPE: u32 = 0x1B;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    const VK_SNAPSHOT: u32 = 0x2C; // PrintScreen
    const VK_F4: u32 = 0x73;
    const VK_F12: u32 = 0x7B;
    const VK_C: u32 = 0x43;
    const VK_V: u32 = 0x56;
    const VK_P: u32 = 0x50;

    // Modifier key flags from KBDLLHOOKSTRUCT
    const LLKHF_ALTDOWN: u32 = 0x20;

    /// Low-level keyboard hook callback
    /// Blocks: Alt+Tab, Alt+Esc, Alt+F4, Windows key, PrintScreen, Ctrl+C/V/P, F12
    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let kb_struct = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk_code = kb_struct.vkCode;
            let flags = kb_struct.flags.0;
            let alt_down = (flags & LLKHF_ALTDOWN) != 0;

            // Check for Ctrl key via GetAsyncKeyState
            let ctrl_down = (GetAsyncKeyState(0x11) as u16 & 0x8000) != 0;

            let is_key_down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;

            if is_key_down {
                // Block Alt+Tab, Alt+Escape, Alt+F4
                if alt_down && (vk_code == VK_TAB || vk_code == VK_ESCAPE || vk_code == VK_F4) {
                    return LRESULT(1);
                }

                // Block Windows key (left and right)
                if vk_code == VK_LWIN || vk_code == VK_RWIN {
                    return LRESULT(1);
                }

                // Block PrintScreen and F12 (DevTools)
                if vk_code == VK_SNAPSHOT || vk_code == VK_F12 {
                    return LRESULT(1);
                }

                // Block Ctrl+C, Ctrl+V, Ctrl+P
                if ctrl_down && (vk_code == VK_C || vk_code == VK_V || vk_code == VK_P) {
                    return LRESULT(1);
                }
            }
        }

        // The hook handle is ignored by CallNextHookEx
        CallNextHookEx(HHOOK::default(), code, wparam, lparam)
    }

    /// Install the low-level keyboard hook
    pub fn install_keyboard_hook() {
        if HOOK_ACTIVE.swap(true, Ordering::SeqCst) {
            return;
        }

        thread::spawn(|| unsafe {
            let h_module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let h_instance = HINSTANCE(h_module.0);

            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), h_instance, 0);

            if let Ok(hook) = hook {
                // Message loop to keep hook alive
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                // Cleanup on exit
                let _ = UnhookWindowsHookEx(hook);
            }
            HOOK_ACTIVE.store(false, Ordering::SeqCst);
        });
    }
}

// ============================================================================
// macOS Security Module - Kiosk mode and screenshot protection
// ============================================================================

#[cfg(target_os = "macos")]
mod macos_security {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplication, NSApplicationPresentationOptions, NSWindow, NSWindowSharingType,
    };

    /// Put the app into kiosk mode: hides the Dock and menu bar and disables
    /// Cmd+Tab process switching and hiding the app. Force-quit and session
    /// termination intentionally remain available as OS-level recovery paths.
    /// Must be called on the main thread.
    #[allow(unused_unsafe)]
    pub fn enable_kiosk_mode() {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("[Lockdown] kiosk mode skipped: not on main thread");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        unsafe {
            app.setPresentationOptions(
                NSApplicationPresentationOptions::HideDock
                    | NSApplicationPresentationOptions::HideMenuBar
                    | NSApplicationPresentationOptions::DisableProcessSwitching
                    | NSApplicationPresentationOptions::DisableHideApplication,
            );
        }
    }

    /// Exclude the window from screenshots and screen recordings
    /// (Cmd+Shift+3/4/5 capture a blank area where the window is).
    #[allow(unused_unsafe)]
    pub fn disable_window_capture(window: &tauri::WebviewWindow) {
        if let Ok(ptr) = window.ns_window() {
            unsafe {
                let ns_window: &NSWindow = &*ptr.cast();
                ns_window.setSharingType(NSWindowSharingType::None);
            }
        }
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

#[tauri::command]
fn close_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn close_lockdown(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn close_during_loading(
    app: AppHandle,
    state: State<'_, Arc<QuizSessionState>>,
) -> Result<(), String> {
    if state.ready.load(Ordering::Acquire) {
        return Err("the quiz is active; use the quiz close flow".into());
    }

    app.exit(0);
    Ok(())
}

#[tauri::command]
fn mark_quiz_ready(app: AppHandle, state: State<'_, Arc<QuizSessionState>>) -> Result<(), String> {
    state.ready.store(true, Ordering::Release);
    if let Some(window) = app.get_webview_window("loading-recovery") {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Check if multiple monitors are connected (for the frontend to react)
#[tauri::command]
fn check_multiple_monitors(app: AppHandle) -> bool {
    app.available_monitors()
        .map(|monitors| monitors.len() > 1)
        .unwrap_or(false)
}

/// Get monitor count
#[tauri::command]
fn get_monitor_count(app: AppHandle) -> usize {
    app.available_monitors()
        .map(|monitors| monitors.len())
        .unwrap_or(1)
}

// ============================================================================
// Auto-Updater
// ============================================================================

async fn check_for_updates(app: AppHandle) -> tauri_plugin_updater::Result<()> {
    let updater = app.updater()?;
    if let Some(update) = updater.check().await? {
        println!("[Lockdown] update {} available", update.version);
        println!("[Lockdown] update deferred until lockdown has ended");
    }
    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// understandly_lockdown://quiz?x=1           →  <base>/quiz?x=1
/// understandly_lockdown://results/987?y=true →  <base>/results/987?y=true
fn to_local(link: &Url, base: &str) -> String {
    let mut target = String::from(base.trim_end_matches('/'));

    if let Some(host) = link.host_str() {
        if !host.is_empty() {
            target.push('/');
            target.push_str(host.trim_start_matches('/'));
        }
    }

    let path = link.path().trim_start_matches('/');
    if !path.is_empty() {
        target.push('/');
        target.push_str(path);
    }

    if let Some(q) = link.query() {
        target.push('?');
        target.push_str(q);
    }

    target
}

// ============================================================================
// Main Entry Point
// ============================================================================

fn main() {
    let config = LockdownConfig::load();

    let base_url = if cfg!(debug_assertions) {
        config.base_url.clone()
    } else {
        config.production_url.clone()
    };
    let loading_recovery_enabled = config.loading_recovery.enabled;
    let loading_recovery_init_script = loading_recovery_script(&config.loading_recovery);
    let quiz_state = Arc::new(QuizSessionState::default());

    tauri::Builder::default()
        .manage(Arc::clone(&quiz_state))
        // single-instance must be the first plugin; with the "deep-link"
        // feature it forwards deep links from second launches to this instance
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            // Register the native recovery path before enabling any lockdown
            // behavior. If registration fails, setup aborts and the app exits
            // without taking control of the machine.
            let app_handle_exit = app.handle().clone();
            let shortcut = Shortcut::new(
                Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT),
                Code::KeyQ,
            );
            app.global_shortcut()
                .on_shortcut(shortcut, move |_app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        println!("[Lockdown] emergency exit triggered");
                        app_handle_exit.exit(0);
                    }
                })?;
            println!("[Lockdown] recovery shortcut ready: Ctrl+Alt+Shift+Q");

            let dl = app.deep_link();

            // Register the URL scheme at runtime so deep links work in dev
            // builds and portable installs (installers also register it)
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            let _ = dl.register_all();

            let entry = dl
                .get_current()
                .ok()
                .and_then(|opt| opt.and_then(|v| v.into_iter().next()))
                .and_then(|u| Url::parse(&to_local(&u, &base_url)).ok())
                .map(WebviewUrl::External)
                .unwrap_or_else(|| WebviewUrl::External(Url::parse(&base_url).unwrap()));

            let _window = WebviewWindowBuilder::new(app, "main", entry)
                .initialization_script(INIT_SCRIPT)
                .fullscreen(config.window.fullscreen)
                .title(&config.window.title)
                .always_on_top(config.window.always_on_top)
                .skip_taskbar(config.window.skip_taskbar)
                .decorations(false)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .closable(false)
                .build()?;

            if loading_recovery_enabled {
                let recovery_window = WebviewWindowBuilder::new(
                    app,
                    "loading-recovery",
                    WebviewUrl::App("loading.html".into()),
                )
                .initialization_script(&loading_recovery_init_script)
                .title("Quiz loading")
                .inner_size(120.0, 56.0)
                .position(16.0, 16.0)
                .always_on_top(true)
                .skip_taskbar(true)
                .decorations(false)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .focused(false)
                .build()?;

                // The remote page can become ready while this small local
                // window is being created. Close it immediately if that race
                // occurred.
                if quiz_state.ready.load(Ordering::Acquire) {
                    let _ = recovery_window.close();
                }
            }

            // Exclude the window from screenshots/screen recordings on macOS
            #[cfg(target_os = "macos")]
            macos_security::disable_window_capture(&_window);

            // Activate platform lockdown only after both the recovery shortcut
            // and browser window have initialized successfully.
            #[cfg(target_os = "windows")]
            windows_security::install_keyboard_hook();

            #[cfg(target_os = "macos")]
            macos_security::enable_kiosk_mode();

            // Check in the background, but never install or restart while a
            // lockdown session is active.
            let updater_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = check_for_updates(updater_handle).await {
                    eprintln!("[Lockdown] update check failed: {e}");
                }
            });

            // Handle deep-links for the already-running instance
            let app_handle: AppHandle = app.handle().clone();
            let base_clone = base_url.clone();
            dl.on_open_url(move |evt: OpenUrlEvent| {
                if let Some(u) = evt.urls().first() {
                    if let Some(win) = app_handle.get_webview_window("main") {
                        // navigate() instead of eval() so a crafted deep link
                        // cannot inject script into the page
                        if let Ok(target) = Url::parse(&to_local(u, &base_clone)) {
                            let _ = win.navigate(target);
                        }
                        let _ = win.set_focus();
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            close_app,
            close_lockdown,
            close_during_loading,
            mark_quiz_ready,
            check_multiple_monitors,
            get_monitor_count
        ])
        .build(tauri::generate_context!())
        .expect("error while building lockdown browser")
        .run(|_app, event| {
            if let RunEvent::ExitRequested { code, api, .. } = event {
                // Block Cmd+Q and other OS-initiated quits; explicit exits
                // (close_lockdown, emergency shortcut, updater restart)
                // carry an exit code and are allowed through
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
