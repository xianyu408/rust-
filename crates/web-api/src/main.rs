mod routes;
mod state;

use anyhow::{anyhow, Result};
use axum::{
    routing::{get, post},
    Router,
};
use std::{net::SocketAddr, path::PathBuf};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let state = state::AppState::new(PathBuf::from("workspaces"));
    tokio::fs::create_dir_all(&state.workspaces_root).await?;

    let app = Router::new()
        .route("/", get(routes::index))
        .route("/app", get(routes::index))
        .route("/health", get(routes::health))
        .route("/api/projects", post(routes::create_project))
        .route("/api/projects/{project_id}", get(routes::get_project))
        .route(
            "/api/projects/{project_id}/design",
            post(routes::start_design),
        )
        .route(
            "/api/projects/{project_id}/simulate",
            post(routes::start_simulation),
        )
        .route(
            "/api/projects/{project_id}/repair",
            post(routes::start_repair),
        )
        .route("/api/jobs/{job_id}", get(routes::get_job))
        .route("/api/jobs/{job_id}/events", get(routes::job_events))
        .route(
            "/api/projects/{project_id}/artifacts",
            get(routes::list_artifacts),
        )
        .route(
            "/api/projects/{project_id}/files/{*path}",
            get(routes::read_project_file),
        )
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let requested_addr: SocketAddr = std::env::var("CHIP_AGENT_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let (_, listener) = bind_listener(requested_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn bind_listener(
    requested_addr: SocketAddr,
) -> Result<(SocketAddr, tokio::net::TcpListener)> {
    match tokio::net::TcpListener::bind(requested_addr).await {
        Ok(listener) => {
            tracing::info!(addr = %requested_addr, "starting chip agent API");
            return Ok((requested_addr, listener));
        }
        Err(error) if std::env::var("CHIP_AGENT_BIND").is_ok() => {
            return Err(error.into());
        }
        Err(error) => {
            tracing::warn!(
                addr = %requested_addr,
                error = %error,
                "default bind address unavailable; trying fallback ports"
            );
        }
    }

    for port in 18080..=18090 {
        let fallback = SocketAddr::from(([127, 0, 0, 1], port));
        if let Ok(listener) = tokio::net::TcpListener::bind(fallback).await {
            tracing::info!(addr = %fallback, "starting chip agent API on fallback port");
            return Ok((fallback, listener));
        }
    }

    Err(anyhow!(
        "failed to bind {requested_addr} and fallback ports 18080-18090"
    ))
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("web_api=info,agent_core=info,eda_runner=info,tower_http=info")
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
