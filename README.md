# Feline

A fast, native desktop client for bulk-downloading posts from **e621** and **e926**. Built in Rust with a Slint UI.

---

## Overview

Feline turns tag-based searches into managed download jobs. Queue multiple queries, pause or resume them, and let the app handle rate limits, retries, and deduplication in the background. Credentials stay in your OS keychain; nothing is written to disk in plaintext.

## Features

- **Tag-based job queue** — add any number of search queries and run them concurrently
- **Site selection** — switch between e621 and e926 per run
- **Secure credentials** — username and API key stored in macOS Keychain, Windows Credential Manager, or Linux Secret Service
- **Smart deduplication** — MD5 index per tag folder skips anything already on disk (supports `{artist}__{md5}.{ext}` and legacy `{md5}.{ext}` layouts)
- **Rate-limit aware** — enforces e621's 2 requests/second policy via a token-bucket limiter
- **Robust retries** — exponential backoff on transient failures, up to 5 attempts per post
- **Pause & resume** — jobs survive app restarts; failed IDs and last-run timestamps are persisted
- **Filtering** — global tag blacklist and per-rating toggles (safe / questionable / explicit)
- **Structured logs** — live in-app log viewer plus daily-rotated log files on disk

## Tech Stack

| Layer | Choice |
|---|---|
| Language | Rust 2024 (MSRV 1.95) |
| UI | [Slint](https://slint.dev) |
| Async runtime | Tokio |
| HTTP | reqwest (rustls, HTTP/2, gzip, brotli) |
| Credentials | keyring (platform-native backends) |
| Rate limiting | governor |
| Retry | backon |
| Logging | tracing + tracing-appender |

## Platforms

Windows, macOS, and Linux. On Windows the build script embeds an app icon generated from `assets/icon.png`.

## Getting Started

### Prerequisites

- Rust **1.95** or newer
- A C toolchain (for native dependencies)

### Build

```bash
cargo build --release
```

### Run

```bash
cargo run --release
```

The binary is written to `target/release/feline`.

## Configuration

Feline stores its state next to the executable:

- `config.json` — download folder, queries, blacklist, rating filters
- `state.json` — per-query failed IDs and last-run timestamps
- OS keychain — username and API key

Logs are written to `log/app*.log` in the same directory. Set `RUST_LOG` to adjust verbosity (default: `info`).

## Project Structure

```
src/
  main.rs          Entry point and runtime setup
  app.rs           UI controller and event wiring
  config.rs        Config load/save
  credentials.rs   Keyring integration
  state.rs         Persistent job state
  logging.rs       Tracing subscribers
  e621/            API client, types, rate limiter
  download/        Job manager, workers, dedup index
ui/                Slint views, widgets, theme
assets/            Icon and fonts
build.rs           Slint compilation, Windows icon embedding
```

## License

Released under the [MIT License](LICENSE). Copyright (c) 2026 Nyabi.
