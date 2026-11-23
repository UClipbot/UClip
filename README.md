# UClip

English | [Simplified Chinese](README.zh-CN.md)

UClip is a local-first clipboard manager. Save what you copy, then quickly search, organize, and paste it again. History is stored locally, with optional encrypted sync.

## Features

- **Clipboard history**: Keep text, images, and file or folder references across restarts.
- **Sync**: Merge history across devices on your local network or using WebDAV or SFTP storage.
- **Quick search**: Search by content and type, with regular expression support.
- **Copy and paste**: Copy history back to the clipboard. Linux X11 also supports pasting into the original app.
- **Capture controls**: Pause or resume capture, set exclusion rules, and limit history capacity.
- **Manual entries**: Create text records for snippets you use often.
- **Pins and deduplication**: Keep useful content pinned and remove its unpinned duplicates from the pinned view.
- **Collections**: Create and rename collections, and move or copy records between them.
- **Trash**: Restore deleted records, permanently delete selected records, or empty Trash.
- **Hover previews**: Read text, images and file references in a separate, resizable desktop window with the latest capture/save time. Hovering keeps your focus; click the preview to keep reading, or press Esc to close it.
- **Custom shortcuts**: Adjust search, copy, preview, delete, and navigation shortcuts.
- **Desktop integration**: Show the window from the tray or a shortcut, keep it always on top, pin it to prevent hiding on blur, and launch at login.
- **Interface languages**: Simplified Chinese and English. The default follows your system language; change it in **Settings → Appearance → Language**. Your choice persists across restarts.
- **Appearance**: Light and dark themes, with native or custom title bars.

## Usage

1. Launch UClip. Capture starts automatically; you can pause or resume it in **Settings → Capture**. Your choice is saved across restarts.
2. Copy content in other apps. New copies appear in your history.
3. Open UClip from the target app with its main-window shortcut. On Linux X11 the default is Ctrl+Shift+V.
4. Select one record and press Enter to paste it into the original app. If automatic paste is unavailable, UClip copies the record for manual paste.

In **Settings → Shortcuts**, configure the main window and Quick Paste panel under **Global shortcuts**. Edit search, paste and navigation in the **Window shortcuts** group on the same page. In Quick Paste, press **1–5** to select a recent item. Use the main-window shortcut to expand the panel. You can also press the panel shortcut's final key again if it is a letter or another non-digit key. To include pinned items, use **Settings → Desktop → Show pinned items in Quick Paste**.

File records store references. The original files must still exist when you use them.

Search and filters are shown by default. Change this in Settings → Appearance.

## Sync history

1. In **Settings → Sync**, create a group on one device. Show the pairing code and enter it under **Join sync group** on each other device. A paired group shares all existing and future history, including its collections, pins, and Trash.
2. Devices discover each other on the same IPv4 local network. The first established host coordinates the merge. Another online device takes over when the host leaves. Keep local network access enabled for UClip in your firewall.
3. For devices that are online at different times, select **WebDAV** or **SFTP** under **Server storage** on both devices. Use the same pairing code and remote directory. WebDAV uses HTTPS and supports HTTP Basic authentication. SFTP uses password authentication and verifies the SSH host key against the SHA-256 fingerprint you provide. The server stores encrypted history. Leave local network sync on to use both paths at the same time.
4. To paste a file from another device, select its item in history and copy or paste it as usual. UClip retrieves the original from the online source over the local network. Transfers support regular files and folders, up to 256 MiB per paste. UClip caches received copies locally and removes copies older than 24 hours at startup or before a later transfer.

Synced items are ordered by capture or save time within the pinned and unpinned groups. A conflict between edits, moves to Trash, and restores keeps the latest operation. Permanent deletion takes precedence for the same record, including restores made while a device was offline. LAN sync and server storage use protocol version 4. Shared history has a 512 MiB payload limit, and synced items are retained across devices independently of the local capture count setting.

In **Server storage**, choose **Sync server storage now** to request an immediate sync with the configured server.

## Common shortcuts

| Action | Default shortcut |
| --- | --- |
| Show or hide search and filters | Ctrl/Cmd+F |
| Settings | Ctrl/Cmd+, |
| Paste one selected record, with copy as a fallback | Enter |
| Copy the selection | Ctrl/Cmd+C |
| View full content | Space |
| Delete selected record | Delete |
| Go back, clear search, or hide window | Esc |

## Platforms

