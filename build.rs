fn main() {
    const COMMANDS: &[&str] = &[
        "close_app",
        "close_lockdown",
        "close_during_loading",
        "mark_quiz_ready",
        "check_multiple_monitors",
        "get_monitor_count",
    ];

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));

    tauri_build::try_build(attributes).expect("failed to build Tauri application context");
}
