//! hydra-panel: web UI and proxy for hydra browsers running as k0s pods.

mod agent;
mod cluster;
mod config;
mod id;
mod mcp;
mod model;
mod podspec;
mod proxy;
mod web;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("rustls crypto provider already installed"))?;

    let cfg = Arc::new(config::Config::from_env()?);
    // In-cluster config, falling back to KUBECONFIG / ~/.kube/config.
    let client = kube::Client::try_default().await?;
    let cluster = cluster::Cluster::start(client, &cfg.namespace);

    let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).build()?;
    let proxy_http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .build()?;

    let (node_tx, node_rx) = watch::channel(None);
    let (snap_tx, snap_rx) = watch::channel(Default::default());
    tokio::spawn(cluster::run_node_egress(http.clone(), cfg.node_ip.clone(), node_tx));
    tokio::spawn(cluster.clone().run_snapshots(cfg.clone(), proxy_http.clone(), node_rx, snap_tx));

    let agent = Arc::new(agent::Agent { http: http.clone(), credentials: cfg.claude_credentials.clone(), model: cfg.agent_model.clone() });
    let state = web::AppState { cfg: cfg.clone(), cluster, proxy_http, snapshot: snap_rx, agent, runs: Default::default() };
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!("hydra-panel listening on http://{}", cfg.listen);
    // No graceful drain: SSE and noVNC streams never end on their own.
    tokio::select! {
        r = axum::serve(listener, web::router(state)).into_future() => r?,
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
    Ok(())
}

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
