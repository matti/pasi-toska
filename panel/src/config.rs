//! Settings from the environment (see docs/panel.md for the list).

use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub namespace: String,
    pub max_browsers: usize,
    pub image: String,
    pub pull_policy: String,
    /// RuntimeClass for browser pods (e.g. "kata" for microVMs); unset = cluster default.
    pub runtime_class: Option<String>,
    pub memory_limit: String,
    pub cpu_limit: Option<String>,
    pub direct_tz: String,
    pub direct_lang: String,
    /// Overrides the looked-up public IP of the node.
    pub node_ip: Option<String>,
    /// Opened in every browser once it is ready, and by the "IP-testi" button.
    pub ip_test_url: String,
    /// Opened by the "Sormenjälkitesti" button.
    pub fp_test_url: String,
    /// Claude subscription credential (Secret hydra-claude), read on every agent run.
    pub claude_credentials: std::path::PathBuf,
    pub agent_model: String,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn or(name: &str, default: &str) -> String {
    var(name).unwrap_or_else(|| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            listen: or("HYDRA_LISTEN", "0.0.0.0:8080").parse()?,
            namespace: or("HYDRA_NAMESPACE", "hydra"),
            max_browsers: or("HYDRA_MAX_BROWSERS", "2").parse()?,
            image: or("HYDRA_BROWSER_IMAGE", "hydra-browser:latest"),
            pull_policy: or("HYDRA_BROWSER_PULL_POLICY", "IfNotPresent"),
            runtime_class: var("HYDRA_RUNTIME_CLASS"),
            memory_limit: or("HYDRA_BROWSER_MEMORY_LIMIT", "2Gi"),
            cpu_limit: var("HYDRA_BROWSER_CPU_LIMIT"),
            direct_tz: or("HYDRA_DIRECT_TZ", "Europe/Helsinki"),
            direct_lang: or("HYDRA_DIRECT_LANG", "fi_FI.UTF-8"),
            node_ip: var("HYDRA_NODE_IP"),
            ip_test_url: or("HYDRA_IP_TEST_URL", "http://127.0.0.1:8080/ip"),
            fp_test_url: or("HYDRA_FP_TEST_URL", "https://deviceandbrowserinfo.com/are_you_a_bot"),
            claude_credentials: or("HYDRA_CLAUDE_CREDENTIALS", "/etc/hydra/claude/credentials.json").into(),
            agent_model: or("HYDRA_AGENT_MODEL", "claude-opus-5-5"),
        })
    }
}
