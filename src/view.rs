//! What the templates render. The answer station only reaches a view through
//! `ResultView`, which exists only once a board is finished, so an unfinished
//! game cannot leak it into the page.

use askama::Template;
use serde_json::json;

use crate::engine::{Clue, Line, Network, Station, ZoneMatch, format_zones};
use crate::game::{Board, Feedback, MAX_GUESSES, Mode, Stats, Status, feedback};

pub const TITLE: &str = "Tubele";
pub const TAGLINE: &str = "Guess the London station";
pub const DESCRIPTION: &str = "A daily London station guessing game. Six guesses, and each one tells you how many stops away you are. Underground, Overground, DLR and the Elizabeth line.";

/// Ring colours for the wordmark discs, written as classes so Tailwind finds them.
const DISCS: [&str; 6] = [
    "border-[#E32017]",
    "border-[#0098D4]",
    "border-[#00782A]",
    "border-[#B36305]",
    "border-[#6950A1]",
    "border-[#EF3340]",
];

/// Gauge fill by band, as literal classes for the same reason.
/// Gauge tone by band.
const GAUGE: [&str; 6] = [
    "gauge-0", "gauge-1", "gauge-2", "gauge-3", "gauge-4", "gauge-5",
];

const FILL: [&str; 6] = [
    "w-[8%]", "w-[20%]", "w-[40%]", "w-[62%]", "w-[82%]", "w-full",
];

const WIN_WORDS: [&str; 4] = [
    "Straight there",
    "Barely broke stride",
    "Nicely judged",
    "Got there",
];

#[derive(Template)]
#[template(path = "page.html")]
pub struct PageTemplate {
    pub g: GameView,
    pub seen_guide: bool,
    pub signals: String,
    pub site_url: String,
    pub description: &'static str,
    pub css_url: &'static str,
    pub js_url: &'static str,
    pub lines_url: &'static str,
}

#[derive(Template)]
#[template(path = "game.html")]
pub struct GameTemplate {
    pub g: GameView,
}

#[derive(Template)]
#[template(path = "suggestions.html")]
pub struct SuggestionsTemplate {
    pub items: Vec<SuggestionView>,
    pub no_match: Option<String>,
}

pub struct SuggestionView {
    pub name: String,
    pub lines: Vec<String>,
}

impl SuggestionView {
    pub fn new(station: &Station) -> Self {
        Self {
            name: station.name.clone(),
            lines: station
                .lines
                .iter()
                .take(6)
                .map(|id| line_class(id))
                .collect(),
        }
    }
}

pub struct GameView {
    pub wordmark: Vec<(char, &'static str)>,
    pub daily: bool,
    pub strap: String,
    pub clues: Vec<Clue>,
    pub rows: Vec<RowView>,
    pub remaining: usize,
    pub pips: Vec<bool>,
    pub announcement: String,
    pub result: Option<ResultView>,
    /// Open the result dialog as the page or patch lands.
    pub open_result: bool,
    pub stats: StatsView,
    pub next_puzzle: i64,
    pub countdown: String,
}

pub struct RowView {
    pub name: String,
    pub correct: bool,
    pub stops: u32,
    pub unit: &'static str,
    pub gauge: &'static str,
    pub fill: &'static str,
    pub zones: String,
    pub zone_class: &'static str,
    pub zone_aria: String,
    pub pills: Vec<PillView>,
    /// Naming Mornington Crescent wins outright in another game. Not this one.
    pub joke: bool,
}

pub struct PillView {
    pub name: String,
    pub colour: String,
    pub state: &'static str,
    pub sr: &'static str,
}

impl PillView {
    fn plain(line: &Line) -> Self {
        Self {
            name: line.short_name.clone(),
            colour: line_class(&line.id),
            state: "",
            sr: "",
        }
    }
}

pub struct ResultView {
    pub won: bool,
    pub title: &'static str,
    pub word: &'static str,
    pub score: String,
    pub name: String,
    pub zones: String,
    pub pills: Vec<PillView>,
    pub top_pills: Vec<PillView>,
    pub fact: Option<String>,
    pub closest: String,
    pub streak: u32,
    pub again: &'static str,
    pub grid: Vec<u8>,
    pub share_text: String,
    pub targets: Vec<TargetView>,
    pub copy_icon: TargetView,
    pub image_icon: TargetView,
    pub card: String,
}

pub struct TargetView {
    pub label: &'static str,
    pub href: String,
    pub icon: &'static str,
    /// Shown even where the OS share sheet exists, which covers the rest.
    pub always: bool,
}

pub struct StatsView {
    pub played: u32,
    pub win_rate: u32,
    pub streak: u32,
    pub max_streak: u32,
    pub bars: Vec<BarView>,
}

pub struct BarView {
    pub guesses: usize,
    pub count: u32,
    pub width: u32,
    pub today: bool,
}

pub fn line_class(id: &str) -> String {
    format!("line-{id}")
}

/// `.line-<id> { --line: <colour> }` for every line, served as a stylesheet so
/// the colours need no inline style attribute.
pub fn lines_css(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| format!(".line-{}{{--line:{}}}\n", l.id, l.colour))
        .collect()
}

