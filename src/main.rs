use anyhow::Result;
use axum::{
    routing::{get, post},
    Router,
};
use tower_http::services::ServeDir;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod config;
mod db;
mod electricity;
mod notify;
mod routes;
mod scheduler;
mod weather;

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub config: config::Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = config::Config::from_env()?;

    let db = db::Db::init_db(&config.db_path).await?;
    info!("Database initialized at {}", config.db_path);

    let state = AppState {
        db: db.clone(),
        config: config.clone(),
    };

    scheduler::spawn(db, config.clone());
    info!("Background scheduler started");

    let app = Router::new()
        .route("/", get(routes::index::handler))
        .route("/radiator", post(routes::index::radiator_handler))
        .route("/push/subscribe", post(routes::push::subscribe))
        .route("/push/unsubscribe", post(routes::push::unsubscribe))
        .route("/push/test-summary", post(routes::push::test_summary))
        .nest_service("/static", ServeDir::new("static"))
        .route("/sw.js", get(serve_sw))
        .route("/manifest.json", get(serve_manifest))
        .with_state(state)
        .merge(css())
        .merge(js(config.vapid_public_key));

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Listening an {addr}");

    axum::serve(listener, app).await?;

    Ok(())
}

async fn serve_sw() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("../static/sw.js"),
    )
}

async fn serve_manifest() -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/manifest+json",
        )],
        include_str!("../static/manifest.json"),
    )
}

const CSS: &'static str = "/assets/styles.css";

fn css() -> Router {
    // Serve embedded css in release
    #[cfg(not(debug_assertions))]
    {
        Router::new().route(
            CSS,
            get({
                (
                    [(axum::http::header::CONTENT_TYPE, "text/css")],
                    include_str!("../static/styles.css"),
                )
            }),
        )
    }

    // Serve static/styles.css in dev
    #[cfg(debug_assertions)]
    {
        use tower_http::services::ServeFile;

        Router::new().route_service(CSS, ServeFile::new("static/styles.css"))
    }
}

const JS: &'static str = "/assets/script.js";

fn js(vapid_public_key: String) -> Router {
    // Serve embedded script in release
    #[cfg(not(debug_assertions))]
    {
        Router::new().route(
            JS,
            get(async move || {
                let js = include_str!("../static/script.js");
                let js = js.replace("{{VAPID_PUBLIC_KEY}}", &vapid_public_key);

                (
                    [(axum::http::header::CONTENT_TYPE, "application/javascript")],
                    js,
                )
            }),
        )
    }

    // Serve static/script.js in dev
    #[cfg(debug_assertions)]
    {
        use axum::response::IntoResponse;
        use http::StatusCode;
        use tokio::fs;

        Router::new().route(
            JS,
            get(
                async move || match fs::read_to_string("static/script.js").await {
                    Ok(content) => {
                        let modified_content =
                            content.replace("{{VAPID_PUBLIC_KEY}}", &vapid_public_key);
                        (StatusCode::OK, modified_content).into_response()
                    }
                    Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
                },
            ),
        )
    }
}
