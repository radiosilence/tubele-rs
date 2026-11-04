//! A board, the feedback it shows, and the player's record. Pure: the store
//! loads and saves these, the handlers render them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{Network, Station, ZoneMatch, band_for, compare_zones};

pub const MAX_GUESSES: usize = 6;
/// Days of upcoming answers kept out of practice, so a practice round never
/// spoils a daily one.
pub const PROTECT_UPCOMING_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Daily,
    Practice,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Daily => "daily",
            Mode::Practice => "practice",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Playing,
    Won,
    Lost,
}

/// Only station ids are stored. Feedback is recomputed from the answer on every
/// render, so the stored board can never disagree with the network.
#[derive(Debug, Clone)]
pub struct Board {
    pub mode: Mode,
    /// Puzzle number for a daily board; 0 for practice.
    pub puzzle: i64,
    pub answer_id: String,
    pub guesses: Vec<String>,
}

impl Board {
    pub fn new(mode: Mode, puzzle: i64, answer_id: &str) -> Self {
        Self {
            mode,
            puzzle,
            answer_id: answer_id.to_owned(),
            guesses: Vec::new(),
        }
    }

    pub fn status(&self) -> Status {
        if self.guesses.last() == Some(&self.answer_id) {
            Status::Won
        } else if self.guesses.len() >= MAX_GUESSES {
            Status::Lost
        } else {
            Status::Playing
        }
    }

    pub fn misses(&self) -> usize {
        self.guesses
            .iter()
            .filter(|g| **g != self.answer_id)
            .count()
    }

    /// Adds a guess. A repeat or a finished board is a no-op, not a wasted turn.
    pub fn guess(&mut self, station_id: &str) -> bool {
        if self.status() != Status::Playing || self.guesses.iter().any(|g| g == station_id) {
            return false;
        }
        self.guesses.push(station_id.to_owned());
        true
    }
}

pub struct LineMark<'a> {
    pub line: &'a crate::engine::Line,
    pub shared: bool,
}

pub struct Feedback<'a> {
    pub station: &'a Station,
    pub stops: u32,
    pub correct: bool,
    pub zone_match: ZoneMatch,
    /// Every line at the guess, flagged with whether the answer shares it.
    pub lines: Vec<LineMark<'a>>,
}

impl Feedback<'_> {
    pub fn band(&self) -> u8 {
        if self.correct {
            5
        } else {
            band_for(self.stops)
        }
    }
}

pub fn feedback<'a>(network: &'a Network, board: &Board) -> Vec<Feedback<'a>> {
    let Some(answer_index) = network.index(&board.answer_id) else {
        return Vec::new();
    };
    let answer = &network.stations[answer_index];
    let distances = network.stops_from(answer_index);
    board
        .guesses
        .iter()
        .filter_map(|id| network.index(id))
        .map(|i| {
            let station = &network.stations[i];
            Feedback {
                station,
                stops: distances[i],
                correct: i == answer_index,
                zone_match: compare_zones(station.zones.as_deref(), answer.zones.as_deref()),
                lines: station
                    .lines
                    .iter()
                    .filter_map(|id| network.line(id))
                    .map(|line| LineMark {
                        line,
                        shared: answer.lines.contains(&line.id),
                    })
                    .collect(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredResult {
    pub guesses: usize,
    pub won: bool,
}

/// Recorded against puzzle numbers, not dates, so finishing puzzle 107 at
/// 00:10 still extends a streak that ran to 106. Daily games only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub played: u32,
    pub won: u32,
    pub current_streak: u32,
    pub max_streak: u32,
    pub distribution: [u32; MAX_GUESSES],
    pub last_puzzle: Option<i64>,
    /// Every daily puzzle finished, which the counters could be rebuilt from.
    pub results: BTreeMap<i64, StoredResult>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            played: 0,
            won: 0,
            current_streak: 0,
            max_streak: 0,
            distribution: [0; MAX_GUESSES],
            last_puzzle: None,
            results: BTreeMap::new(),
        }
    }
}

impl Stats {
    /// Fold a finished daily game in. Recording the same puzzle twice, or an
    /// older one, changes nothing.
    pub fn record(&mut self, puzzle: i64, won: bool, guesses: usize) {
        if self.last_puzzle.is_some_and(|last| puzzle <= last) {
            return;
        }
        let continues = self.last_puzzle == Some(puzzle - 1);
        self.current_streak = match (won, continues) {
            (false, _) => 0,
            (true, true) => self.current_streak + 1,
            (true, false) => 1,
        };
        self.max_streak = self.max_streak.max(self.current_streak);
        self.played += 1;
        if won {
            self.won += 1;
            if let Some(slot) = guesses
                .checked_sub(1)
                .and_then(|i| self.distribution.get_mut(i))
            {
                *slot += 1;
            }
        }
        self.last_puzzle = Some(puzzle);
        self.results.insert(puzzle, StoredResult { guesses, won });
    }

    /// A streak only runs if the last game was today or yesterday; otherwise
    /// "current streak 12" to someone who last played in March is a lie.
    pub fn live_streak(&self, today: i64) -> u32 {
        match self.last_puzzle {
            Some(last) if today - last <= 1 => self.current_streak,
            _ => 0,
        }
    }

    pub fn win_rate(&self) -> u32 {
        if self.played == 0 {
            0
        } else {
            (f64::from(self.won) / f64::from(self.played) * 100.0).round() as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_wins_on_the_answer_and_loses_on_the_sixth_miss() {
        let mut board = Board::new(Mode::Daily, 1, "bank");
        assert!(board.guess("oval"));
        assert!(!board.guess("oval"), "a repeat is not a turn");
        assert!(board.guess("bank"));
        assert_eq!(board.status(), Status::Won);
        assert!(!board.guess("angel"), "a finished board takes nothing");

        let mut board = Board::new(Mode::Daily, 1, "bank");
        for id in ["a", "b", "c", "d", "e", "f"] {
            board.guess(id);
        }
        assert_eq!(board.status(), Status::Lost);
        assert_eq!(board.misses(), 6);
    }

    #[test]
    fn streaks_continue_only_across_consecutive_puzzles() {
        let mut stats = Stats::default();
        stats.record(1, true, 3);
        stats.record(2, true, 2);
        assert_eq!(stats.current_streak, 2);
        stats.record(2, true, 1);
        assert_eq!(stats.played, 2, "recording a puzzle twice is a no-op");
        stats.record(4, true, 4);
        assert_eq!(stats.current_streak, 1);
        assert_eq!(stats.max_streak, 2);
        stats.record(5, false, 6);
        assert_eq!(stats.current_streak, 0);
        assert_eq!(stats.distribution, [0, 1, 1, 1, 0, 0]);
        assert_eq!(stats.win_rate(), 75);
    }

    #[test]
    fn a_streak_lapses_after_a_missed_day() {
        let mut stats = Stats::default();
        stats.record(10, true, 2);
        assert_eq!(stats.live_streak(11), 1);
        assert_eq!(stats.live_streak(12), 0);
    }

    #[test]
    fn feedback_marks_shared_lines_and_distance() {
        let network = Network::load().unwrap();
        let mut board = Board::new(Mode::Practice, 0, "bank");
        board.guess("liverpool-street");
        let rows = feedback(&network, &board);
        assert_eq!(rows[0].stops, 1);
        assert!(rows[0].lines.iter().any(|l| l.shared));
        assert_eq!(rows[0].band(), 5);
    }
}
