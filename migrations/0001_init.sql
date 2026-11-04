CREATE TABLE players (
  id TEXT PRIMARY KEY,
  stats TEXT NOT NULL,
  seen_guide INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE boards (
  player_id TEXT NOT NULL,
  mode TEXT NOT NULL,
  puzzle INTEGER NOT NULL,
  answer_id TEXT NOT NULL,
  guesses TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (player_id, mode, puzzle)
);
