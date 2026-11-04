//! The station graph and everything derived from it: distances, the clue
//! ladder, autocomplete and the answer schedule. Pure: no clock, no storage.
//! Time arrives as an argument, which is what keeps the puzzle-number tests
//! independent of the machine running them.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use jiff::{Timestamp, ToSpan, Zoned, civil::Date, tz::TimeZone};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub colour: String,
    pub mode: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: String,
    pub name: String,
    /// Every fare zone the station sits in; `None` is outside the London zones.
    pub zones: Option<Vec<u8>>,
    pub lines: Vec<String>,
    pub fact: Option<String>,
    /// One node that is really two stations a walk apart (Bethnal Green), so
    /// "an interchange" would mislead. Only changes the wording of a clue.
    #[serde(default)]
    pub out_of_station_interchange: bool,
}

#[derive(Deserialize)]
struct Edge {
    a: String,
    b: String,
    lines: Vec<String>,
}

#[derive(Deserialize)]
struct PackedSchedule {
    epoch: String,
    packed: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Terminus,
    Interchange,
    Through,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneMatch {
    Exact,
    Near,
    None,
}

pub struct Clue {
    pub label: &'static str,
    pub value: String,
}

struct SearchEntry {
    /// The normalised name first, then every alias.
    terms: Vec<String>,
    words: Vec<String>,
}

pub struct Network {
    pub stations: Vec<Station>,
    pub lines: Vec<Line>,
    line_index: HashMap<String, usize>,
    index_of: HashMap<String, usize>,
    neighbours: Vec<Vec<usize>>,
    roles: Vec<Role>,
    /// Stops to the nearest end of any line serving the station.
    stops_to_line_end: Vec<u32>,
    /// Stops to the end of each named line. The Circle, a closed loop, has none.
    stops_to_end_by_line: Vec<HashMap<String, u32>>,
    search: Vec<SearchEntry>,
    pub epoch: Date,
    answers: Vec<String>,
}

const STATIONS: &str = include_str!("../data/stations.json");
const EDGES: &str = include_str!("../data/edges.json");
pub const LINES: &str = include_str!("../data/lines.json");
const SCHEDULE: &str = include_str!("../data/schedule.json");

/// The key the committed schedule is XOR-packed with. The schedule never leaves
/// the server, so this only keeps the answers out of a casual read of the repo.
const SCHEDULE_KEY: &[u8] = b"mind-the-gap";

fn london() -> TimeZone {
    TimeZone::get("Europe/London").expect("Europe/London is in the bundled tzdb")
}

impl Network {
    pub fn load() -> Result<Self> {
        let stations: Vec<Station> = serde_json::from_str(STATIONS).context("stations.json")?;
        let edges: Vec<Edge> = serde_json::from_str(EDGES).context("edges.json")?;
        let lines: Vec<Line> = serde_json::from_str(LINES).context("lines.json")?;
        let schedule: PackedSchedule = serde_json::from_str(SCHEDULE).context("schedule.json")?;

        let index_of: HashMap<String, usize> = stations
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();
        let line_index = lines
            .iter()
            .enumerate()
            .map(|(i, l)| (l.id.clone(), i))
            .collect();

        let mut neighbours = vec![Vec::new(); stations.len()];
        let mut neighbour_lines: Vec<Vec<Vec<String>>> = vec![Vec::new(); stations.len()];
        for edge in &edges {
            let (Some(&a), Some(&b)) = (index_of.get(&edge.a), index_of.get(&edge.b)) else {
                bail!(
                    "edge {}—{} names a station not in the dataset",
                    edge.a,
                    edge.b
                );
            };
            neighbours[a].push(b);
            neighbour_lines[a].push(edge.lines.clone());
            neighbours[b].push(a);
            neighbour_lines[b].push(edge.lines.clone());
        }

        let roles = stations
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if neighbours[i].len() == 1 {
                    Role::Terminus
                } else if s.lines.len() > 1 {
                    Role::Interchange
                } else {
                    Role::Through
                }
            })
            .collect();

        let (stops_to_line_end, stops_to_end_by_line) =
            stops_to_line_end(&stations, &neighbours, &neighbour_lines)?;

        let search = stations.iter().map(|s| search_entry(&s.name)).collect();

        let answers = unpack_answers(&schedule.packed)?;
        for id in &answers {
            if !index_of.contains_key(id) {
                bail!("schedule names unknown station {id}");
            }
        }

