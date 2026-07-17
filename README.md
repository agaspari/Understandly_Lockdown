# Understandly Lockdown Browser

A lightweight lockdown (secure exam) browser for **Windows and macOS**, built with [Tauri 2](https://tauri.app). Originally made for Understandly, but designed so other developers can rebrand and configure it for their own platforms.

It opens your hosted web app in a fullscreen, always-on-top kiosk window and blocks the usual ways to leave the exam or exfiltrate content.

## What it blocks

| Protection | Windows | macOS |
|---|---|---|
| App switching (Alt+Tab / Cmd+Tab) | Low-level keyboard hook | Kiosk presentation options |
| OS key (Win key / Dock & menu bar) | Blocked | Hidden |
| Screenshots / screen recording | PrintScreen blocked | Window excluded from capture (`NSWindowSharingNone`) |
| Quit / close (Alt+F4 / Cmd+Q / Cmd+W) | Blocked | Blocked |
| Force quit / log out | Available as OS recovery | Available as OS recovery |
| Copy / cut / paste / print / save / view source | Blocked (OS hook + page script) | Blocked (page script) |
| DevTools (F12, Ctrl/Cmd+Shift+I/J/C) | Blocked | Blocked |
| Right-click, text selection, drag & drop | Blocked | Blocked |
| Multiple monitors | Detectable via `get_monitor_count` / `check_multiple_monitors` commands (both platforms) |

The app can only be exited by your web app calling the `close_lockdown` (or `close_app`) Tauri command, e.g.:

```js
import { invoke } from '@tauri-apps/api/core';
await invoke('close_lockdown');
```

The hosted quiz owns the active-quiz close flow: confirm, submit the attempt,
then invoke `close_lockdown`. Rust separately displays a loading-only Exit
control. Hide it as soon as the quiz and its session data are genuinely ready:

```js
await invoke('mark_quiz_ready');
```

Do not call `mark_quiz_ready` from a generic page-load event. Call it after the
quiz attempt has loaded and the normal Understandly close-and-submit flow is
available. Until then, Rust allows `close_during_loading`; after readiness it
rejects that command even if page code tries to invoke it.

## How it's launched

The app registers a deep-link scheme (default `understandly-lockdown://`). Links map onto your configured base URL:

```
understandly-lockdown://quiz?x=1            →  <base_url>/quiz?x=1
understandly-lockdown://results/987?y=true  →  <base_url>/results/987?y=true
```

If the app is already running, the link navigates the existing window (single-instance is enforced).

## Development

Prerequisites: [Rust](https://rustup.rs), the [Tauri CLI](https://tauri.app/start/) (`cargo install tauri-cli`), and on Windows the WebView2 runtime (preinstalled on Windows 10/11).

```bash
cargo tauri dev     # runs against base_url (e.g. http://localhost:3000)
cargo tauri build   # production bundle against production_url
```

All builds provide a native **emergency exit** shortcut: `Ctrl+Alt+Shift+Q`. It is intentionally independent of the hosted page and network so a parent can always recover from a failed or frozen session.

Auto-update: release builds check for signed updates during the pre-quiz loading phase. If an update is available, it installs and restarts before the quiz becomes active. A failed or timed-out check releases the quiz normally, and debug builds never replace themselves.

## Custom Configuration

If you are setting this up for your own platform, update the following files:

### 1. `lockdown.config.json`
Compiled into the binary at build time:
- `base_url`: Local development server URL (e.g., `http://localhost:3000`)
- `production_url`: Your hosted application URL (e.g., `https://www.yourdomain.com`)
- `window.title`: The title of the browser window
- `window.fullscreen` / `always_on_top` / `skip_taskbar`: Kiosk window behavior
- `loading_recovery.enabled`: Whether Rust displays an Exit button while the quiz is loading
- `loading_recovery.button_label`: The loading Exit button text
- `loading_recovery.confirmation_message`: The optional confirmation shown before closing during loading; use an empty string to disable it

### 2. `tauri.conf.json`
Application metadata and security:
- `identifier`: Your unique application identifier (e.g., `com.yourcompany.lockdown`)
- `productName` and `version` (keep `version` in sync with `Cargo.toml`)
- `plugins.deep-link.desktop.schemes`: Your custom URL scheme (replace `understandly-lockdown`)
- `plugins.updater.pubkey` & `endpoints`: Your own updater signing key and release URL (generate a keypair with `cargo tauri signer generate`)
- `app.security.csp`: Whitelist your own domains (`default-src`, `connect-src`, `img-src`, ...)
- In `app.security.capabilities`, find `hosted-exam-capability` and replace its `remote.urls` entries with the exact hosted origins allowed to invoke the six app commands

### Customizing Icons
1. Replace the base image with your own 1024x1024 PNG.
2. Generate all system icon formats:
   ```bash
   cargo tauri icon path/to/your-icon.png
   ```

## Releases (CI/CD)

- **CI** (`.github/workflows/ci.yml`): every push/PR is format-checked, linted (clippy), and built on Windows and macOS, with an additional Windows ARM64 cross-build.
- **Release** (`.github/workflows/release.yml`): pushing a tag like `v0.3.0` builds signed installers (NSIS for Windows x64 + ARM64, DMG for macOS aarch64 + x86_64), creates a GitHub release, and publishes the updater manifest (`latest.json`).

Before tagging, bump `version` in both `Cargo.toml` and `tauri.conf.json` to match the tag — the auto-updater compares against these.

Required GitHub secrets:

| Secret | Purpose |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Updater artifact signing |
| `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD` / `KEYCHAIN_PASSWORD` | macOS code signing (Developer ID Application `.p12`, base64) |
| `APPLE_API_ISSUER` / `APPLE_API_KEY` / `APPLE_API_KEY_BASE64` | macOS notarization (App Store Connect API key) |

### Setting up Windows Code Signing
To prevent Microsoft SmartScreen warnings on Windows, code sign the built executable.

1. **Purchase or Generate a Certificate**:
   - **For Production**: Purchase a Code Signing Certificate from a trusted CA (DigiCert, Sectigo, SSL.com, GlobalSign). *(Note: CAs now require hardware tokens or cloud HSMs like Azure Key Vault. If your CA doesn't provide a downloadable `.pfx`, use their cloud signing tool via `bundle.windows.signCommand` in `tauri.conf.json` instead of the steps below.)*
   - **For Testing (Self-Signed)**: Open PowerShell as Administrator:
     ```powershell
     $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Understandly Testing" -KeyExportPolicy Exportable -KeySpec Signature
     $pwd = ConvertTo-SecureString -String "testpassword123" -Force -AsPlainText
     Export-PfxCertificate -Cert $cert -FilePath "C:\certificate.pfx" -Password $pwd
     ```
2. **Base64 Encode the Certificate**:
   ```powershell
   $fileContentBytes = Get-Content 'C:\certificate.pfx' -AsByteStream
   [System.Convert]::ToBase64String($fileContentBytes) | Out-File -FilePath 'base64cert.txt'
   ```
3. **Add GitHub Secrets**: In **Settings > Secrets and variables > Actions**, add:
   - `WINDOWS_CERTIFICATE` (content of `base64cert.txt`)
   - `WINDOWS_CERTIFICATE_PASSWORD` (the `.pfx` password)
4. **Update `release.yml`**: Tauri 2 signs using a certificate from the Windows certificate store, referenced by thumbprint. Add this step before "Build Tauri App" (Windows only):
   ```yaml
      - name: Import Windows Certificate
        if: matrix.platform == 'windows-latest'
        shell: pwsh
        env:
          WINDOWS_CERTIFICATE: ${{ secrets.WINDOWS_CERTIFICATE }}
          WINDOWS_CERTIFICATE_PASSWORD: ${{ secrets.WINDOWS_CERTIFICATE_PASSWORD }}
        run: |
          $bytes = [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE)
          [IO.File]::WriteAllBytes("certificate.pfx", $bytes)
          $pwd = ConvertTo-SecureString -String $env:WINDOWS_CERTIFICATE_PASSWORD -Force -AsPlainText
          $cert = Import-PfxCertificate -FilePath certificate.pfx -CertStoreLocation Cert:\CurrentUser\My -Password $pwd
          $conf = Get-Content tauri.conf.json -Raw | ConvertFrom-Json
          $conf.bundle.windows.certificateThumbprint = $cert.Thumbprint
          $conf.bundle.windows.timestampUrl = "http://timestamp.digicert.com"
          $conf | ConvertTo-Json -Depth 32 | Set-Content tauri.conf.json -Encoding utf8
   ```

## License

[MIT](LICENSE)