fn plural(n: impl Into<u64>, one: &'static str, many: &'static str) -> &'static str {
    if n.into() == 1 { one } else { many }
}

pub fn countdown(ms: i64) -> String {
    let total = ms.max(0) / 1000;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        total / 60 % 60,
        total % 60
    )
}

pub struct Context<'a> {
    pub network: &'a Network,
    pub board: &'a Board,
    pub stats: &'a Stats,
    pub today: i64,
    pub ms_to_next: i64,
    pub site_url: &'a str,
    pub announce: bool,
    pub open_result: bool,
}

pub fn game(cx: &Context) -> GameView {
    let Context {
        network,
        board,
        stats,
        today,
        ..
    } = *cx;
    let rows = feedback(network, board);
    let status = board.status();
    let daily = board.mode == Mode::Daily;
    let streak = stats.live_streak(today);
    let answer = network
        .station(&board.answer_id)
        .expect("boards name known stations");

    let announcement = match (cx.announce, rows.last()) {
        (true, Some(last)) => {
            let line = announce(last, rows.len());
            if status == Status::Lost {
                format!("{line} Out of guesses. The answer was {}.", answer.name)
            } else {
                line
            }
        }
        _ => String::new(),
    };

    let result = (status != Status::Playing).then(|| result(cx, answer, &rows, streak));

    GameView {
        wordmark: TITLE
            .to_uppercase()
            .chars()
            .zip(DISCS.iter().cycle().copied())
            .collect(),
        daily,
        strap: if daily {
            format!("No. {}", board.puzzle)
        } else {
            "Practice".into()
        },
        clues: network.clues(answer, board.misses()),
        rows: rows.iter().map(row).collect(),
        remaining: MAX_GUESSES - rows.len(),
        pips: (0..MAX_GUESSES).map(|i| i < rows.len()).collect(),
        announcement,
        open_result: cx.open_result && result.is_some(),
        result,
        stats: stats_view(
            stats,
            streak,
            (daily && status == Status::Won).then_some(rows.len()),
        ),
        next_puzzle: today + 1,
        countdown: countdown(cx.ms_to_next),
    }
}

fn announce(f: &Feedback, n: usize) -> String {
    if f.correct {
        return format!(
            "Correct. {} in {n} {}.",
            f.station.name,
            plural(n as u64, "guess", "guesses")
        );
    }
    let zone = match f.zone_match {
        ZoneMatch::Exact => "same zone",
        ZoneMatch::Near => "neighbouring zone",
        ZoneMatch::None => "different zone",
    };
    let shared = f.lines.iter().filter(|l| l.shared).count();
    let lines = match shared {
        0 => "no shared lines".into(),
        1 => "1 shared line".into(),
        n => format!("{n} shared lines"),
    };
    format!(
        "Guess {n} of {MAX_GUESSES}. {}, {} {} away, {zone}, {lines}.",
        f.station.name,
        f.stops,
        plural(f.stops, "stop", "stops")
    )
}

fn row(f: &Feedback) -> RowView {
    let level = f.band();
    // Shared lines first: they are the information. Ruled-out lines follow.
    let mut marks: Vec<_> = f.lines.iter().collect();
    if !f.correct {
        marks.sort_by_key(|m| !m.shared);
    }
    let zones = format_zones(f.station.zones.as_deref());
    let (zone_class, zone_label) = match f.zone_match {
        ZoneMatch::Exact => ("zone-exact", "Same zone"),
        ZoneMatch::Near => ("zone-near", "One zone out"),
        ZoneMatch::None => ("zone-none", "Different zone"),
    };
    RowView {
        name: f.station.name.clone(),
        correct: f.correct,
        stops: f.stops,
        unit: plural(f.stops, "stop", "stops"),
        gauge: GAUGE[usize::from(level)],
        fill: FILL[usize::from(level)],
        zone_aria: format!("{zones} — {zone_label}"),
        zones,
        zone_class,
        pills: marks
            .into_iter()
            .map(|m| {
                let shared = f.correct || m.shared;
                PillView {
                    name: m.line.short_name.clone(),
                    colour: line_class(&m.line.id),
                    state: if shared { "pill-shared" } else { "pill-miss" },
                    sr: if shared {
                        "also serves the answer"
                    } else {
                        "not on the answer"
                    },
                }
            })
            .collect(),
        joke: f.station.id == "mornington-crescent" && !f.correct,
    }
}

