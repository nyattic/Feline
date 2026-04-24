# Changelog

## 0.3.1 — 2026-04-24

### Fixed
- Switch labels in Settings (rating filter, skip media types) were unreadable against the dark background — Slint widget style switched to `fluent-dark` to match the app theme

## 0.3.0 — 2026-04-24

### Added
- Skip-media-type switches in Settings (videos, flash, animations) — applied to every search via `-type:` filters

### Changed
- Rating filter rendered as switches instead of the chip selector

## 0.2.0 — 2026-04-24

### Added
- Cancel button on each query — stops the active job or drops a pending one without losing the saved query
- Serial job queue: extra Download requests wait behind the active job
- Animated phase label while a job is starting, discovering, or downloading

### Changed
- One-step download: typing a tag and pressing Download saves the query and starts the job in one action; the separate Add button is gone
- Discovery now finishes before downloads start, so the total count and progress bar are accurate from the first frame
- Paused state shows scan progress when interrupted mid-discovery instead of "0/N"
- Card header packs title, Cancel/Remove, and Download into one row
- Login failures report a clear reason

### Security
- File downloads are restricted to the official e621/e926 static hosts
- Streamed downloads are rejected on size mismatch with the post manifest
