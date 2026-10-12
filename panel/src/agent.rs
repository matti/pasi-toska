//! "Agentti": a small Claude agent loop that drives one browser through its MCP tools.
//!
//! Tools come from the pod's MCP server (`tools/list`), are offered to Claude as
//! client tools, and every `tool_use` is executed as an MCP `tools/call`. Steps
//! are streamed to the UI as NDJSON.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rmcp::model::Tool;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::mcp::{call_tool, connect};
use crate::podspec::MCP_PORT;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// `oauth-2025-04-20`: the credential is the user's Claude subscription
/// (`hydra login` copies it from the Mac's Keychain into Secret hydra-claude).
const BETAS: &str = "oauth-2025-04-20,server-side-fallback-2026-07-01,thinking-display-updates-2026-08-18,context-management-2025-06-27";
const MAX_ROUNDS: usize = 25;
const PREVIEW_CHARS: usize = 240;
/// A run is refused when the subscription token expires sooner than this.
pub const MIN_MINUTES: i64 = 10;

/// Subscription (OAuth) tokens are accepted only with this first system block.
const OAUTH_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

const ASK_HUMAN: &str = "ask_human";
/// How long a paused run waits for "Jatka" before giving up.
const ASK_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const HUMAN_DONE: &str = "The person pressed Jatka: they are done (for example solved the CAPTCHA or logged in). \
Take a fresh snapshot and continue the task.";

/// Client tool the panel runs itself: pauses the run until the person watching presses "Jatka".
fn ask_human_tool() -> Value {
    json!({
        "name": ASK_HUMAN,
        "description": "Pause and ask the person watching the live browser for help with something only a human should do: \
a CAPTCHA, a login or 2FA prompt, a payment, a consent that needs a decision. The browser view becomes interactive for them; \
this call returns when they press Jatka. Do not try to solve CAPTCHAs yourself.",
        "input_schema": {
            "type": "object",
            "properties": { "reason": { "type": "string", "description": "Short Finnish explanation shown to the person, e.g. 'Sivu pyytää CAPTCHAa.'" } },
            "required": ["reason"],
        },
    })
}

const SYSTEM: &str = "You control a real Chromium browser through the browser tools. \
A person is watching the browser screen live, so work visibly in the browser rather than guessing. \
For web searches use DuckDuckGo's HTML version (https://html.duckduckgo.com/html/?q=...) or Bing; never Google, it blocks automated traffic. \
Read pages with browser_snapshot (or browser_evaluate to extract text or lists); take screenshots only when the layout matters. \
Dismiss cookie banners when they block the page. Finish within about 25 tool calls. \
If you hit a CAPTCHA, a login or 2FA prompt, a payment or anything else only a person should do, call ask_human with a short Finnish reason and continue after it returns. \
Write your short progress notes in Finnish. \
End with a short final answer in Finnish, at most about eight lines: the key findings (a few bullets at most), then the sources as markdown links with their URLs. \
If something cannot be done, say briefly what you tried.";

/// The Claude subscription token, read from the mounted Secret on every use
/// so a refreshed Secret takes effect without a restart.
#[derive(Debug, PartialEq)]
pub struct Credential {
    pub token: String,
    pub expires_at_ms: i64,
}

pub fn read_credential(path: &Path) -> anyhow::Result<Credential> {
    let v: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let o = &v["claudeAiOauth"];
    let token = o["accessToken"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| anyhow::anyhow!("accessToken puuttuu"))?;
    Ok(Credential { token: token.to_string(), expires_at_ms: o["expiresAt"].as_i64().unwrap_or(0) })
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub fn minutes_left(c: &Credential, now_ms: i64) -> i64 {
    (c.expires_at_ms - now_ms) / 60_000
}

/// A credential valid for at least MIN_MINUTES, or the Finnish reason why the agent cannot run.
pub fn usable_credential(path: &Path) -> Result<Credential, String> {
    let c = read_credential(path).map_err(|_| "Claude-tunnusta ei ole: aja Macilla `hydra login`.".to_string())?;
    let left = minutes_left(&c, now_ms());
    if left < MIN_MINUTES {
        return Err(format!("Claude-tunnus vanhenee {} min kuluttua: aja Macilla `hydra login`.", left.max(0)));
    }
    Ok(c)
}

/// Minutes of token validity left, for the header; None without a credential.
pub fn credential_minutes(path: &Path) -> Option<i64> {
    read_credential(path).ok().map(|c| minutes_left(&c, now_ms()))
}

/// One UI-visible step of an agent run.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    Text { text: String },
    Tool { call: String, name: String, input: Value },
    ToolResult { call: String, ok: bool, preview: String },
    /// The run is paused until the person presses "Jatka" (POST .../agent/resume).
    AskHuman { call: String, reason: String },
    Resumed,
    Final { text: String },
    Error { message: String },
}

