# Changelog

## 0.1.2

### Security

- **cookie 0.18.3.** RUSTSEC-2026-0594 found that private cookie nonces can repeat in a process that forks; the game's cookie is a plain one, so the advisory does not reach it, but the patched release clears the audit.

### Changed

- **Datastar 1.0.5, through askama_datastar.** The game's patches, its POST guard and Datastar's script come from the shared crate rather than code of the game's own, and Datastar moves from 1.0.4 to 1.0.5, which fixes reconnecting after a tab returns to the foreground.

### Fixed

- **Fonts are cached for good.** The stylesheet named them by bare URLs, which browsers rechecked every hour; it now names each by a fingerprinted URL, kept until a release changes it.
- **An overloaded or misbehaving request no longer takes others down with it.** Requests had no bounds; a body over 8 KiB is now refused, a request still going after 10 s ends with a 503, a panicking handler answers 500 instead of dropping the connection, and past 64 requests at once the surplus is turned away with a 503 rather than queued.
- **Deploys no longer stall for half a minute.** The server ignored the SIGTERM a rollout sends, as PID 1 in a scratch image has no default handler, so each restart waited out the grace period; it now keeps serving for five seconds while the ingress lets go, then finishes what is in flight and exits.

### Internal

- **The advisory audit runs `cargo audit` directly.** `rustsec/audit-check` reported through a check run it created and then updated, and the update sometimes came back 404, failing the job after a clean audit; `cargo audit`'s exit code is now the gate, as in koan.

## 0.1.1

### Changed

- **Brotli and zstd compression.** Pages, assets and fragment responses are now compressed with brotli or zstd when the browser accepts them, falling back to gzip; previously only gzip was built in.

## 0.1.0

### Added

- **Server-driven Tubele.** A port of Tubele to Rust (axum, askama, Datastar, Tailwind). The schedule, every board and every player's record live on the server, so no answer reaches the browser before its board is finished. Gameplay, clues, search, sharing and the Stories card match the original.
