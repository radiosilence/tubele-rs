//! The routes. Every interaction is a Datastar request answered with SSE
//! patches rendered here; the browser holds no game state and never sees an
//! answer before the board that hides it is finished.

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::{Arc, LazyLock};

use askama::Template;
use axum::extract::{Query, Request, State};
use axum::http::{self, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::engine::{Line, Network, ms_until_next_puzzle};
use crate::game::{Board, Mode, PROTECT_UPCOMING_DAYS, Stats, Status};
use crate::store::{self, Db};
use crate::view::{self, Context, GameTemplate, PageTemplate, SuggestionView, SuggestionsTemplate};

#[derive(Clone)]
pub struct AppState {
    pub network: Arc<Network>,
    pub db: Db,
    pub site_url: Arc<str>,
}

const COOKIE: &str = "tubele";

/// Datastar compiles each `data-*` expression with `Function()`, hence
/// `unsafe-eval`; nothing is inline, so escaping in the templates is the
/// control on what those expressions contain.
const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-eval'; style-src 'self'; img-src 'self' data: blob:; \
     font-src 'self'; connect-src 'self'; manifest-src 'self'; form-action 'self'; frame-ancestors 'none'; \
     base-uri 'none'; object-src 'none'";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/suggest", get(suggest))
        .route("/guess", post(guess))
        .route("/practice", post(practice))
        .route("/daily", post(daily))
        .route("/seen", post(seen))
        .route("/healthz", get(|| async { "ok" }))
        .route("/robots.txt", get(robots))
        .route("/sitemap.xml", get(sitemap))
        .route("/assets/lines.css", get(lines_css))
        .merge(assets())
        .fallback(not_found)
        .layer(middleware::from_fn(require_datastar_on_post))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

// ─── Players ───

/// The player's id, minted on first sight. A random id in a cookie is the
/// whole of what the server knows about anyone.
fn player(jar: CookieJar, secure: bool) -> (String, CookieJar) {
    if let Some(id) = jar.get(COOKIE).map(|c| c.value().to_owned())
        && id.len() == 32
        && id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return (id, jar);
    }
    let id = crate::new_id();
    let cookie = Cookie::build((COOKIE, id.clone()))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        // Browsers cap a cookie's life at 400 days whatever is asked for.
        .permanent()
        .build();
    (id, jar.add(cookie))
}

impl AppState {
    fn secure(&self) -> bool {
        self.site_url.starts_with("https://")
    }
}

// ─── Signals ───

#[derive(Deserialize, Default)]
#[serde(default)]
struct Signals {
    q: String,
    active: usize,
    mode: Option<Mode>,
    puzzle: i64,
}

/// A GET carries the signals as JSON in the `datastar` query parameter.
#[derive(Deserialize)]
struct SignalsQuery {
    datastar: Option<String>,
}

impl SignalsQuery {
    fn parse(self) -> Signals {
        self.datastar
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

// ─── Board resolution ───

struct Now {
    today: i64,
    ms_to_next: i64,
    at: i64,
}

fn now(network: &Network) -> Now {
    let t = Timestamp::now();
    Now {
        today: network.puzzle_number(t),
        ms_to_next: ms_until_next_puzzle(t),
        at: t.as_millisecond(),
    }
}

/// The board a request is about. Practice when the page says so and one
/// exists; otherwise today's daily, or yesterday's if the player already
/// started it in a tab left open over midnight. Never a future puzzle.
fn resolve(
    conn: &rusqlite::Connection,
    network: &Network,
    player: &str,
    signals: &Signals,
    today: i64,
) -> anyhow::Result<Option<Board>> {
    if signals.mode == Some(Mode::Practice)
        && let Some(board) = store::board(conn, player, Mode::Practice, 0)?
    {
        return Ok(Some(board));
    }
    if signals.puzzle == today - 1
        && let Some(board) = store::board(conn, player, Mode::Daily, today - 1)?
    {
        return Ok(Some(board));
    }
    daily_board(conn, network, player, today)
}

fn daily_board(
    conn: &rusqlite::Connection,
    network: &Network,
    player: &str,
    today: i64,
) -> anyhow::Result<Option<Board>> {
    if let Some(board) = store::board(conn, player, Mode::Daily, today)? {
        return Ok(Some(board));
    }
    Ok(network
        .answer_for(today)
        .map(|id| Board::new(Mode::Daily, today, id)))
}

fn render_game(
    state: &AppState,
    board: &Board,
    stats: &Stats,
    now: &Now,
    announce: bool,
    open_result: bool,
) -> view::GameView {
    view::game(&Context {
        network: &state.network,
        board,
        stats,
        today: now.today,
        ms_to_next: now.ms_to_next,
        site_url: &state.site_url,
        announce,
        open_result,
    })
}

// ─── Handlers ───

async fn page(State(state): State<AppState>, jar: CookieJar) -> Result<Response, AppError> {
    let (id, jar) = player(jar, state.secure());
    let now = now(&state.network);
    let network = state.network.clone();
    let today = now.today;
    let (board, player) = state
        .db
        .run(move |c| {
            Ok((
                daily_board(c, &network, &id, today)?,
                store::player(c, &id)?,
            ))
        })
        .await?;
    let Some(board) = board else {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            "The schedule has run out of stations.",
        )
            .into_response());
    };

