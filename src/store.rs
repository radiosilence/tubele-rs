//! SQLite: one row per player for their record, one per board in play. A
//! player is an anonymous id in a cookie; nothing else about them is kept.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Result, TransactionBehavior, params};

use crate::game::{Board, Mode, Stats};

/// Migrations in order; `PRAGMA user_version` counts those applied.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// Idle connections kept for reuse; a burst past it opens and drops extras.
const KEEP_IDLE: usize = 8;

#[derive(Clone)]
pub struct Db(Arc<Pool>);

struct Pool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl Db {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Self> {
            let mut conn = Connection::open(&path)?;
            configure(&conn)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            migrate(&mut conn)?;
            Ok(Self(Arc::new(Pool {
                path,
                idle: Mutex::new(vec![conn]),
            })))
        })
        .await?
    }

    /// A fresh database in a temporary file, for tests: a file rather than
    /// `:memory:` so every pooled connection sees the same data.
    #[cfg(test)]
    pub async fn temporary() -> Self {
        let path = std::env::temp_dir().join(format!("tubele-test-{}.db", crate::new_id()));
        Self::open(&path).await.expect("test database")
    }

    /// Run `f` on a pooled connection, on the blocking pool.
    pub async fn run<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
    {
        let pool = self.0.clone();
        match tokio::task::spawn_blocking(move || {
            let popped = pool.idle.lock().expect("pool lock").pop();
            let mut conn = match popped {
                Some(c) => c,
                None => {
                    let c = Connection::open(&pool.path)?;
                    configure(&c)?;
                    c
                }
            };
            let out = f(&mut conn);
            let mut idle = pool.idle.lock().expect("pool lock");
            if idle.len() < KEEP_IDLE {
                idle.push(conn);
            }
            out
        })
        .await
        {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY;")
}

fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let applied = usize::try_from(applied).unwrap_or(0);
    if applied > MIGRATIONS.len() {
        anyhow::bail!(
            "database schema version {applied} is newer than this build's {}",
            MIGRATIONS.len()
        );
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

/// A write transaction taken up front, so a read-modify-write of a board
/// cannot interleave with a second request from the same player.
pub fn write(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
}

pub struct Player {
    pub stats: Stats,
    pub seen_guide: bool,
}

pub fn player(conn: &Connection, id: &str) -> Result<Player> {
    let row = conn
        .query_row(
            "SELECT stats, seen_guide FROM players WHERE id = ?",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((stats, seen_guide)) => Player {
            stats: serde_json::from_str(&stats).unwrap_or_default(),
            seen_guide,
        },
        None => Player {
            stats: Stats::default(),
            seen_guide: false,
        },
    })
}

pub fn save_stats(conn: &Connection, id: &str, stats: &Stats) -> Result<()> {
    let json = serde_json::to_string(stats).expect("stats serialise");
    conn.execute(
        "INSERT INTO players (id, stats) VALUES (?1, ?2)
         ON CONFLICT (id) DO UPDATE SET stats = ?2",
        params![id, json],
    )?;
    Ok(())
}

pub fn mark_guide_seen(conn: &Connection, id: &str) -> Result<()> {
    let empty = serde_json::to_string(&Stats::default()).expect("stats serialise");
    conn.execute(
        "INSERT INTO players (id, stats, seen_guide) VALUES (?1, ?2, 1)
         ON CONFLICT (id) DO UPDATE SET seen_guide = 1",
        params![id, empty],
    )?;
    Ok(())
}

/// The daily board for a puzzle, or the player's one practice board (puzzle 0).
pub fn board(conn: &Connection, id: &str, mode: Mode, puzzle: i64) -> Result<Option<Board>> {
    conn.query_row(
        "SELECT answer_id, guesses FROM boards WHERE player_id = ? AND mode = ? AND puzzle = ?",
        params![id, mode.as_str(), puzzle],
        |r| {
            Ok(Board {
                mode,
                puzzle,
                answer_id: r.get(0)?,
                guesses: serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default(),
            })
        },
    )
    .optional()
}

pub fn save_board(conn: &Connection, id: &str, board: &Board) -> Result<()> {
    let guesses = serde_json::to_string(&board.guesses).expect("guesses serialise");
    conn.execute(
        "INSERT INTO boards (player_id, mode, puzzle, answer_id, guesses) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (player_id, mode, puzzle)
         DO UPDATE SET answer_id = ?4, guesses = ?5, updated_at = unixepoch()",
        params![id, board.mode.as_str(), board.puzzle, board.answer_id, guesses],
    )?;
    Ok(())
}

/// Daily boards older than yesterday can no longer be played, and their result
/// is already in the player's record.
pub fn prune(conn: &Connection, today: i64) -> Result<usize> {
    conn.execute(
        "DELETE FROM boards WHERE mode = 'daily' AND puzzle < ?",
        [today - 1],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boards_and_stats_round_trip() {
        let db = Db::temporary().await;
        db.run(|c| {
            let mut board = Board::new(Mode::Daily, 3, "bank");
            board.guess("oval");
            save_board(c, "p", &board)?;
            let back = super::board(c, "p", Mode::Daily, 3)?.unwrap();
            assert_eq!(back.guesses, ["oval"]);
            assert!(super::board(c, "p", Mode::Daily, 4)?.is_none());

            let mut stats = Stats::default();
            stats.record(3, true, 2);
            save_stats(c, "p", &stats)?;
            mark_guide_seen(c, "p")?;
            let p = player(c, "p")?;
            assert_eq!(p.stats, stats);
            assert!(p.seen_guide);

            assert_eq!(prune(c, 10)?, 1);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_practice_board_is_replaced_by_the_next_round() {
        let db = Db::temporary().await;
        db.run(|c| {
            save_board(c, "p", &Board::new(Mode::Practice, 0, "bank"))?;
            save_board(c, "p", &Board::new(Mode::Practice, 0, "oval"))?;
            assert_eq!(board(c, "p", Mode::Practice, 0)?.unwrap().answer_id, "oval");
            Ok(())
        })
        .await
        .unwrap();
    }
}
