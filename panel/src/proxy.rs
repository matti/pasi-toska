//! Reverse proxy `/b/<id>/...` -> browser pod, so only the panel port is exposed.
//!
//! Plain requests are streamed both ways through reqwest (MCP SSE responses
//! flow chunk by chunk). Upgrade requests (noVNC `/websockify`) are tunnelled
//! byte for byte over a raw hyper connection, so subprotocols and frames pass
//! through untouched.

use std::net::SocketAddr;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::Empty;
use hyper_util::rt::TokioIo;

use crate::id;
use crate::podspec::{MCP_PORT, STATUS_PORT, VNC_PORT};

#[derive(Debug, PartialEq)]
pub struct Target {
    pub id: String,
    pub port: u16,
    /// Upstream path and query, e.g. `/websockify` or `/mcp`.
    pub path: String,
}

/// Map a panel path to a pod port and path. `None` = not a proxied path.
pub fn route(path: &str, query: Option<&str>) -> Option<Target> {
    let (id, rest) = path.strip_prefix("/b/")?.split_once('/')?;
    if !id::is_valid(id) {
        return None;
    }
    let (port, mut upstream) = match rest {
        "status" => (STATUS_PORT, "/status".to_string()),
        "replay.mp4" => (STATUS_PORT, "/replay.mp4".to_string()),
        "mcp" => (MCP_PORT, "/mcp".to_string()),
        r if r.starts_with("mcp/") => (MCP_PORT, format!("/{r}")),
        r => (VNC_PORT, format!("/{}", r.strip_prefix("vnc/")?)),
    };
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        upstream.push('?');
        upstream.push_str(q);
    }
    Some(Target { id: id.to_string(), port, path: upstream })
}

/// noVNC page URL whose websocket also goes through the proxy. The UI appends
/// `host`, `port`, `encrypt` and `view_only` (it knows the address the user sees).
pub fn vnc_url(id: &str) -> String {
    format!("/b/{id}/vnc/vnc.html?autoconnect=1&reconnect=1&reconnect_delay=1000&resize=scale&show_dot=1&path=b/{id}/vnc/websockify")
}

const HOP_BY_HOP: [HeaderName; 7] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
];

/// Copy end-to-end headers; Host is set by the client for the upstream.
fn end_to_end(src: &HeaderMap) -> HeaderMap {
    let mut out = src.clone();
    for h in HOP_BY_HOP.iter().chain([&header::UPGRADE, &header::HOST]) {
        out.remove(h);
    }
    out
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        && headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")))
}

pub fn bad_gateway(msg: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_GATEWAY, format!("Selaimeen ei saatu yhteyttä: {msg}")).into_response()
}

pub async fn forward(http: &reqwest::Client, addr: SocketAddr, path: &str, req: Request<Body>) -> Response {
    if is_upgrade(req.headers()) {
        return tunnel(addr, path, req).await.unwrap_or_else(bad_gateway);
    }
    let (parts, body) = req.into_parts();
    let upstream = http
        .request(parts.method, format!("http://{addr}{path}"))
        .headers(end_to_end(&parts.headers))
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    let resp = match upstream {
        Ok(r) => r,
        Err(e) => return bad_gateway(e),
    };
    let mut out = Response::builder().status(resp.status());
    *out.headers_mut().unwrap() = end_to_end(resp.headers());
    out.body(Body::from_stream(resp.bytes_stream())).unwrap_or_else(bad_gateway)
}

async fn tunnel(addr: SocketAddr, path: &str, mut req: Request<Body>) -> anyhow::Result<Response> {
    let client_side = hyper::upgrade::on(&mut req);
    let stream = tokio::net::TcpStream::connect(addr).await?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(conn.with_upgrades());

    let mut up = Request::builder().method(req.method()).uri(path).body(Empty::<bytes::Bytes>::new())?;
    *up.headers_mut() = req.headers().clone();
    up.headers_mut().insert(header::HOST, HeaderValue::from_str(&addr.to_string())?);
    let mut resp = sender.send_request(up).await?;

    let mut out = Response::builder().status(resp.status());
    *out.headers_mut().unwrap() = resp.headers().clone();
    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Ok(out.body(Body::empty())?);
    }
    let pod_side = hyper::upgrade::on(&mut resp);
    tokio::spawn(async move {
        let (Ok(a), Ok(b)) = tokio::join!(client_side, pod_side) else { return };
        let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
    });
    Ok(out.body(Body::empty())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, port: u16, path: &str) -> Option<Target> {
        Some(Target { id: id.into(), port, path: path.into() })
    }

    #[test]
    fn routes_vnc_mcp_status() {
        assert_eq!(route("/b/abc123/vnc/vnc.html", Some("autoconnect=1")), t("abc123", 6080, "/vnc.html?autoconnect=1"));
        assert_eq!(route("/b/abc123/vnc/websockify", None), t("abc123", 6080, "/websockify"));
        assert_eq!(route("/b/abc123/vnc/app/ui.js", Some("")), t("abc123", 6080, "/app/ui.js"));
        assert_eq!(route("/b/abc123/vnc/", None), t("abc123", 6080, "/"));
        assert_eq!(route("/b/abc123/mcp", None), t("abc123", 8931, "/mcp"));
        assert_eq!(route("/b/abc123/mcp/x", Some("a=b")), t("abc123", 8931, "/mcp/x?a=b"));
        assert_eq!(route("/b/abc123/status", None), t("abc123", 8080, "/status"));
    }

    #[test]
    fn rejects_unknown_and_bad_ids() {
        for p in ["/b/abc123", "/b/abc123/", "/b/abc123/vnc", "/b/abc123/other", "/b/ABC123/status", "/b/../status", "/x/abc123/status"] {
            assert_eq!(route(p, None), None, "{p}");
        }
    }

    #[test]
    fn vnc_url_routes_websocket_through_proxy() {
        let u = vnc_url("abc123");
        assert!(u.starts_with("/b/abc123/vnc/vnc.html?"));
        assert!(u.contains("path=b/abc123/vnc/websockify"));
        let (_, q) = u.split_once('?').unwrap();
        let path = q.split('&').find_map(|kv| kv.strip_prefix("path=")).unwrap();
        let (_, ws) = route(&format!("/{path}"), None).map(|t| (t.port, t.path)).unwrap();
        assert_eq!(ws, "/websockify");
    }

    #[test]
    fn strips_hop_by_hop_but_keeps_mcp_headers() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("transfer-encoding", "chunked"),
            ("host", "panel:30080"),
            ("mcp-session-id", "s1"),
            ("accept", "application/json, text/event-stream"),
            ("content-type", "application/json"),
        ] {
            h.insert(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        let out = end_to_end(&h);
        let keys: Vec<_> = out.keys().map(|k| k.as_str()).collect();
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert_eq!(out["mcp-session-id"], "s1");
        assert_eq!(out["accept"], "application/json, text/event-stream");
    }

    #[test]
    fn detects_websocket_upgrade() {
        let mut h = HeaderMap::new();
        h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        assert!(!is_upgrade(&h));
        h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        assert!(is_upgrade(&h));
    }
}