fn result(cx: &Context, answer: &Station, rows: &[Feedback], streak: u32) -> ResultView {
    let board = cx.board;
    let won = board.status() == Status::Won;
    let daily = board.mode == Mode::Daily;
    let streak = if daily { streak } else { 0 };
    let score = if won {
        format!("{}/{MAX_GUESSES}", rows.len())
    } else {
        format!("X/{MAX_GUESSES}")
    };
    let closest = rows.iter().map(|r| r.stops).min().unwrap_or(0);
    let grid: Vec<u8> = rows.iter().map(Feedback::band).collect();
    let pills: Vec<PillView> = answer
        .lines
        .iter()
        .filter_map(|id| cx.network.line(id))
        .map(PillView::plain)
        .collect();

    let heading = if daily {
        format!("{TITLE} #{}", board.puzzle)
    } else {
        format!("{TITLE} practice")
    };
    let link = |source: &str| format!("{}?s={source}", cx.site_url);
    let streak_suffix = if streak >= 2 {
        format!("  🔥{streak}")
    } else {
        String::new()
    };
    let squares: Vec<String> = grid
        .iter()
        .map(|&n| "🟩".repeat(n.into()) + &"⬜".repeat(5 - usize::from(n)))
        .collect();
    let share_text = format!(
        "{heading}  {score}{streak_suffix}\n{}\n{TAGLINE}\n{}",
        squares.join("\n"),
        link("t")
    );

    let card = json!({
        "title": TITLE,
        "puzzle": daily.then_some(board.puzzle),
        "rows": grid,
        "maxGuesses": MAX_GUESSES,
        "won": won,
        "streak": streak,
        "tagline": TAGLINE,
        "url": cx.site_url,
        "link": link("i"),
        "caption": format!("{} {score}\n{TAGLINE}", if daily { format!("{TITLE} #{}", board.puzzle) } else { format!("{TITLE} practice") }),
        "filename": if daily { format!("tubele-{}.png", board.puzzle) } else { "tubele-practice.png".into() },
    })
    .to_string();

    ResultView {
        won,
        title: if won { "You got it" } else { "End of the line" },
        word: if won {
            WIN_WORDS[(rows.len() - 1).min(WIN_WORDS.len() - 1)]
        } else {
            "End of the line"
        },
        score,
        name: answer.name.clone(),
        zones: format_zones(answer.zones.as_deref()),
        top_pills: answer
            .lines
            .iter()
            .take(3)
            .filter_map(|id| cx.network.line(id))
            .map(PillView::plain)
            .collect(),
        pills,
        fact: answer.fact.clone(),
        closest: format!("{closest} {}", plural(closest, "stop", "stops")),
        streak,
        again: if daily {
            "Practice round"
        } else {
            "Another round"
        },
        targets: targets(&share_text, &link("t")),
        copy_icon: icon(
            "M9 3h9a2 2 0 0 1 2 2v9M6 7h9a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2Z",
        ),
        image_icon: icon("M4 5h16v14H4V5Zm2 10 4-4 3 3 2-2 3 3M9 9.5a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"),
        share_text,
        grid,
        card,
    }
}

fn icon(path: &'static str) -> TargetView {
    TargetView {
        label: "",
        href: String::new(),
        icon: path,
        always: true,
    }
}

