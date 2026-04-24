# Feline

![Release](https://img.shields.io/github/v/release/nyabi021/Ditto?style=flat&color=6366f1)
![Downloads](https://img.shields.io/github/downloads/nyabi021/Ditto/total?style=flat&color=10b981)
![Last Commit](https://img.shields.io/github/last-commit/nyabi021/Ditto?style=flat&color=f59e0b)
![License](https://img.shields.io/badge/license-MIT-8b5cf6?style=flat)

A native desktop downloader for e621 and e926 tag searches.

Feline turns saved tag queries into managed download jobs. It handles API rate
limits, retries, deduplication, filtering, and failed-post state while keeping
credentials in your OS keychain.

## Features

- Queue multiple tag searches
- Pause and resume active jobs
- Switch between e621 and e926
- Filter by rating and global blacklist tags
- Deduplicate files per query folder by MD5
- Verify downloaded files by size and checksum
- Store username and API key in the OS credential store
- Keep live logs in-app and daily log files on disk

## Usage

1. Generate an API key from your e621/e926 account settings.
2. Open Feline and log in from Settings. Credentials are saved in your OS
   credential store, not in `config.json`.
3. Choose a download folder, site, rating filter, and optional blacklist tags.
4. Add one or more tag queries on the Queue page.
5. Click Download to scan matching posts and fetch anything missing.

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

The release binary is written to `target/release/feline`.

## Configuration

Feline stores app data next to the executable:

- `config.json` for download folder, saved queries, blacklist, and rating filters
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