        Ok(Self {
            epoch: schedule.epoch.parse().context("schedule epoch")?,
            stations,
            lines,
            line_index,
            index_of,
            neighbours,
            roles,
            stops_to_line_end,
            stops_to_end_by_line,
            search,
            answers,
        })
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.index_of.get(id).copied()
    }

    pub fn station(&self, id: &str) -> Option<&Station> {
        self.index(id).map(|i| &self.stations[i])
    }

    pub fn line(&self, id: &str) -> Option<&Line> {
        self.line_index.get(id).map(|&i| &self.lines[i])
    }

    /// Breadth-first distance in stops from one station to every other.
    /// Interchanges are free, so "1 stop away" always means next door.
    pub fn stops_from(&self, source: usize) -> Vec<u32> {
        let mut distances = vec![u32::MAX; self.stations.len()];
        distances[source] = 0;
        let mut queue = VecDeque::from([source]);
        while let Some(node) = queue.pop_front() {
            let next = distances[node] + 1;
            for &n in &self.neighbours[node] {
                if distances[n] == u32::MAX {
                    distances[n] = next;
                    queue.push_back(n);
                }
            }
        }
        distances
    }

    /// The mode shown before the first guess: exactly one, chosen by a stable
    /// hash. Listing every mode would fingerprint the big interchanges.
    fn revealed_mode(&self, station: &Station) -> &'static str {
        let modes: Vec<&str> = station
            .lines
            .iter()
            .filter_map(|id| self.line(id).map(|l| l.mode.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        match *pick_stable(&modes, &station.name) {
            "tube" => "Underground",
            "overground" => "Overground",
            "dlr" => "DLR",
            "elizabeth" => "Elizabeth line",
            _ => "Rail",
        }
    }

    /// The mode, always, plus one rung of the ladder per miss. Rungs run weak to
    /// strong: zone, role, one line, distance to that line's end, first letter.
    pub fn clues(&self, station: &Station, misses: usize) -> Vec<Clue> {
        let index = self.index_of[&station.id];
        let line_id = pick_stable(&station.lines, &station.name);
        let line = self.line(line_id).expect("station lines are validated");
        // Measured on the line just named so the two rungs agree; the Circle has
        // no end, so it falls back to the nearest end of any line.
        let stops_to_end = self.stops_to_end_by_line[index]
            .get(line_id)
            .copied()
            .unwrap_or(self.stops_to_line_end[index]);

        let role = self.roles[index];
        let ladder = [
            Clue {
                label: "Fare zone",
                value: format_zones(station.zones.as_deref()),
            },
            Clue {
                label: "On the network",
                value: match role {
                    Role::Interchange if station.out_of_station_interchange => {
                        "Two stations, one name"
                    }
                    Role::Terminus => "End of the line",
                    Role::Interchange => "An interchange",
                    Role::Through => "A through station",
                }
                .into(),
            },
            Clue {
                label: "One of its lines",
                value: if line.mode == "dlr" {
                    line.name.clone()
                } else {
                    format!("{} line", line.name)
                },
            },
            Clue {
                label: "From the end of the line",
                value: match stops_to_end {
                    0 => "It is the end of the line".into(),
                    1 => "1 stop".into(),
                    n => format!("{n} stops"),
                },
            },
            Clue {
                label: "First letter",
                value: format!("“{}”", station.name.chars().next().unwrap_or(' ')),
            },
        ];

        std::iter::once(Clue {
            label: "Mode",
            value: self.revealed_mode(station).into(),
        })
        .chain(ladder.into_iter().take(misses))
        .collect()
    }

    /// Autocomplete, best first: exact name, exact alias, name prefix, word
    /// prefix, alias prefix, then substrings. Shorter names win ties, so "Bank"
    /// beats "Barbican" for "ban".
    pub fn search(&self, raw: &str, exclude: &HashSet<&str>) -> Vec<&Station> {
        let query = normalise_name(raw);
        if query.len() < 2 {
            return Vec::new();
        }
        let mut scored: Vec<(u8, &Station)> = self
            .search
            .iter()
            .zip(&self.stations)
            .filter(|(_, s)| !exclude.contains(s.id.as_str()))
            .filter_map(|(entry, s)| rank(entry, &query).map(|r| (r, s)))
            .collect();
        scored.sort_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then(a.name.len().cmp(&b.name.len()))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        scored.into_iter().take(7).map(|(_, s)| s).collect()
    }

    /// The station id for a puzzle number, if the schedule reaches it.
    pub fn answer_for(&self, puzzle: i64) -> Option<&str> {
        let index = usize::try_from(puzzle.checked_sub(1)?).ok()?;
        self.answers.get(index).map(String::as_str)
    }

    /// Puzzle #1 is the epoch date in London. Counting London calendar days,
    /// never subtracting instants, keeps the rollover at midnight all year.
    pub fn puzzle_number(&self, now: Timestamp) -> i64 {
        let today = now.to_zoned(london()).date();
        i64::from((today - self.epoch).get_days()) + 1
    }
}

