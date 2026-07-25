# Feline

![Release](https://img.shields.io/github/v/release/nyattic/Feline?style=for-the-badge&logo=github&logoColor=white&labelColor=1e1b2e&color=6366f1)
![Downloads](https://img.shields.io/github/downloads/nyattic/Feline/total?style=for-the-badge&logo=github&logoColor=white&labelColor=1e1b2e&color=6366f1)
![Last Commit](https://img.shields.io/github/last-commit/nyattic/Feline?style=for-the-badge&logo=git&logoColor=white&labelColor=1e1b2e&color=6366f1)
![License](https://img.shields.io/badge/license-MIT-6366f1?style=for-the-badge&logo=opensourceinitiative&logoColor=white&labelColor=1e1b2e)

A native desktop downloader for e621 and e926 tag searches.

## Features

- Save tag searches as bookmarks; re-run them to skip files already present locally
- Serial job queue with pause, resume, cancel, and bounded completion history
- Filter by rating, blacklist tags, and skip-media-type toggles (videos, flash, animations)
- Streaming discovery with a configurable per-run download limit
- MD5-based deduplication with size and checksum verification
- Credentials stored in the OS credential store

## Usage

1. Generate an API key from your e621/e926 account settings.
2. Open Feline and log in from Settings.
3. Choose a download folder, site, rating filter, blacklist, and any media types to skip.
4. On the Queue page, type a tag search and press Download — the query is saved and the job starts.
5. Re-run a saved query later with its row's Download button. Feline scans the query folder and skips posts whose MD5 is already present.

Files are saved as `{query}/{artist}__{md5}.{ext}` under the chosen folder.
New installations limit each run to 5,000 new files by default; Settings can change the limit or remove it.

Media cache operations exposed through the FFI layer are limited to cache directories created by Feline, and direct file downloads through that layer must target the configured cache directory. These checks live at the FFI boundary. The underlying `feline-core` functions take arbitrary paths and enforce no scoping of their own, so embedders that call the library directly are responsible for validating the paths they pass.

## Network Access

Feline connects directly to e621/e926. If those sites are blocked in your network or country (for example, South Korea), the app will fail to log in or download. Use a VPN or another lawful route.

## Build

Requires a native C toolchain. The repository includes `rust-toolchain.toml` and pins Rust 1.95.0.

```bash
cargo run --release
```

The binary is written to `target/release/feline` (or `feline.exe` on Windows).

Desktop data uses each platform's standard user directories:

- macOS: data in `~/Library/Application Support/Feline`, logs in `~/Library/Logs/Feline`
- Windows: data in `%APPDATA%\Feline`, logs in `%LOCALAPPDATA%\Feline\log`
- Linux: data in `$XDG_CONFIG_HOME/Feline` and logs in `$XDG_STATE_HOME/Feline/log`, with the usual `~/.config` and `~/.local/state` fallbacks

The default download folder is `~/Downloads/Feline`. Existing Windows and Linux `config.json` and `state.json` files beside the executable are copied to the new data directory on first launch. Credentials remain in the OS credential store.

## License

MIT. See [LICENSE](LICENSE).