pub struct Agent {
    pub http: reqwest::Client,
    pub credentials: PathBuf,
    pub model: String,
}

/// MCP tool -> Claude client tool definition.
fn claude_tool(t: &Tool) -> Value {
    let mut schema = (*t.input_schema).clone();
    schema.remove("$schema");
    json!({
        "name": t.name,
        "description": t.description.as_deref().unwrap_or_default(),
        "input_schema": schema,
    })
}

/// MCP content blocks (wire JSON) -> Claude `tool_result` content blocks.
fn result_blocks(content: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = content
        .iter()
        .filter_map(|c| match c["type"].as_str()? {
            "text" => Some(json!({ "type": "text", "text": c["text"].as_str().unwrap_or_default() })),
            "image" => Some(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": c["mimeType"], "data": c["data"] },
            })),
            "resource" => c["resource"]["text"].as_str().map(|t| json!({ "type": "text", "text": t })),
            _ => None,
        })
        .collect();
    if out.is_empty() {
        out.push(json!({ "type": "text", "text": "(tyhjä tulos)" }));
    }
    out
}

fn preview(content: &[Value]) -> String {
    let text: String = content
        .iter()
        .map(|c| match c["type"].as_str() {
            Some("text") => c["text"].as_str().unwrap_or_default().to_string(),
            Some("image") => "[kuva]".to_string(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(PREVIEW_CHARS) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}

/// Visible text of an assistant turn: text blocks, plus progress notes that
/// come back as `thinking` blocks with `display: "updates"`.
fn visible_texts(content: &[Value]) -> Vec<String> {
    content
        .iter()
        .filter_map(|b| match b["type"].as_str()? {
            "text" => b["text"].as_str(),
            "thinking" => b["thinking"].as_str(),
            _ => None,
        })
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

impl Agent {
    pub fn credential(&self) -> Result<Credential, String> {
        usable_credential(&self.credentials)
    }

    /// Runs `task` against the browser at `pod`; ends when the task is done,
    /// fails, or the receiver of `steps` goes away (UI pressed stop).
    pub async fn run(&self, pod: std::net::IpAddr, task: String, steps: mpsc::Sender<Step>, mut resume: mpsc::Receiver<()>) {
        if let Err(e) = self.run_inner(SocketAddr::new(pod, MCP_PORT), task, &steps, &mut resume).await {
            let _ = steps.send(Step::Error { message: format!("{e:#}") }).await;
        }
    }

    /// Pauses for the person; Err when the run was stopped or nobody came.
    async fn wait_for_human(steps: &mpsc::Sender<Step>, resume: &mut mpsc::Receiver<()>) -> anyhow::Result<()> {
        // Presses from an earlier pause do not count.
        while resume.try_recv().is_ok() {}
        tokio::select! {
            r = resume.recv() => r.ok_or_else(|| anyhow::anyhow!("Pysäytetty.")),
            _ = steps.closed() => anyhow::bail!("Pysäytetty."),
            _ = tokio::time::sleep(ASK_TIMEOUT) => anyhow::bail!("Kukaan ei jatkanut 15 minuutissa, ajo lopetettiin."),
        }
    }

    async fn run_inner(
        &self,
        mcp_addr: SocketAddr,
        task: String,
        steps: &mpsc::Sender<Step>,
        resume: &mut mpsc::Receiver<()>,
    ) -> anyhow::Result<()> {
        let mcp = connect(&self.http, mcp_addr).await?;
        let mut tools: Vec<Value> = mcp.list_all_tools().await?.iter().map(claude_tool).collect();
        tools.push(ask_human_tool());

        let mut messages = vec![json!({ "role": "user", "content": task })];
        let result = async {
            for _ in 0..MAX_ROUNDS {
                let resp = self.create(&tools, &messages).await?;
                let content = resp["content"].as_array().cloned().unwrap_or_default();
                let stop = resp["stop_reason"].as_str().unwrap_or_default().to_string();
                messages.push(json!({ "role": "assistant", "content": content }));

                match stop.as_str() {
                    "end_turn" | "stop_sequence" => {
                        let text = visible_texts(&content).last().cloned().unwrap_or_else(|| "Valmis.".into());
                        steps.send(Step::Final { text }).await?;
                        return Ok(());
                    }
                    "refusal" => anyhow::bail!("Claude kieltäytyi tehtävästä."),
                    "max_tokens" => anyhow::bail!("Vastaus katkesi (max_tokens)."),
                    "tool_use" | "pause_turn" => {}
                    other => anyhow::bail!("Odottamaton stop_reason: {other}"),
                }
                for text in visible_texts(&content) {
                    steps.send(Step::Text { text }).await?;
                }

                let mut results = Vec::new();
                for block in content.iter().filter(|b| b["type"] == "tool_use") {
                    let call = block["id"].as_str().unwrap_or_default().to_string();
                    let name = block["name"].as_str().unwrap_or_default().to_string();
                    let input = block["input"].clone();
                    steps.send(Step::Tool { call: call.clone(), name: name.clone(), input: input.clone() }).await?;
                    let (ok, content) = if name == ASK_HUMAN {
                        let reason = input["reason"].as_str().unwrap_or("Tarvitsen apua.").to_string();
                        steps.send(Step::AskHuman { call: call.clone(), reason }).await?;
                        Self::wait_for_human(steps, resume).await?;
                        steps.send(Step::Resumed).await?;
                        (true, vec![json!({ "type": "text", "text": HUMAN_DONE })])
                    } else {
                        call_tool(&mcp, name, input).await
                    };
                    steps.send(Step::ToolResult { call: call.clone(), ok, preview: preview(&content) }).await?;
                    results.push(json!({ "type": "tool_result", "tool_use_id": call, "content": result_blocks(&content), "is_error": !ok }));
                }
                if !results.is_empty() {
                    messages.push(json!({ "role": "user", "content": results }));
                }
            }
            anyhow::bail!("Agentti lopetti {MAX_ROUNDS} kierroksen jälkeen.")
        }
        .await;
        let _ = mcp.cancel().await;
        result
    }

    async fn create(&self, tools: &[Value], messages: &[Value]) -> anyhow::Result<Value> {
        let cred = self.credential().map_err(anyhow::Error::msg)?;
        let body = json!({
            "model": self.model,
            "max_tokens": 16000,
            "system": [{ "type": "text", "text": OAUTH_IDENTITY }, { "type": "text", "text": SYSTEM }],
            "tools": tools,
            "messages": messages,
            "thinking": { "type": "adaptive", "display": "updates" },
            "output_config": { "effort": "medium" },
            "fallbacks": "default",
            "cache_control": { "type": "ephemeral" },
            // Old page snapshots are cleared server-side (history is never edited client-side).
            "context_management": { "edits": [{
                "type": "clear_tool_uses_20250919",
                "trigger": { "type": "input_tokens", "value": 60000 },
                "keep": { "type": "tool_uses", "value": 4 },
            }] },
        });
        let resp = self
            .http
            .post(API_URL)
            .bearer_auth(&cred.token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", BETAS)
            .timeout(Duration::from_secs(300))
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        // No retries: on the subscription a 429 means the plan's limit.
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("tilauksen raja tuli vastaan");
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            anyhow::bail!("Claude-tunnus ei kelpaa: aja Macilla `hydra login`.");
        }
        let value: Value = resp.json().await?;
        if !status.is_success() {
            let msg = value["error"]["message"].as_str().unwrap_or("tuntematon virhe");
            anyhow::bail!("Claude API {status}: {msg}");
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tool_maps_to_claude_tool() {
        let t: Tool = serde_json::from_value(json!({
            "name": "browser_navigate",
            "description": "Navigate to a URL",
            "inputSchema": { "$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
        }))
        .unwrap();
        let c = claude_tool(&t);
        assert_eq!(c["name"], "browser_navigate");
        assert_eq!(c["input_schema"]["required"][0], "url");
        assert!(c.get("inputSchema").is_none());
        assert!(c["input_schema"].get("$schema").is_none());
    }

    #[test]
    fn mcp_content_maps_to_tool_result_blocks() {
        let content = [
            json!({ "type": "text", "text": "Page title: X" }),
            json!({ "type": "image", "data": "aGk=", "mimeType": "image/png" }),
            json!({ "type": "audio", "data": "x", "mimeType": "audio/wav" }),
        ];
        let b = result_blocks(&content);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], json!({ "type": "text", "text": "Page title: X" }));
        assert_eq!(b[1]["source"], json!({ "type": "base64", "media_type": "image/png", "data": "aGk=" }));
        assert_eq!(result_blocks(&[])[0]["type"], "text");
    }

    #[test]
    fn preview_flattens_and_truncates() {
        let long = "a ".repeat(500);
        let p = preview(&[json!({ "type": "text", "text": long })]);
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1);
        assert!(p.ends_with('…'));
        assert_eq!(preview(&[json!({ "type": "text", "text": "x\n\n y" }), json!({ "type": "image" })]), "x y [kuva]");
    }

    #[test]
    fn visible_texts_include_progress_notes() {
        let content = [
            json!({ "type": "thinking", "thinking": "", "signature": "s" }),
            json!({ "type": "thinking", "thinking": "Avaan sivun.", "signature": "s" }),
            json!({ "type": "text", "text": " Valmis \n" }),
            json!({ "type": "tool_use", "id": "t1", "name": "x", "input": {} }),
        ];
        assert_eq!(visible_texts(&content), ["Avaan sivun.", "Valmis"]);
    }

    /// Live check of the MCP half against a running hydra-browser, e.g.
    /// `HYDRA_MCP_PROBE=127.0.0.1:18931 cargo test mcp_probe -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn mcp_probe() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let addr = std::env::var("HYDRA_MCP_PROBE").expect("HYDRA_MCP_PROBE=host:port").parse().unwrap();
        let mcp = connect(&reqwest::Client::new(), addr).await.unwrap();
        let tools = mcp.list_all_tools().await.unwrap();
        println!("{} tools: {:?}", tools.len(), tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>());
        let nav = tools.iter().find(|t| t.name == "browser_navigate").expect("browser_navigate");
        println!("{}", claude_tool(nav));
        let (ok, content) = call_tool(&mcp, "browser_navigate".into(), json!({ "url": "https://example.com" })).await;
        println!("ok={ok} {}", preview(&content));
        assert!(ok);
        mcp.cancel().await.unwrap();
    }

    #[test]
    fn credential_from_keychain_json() {
        let dir = std::env::temp_dir().join(format!("hydra-cred-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("credentials.json");
        std::fs::write(&p, r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1800000000000,"subscriptionType":"max"}}"#).unwrap();
        let c = read_credential(&p).unwrap();
        assert_eq!(c, Credential { token: "tok".into(), expires_at_ms: 1_800_000_000_000 });
        assert_eq!(minutes_left(&c, 1_800_000_000_000 - 90 * 60_000), 90);
        std::fs::write(&p, r#"{"claudeAiOauth":{"accessToken":"","expiresAt":0}}"#).unwrap();
        assert!(read_credential(&p).is_err());
        assert!(read_credential(&dir.join("missing.json")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn expiring_credential_is_refused() {
        let dir = std::env::temp_dir().join(format!("hydra-cred2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("credentials.json");
        assert!(usable_credential(&p).unwrap_err().contains("hydra login"));
        let soon = now_ms() + 5 * 60_000;
        std::fs::write(&p, format!(r#"{{"claudeAiOauth":{{"accessToken":"t","expiresAt":{soon}}}}}"#)).unwrap();
        assert!(usable_credential(&p).unwrap_err().contains("vanhenee"));
        let later = now_ms() + 120 * 60_000;
        std::fs::write(&p, format!(r#"{{"claudeAiOauth":{{"accessToken":"t","expiresAt":{later}}}}}"#)).unwrap();
        assert!(usable_credential(&p).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ask_human_tool_is_a_valid_client_tool() {
        let t = ask_human_tool();
        assert_eq!(t["name"], ASK_HUMAN);
        assert_eq!(t["input_schema"]["required"][0], "reason");
        assert_eq!(serde_json::to_string(&Step::AskHuman { call: "c".into(), reason: "r".into() }).unwrap(), r#"{"type":"ask_human","call":"c","reason":"r"}"#);
    }

    #[tokio::test]
    async fn paused_run_resumes_on_jatka_and_stops_when_closed() {
        let (tx, _rx) = mpsc::channel(4);
        let (rtx, mut rrx) = mpsc::channel(1);
        let waiter = tokio::spawn(async move { Agent::wait_for_human(&tx, &mut rrx).await.is_ok() });
        tokio::task::yield_now().await;
        rtx.send(()).await.unwrap();
        assert!(waiter.await.unwrap());

        // UI closed the stream (Pysäytä): the pause ends with an error.
        let (tx2, rx2) = mpsc::channel::<Step>(4);
        let (_rtx2, mut rrx2) = mpsc::channel(1);
        drop(rx2);
        assert!(Agent::wait_for_human(&tx2, &mut rrx2).await.is_err());
    }

    #[test]
    fn steps_serialize_as_tagged_ndjson() {
        let s = serde_json::to_string(&Step::ToolResult { call: "t1".into(), ok: true, preview: "p".into() }).unwrap();
        assert_eq!(s, r#"{"type":"tool_result","call":"t1","ok":true,"preview":"p"}"#);
    }
}
