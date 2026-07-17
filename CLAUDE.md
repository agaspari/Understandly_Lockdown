# Understandly Lockdown Browser

Tauri 2 (pure Rust, no bundled frontend) lockdown/exam browser. It opens a remote web
app in a fullscreen, always-on-top kiosk window and blocks ways to leave or exfiltrate
content (app switching, screenshots, clipboard, DevTools).

## Commands

- `cargo build` — compile check
- `cargo tauri dev` — run in dev mode (loads `base_url` from lockdown.config.json; emergency exit always enabled)
- `cargo tauri build` — production bundle (loads `production_url`)
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` — enforced by CI on Windows + macOS

## Architecture

- `src/main.rs` — the entire app. Platform enforcement lives in two modules:
  `windows_security` (low-level keyboard hook) and `macos_security` (NSApplication
  kiosk presentation options + NSWindowSharingNone). A JS `INIT_SCRIPT` is injected
  via `initialization_script` so it survives navigation.
- `lockdown.config.json` — compiled in via `include_str!`; URLs, window behavior,
  emergency exit toggle. Changing it requires a rebuild.
- `tauri.conf.json` — bundle targets (NSIS, DMG, app), updater pubkey/endpoint, CSP,
  capabilities. Keep `version` in sync with `Cargo.toml`.
- `empty/` — placeholder `frontendDist`; there is no local frontend.
- Entry is via deep link (`understandly-lockdown://...`), mapped onto the base URL by
  `to_local()`. Runtime deep links for a running instance are handled through
  `tauri-plugin-single-instance` (must stay the first registered plugin).

## Gotchas

- macOS code (`macos_security`, objc2) cannot be compiled on Windows — CI's macos-latest job is the compile check for it.
- `app.exit(code)` is the only programmatic way out: `RunEvent::ExitRequested` with `code: None` (Cmd+Q, window close) is prevented. The native Ctrl+Alt+Shift+Q recovery shortcut is always registered before lockdown activates.
- Release builds check for and install signed updates only while Rust still owns the pre-quiz loading phase. Debug builds skip auto-installation. Git tags, `Cargo.toml`, and `tauri.conf.json` versions must agree; releases are cut by pushing a `v*` tag (`.github/workflows/release.yml`).
- `tauri-plugin-updater` intentionally uses `native-tls` (not the default rustls) so local builds don't need clang for `ring` on Windows ARM64.