    let signals = json!({
        "q": "", "active": 0, "mode": "daily", "puzzle": board.puzzle,
        "_count": 0, "_open": false, "_now": now.at, "_next": now.at + now.ms_to_next,
    });
    let html = PageTemplate {
        g: render_game(&state, &board, &player.stats, &now, false, true),
        seen_guide: player.seen_guide,
        signals: signals.to_string(),
        site_url: state.site_url.to_string(),
        description: view::DESCRIPTION,
        css_url: &CSS_URL,
        js_url: &JS_URL,
        lines_url: &LINES_URL,
    }
    .render()?;
    Ok((jar, Html(html)).into_response())
}

async fn suggest(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<SignalsQuery>,
) -> Result<Response, AppError> {
    let (id, jar) = player(jar, state.secure());
    let signals = query.parse();
    let network = state.network.clone();
    let today = now(&network).today;
    let (items, q) = state
        .db
        .run(move |c| {
            let guessed = resolve(c, &network, &id, &signals, today)?
                .map(|b| b.guesses)
                .unwrap_or_default();
            let exclude: HashSet<&str> = guessed.iter().map(String::as_str).collect();
            let items: Vec<SuggestionView> = network
                .search(&signals.q, &exclude)
                .into_iter()
                .map(SuggestionView::new)
                .collect();
            Ok((items, signals.q))
        })
        .await?;

    let count = items.len();
    let q = q.trim();
    let html = SuggestionsTemplate {
        no_match: (count == 0 && q.len() >= 2).then(|| q.to_owned()),
        items,
    }
    .render()?;
    Ok((
        jar,
        many(vec![patch(&html), set_signals(&json!({ "_count": count }))]),
    )
        .into_response())
}

async fn guess(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(signals): Json<Signals>,
) -> Result<Response, AppError> {
    let (id, jar) = player(jar, state.secure());
    let network = state.network.clone();
    let now = now(&network);
    let today = now.today;
    let outcome = state
        .db
        .run(move |c| {
            let tx = store::write(c)?;
            let Some(mut board) = resolve(&tx, &network, &id, &signals, today)? else {
                return Ok(None);
            };
            let exclude: HashSet<&str> = board.guesses.iter().map(String::as_str).collect();
            let picks = network.search(&signals.q, &exclude);
            let pick = picks
                .get(signals.active.min(picks.len().saturating_sub(1)))
                .map(|s| s.id.clone());
            let changed = pick.is_some_and(|pick| board.guess(&pick));

            let mut stats = store::player(&tx, &id)?.stats;
            if changed {
                store::save_board(&tx, &id, &board)?;
                let status = board.status();
                if board.mode == Mode::Daily && status != Status::Playing {
                    stats.record(board.puzzle, status == Status::Won, board.guesses.len());
                    store::save_stats(&tx, &id, &stats)?;
                }
            }
            tx.commit()?;
            Ok(changed.then_some((board, stats)))
        })
        .await?;

    let Some((board, stats)) = outcome else {
        return Ok((jar, many(vec![])).into_response());
    };
    let html = GameTemplate {
        g: render_game(&state, &board, &stats, &now, true, true),
    }
    .render()?;
    Ok((
        jar,
        many(vec![
            patch(&html),
            set_signals(&json!({ "q": "", "active": 0, "_count": 0, "_open": false })),
        ]),
    )
        .into_response())
}

