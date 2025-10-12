#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::{DeepLinkExt, OpenUrlEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use url::Url;

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

#[tauri::command]
fn close_app(app: tauri::AppHandle) {
    app.exit(0);
}

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

#[tauri::command]
fn close_lockdown() {
    std::process::exit(0);
}

fn main() {
    let config = LockdownConfig::load();

    let base_url = if cfg!(debug_assertions) {
        config.base_url.clone()
    } else {
        config.production_url.clone()
    };

    let enable_emergency_exit =
        config.debug_settings.enable_emergency_exit || cfg!(debug_assertions);

    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            // Register emergency exit shortcut if enabled
            if enable_emergency_exit {
                println!("DEBUG MODE: Emergency exit enabled!");
                println!("Press Ctrl+Alt+Shift+Q to exit");

                let app_handle_exit = app.handle().clone();

                // Register Ctrl+Alt+Shift+Q
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

            WebviewWindowBuilder::new(app, "main", entry)
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
        .invoke_handler(tauri::generate_handler![close_app, close_lockdown])
        .run(tauri::generate_context!())
        .expect("error while running lockdown browser");
}
