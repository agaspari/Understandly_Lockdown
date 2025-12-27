#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::{DeepLinkExt, OpenUrlEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
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

#[derive(Deserialize)]
struct DebugConfig {
    enable_emergency_exit: bool,
}

#[derive(Deserialize)]
struct LockdownConfig {
    base_url: String,
    production_url: String,
    window: WindowConfig,
    debug_settings: DebugConfig,
}

impl LockdownConfig {
    fn load() -> Self {
        let config_str = include_str!("../lockdown.config.json");
        serde_json::from_str(config_str).expect("Invalid lockdown.config.json")
    }
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
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
        WM_SYSKEYDOWN,
    };

    static HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
    static mut KEYBOARD_HOOK: Option<HHOOK> = None;

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
                // Block Alt+Tab
                if alt_down && vk_code == VK_TAB {
                    return LRESULT(1);
                }

                // Block Alt+Escape
                if alt_down && vk_code == VK_ESCAPE {
                    return LRESULT(1);
                }

                // Block Alt+F4 (already blocked by window event, but reinforce)
                if alt_down && vk_code == VK_F4 {
                    return LRESULT(1);
                }

                // Block Windows key (left and right)
                if vk_code == VK_LWIN || vk_code == VK_RWIN {
                    return LRESULT(1);
                }

                // Block PrintScreen
                if vk_code == VK_SNAPSHOT {
                    return LRESULT(1);
                }

                // Block F12 (DevTools)
                if vk_code == VK_F12 {
                    return LRESULT(1);
                }

                // Block Ctrl+C, Ctrl+V, Ctrl+P
                if ctrl_down && (vk_code == VK_C || vk_code == VK_V || vk_code == VK_P) {
                    return LRESULT(1);
                }
            }
        }

        // Pass to next hook
        unsafe {
            if let Some(hook) = KEYBOARD_HOOK {
                return CallNextHookEx(hook, code, wparam, lparam);
            }
        }
        CallNextHookEx(HHOOK::default(), code, wparam, lparam)
    }

    /// Install the low-level keyboard hook
    pub fn install_keyboard_hook() {
        if HOOK_ACTIVE.load(Ordering::SeqCst) {
            return;
        }

        thread::spawn(|| {
            unsafe {
                let h_module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
                let h_instance: HINSTANCE = std::mem::transmute(h_module);

                let hook =
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), h_instance, 0);

                if let Ok(h) = hook {
                    KEYBOARD_HOOK = Some(h);
                    HOOK_ACTIVE.store(true, Ordering::SeqCst);

                    // Message loop to keep hook alive
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }

                    // Cleanup on exit
                    let _ = UnhookWindowsHookEx(h);
                    HOOK_ACTIVE.store(false, Ordering::SeqCst);
                }
            }
        });
    }

    /// Count the number of connected monitors
    pub fn get_monitor_count() -> i32 {
        unsafe extern "system" fn monitor_enum_proc(
            _hmonitor: HMONITOR,
            _hdc: HDC,
            _lprect: *mut windows::Win32::Foundation::RECT,
            lparam: LPARAM,
        ) -> windows::Win32::Foundation::BOOL {
            let count = lparam.0 as *mut i32;
            *count += 1;
            windows::Win32::Foundation::BOOL(1)
        }

        let mut count: i32 = 0;
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut count as *mut i32 as isize),
            );
        }
        count
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

#[tauri::command]
fn close_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn close_lockdown() {
    std::process::exit(0);
}