/// A practice round: any station except today's answer and the next few, so
/// practice never spoils a daily. Its result is never recorded.
async fn practice(State(state): State<AppState>, jar: CookieJar) -> Result<Response, AppError> {
    let (id, jar) = player(jar, state.secure());
    let network = state.network.clone();
    let now = now(&network);
    let today = now.today;
    let (board, stats) = state
        .db
        .run(move |c| {
            let forbidden: HashSet<&str> = (today..today + PROTECT_UPCOMING_DAYS)
                .filter_map(|day| network.answer_for(day))
                .collect();
            let pool: Vec<&str> = network
                .stations
                .iter()
                .map(|s| s.id.as_str())
                .filter(|id| !forbidden.contains(id))
                .collect();
            let pick = pool[rand::random_range(0..pool.len())];
            let board = Board::new(Mode::Practice, 0, pick);
            store::save_board(c, &id, &board)?;
            Ok((board, store::player(c, &id)?.stats))
        })
        .await?;
    switch(&state, jar, &board, &stats, &now)
}

/// Back to the daily board: the rollover banner's "Play it".
async fn daily(State(state): State<AppState>, jar: CookieJar) -> Result<Response, AppError> {
    let (id, jar) = player(jar, state.secure());
    let network = state.network.clone();
    let now = now(&network);
    let today = now.today;
    let (board, stats) = state
        .db
        .run(move |c| {
            Ok((
                daily_board(c, &network, &id, today)?,
                store::player(c, &id)?.stats,
            ))
        })
        .await?;
    let Some(board) = board else {
        return Ok((jar, many(vec![])).into_response());
    };
    switch(&state, jar, &board, &stats, &now)
}

fn switch(
    state: &AppState,
    jar: CookieJar,
    board: &Board,
    stats: &Stats,
    now: &Now,
) -> Result<Response, AppError> {
    let html = GameTemplate {
        g: render_game(state, board, stats, now, false, true),
    }
    .render()?;
    let signals = set_signals(&json!({
        "q": "", "active": 0, "_count": 0, "_open": false,
        "mode": board.mode.as_str(), "puzzle": board.puzzle,
        "_next": now.at + now.ms_to_next,
    }));
    Ok((jar, many(vec![patch(&html), signals])).into_response())
}

async fn seen(State(state): State<AppState>, jar: CookieJar) -> Result<Response, AppError> {
    let (id, jar) = player(jar, state.secure());
    state
        .db
        .run(move |c| Ok(store::mark_guide_seen(c, &id)?))
        .await?;
    Ok((jar, many(vec![])).into_response())
}

async fn robots(State(state): State<AppState>) -> impl IntoResponse {
    format!(
        "User-agent: *\nAllow: /\nDisallow: /suggest\n\nSitemap: {}/sitemap.xml\n",
        state.site_url
    )
}

async fn sitemap(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/xml")],
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>{}/</loc><changefreq>daily</changefreq><priority>1.0</priority></url>
</urlset>
"#,
            state.site_url
        ),
    )
}

async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Html(format!(
            r#"<!doctype html><html lang="en-GB"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Not found — Tubele</title><meta name="robots" content="noindex, follow"><link rel="stylesheet" href="{}"></head>
<body class="grid min-h-dvh place-content-center gap-[18px] px-5 py-8 text-center leading-normal"><h1 class="text-[26px] font-extrabold tracking-[-0.01em]">Service not stopping here</h1><p class="text-soft">There is no page at this address. There is, however, a station to guess.</p><p><a class="btn btn-primary inline-flex items-center" href="/">Play Tubele</a></p></body></html>"#,
            *CSS_URL
        )),
    )
}

// ─── SSE ───

