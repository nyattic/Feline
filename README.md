# Feline

![Release](https://img.shields.io/github/v/release/nyattic/Feline?style=flat&color=6366f1)
![Downloads](https://img.shields.io/github/downloads/nyattic/Feline/total?style=flat&color=10b981)
![Last Commit](https://img.shields.io/github/last-commit/nyattic/Feline?style=flat&color=f59e0b)
![License](https://img.shields.io/badge/license-MIT-8b5cf6?style=flat)

A native desktop downloader for e621 and e926 tag searches.

## Features

- Save tag searches as bookmarks; re-run them to skip files already present locally
- Serial job queue with pause, resume, and cancel
- Filter by rating, blacklist tags, and skip-media-type toggles (videos, flash, animations)
- MD5-based deduplication and size/checksum verification
- Credentials stored in the OS credential store

## Usage

1. Generate an API key from your e621/e926 account settings.
2. Open Feline and log in from Settings.
3. Choose a download folder, site, rating filter, blacklist, and any media types to skip.
4. On the Queue page, type a tag search and press Download — the query is saved and the job starts.
5. Re-run a saved query later with its row's Download button. Feline scans the query folder and skips posts whose MD5 is already present.

Files are saved as `{query}/{artist}__{md5}.{ext}` under the chosen folder.

Media cache operations exposed through the core library are limited to cache directories created by Feline, and direct file downloads through the FFI layer must target that configured cache directory.

## Network Access

Feline connects directly to e621/e926. If those sites are blocked in your network or country (for example, South Korea), the app will fail to log in or download. Use a VPN or another lawful route.

## Build

Requires a native C toolchain. The repository includes `rust-toolchain.toml` and pins Rust 1.95.0.

```bash
cargo run --release
```

The binary is written to `target/release/feline` (or `feline.exe` on Windows).

On macOS, app data is stored under `~/Library/Application Support/Feline` and logs under `~/Library/Logs/Feline`. On other desktop platforms, app data lives next to the executable as `config.json`, `state.json`, `downloads/`, and `log/`. Credentials are stored separately in the OS keychain.

## License

MIT. See [LICENSE](LICENSE).
