# Understandly Lockdown Browser

This project provides a lightweight, basic, lockdown experience originally made for Understandly.

However, the goal is for this application to work independently. Other developers can build from it by setting specific configurations for their own use cases.

## Custom Configuration

If you are setting this up for your own platform, you will need to update the following files to point to your services:

### 1. `lockdown.config.json`
Update the connection URLs and window title:
- `base_url`: Local development server URL (e.g., `http://localhost:3000`)
- `production_url`: Your actual hosted application URL (e.g., `https://www.yourdomain.com`)
- `window.title`: The title that appears on the browser window.

### 2. `tauri.conf.json`
Update the application metadata to match your brand:
- `identifier`: Your unique application identifier (e.g., `com.yourcompany.lockdown`)
- `productName`: The name of your built application.
- `plugins.deep-link.desktop.schemes`: The custom URL scheme to open the app (e.g., change `understandly-lockdown` to `yourbrand-lockdown`).
- `plugins.updater.pubkey` & `endpoints`: Update with your own Tauri updater configuration and release URL.
- `app.security.csp`: Update the Content Security Policy rules (like `default-src`, `connect-src`, `img-src`) to whitelist your own domains.

### Customizing Icons
To replace the default Understandly icons with your own:
1. Replace the base image with your own 1024x1024 PNG image.
2. Generate all the necessary system icon formats by running:
   ```bash
   cargo tauri icon path/to/your-icon.png
   ```