/// Check if multiple monitors are connected (for the frontend to react)
#[tauri::command]
fn check_multiple_monitors() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_security::get_monitor_count() > 1
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Get monitor count
#[tauri::command]
fn get_monitor_count() -> i32 {
    #[cfg(target_os = "windows")]
    {
        windows_security::get_monitor_count()
    }
    #[cfg(not(target_os = "windows"))]
    {
        1
    }
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

    let enable_emergency_exit =
        config.debug_settings.enable_emergency_exit || cfg!(debug_assertions);

    // Install keyboard hook on Windows (blocks Alt+Tab, PrintScreen, etc.)
    #[cfg(target_os = "windows")]
    {
        windows_security::install_keyboard_hook();
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            // Register emergency exit shortcut if enabled (Ctrl+Alt+Shift+Q)
            if enable_emergency_exit {
                println!("DEBUG MODE: Emergency exit enabled!");
                println!("Press Ctrl+Alt+Shift+Q to exit");

                let app_handle_exit = app.handle().clone();

                let shortcut = Shortcut::new(
                    Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT),
                    Code::KeyQ,
                );

                let _ =
                    app.global_shortcut()
                        .on_shortcut(shortcut, move |_app, _shortcut, event| {
                            if event.state == ShortcutState::Pressed {
                                println!("Emergency exit triggered!");
                                app_handle_exit.exit(0);
                            }
                        });
            }

            let dl = app.deep_link();

            let entry = dl
                .get_current()
                .ok()
                .and_then(|opt| opt.and_then(|v| v.into_iter().next()))
                .map(|u| WebviewUrl::External(Url::parse(&to_local(&u, &base_url)).unwrap()))
                .unwrap_or_else(|| WebviewUrl::External(Url::parse(&base_url).unwrap()));

            let window = WebviewWindowBuilder::new(app, "main", entry)
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

            // Inject JavaScript to disable right-click, keyboard shortcuts, and DevTools
            let init_script = r#"
                // Disable right-click context menu
                document.addEventListener('contextmenu', function(e) {
                    e.preventDefault();
                    return false;
                });
                
                // Disable keyboard shortcuts
                document.addEventListener('keydown', function(e) {
                    // Block F12 (DevTools)
                    if (e.key === 'F12') {
                        e.preventDefault();
                        return false;
                    }
                    
                    // Block Ctrl+Shift+I (DevTools)
                    if (e.ctrlKey && e.shiftKey && e.key === 'I') {
                        e.preventDefault();
                        return false;
                    }
                    
                    // Block Ctrl+Shift+J (Console)
                    if (e.ctrlKey && e.shiftKey && e.key === 'J') {
                        e.preventDefault();
                        return false;
                    }
                    
                    // Block Ctrl+U (View Source)
                    if (e.ctrlKey && e.key === 'u') {
                        e.preventDefault();
                        return false;
                    }
                    
                    // Block Ctrl+S (Save)
                    if (e.ctrlKey && e.key === 's') {
                        e.preventDefault();
                        return false;
                    }
                    
                    // Block Ctrl+P (Print) - backup for OS-level block
                    if (e.ctrlKey && e.key === 'p') {
                        e.preventDefault();
                        return false;
                    }
                });
                
                // Disable text selection (except in input fields)
                document.addEventListener('selectstart', function(e) {
                    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') {
                        return true;
                    }
                    e.preventDefault();
                    return false;
                });
                
                // Disable drag and drop
                document.addEventListener('dragstart', function(e) {
                    e.preventDefault();
                    return false;
                });
                
                console.log('[Lockdown] Security features initialized');
            "#;

            // Execute the init script
            let _ = window.eval(init_script);

            // Handle deep-links for already-running instance
            let app_handle: AppHandle = app.handle().clone();
            let base_clone = base_url.clone();
            dl.on_open_url(move |evt: OpenUrlEvent| {
                if let Some(u) = evt.urls().first() {
                    if let Some(win) = app_handle.get_webview_window("main") {
                        let _ = win.eval(&format!(
                            "window.location.replace('{}')",
                            to_local(u, &base_clone)
                        ));
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
            }
            if let tauri::WindowEvent::Focused(false) = event {
                let _ = _window.set_focus();
            }
        })
        .invoke_handler(tauri::generate_handler![
            close_app,
            close_lockdown,
            check_multiple_monitors,
            get_monitor_count
        ])
        .run(tauri::generate_context!())
        .expect("error while running lockdown browser");
}
