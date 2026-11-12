# UClip

UClip is a local-first clipboard manager. Version 0.1.0 currently provides a UI preview.

## Try the preview

- Moss dark and light themes.
- Sample text, images, and file entries with on-demand previews.
- Search, type filters, and regular expressions.
- Basic keyboard navigation and saved theme preferences.

The list uses in-memory examples and resets on refresh. System clipboard capture, persistent history, automatic pasting, and synchronization are not connected yet.

## Run from source

Install Node.js, Rust, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform, then run:

```bash
npm ci
npm run tauri:dev
```

Use `npm run dev` to try the sample interface in a browser.

## Platforms

Windows, macOS, and Linux are planned targets. The native interface has been verified on Linux; other platforms still require validation.
