# Tubele

A daily London station guessing game. Six guesses; each one tells you how many stops away the answer is by train, whether it is in the same fare zone, and which lines it shares. Every miss reveals another clue.

This is a server-driven port of [Mike Keefe's Tubele](https://github.com/mike-keefe/tubele). The original is a React app that ships the whole answer schedule to the browser, obfuscated but recoverable. Here the schedule, every board and every player's record live on the server: the page is rendered by askama, interactions are Datastar requests answered with SSE patches, and the answer reaches the browser only once the board it belongs to is finished.

## Why this exists

Mike's Tubele is a lovely game with one flaw: it believes everything your browser tells it.

- **A year of answers ships to every player.** `schedule.json` holds the whole schedule, XOR-packed with the key `mind-the-gap`, and the key is in the bundle next to it. The original README calls this "a speed bump, not security", which is generous to the speed bump. Thirty seconds in devtools gets you every answer until next August.
- **Your streak is whatever you say it is.** Stats live in `localStorage`, so a 400-day streak is one `localStorage.setItem` away.
- **The board is yours to rewind.** Lose, clear site data, and replay the day knowing the answer.

Here the schedule never leaves the server, the board and your record are rows in SQLite, and the browser is told the answer only after it has earned it. Search and clues are computed server-side, and so is every guess: the client sends what you typed, not which station you meant.

It does this with less in the browser, too. The original is a React single-page app; this is HTML rendered in Rust, with Datastar (about 13 KB gzipped) patching it over SSE and one 3.5 KB script for the clipboard and the Stories card. Everything that made the original good, including its accessibility work (the ARIA combobox, the live region announcing every guess), came across unchanged, because it was good.

You can still open a private window, lose on purpose and come back knowing the answer. Some problems are not software.

## Running it

```sh
mise install
cargo run          # http://localhost:8080, database in ./tubele.db
```

| Variable        | Default                 | Meaning                                                  |
| --------------- | ----------------------- | -------------------------------------------------------- |
| `BIND_ADDR`     | `0.0.0.0:8080`          | Where to listen                                          |
| `DATABASE_PATH` | `tubele.db`             | The SQLite file; created and migrated on start           |
| `SITE_URL`      | `http://localhost:8080` | The public origin, used in share links and the canonical |

After changing `styles/`, a template or the class strings in `src/`, run `just css`: the compiled `assets/app.css` is committed because the binary embeds it.

## How it is put together

```
data/        The station graph, lines and packed answer schedule, from the original game's pipeline.
src/engine   The graph, distances, the clue ladder, search and the schedule. Pure.
src/game     A board and a player's record.
src/store    SQLite through rusqlite; migrations counted by PRAGMA user_version.
src/ui       Routes, SSE patches, assets and security headers.
src/view     View models. The answer enters a view only through ResultView, which exists only for a finished board.
templates/   askama templates.
assets/      Datastar, app.js (sharing, the Stories card, list placement), fonts and icons, all embedded.
deploy/      The Pulumi component, published as @radiosilence/tubele-pulumi.
```

A player is a random id in an HttpOnly cookie; nothing else about them is stored. Practice boards live server-side too and never touch the record. Every POST must carry Datastar's request header, which a cross-site form cannot set.

The game logic follows the original exactly where it is observable: the same stable hash picks the revealed mode and line, so clues match the React version for the same station; distances are breadth-first with free interchanges; the share grid uses the same non-linear bands.

Analytics, the `/stats` dashboard and the disabled trend and journey hints from the original are not ported.

## Releasing

`just release patch|minor|major` opens a release PR that bumps the crate, the Pulumi package and the image version it pins together. Merging it pushes the image, publishes the package and creates the GitHub release. See `CLAUDE.md`.

## Licence and attribution

Station data is derived from TfL Open Data and contains OS data © Crown copyright and database rights. Not affiliated with Transport for London. The game design, data pipeline, station facts and answer schedule are Mike Keefe's.