/// Plain links, no SDKs. WhatsApp first: this is a game shared between people
/// who know each other. Where the OS share sheet exists only WhatsApp and X
/// stay, since the sheet already offers the rest.
fn targets(text: &str, url: &str) -> Vec<TargetView> {
    let t = encode(text);
    let u = encode(url);
    let title = encode(text.lines().next().unwrap_or(TITLE));
    vec![
        TargetView {
            label: "WhatsApp",
            href: format!("https://wa.me/?text={t}"),
            icon: "M12 3a9 9 0 0 0-7.7 13.6L3 21l4.5-1.2A9 9 0 1 0 12 3Zm4.3 12.4c-.2.5-1 1-1.4 1-.4.1-.8.1-1.3-.1a11 11 0 0 1-4-2.5 8.6 8.6 0 0 1-1.7-2.3c-.2-.5 0-1 .2-1.2l.5-.6c.2-.2.2-.3.3-.5v-.4l-.7-1.6c-.2-.4-.4-.4-.6-.4h-.5c-.2 0-.5.1-.7.4-.3.3-.9.9-.9 2.1s.9 2.4 1 2.6c.1.2 1.8 2.9 4.4 4 .6.3 1.1.4 1.5.5.6.2 1.2.2 1.6.1.5-.1 1.5-.6 1.7-1.2.2-.6.2-1.1.1-1.2l-.5-.3Z",
            always: true,
        },
        TargetView {
            label: "X",
            href: format!("https://x.com/intent/post?text={t}"),
            icon: "M4 4h4.2l4 5.6L17 4h3l-6.3 7.6L20.5 20h-4.2l-4.4-6.1L6.6 20H4l6.8-8.1L4 4Z",
            always: true,
        },
        TargetView {
            label: "Telegram",
            href: format!("https://t.me/share/url?url={u}&text={t}"),
            icon: "M21 5 3 12l4.5 1.5L18 7l-8 7.5.3 4.5 2.7-3.3 4 3 4-13.7Z",
            always: false,
        },
        TargetView {
            label: "Reddit",
            href: format!("https://www.reddit.com/submit?url={u}&title={title}"),
            icon: "M12 4c-.6 0-1 .4-1 1v3.1c-2 .1-3.8.7-5.1 1.6a2 2 0 1 0-2.2 3.3c0 .3-.1.6-.1 1 0 3 3.8 5.5 8.4 5.5s8.4-2.5 8.4-5.5c0-.4 0-.7-.1-1a2 2 0 1 0-2.2-3.3C16.8 8.8 15 8.2 13 8.1V5c0-.6-.4-1-1-1Zm-3.2 8.6a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6Zm6.4 0a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6ZM8.9 17c1.6 1.1 4.6 1.1 6.2 0",
            always: false,
        },
        TargetView {
            label: "Email",
            href: format!("mailto:?subject={}&body={t}", encode(TITLE)),
            icon: "M3 6h18v12H3V6Zm1.5 1.5L12 13l7.5-5.5",
            always: false,
        },
    ]
}

/// encodeURIComponent: everything but the unreserved set is escaped.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn stats_view(stats: &Stats, streak: u32, highlight: Option<usize>) -> StatsView {
    let most = stats.distribution.iter().copied().max().unwrap_or(0).max(1);
    StatsView {
        played: stats.played,
        win_rate: stats.win_rate(),
        streak,
        max_streak: stats.max_streak,
        bars: stats
            .distribution
            .iter()
            .enumerate()
            .map(|(i, &count)| BarView {
                guesses: i + 1,
                count,
                width: (count * 100 / most).max(6),
                today: highlight == Some(i + 1),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(board: &Board) -> String {
        let network = Network::load().unwrap();
        let stats = Stats::default();
        GameTemplate {
            g: game(&Context {
                network: &network,
                board,
                stats: &stats,
                today: board.puzzle.max(1),
                ms_to_next: 1000,
                site_url: "https://example.test",
                announce: true,
                open_result: true,
            }),
        }
        .render()
        .unwrap()
    }

    #[test]
    fn an_unfinished_board_never_names_the_answer() {
        let mut board = Board::new(Mode::Daily, 5, "mornington-crescent");
        for id in ["bank", "oval", "angel", "euston", "camden-town"] {
            board.guess(id);
        }
        let html = render(&board);
        assert!(!html.contains("Mornington"), "answer leaked into the page");
        assert!(!html.contains("id=\"result\""));
    }

    #[test]
    fn a_finished_board_shows_the_answer_and_its_fact() {
        let mut board = Board::new(Mode::Daily, 5, "abbey-road");
        board.guess("abbey-road");
        let html = render(&board);
        assert!(html.contains("Straight there"));
        assert!(html.contains("Beatles"));
        assert!(html.contains("@modal(el)"), "the result opens itself");
    }

    #[test]
    fn the_share_text_matches_the_original_format() {
        let network = Network::load().unwrap();
        let mut board = Board::new(Mode::Daily, 12, "bank");
        board.guess("liverpool-street");
        board.guess("bank");
        let stats = Stats::default();
        let cx = Context {
            network: &network,
            board: &board,
            stats: &stats,
            today: 12,
            ms_to_next: 0,
            site_url: "https://tubele.app",
            announce: false,
            open_result: false,
        };
        let view = game(&cx);
        assert_eq!(
            view.result.unwrap().share_text,
            "Tubele #12  2/6\n🟩🟩🟩🟩🟩\n🟩🟩🟩🟩🟩\nGuess the London station\nhttps://tubele.app?s=t"
        );
    }

    #[test]
    fn encoding_matches_encode_uri_component() {
        assert_eq!(encode("a b&c/🟩"), "a%20b%26c%2F%F0%9F%9F%A9");
    }

    #[test]
    fn countdown_formats_as_a_clock() {
        assert_eq!(countdown(3_723_000), "01:02:03");
    }
}
