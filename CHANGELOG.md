# Changelog

## 0.1.1

### Changed

- **Brotli and zstd compression.** Pages, assets and fragment responses are now compressed with brotli or zstd when the browser accepts them, falling back to gzip; previously only gzip was built in.

## 0.1.0

### Added

- **Server-driven Tubele.** A port of Tubele to Rust (axum, askama, Datastar, Tailwind). The schedule, every board and every player's record live on the server, so no answer reaches the browser before its board is finished. Gameplay, clues, search, sharing and the Stories card match the original.
