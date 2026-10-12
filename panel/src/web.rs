//! HTTP surface: embedded UI, JSON API, SSE snapshots and the pod proxy.

use std::convert::Infallible;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use futures::{Stream, StreamExt};
use rust_embed::RustEmbed;
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::{ReceiverStream, WatchStream};

use crate::agent::Agent;
use crate::cluster::Cluster;
use crate::config::Config;
use crate::model::Snapshot;
use crate::proxy;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub cluster: Arc<Cluster>,
    /// For proxying: no overall timeout (SSE and noVNC streams are long-lived), no redirects.
    pub proxy_http: reqwest::Client,
    pub snapshot: watch::Receiver<Snapshot>,
    pub agent: Arc<Agent>,
    /// "Jatka" senders of running agent runs, by browser id.
    pub runs: Arc<Mutex<HashMap<String, mpsc::Sender<()>>>>,
}

/// User-facing error: status code + Finnish message, rendered as `{"error": ...}`.
#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| asset("index.html")))
        .route("/static/{*path}", get(|Path(p): Path<String>| asset(p)))
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/state", get(state_json))
        .route("/api/events", get(events))
        .route("/api/browsers", post(launch))
        .route("/api/browsers/{id}", delete(stop))
        .route("/api/browsers/{id}/agent", post(agent_run))
        .route("/api/browsers/{id}/agent/resume", post(agent_resume))
        .route("/api/browsers/{id}/exit", post(switch_exit))
        .route("/api/browsers/{id}/fptest", post(fp_test))
        .route("/api/browsers/{id}/iptest", post(ip_test))
        .route("/b/{*rest}", any(proxy_handler))
        .with_state(state)
}

async fn asset(path: impl AsRef<str>) -> Response {
    let path = path.as_ref();
    match Assets::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref()), (header::CACHE_CONTROL, "no-cache")], f.data).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn state_json(State(s): State<AppState>) -> Json<Snapshot> {
    Json(s.snapshot.borrow().clone())
}

async fn events(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = WatchStream::new(s.snapshot.clone())
        .map(|snap| Ok(Event::default().event("snapshot").json_data(&snap).unwrap_or_default()));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct LaunchReq {
    exit: String,
}

async fn launch(State(s): State<AppState>, Json(req): Json<LaunchReq>) -> Result<Json<serde_json::Value>, ApiError> {
    let id = s.cluster.launch(&s.cfg, &req.exit).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn stop(State(s): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    s.cluster.stop(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn open_in_browser(s: &AppState, id: &str, url: String) -> Result<StatusCode, ApiError> {
    let ip = s.cluster.pod_ip(id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Selainta ei löydy.".into()))?;
    crate::mcp::navigate(s.proxy_http.clone(), ip, url).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ip_test(State(s): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let url = s.cfg.ip_test_url.clone();
    open_in_browser(&s, &id, url).await
}

async fn fp_test(State(s): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let url = s.cfg.fp_test_url.clone();
    open_in_browser(&s, &id, url).await
}

#[derive(Deserialize)]
struct ExitReq {
    exit: String,
}

/// Switches the exit, then reopens the IP page so the live view shows the new egress.
async fn switch_exit(State(s): State<AppState>, Path(id): Path<String>, Json(req): Json<ExitReq>) -> Result<StatusCode, ApiError> {
    s.cluster.switch_exit(&s.proxy_http, &id, &req.exit).await?;
    let url = s.cfg.ip_test_url.clone();
    tokio::spawn(async move {
        if let Err(e) = open_in_browser(&s, &id, url).await {
            tracing::warn!(id, "IP page after exit switch: {}", e.1);
        }
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn agent_resume(State(s): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let tx = s.runs.lock().unwrap().get(&id).cloned();
    match tx.map(|t| t.try_send(())) {
        Some(Ok(())) | Some(Err(mpsc::error::TrySendError::Full(_))) => Ok(StatusCode::NO_CONTENT),
        _ => Err(ApiError(StatusCode::NOT_FOUND, "Agentti ei odota tässä selaimessa.".into())),
    }
}

#[derive(Deserialize)]
struct AgentReq {
    task: String,
}

/// Runs the agent and streams its steps as NDJSON; closing the request stops it.
async fn agent_run(State(s): State<AppState>, Path(id): Path<String>, Json(req): Json<AgentReq>) -> Result<Response, ApiError> {
    let agent = s.agent.clone();
    agent.credential().map_err(|msg| ApiError(StatusCode::CONFLICT, msg))?;
    let ip = s.cluster.pod_ip(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Selainta ei löydy.".into()))?;
    let task = req.task.trim().to_string();
    if task.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Anna agentille tehtävä.".into()));
    }
    let (tx, rx) = mpsc::channel(32);
    let (resume_tx, resume_rx) = mpsc::channel(1);
    s.runs.lock().unwrap().insert(id.clone(), resume_tx.clone());
    let runs = s.runs.clone();
    tokio::spawn(async move {
        agent.run(ip, task, tx, resume_rx).await;
        let mut runs = runs.lock().unwrap();
        if runs.get(&id).is_some_and(|t| t.same_channel(&resume_tx)) {
            runs.remove(&id);
        }
    });
    let lines = ReceiverStream::new(rx).map(|step| {
        let mut line = serde_json::to_vec(&step).unwrap_or_default();
        line.push(b'\n');
        Ok::<_, Infallible>(bytes::Bytes::from(line))
    });
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson"), (header::CACHE_CONTROL, "no-cache")], Body::from_stream(lines)).into_response())
}

async fn proxy_handler(State(s): State<AppState>, req: Request<Body>) -> Response {
    let Some(t) = proxy::route(req.uri().path(), req.uri().query()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(ip) = s.cluster.pod_ip(&t.id) else {
        return (StatusCode::NOT_FOUND, "Selainta ei löydy tai sillä ei vielä ole IP-osoitetta.").into_response();
    };
    proxy::forward(&s.proxy_http, SocketAddr::new(ip, t.port), &t.path, req).await
}