/// Milliseconds until the London date next changes, honest across DST.
pub fn ms_until_next_puzzle(now: Timestamp) -> i64 {
    let here: Zoned = now.to_zoned(london());
    let tomorrow = here
        .date()
        .checked_add(1.day())
        .and_then(|d| d.to_zoned(london()))
        .expect("tomorrow exists in London");
    (tomorrow.timestamp().as_millisecond() - now.as_millisecond()).max(0)
}

pub fn compare_zones(guess: Option<&[u8]>, answer: Option<&[u8]>) -> ZoneMatch {
    let (Some(guess), Some(answer)) = (guess, answer) else {
        // Stations outside the zones match each other and nothing else.
        return if guess.is_none() && answer.is_none() {
            ZoneMatch::Exact
        } else {
            ZoneMatch::None
        };
    };
    if guess.iter().any(|z| answer.contains(z)) {
        ZoneMatch::Exact
    } else if guess
        .iter()
        .any(|z| answer.contains(&(z + 1)) || answer.contains(&z.saturating_sub(1)))
    {
        ZoneMatch::Near
    } else {
        ZoneMatch::None
    }
}

pub fn format_zones(zones: Option<&[u8]>) -> String {
    match zones {
        None | Some([]) => "Outside the zones".into(),
        Some([z]) => format!("Zone {z}"),
        Some(zs) => format!(
            "Zones {}",
            zs.iter().map(u8::to_string).collect::<Vec<_>>().join(" & ")
        ),
    }
}

/// Share-grid band for a distance: filled cells out of five. Non-linear, so one
/// stop away and six stops away no longer look the same.
pub fn band_for(stops: u32) -> u8 {
    match stops {
        0..=1 => 5,
        2..=3 => 4,
        4..=7 => 3,
        8..=13 => 2,
        14..=21 => 1,
        _ => 0,
    }
}

/// A small FNV-style hash. It picks the revealed mode and line, so it must give
/// the same answer forever; it matches the original game's JavaScript exactly,
/// UTF-16 code units included.
fn stable_hash(text: &str) -> u32 {
    text.encode_utf16()
        .fold(7u32, |h, c| h.wrapping_mul(31).wrapping_add(u32::from(c)))
}

fn pick_stable<'a, T>(items: &'a [T], key: &str) -> &'a T {
    &items[stable_hash(key) as usize % items.len()]
}

fn unpack_answers(packed: &str) -> Result<Vec<String>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(packed)
        .context("schedule is not base64")?;
    let plain: Vec<u8> = bytes
        .iter()
        .zip(SCHEDULE_KEY.iter().cycle())
        .map(|(b, k)| b ^ k)
        .collect();
    let answers: Vec<String> = String::from_utf8(plain)
        .context("schedule failed to decode")?
        .split('\n')
        .map(str::to_owned)
        .collect();
    if answers.iter().any(String::is_empty) {
        bail!("schedule failed to decode");
    }
    Ok(answers)
}

type StopsToEnd = (Vec<u32>, Vec<HashMap<String, u32>>);

