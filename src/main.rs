//! Tubele, served: a daily London station guessing game whose state and
//! answers live on the server. The page is rendered by askama and driven by
//! Datastar; nothing that would reveal an answer reaches the browser before
//! the board it belongs to is finished.

mod engine;
mod game;
mod store;
mod ui;
mod view;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

/// A random 128-bit id as hex: a player.
pub fn new_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tubele=info,tower_http=info,warn".into()),
        )
        .init();

    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let db_path =
        PathBuf::from(std::env::var("DATABASE_PATH").unwrap_or_else(|_| "tubele.db".into()));
    let site_url = std::env::var("SITE_URL").unwrap_or_else(|_| "http://localhost:8080".into());

    let network = Arc::new(engine::Network::load()?);
    let db = store::Db::open(&db_path).await?;
    tracing::info!(stations = network.stations.len(), db = %db_path.display(), "loaded");

    // Finished daily boards are already in each player's record.
    tokio::spawn({
        let (db, network) = (db.clone(), network.clone());
        async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            loop {
                tick.tick().await;
                let today = network.puzzle_number(jiff::Timestamp::now());
                match db.run(move |c| Ok(store::prune(c, today)?)).await {
                    Ok(n) if n > 0 => tracing::info!(boards = n, "pruned"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "prune failed"),
                }
            }
        }
    });

    let app = ui::router(ui::AppState {
        network,
        db,
        site_url: site_url.trim_end_matches('/').into(),
    })
    .layer(CompressionLayer::new())
    .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
