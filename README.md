# Feline

![Release](https://img.shields.io/github/v/release/nyabi021/Feline?style=flat&color=6366f1)
![Downloads](https://img.shields.io/github/downloads/nyabi021/Feline/total?style=flat&color=10b981)
![Last Commit](https://img.shields.io/github/last-commit/nyabi021/Feline?style=flat&color=f59e0b)
![License](https://img.shields.io/badge/license-MIT-8b5cf6?style=flat)

A native desktop downloader for e621 and e926 tag searches.

Feline turns saved tag queries into managed download jobs. It handles API rate
limits, retries, deduplication, filtering, and failed-post state while keeping
credentials in your OS keychain.

## Features

- Save tag searches as bookmarks; re-run them to fetch only new posts
- Serial job queue: extra Download requests wait behind the active job
- Pause, resume, or cancel the active job at any time
- Switch between e621 and e926
- Filter by rating, blacklist tags, and skip-media-type toggles (videos, flash, animations)
- Deduplicate files per query folder by MD5
- Verify downloaded files by size and checksum
- Store username and API key in the OS credential store
- Keep live logs in-app and daily log files on disk

## Usage

1. Generate an API key from your e621/e926 account settings.
2. Open Feline and log in from Settings. Credentials are saved in your OS
   credential store, not in `config.json`.
3. Choose a download folder, site, rating filter, blacklist, and any media types to skip.
4. On the Queue page, type a tag search and press Download — the query is
   saved and the job starts in one action.
5. Re-run a saved query later with its row's Download button to pull only
   posts that are new since last run.

Downloaded files are saved under the selected download folder as:

```text
{query}/{artist}__{md5}.{ext}
```

The query folder name is sanitized for the local filesystem. The MD5 is the
post file hash from e621/e926.

## Network Access

Feline connects directly to e621/e926. If those sites are blocked or restricted
in your country or network, for example in South Korea, the app may fail to log
in, search, or download files.

Use a VPN or another lawful network route that can access e621/e926.

## API Notes

Feline uses a descriptive `User-Agent` and authenticates with HTTP Basic auth
when credentials are available. API requests are rate-limited to e621/e926's
documented hard limit of 2 requests per second, and large searches are paginated
with `page=b<ID>`.

## Build

Requirements:

- Rust 1.95
- A native C toolchain

```bash
cargo build --release
```

Run from source:

```bash
cargo run --release
```

The release binary is written to `target/release/feline` (or `feline.exe` on Windows).

## Configuration

Feline stores app data next to the executable:

- `config.json` for download folder, saved queries, blacklist, rating, and skip-media-type filters
- `state.json` for per-query failed post IDs and last-run timestamps
- `log/app*.log` for daily-rotated logs
- OS keychain, Credential Manager, or Secret Service for credentials

Set `RUST_LOG` to adjust logging verbosity. The default is `info`.

## Development

Useful checks:

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

Tech stack: Rust, Slint, Tokio, reqwest, keyring, governor, tracing.

## License

MIT. See [LICENSE](LICENSE).