| Platform | Current status |
| --- | --- |
| Linux X11 | Natively verified on Linux Mint 22; capture and automatic paste are supported |
| Linux Wayland | Local UI and history queries verified; capture and automatic paste are not supported yet |
| Windows / macOS | Platform adapters exist; native acceptance testing is pending |

UClip is in development. Migration from older data is pending.

## Command line and headless synchronization

The desktop installers include the `uclip` command. A Linux deb installs it on the system path. On Windows, run `uclip.exe` from the UClip installation directory. On macOS, use `/Applications/UClip.app/Contents/MacOS/uclip`. To install it without the desktop app, use the Rust and C build toolchain below. History and headless sync commands can run without a display server.

```bash
cargo install --locked --path crates/uclip-cli
printf 'A note\n' | uclip save
uclip list --json
uclip search note --full
uclip get latest --raw
uclip copy --file ./notes.txt
```

`get` accepts a UUID, `latest`, `--position` (one-based), or `--index` (zero-based). Text displayed in a terminal escapes control characters except newlines and tabs. Use `--raw` for the original text bytes with no added newline. Images are read as PNG, and file references are read as a JSON array of paths. `list` and `search` accept `--collection` (name or UUID) and `--limit`. `--full` produces JSON Lines with complete content from one history snapshot. Use `--image` to read an image, `--file` to read text, or repeat `--reference` to save paths. `completion bash` and `completion zsh` generate shell completion scripts. Diagnostics go to stderr. Exit codes are `0` for success, `1` for other errors, `2` for invalid input, `3` for missing data, and `4` for conflicts or a busy library. An empty query result succeeds with zero rows.

The default library follows the desktop installation for the same build profile. `--dev` selects the `tauri:dev` library, `--portable` selects a profile under `uclip-data` beside the executable, and `--data-dir` or `UCLIP_DATA_DIR` selects an absolute library directory. On Unix, use a directory private to the account running UClip. History access works without a GUI. Clipboard writes require a supported desktop session, and copying a remote file reference still requires the desktop's LAN retrieval path. On X11, content that exceeds the server's request size limit returns `limit_exceeded`.

CLI help and diagnostics follow the terminal locale (`LC_ALL`, `LC_MESSAGES`, then `LANG`) or the system display language on Windows and macOS. Chinese locales use Simplified Chinese, and other locales use English. Set `UCLIP_LANG` to `zh-CN` or `en` to choose a language, or to `system` for automatic selection. History output and exit codes are language-independent.

To keep an encrypted sync peer online, initialize a new private library and start the service in the foreground:

```bash
export UCLIP_DATA_DIR="$HOME/.local/share/uclip-server"
uclip sync create
uclip sync code
uclip serve
```

Treat the pairing code like a password and enter it on other devices. To reuse an existing group, pass its code to `uclip sync join` through stdin. `uclip sync remote < profile.json` accepts a WebDAV or SFTP profile through stdin. Use `uclip sync remote --help` for the format, and `uclip sync status` to inspect saved settings without credentials. LAN discovery uses IPv4 multicast, and the headless peer follows the same host election as desktop peers. Run one sync process per library and stop it before changing sync settings. `serve` handles Ctrl+C and Unix SIGTERM, so a service manager can supervise it directly.

## Run from source

Install Node.js, Rust, and the [Tauri build prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform, then run:

```bash
npm ci
npm run tauri:dev
```

Use `npm run dev` for the in-memory browser preview. The browser remembers only language and appearance preferences; reloading resets sample records.

## Install locally on Linux

With the build prerequisites above, run `./scripts/install.sh` from a terminal in your Linux desktop session. The script builds UClip, installs it for your user without `sudo`, adds **UClip** to the applications menu, and starts it. Run it again after updating the source. Once the build is ready, the script closes the running copy of this installation and starts the newly installed build.

## Build installers

```bash
npm run package
```

Build on the target operating system: Linux produces deb and AppImage packages, Windows produces an NSIS installer, and macOS produces app and dmg bundles.

The Windows setup executable includes the WebView2 Evergreen Standalone Installer. It can install the runtime without internet access when WebView2 is missing. The MSVC C runtime is statically linked into UClip. Run the setup executable from the `bundle/nsis` directory to install UClip and its prerequisites. Building the Windows installer may download the WebView2 payload.

Linux deb packages declare the GTK, WebKitGTK, and tray dependencies for the package manager. AppImage includes those libraries and still uses the host's glibc. Build Linux distribution packages on the oldest target distribution you support, using the packaging workflow's Ubuntu 22.04 build environment as a baseline. macOS app bundles use the system's WKWebView.
