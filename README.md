# UClip

UClip is a local-first clipboard manager for saving copied content, finding it quickly, and pasting it again. History stays on your computer.

## Features

- **Clipboard history:** keep copied text, images, and file or folder references across restarts.
- **Quick search:** find entries by content or type, with regular expression support.
- **Copy and paste:** copy a saved entry again, or paste it into the previous application on Linux X11.
- **Capture controls:** pause or resume capture, set exclusions, and choose a history limit.
- **Manual entries:** save text snippets directly.
- **Custom shortcuts:** configure search, copy, preview, deletion, and navigation keys.
- **Desktop integration:** use the tray or a shortcut to reopen the window, with pinning, hide-on-blur, and login startup options.
- **Light and dark themes:** choose an appearance and keep your preference.

## Getting started

1. Open the app and enable capture.
2. Copy content in another application. New copies appear in the history list.
3. Search for an entry and press Enter or click Copy, then paste it into your destination application.
4. On Linux X11, press Ctrl+Shift+V to open the window and choose the action to paste into the previous application.

File entries store references, so the original files must still be available when used.

## Default shortcuts

| Action | Shortcut |
| --- | --- |
| Search | Ctrl/Cmd+K |
| Settings | Ctrl/Cmd+, |
| Copy the selected entry | Enter |
| Preview full content | Space |
| Delete the selected entry | Delete |
| Go back, clear search, or hide the window | Esc |

## Platforms

| Platform | Status |
| --- | --- |
| Linux X11 | Native capture and automatic pasting verified on Linux Mint 22 |
| Linux Wayland | Local UI and history queries verified; capture and automatic pasting are unavailable |
| Windows / macOS | Platform adapters are in development; native acceptance testing is pending |

This is a development version. Cloud synchronization, a CLI, and migration from earlier products are not available yet.

## Run from source

Install Node.js, Rust, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform, then run:

```bash
npm ci
npm run tauri:dev
```

Use `npm run dev` to try the in-memory sample interface in a browser.
