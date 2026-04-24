# Feline

A native desktop downloader for e621 and e926 tag searches.

Feline turns saved tag queries into managed download jobs. It handles API rate
limits, retries, deduplication, filtering, and failed-post state while keeping
credentials in your OS keychain.

Repository: <https://github.com/nyabi021/Feline>

## Features

- Queue multiple tag searches
- Pause and resume active jobs
- Switch between e621 and e926
- Filter by rating and global blacklist tags
- Deduplicate files per query folder by MD5
- Verify downloaded files by size and checksum
- Store username and API key in the OS credential store
- Keep live logs in-app and daily log files on disk

## Why Feline?

Feline is meant for people who want to repeatedly archive specific tag searches,
artists, or favorite-style queries without managing command-line scripts or
browser tabs. It keeps each query as its own repeatable job, scans the target
folder before downloading, skips files that are already present, and remembers
posts that are permanently unavailable.

It is not a gallery manager like Hydrus and it is not intended to replace
general-purpose tools like gallery-dl. The focus is a small native UI for
e621/e926 search downloads with predictable folders and verified files.

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
post file hash from e621/e926, so Feline can detect already-downloaded files in
that query folder.

## API Notes

Feline uses a descriptive `User-Agent` and authenticates with HTTP Basic auth
when credentials are available. API requests are rate-limited to e621/e926's
documented hard limit of 2 requests per second, and large searches are paginated
with `page=b<ID>`.

## Build

Requirements:

- Rust 1.95 or newer
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