fn stops_to_line_end(
    stations: &[Station],
    neighbours: &[Vec<usize>],
    neighbour_lines: &[Vec<Vec<String>>],
) -> Result<StopsToEnd> {
    let size = stations.len();
    let mut per_line: HashMap<&str, HashMap<usize, Vec<usize>>> = HashMap::new();
    for station in 0..size {
        for (n, &other) in neighbours[station].iter().enumerate() {
            for line in &neighbour_lines[station][n] {
                per_line
                    .entry(line)
                    .or_default()
                    .entry(station)
                    .or_default()
                    .push(other);
            }
        }
    }

    let mut minimum = vec![u32::MAX; size];
    let mut by_line = vec![HashMap::new(); size];
    for (line, graph) in &per_line {
        // A line's ends have a single neighbour on it; a loop has none.
        let ends: Vec<usize> = graph
            .iter()
            .filter(|(_, ns)| ns.iter().collect::<HashSet<_>>().len() <= 1)
            .map(|(&s, _)| s)
            .collect();
        if ends.is_empty() {
            continue;
        }
        let mut distance: HashMap<usize, u32> = ends.iter().map(|&e| (e, 0)).collect();
        let mut queue: VecDeque<usize> = ends.into();
        while let Some(node) = queue.pop_front() {
            let next = distance[&node] + 1;
            for &n in graph.get(&node).into_iter().flatten() {
                distance.entry(n).or_insert_with(|| {
                    queue.push_back(n);
                    next
                });
            }
        }
        for (station, stops) in distance {
            minimum[station] = minimum[station].min(stops);
            by_line[station].insert((*line).to_owned(), stops);
        }
    }

    if let Some(i) = minimum.iter().position(|&m| m == u32::MAX) {
        bail!("{} is not on any line with an end", stations[i].name);
    }
    Ok((minimum, by_line))
}

/// Lowercase, no punctuation, single-spaced: "Shepherd's Bush" and "shepherds
/// bush" are the same string.
pub fn normalise_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '’' | '\'' | '`' | '.' => {}
            '&' => out.push_str(" and "),
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_lowercase()),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Aliases nobody guesses are needed until they watch someone play.
const MANUAL_ALIASES: &[(&str, &[&str])] = &[
    (
        "King's Cross St. Pancras",
        &["kings x", "kx", "kings cross", "st pancras"],
    ),
    ("Tottenham Court Road", &["tcr"]),
    ("Liverpool Street", &["lst"]),
    ("Oxford Circus", &["oxford st"]),
    ("Elephant & Castle", &["elephant"]),
    (
        "Heathrow Terminals 2 & 3",
        &["heathrow", "lhr", "heathrow t2", "heathrow t3"],
    ),
    ("Heathrow Terminal 4", &["heathrow t4", "lhr t4"]),
    ("Heathrow Terminal 5", &["heathrow t5", "lhr t5"]),
    ("London City Airport", &["lcy"]),
    ("Highbury & Islington", &["highbury"]),
    ("Hammersmith (District & Piccadilly)", &["hammersmith"]),
    ("Hammersmith (H&C & Circle)", &["hammersmith"]),
    ("Edgware Road (Bakerloo)", &["edgware road"]),
    ("Edgware Road (Circle)", &["edgware road"]),
];

fn search_entry(name: &str) -> SearchEntry {
    let normalised = normalise_name(name);
    let mut terms = vec![normalised.clone()];
    let mut add = |term: String| {
        if !terms.contains(&term) {
            terms.push(term);
        }
    };

    // Bracketed qualifiers dropped, and "St." / "St" / "Rd" spelled out.
    let without_brackets = strip_brackets(name);
    if without_brackets != name && !without_brackets.is_empty() {
        add(normalise_name(&without_brackets));
    }
    let expanded = expand_abbreviations(name);
    if expanded != name {
        add(normalise_name(&expanded));
    }
    if let Some((_, aliases)) = MANUAL_ALIASES
        .iter()
        .find(|(n, _)| normalise_name(n) == normalised)
    {
        for alias in *aliases {
            add(normalise_name(alias));
        }
    }

    SearchEntry {
        words: normalised.split(' ').map(str::to_owned).collect(),
        terms,
    }
}

