#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::{DeepLinkExt, OpenUrlEvent};
use url::Url;

#[tauri::command]
fn close_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// understandly_lockdown://quiz?x=1           →  <base>/quiz?x=1
/// understandly_lockdown://results/987?y=true →  <base>/results/987?y=true
fn to_local(link: &Url, base: &str) -> String {
    let mut target = String::from(base.trim_end_matches('/'));

    // Treat the "host" part (everything before the first slash) as the first path segment
    if let Some(host) = link.host_str() {
        if !host.is_empty() {
            target.push('/');
            target.push_str(host.trim_start_matches('/'));
        }
    }

    // Append the regular path
    let path = link.path().trim_start_matches('/');
    if !path.is_empty() {
        target.push('/');
        target.push_str(path);
    }

    // Keep query string
    if let Some(q) = link.query() {
        target.push('?');
        target.push_str(q);
    }

    target
}

fn main() {
    // ── customizable base URL ───────────────────────────────────────────────
    // Set UNDERSTANDLY_LOCKDOWN_BASE env var at build-time or run-time; falls back to localhost.
    let base = std::env::var("UNDERSTANDLY_LOCKDOWN_BASE")
        .unwrap_or_else(|_| "http://localhost:3000".into());

    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_process::init())
        .setup(move |app| {
            let dl = app.deep_link();

            // ── cold-start: was launched via understandly_lockdown://… ? ─────────────────
            let entry = dl
                .get_current() // Result<Option<Vec<Url>>, _>
                .ok()
                .and_then(|opt| opt.and_then(|v| v.into_iter().next()))
                .map(|u| WebviewUrl::External(Url::parse(&to_local(&u, &base)).unwrap()))
                .unwrap_or_else(|| WebviewUrl::External(Url::parse(&base).unwrap()));

            WebviewWindowBuilder::new(app, "main", entry)
                .fullscreen(true)
                .title("Understandly Lockdown")
                .build()?;

            // ── already-running instance receives a new deep-link ────────────
            let app_handle: AppHandle = app.handle().clone(); // Send + Sync
            let base_clone = base.clone();
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
        })
        .invoke_handler(tauri::generate_handler![close_app])
        .run(tauri::generate_context!())
        .expect("error while running Understandly Lockdown");
}