/// A bare `\r` would start an SSE data line outside the `elements` prefix.
fn patch(html: &str) -> Event {
    Event::default().event("datastar-patch-elements").data(
        html.replace('\r', "")
            .lines()
            .map(|l| format!("elements {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn set_signals(value: &serde_json::Value) -> Event {
    Event::default()
        .event("datastar-patch-signals")
        .data(format!("signals {value}"))
}

fn many(events: Vec<Event>) -> Response {
    Sse::new(futures_util::stream::iter(
        events.into_iter().map(Ok::<_, Infallible>),
    ))
    .into_response()
}

// ─── Assets ───

const APP_CSS: &str = include_str!("../assets/app.css");
const DATASTAR_JS: &str = include_str!("../assets/datastar.js");
const APP_JS: &str = include_str!("../assets/app.js");

static DATASTAR_URL: LazyLock<String> =
    LazyLock::new(|| versioned("/assets/datastar.js", DATASTAR_JS.as_bytes()));
static CSS_URL: LazyLock<String> =
    LazyLock::new(|| versioned("/assets/app.css", APP_CSS.as_bytes()));
/// app.js imports Datastar by the same fingerprinted URL the browser caches,
/// so there is one Datastar on the page, not two.
static APP_JS_BODY: LazyLock<String> =
    LazyLock::new(|| APP_JS.replace("\"./datastar.js\"", &format!("\"{}\"", *DATASTAR_URL)));
static JS_URL: LazyLock<String> =
    LazyLock::new(|| versioned("/assets/app.js", APP_JS_BODY.as_bytes()));
static LINES_CSS: LazyLock<String> = LazyLock::new(|| {
    view::lines_css(
        &serde_json::from_str::<Vec<Line>>(crate::engine::LINES).expect("lines.json parses"),
    )
});
static LINES_URL: LazyLock<String> =
    LazyLock::new(|| versioned("/assets/lines.css", LINES_CSS.as_bytes()));

/// `path` with a hash of `body`, so a new build's page fetches its own assets.
fn versioned(path: &str, body: &[u8]) -> String {
    let hash = body.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{path}?v={hash:016x}")
}

/// Asked for by its fingerprinted URL, an asset never changes, so it is kept
/// for good; a bare URL is rechecked hourly.
fn cached(kind: &'static str, body: impl IntoResponse, query: Option<&str>) -> Response {
    let cache = if query.is_some_and(|q| q.starts_with("v=")) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    (
        [(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, cache)],
        body,
    )
        .into_response()
}

async fn lines_css(request: Request) -> Response {
    cached(
        "text/css; charset=utf-8",
        LINES_CSS.as_str(),
        request.uri().query(),
    )
}

macro_rules! file {
    ($router:expr, $path:literal, $kind:literal, $file:literal) => {
        $router.route(
            $path,
            get(|request: Request| async move {
                cached(
                    $kind,
                    &include_bytes!(concat!("../assets/", $file))[..],
                    request.uri().query(),
                )
            }),
        )
    };
}

fn assets() -> Router<AppState> {
    let r = Router::new()
        .route(
            "/assets/app.css",
            get(|request: Request| async move {
                cached("text/css; charset=utf-8", APP_CSS, request.uri().query())
            }),
        )
        .route(
            "/assets/datastar.js",
            get(|request: Request| async move {
                cached(
                    "text/javascript; charset=utf-8",
                    DATASTAR_JS,
                    request.uri().query(),
                )
            }),
        )
        .route(
            "/assets/app.js",
            get(|request: Request| async move {
                cached(
                    "text/javascript; charset=utf-8",
                    APP_JS_BODY.as_str(),
                    request.uri().query(),
                )
            }),
        );
    // Fonts never change under the same name; the query makes them immutable.
    let r = file!(
        r,
        "/fonts/archivo-400.woff2",
        "font/woff2",
        "fonts/archivo-400.woff2"
    );
    let r = file!(
        r,
        "/fonts/archivo-700.woff2",
        "font/woff2",
        "fonts/archivo-700.woff2"
    );
    let r = file!(
        r,
        "/fonts/archivo-800.woff2",
        "font/woff2",
        "fonts/archivo-800.woff2"
    );
    let r = file!(
        r,
        "/fonts/archivo-900.woff2",
        "font/woff2",
        "fonts/archivo-900.woff2"
    );
    let r = file!(
        r,
        "/fonts/spline-sans-mono-400.woff2",
        "font/woff2",
        "fonts/spline-sans-mono-400.woff2"
    );
    let r = file!(
        r,
        "/fonts/spline-sans-mono-600.woff2",
        "font/woff2",
        "fonts/spline-sans-mono-600.woff2"
    );
    let r = file!(r, "/favicon.svg", "image/svg+xml", "favicon.svg");
    let r = file!(r, "/icon.svg", "image/svg+xml", "icon.svg");
    let r = file!(r, "/icon-192.png", "image/png", "icon-192.png");
    let r = file!(r, "/icon-512.png", "image/png", "icon-512.png");
    let r = file!(
        r,
        "/icon-maskable-512.png",
        "image/png",
        "icon-maskable-512.png"
    );
    let r = file!(
        r,
        "/apple-touch-icon.png",
        "image/png",
        "apple-touch-icon.png"
    );
    let r = file!(r, "/og.png", "image/png", "og.png");
    file!(
        r,
        "/manifest.webmanifest",
        "application/manifest+json",
        "manifest.webmanifest"
    )
}

// ─── Middleware and errors ───

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

/// Every state change comes from Datastar, which sets a header a cross-site
/// form cannot, so refusing POSTs without it stops request forgery.
async fn require_datastar_on_post(request: Request, next: Next) -> Response {
    if request.method() == http::Method::POST && !request.headers().contains_key("datastar-request")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

pub struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Signal failure. Reload and try again.",
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests;