fn strip_brackets(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn expand_abbreviations(name: &str) -> String {
    name.split(' ')
        .map(|word| match word {
            "St." => "Saint",
            "St" => "Street",
            "Rd" => "Road",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn rank(entry: &SearchEntry, query: &str) -> Option<u8> {
    let name = &entry.terms[0];
    if name == query {
        Some(0)
    } else if entry.terms.iter().any(|t| t == query) {
        Some(1)
    } else if name.starts_with(query) {
        Some(2)
    } else if entry.words.iter().any(|w| w.starts_with(query)) {
        Some(3)
    } else if entry.terms.iter().any(|t| t.starts_with(query)) {
        Some(4)
    } else if name.contains(query) {
        Some(5)
    } else if entry.terms.iter().any(|t| t.contains(query)) {
        Some(6)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> Network {
        Network::load().unwrap()
    }

    #[test]
    fn the_dataset_loads_and_is_connected() {
        let n = network();
        assert_eq!(n.stations.len(), 420);
        assert!(n.stops_from(0).iter().all(|&d| d != u32::MAX));
    }

    #[test]
    fn neighbours_are_one_stop_apart() {
        let n = network();
        let bank = n.index("bank").unwrap();
        let distances = n.stops_from(bank);
        assert_eq!(distances[n.index("liverpool-street").unwrap()], 1);
    }

    #[test]
    fn the_hash_matches_the_javascript() {
        // Math.imul-based loop from the original, evaluated in node.
        assert_eq!(stable_hash(""), 7);
        assert_eq!(stable_hash("Bank"), 8_527_587);
        assert_eq!(stable_hash("King's Cross St. Pancras"), 3_781_227_221);
        assert_eq!(stable_hash("Heathrow Terminals 2 & 3"), 704_403_243);
    }

    #[test]
    fn the_schedule_unpacks() {
        let n = network();
        assert!(n.answer_for(1).is_some());
        assert!(n.answer_for(0).is_none());
    }

    #[test]
    fn puzzle_numbers_roll_over_at_london_midnight() {
        let n = network();
        let epoch_noon: Timestamp = "2026-08-15T11:00:00Z".parse().unwrap();
        assert_eq!(n.puzzle_number(epoch_noon), 1);
        // 23:30 UTC in August is 00:30 BST the next day.
        let after_midnight: Timestamp = "2026-08-15T23:30:00Z".parse().unwrap();
        assert_eq!(n.puzzle_number(after_midnight), 2);
        // In winter London is on UTC, so 23:30 UTC is still the same day.
        let winter: Timestamp = "2026-12-01T23:30:00Z".parse().unwrap();
        let winter_next: Timestamp = "2026-12-02T00:00:00Z".parse().unwrap();
        assert_eq!(n.puzzle_number(winter) + 1, n.puzzle_number(winter_next));
    }

    #[test]
    fn the_countdown_reaches_london_midnight() {
        let now: Timestamp = "2026-08-15T22:00:00Z".parse().unwrap();
        assert_eq!(ms_until_next_puzzle(now), 3_600_000);
        let winter: Timestamp = "2026-12-01T22:00:00Z".parse().unwrap();
        assert_eq!(ms_until_next_puzzle(winter), 7_200_000);
    }

    #[test]
    fn search_ranks_exact_and_aliases_first() {
        let n = network();
        let none = HashSet::new();
        assert_eq!(n.search("bank", &none)[0].id, "bank");
        assert_eq!(n.search("kings x", &none)[0].id, "kings-cross-st-pancras");
        assert_eq!(n.search("tcr", &none)[0].name, "Tottenham Court Road");
        assert!(n.search("b", &none).is_empty());
    }

    #[test]
    fn search_drops_excluded_stations() {
        let n = network();
        let exclude = HashSet::from(["bank"]);
        assert!(n.search("bank", &exclude).iter().all(|s| s.id != "bank"));
    }

    #[test]
    fn zones_compare_with_neighbours_and_outsiders() {
        assert_eq!(compare_zones(Some(&[2, 3]), Some(&[3])), ZoneMatch::Exact);
        assert_eq!(compare_zones(Some(&[1]), Some(&[2])), ZoneMatch::Near);
        assert_eq!(compare_zones(Some(&[1]), Some(&[4])), ZoneMatch::None);
        assert_eq!(compare_zones(None, None), ZoneMatch::Exact);
        assert_eq!(compare_zones(None, Some(&[6])), ZoneMatch::None);
        assert_eq!(format_zones(Some(&[2, 3])), "Zones 2 & 3");
        assert_eq!(format_zones(None), "Outside the zones");
    }

    #[test]
    fn the_ladder_reveals_one_rung_per_miss() {
        let n = network();
        let bank = n.station("bank").unwrap();
        assert_eq!(n.clues(bank, 0).len(), 1);
        assert_eq!(n.clues(bank, 3).len(), 4);
        assert_eq!(n.clues(bank, 9).len(), 6);
        assert_eq!(n.clues(bank, 5)[5].value, "“B”");
    }

    #[test]
    fn bands_are_non_linear() {
        assert_eq!(band_for(1), 5);
        assert_eq!(band_for(6), 3);
        assert_eq!(band_for(30), 0);
    }
}
