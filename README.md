# UClip

English | [Simplified Chinese](README.zh-CN.md)

UClip is a local-first clipboard manager. Save what you copy, then quickly search, organize, and paste it again. Your history stays on your device.

## Features

- **Clipboard history**: Keep text, images, and file or folder references across restarts.
- **Quick search**: Search by content and type, with regular expression support.
- **Copy and paste**: Copy history back to the clipboard. Linux X11 also supports pasting into the original app.
- **Capture controls**: Pause or resume capture, set exclusion rules, and limit history capacity.
- **Manual entries**: Create text records for snippets you use often.
- **Pins and deduplication**: Keep useful content pinned and remove its unpinned duplicates from the pinned view.
- **Collections**: Create and rename collections, and move or copy records between them.
- **Hover previews**: Read text, images and file references in a separate, resizable desktop window with the latest capture/save time. Hovering keeps your focus; click the preview to keep reading, or press Esc to close it.
- **Custom shortcuts**: Adjust search, copy, preview, delete, and navigation shortcuts.
- **Desktop integration**: Show the window from the tray or a shortcut, keep it always on top, pin it to prevent hiding on blur, and launch at login.
- **Interface languages**: Simplified Chinese and English. The default follows your system language; change it in **Settings → Appearance → Language**. Your choice persists across restarts.
- **Appearance**: Light and dark themes, with native or custom title bars.

## Usage

1. Launch UClip. Capture starts automatically; you can pause or resume it in **Settings → Capture**. Your choice is saved across restarts.
2. Copy content in other apps. New copies appear in your history.
3. Search and select a record, then press Enter or click Copy. Paste it in the target app.
4. On Linux X11, use Ctrl+Shift+V to show UClip, then choose **Paste to original app**.

File records store references. The original files must still exist when you use them.

## Common shortcuts

| Action | Default shortcut |
| --- | --- |
| Search | Ctrl/Cmd+K |
| Settings | Ctrl/Cmd+, |
| Copy selected record | Enter |
| View full content | Space |
| Delete selected record | Delete |
| Go back, clear search, or hide window | Esc |

## Platforms

| Platform | Current status |
| --- | --- |
| Linux X11 | Natively verified on Linux Mint 22; capture and automatic paste are supported |
| Linux Wayland | Local UI and history queries verified; capture and automatic paste are not supported yet |
| Windows / macOS | Platform adapters exist; native acceptance testing is pending |

UClip is in development. Cloud sync, CLI, and migration from older data are not available yet.

## Run from source

Install Node.js, Rust, and the [Tauri build prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform, then run:

```bash
npm ci
npm run tauri:dev
```

Use `npm run dev` for the in-memory browser preview. The browser remembers only language and appearance preferences; reloading resets sample records.

## Build installers

```bash
npm run package
```

Build on the target operating system: Linux produces deb and AppImage packages, Windows produces an NSIS installer, and macOS produces app and dmg bundles.
