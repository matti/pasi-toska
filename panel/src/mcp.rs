//! Minimal MCP client glue for the browser pod's Playwright MCP server.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

use crate::podspec::MCP_PORT;

pub type Mcp = rmcp::service::RunningService<rmcp::RoleClient, ()>;

pub async fn connect(http: &reqwest::Client, addr: SocketAddr) -> anyhow::Result<Mcp> {
    let config = StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"));
    let transport = StreamableHttpClientTransport::with_client(http.clone(), config);
    ().serve(transport).await.map_err(|e| anyhow::anyhow!("MCP-yhteys selaimeen epäonnistui: {e}"))
}

/// `tools/call`; returns (ok, MCP content blocks as wire JSON).
pub async fn call_tool(mcp: &Mcp, name: String, input: Value) -> (bool, Vec<Value>) {
    let mut params = CallToolRequestParams::new(name);
    if let Value::Object(args) = input {
        params = params.with_arguments(args);
    }
    match mcp.call_tool(params).await {
        Ok(r) => (!r.is_error.unwrap_or(false), r.content.iter().filter_map(|c| serde_json::to_value(c).ok()).collect()),
        Err(e) => (false, vec![json!({ "type": "text", "text": format!("MCP error: {e}") })]),
    }
}

/// Opens `url` in the browser's current tab (the "IP-testi" page, by default
/// hydra-browser's own big-font page at 127.0.0.1:8080/ip).
pub async fn navigate(http: reqwest::Client, pod: IpAddr, url: String) -> anyhow::Result<()> {
    let run = async {
        let mcp = connect(&http, SocketAddr::new(pod, MCP_PORT)).await?;
        let (ok, content) = call_tool(&mcp, "browser_navigate".into(), json!({ "url": url })).await;
        let _ = mcp.cancel().await;
        anyhow::ensure!(ok, "browser_navigate epäonnistui: {}", content.first().and_then(|c| c["text"].as_str()).unwrap_or("?"));
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(60), run).await.map_err(|_| anyhow::anyhow!("browser_navigate: aikakatkaisu"))?
}
