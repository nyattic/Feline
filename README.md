# Feline

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

Downloaded files are saved under the selected download folder as:

```text
{query}/{artist}__{md5}.{ext}
```

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
