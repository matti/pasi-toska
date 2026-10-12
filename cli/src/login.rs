//! `hydra login`: copies the Claude subscription token from the macOS
//! Keychain (Claude Code's credentials) into kube Secret `hydra-claude`, which
//! the panel's agent reads. `--install` keeps it fresh with a launchd agent.
//! The token itself is never printed.

use crate::exit::{state_dir, write_secret};
use crate::{capture, kube};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const ITEM: &str = "Claude Code-credentials";
/// The cluster copy must outlive the next refresh with a margin.
const MIN_MINUTES: i64 = 60;
const LABEL: &str = "dev.hydra.login";
const INTERVAL_SECS: u32 = 600;

#[derive(Debug, PartialEq)]
struct Credential {
    item: String,
    token: String,
    expires_at_ms: i64,
    subscription: String,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn parse(item: &str, json_text: &str) -> Option<Credential> {
    let v: Value = serde_json::from_str(json_text.trim()).ok()?;
    let o = &v["claudeAiOauth"];
    Some(Credential {
        item: item.to_string(),
        token: o["accessToken"].as_str().filter(|t| !t.is_empty())?.to_string(),
        expires_at_ms: o["expiresAt"].as_i64()?,
        subscription: o["subscriptionType"].as_str().unwrap_or("").to_string(),
    })
}

/// Item names in the login Keychain: "Claude Code-credentials" and the
/// per-config-dir variants "Claude Code-credentials-<hash>".
fn keychain_items(dump: &str) -> Vec<String> {
    let mut names: Vec<String> = dump
        .lines()
        .filter_map(|l| l.trim().strip_prefix("\"svce\"<blob>=\"")?.strip_suffix('"').map(String::from))
        .filter(|n| n == ITEM || n.starts_with(&format!("{ITEM}-")))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The plain item if it is good for MIN_MINUTES, else the freshest Max
/// (then any) subscription credential.
fn choose(mut creds: Vec<Credential>, now: i64) -> Option<Credential> {
    let ok = |c: &Credential| (c.expires_at_ms - now) / 60_000 >= MIN_MINUTES;
    if let Some(i) = creds.iter().position(|c| c.item == ITEM && ok(c)) {
        return Some(creds.swap_remove(i));
    }
    creds.into_iter().filter(ok).max_by_key(|c| (c.subscription == "max", c.expires_at_ms))
}

fn read_item(name: &str) -> Option<Credential> {
    let mut cmd = Command::new("security");
    cmd.args(["find-generic-password", "-s", name, "-w"]);
    parse(name, &capture(cmd, None).ok()?)
}

fn find_credential(item: Option<&str>) -> Result<Credential> {
    let names = match item {
        Some(n) => vec![n.to_string()],
        None => {
            let mut cmd = Command::new("security");
            cmd.arg("dump-keychain");
            keychain_items(&capture(cmd, None).context("cannot read the macOS Keychain")?)
        }
    };
    let creds: Vec<Credential> = names.iter().filter_map(|n| read_item(n)).collect();
    let best = creds.iter().map(|c| (c.expires_at_ms - now_ms()) / 60_000).max();
    choose(creds, now_ms()).with_context(|| match best {
        None => "no Claude Code credentials in the Keychain: log in with `claude` first".to_string(),
        Some(m) => {
            format!("Claude token expires in {m} min (need {MIN_MINUTES}): run `claude` once to refresh it, then retry")
        }
    })
}

/// What goes to the cluster: token and expiry only, never the refresh token.
fn secret_payload(c: &Credential) -> String {
    json!({"claudeAiOauth": {"accessToken": c.token, "expiresAt": c.expires_at_ms, "subscriptionType": c.subscription}})
        .to_string()
}

fn default_kubeconfig(kubeconfig: Option<PathBuf>) -> Result<PathBuf> {
    let path = kubeconfig.unwrap_or_else(|| state_dir().join("kubeconfig"));
    if !path.exists() {
        bail!("kubeconfig {} not found (use --kubeconfig)", path.display());
    }
    Ok(std::fs::canonicalize(path)?)
}

pub fn login(kubeconfig: Option<PathBuf>, item: Option<String>, install: bool) -> Result<()> {
    let kubeconfig = default_kubeconfig(kubeconfig)?;
    let cred = find_credential(item.as_deref())?;
    kube::apply(&kubeconfig, &kube::claude_manifest(&secret_payload(&cred)))?;
    let m = (cred.expires_at_ms - now_ms()) / 60_000;
    println!("hydra-claude updated from '{}' ({}), valid {} h {} min", cred.item, cred.subscription, m / 60, m % 60);
    if install {
        install_agent(&kubeconfig)?;
    }
    Ok(())
}

fn plist(exe: &Path, kubeconfig: &Path, log: &Path) -> String {
    let esc = |p: &Path| p.display().to_string().replace('&', "&amp;").replace('<', "&lt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>login</string><string>--kubeconfig</string><string>{}</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
  <key>StartInterval</key><integer>{INTERVAL_SECS}</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        esc(exe),
        esc(kubeconfig),
        esc(log),
        esc(log)
    )
}

/// Copies this binary to ~/.local/bin/hydra and (re)loads a launchd agent
/// that runs `hydra login` every 10 minutes.
fn install_agent(kubeconfig: &Path) -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME not set")?);
    let bin = home.join(".local/bin/hydra");
    std::fs::create_dir_all(bin.parent().unwrap())?;
    let exe = std::env::current_exe()?;
    if std::fs::canonicalize(&exe).ok() != std::fs::canonicalize(&bin).ok() {
        let tmp = bin.with_extension("new");
        std::fs::copy(&exe, &tmp)?;
        std::fs::rename(&tmp, &bin)?;
    }
    let agents = home.join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents)?;
    let plist_path = agents.join(format!("{LABEL}.plist"));
    write_secret(&plist_path, &plist(&bin, kubeconfig, &home.join("Library/Logs/hydra-login.log")))?;
    let mut id = Command::new("id");
    id.arg("-u");
    let uid = capture(id, None)?.trim().to_string();
    let domain = format!("gui/{uid}");
    let _ = Command::new("launchctl").args(["bootout", &format!("{domain}/{LABEL}")]).output();
    let mut boot = Command::new("launchctl");
    boot.args(["bootstrap", &domain]).arg(&plist_path);
    capture(boot, None)?;
    println!("launchd agent {LABEL} installed: {} every {} min", bin.display(), INTERVAL_SECS / 60);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(item: &str, mins: i64, sub: &str) -> Credential {
        Credential {
            item: item.into(),
            token: "t".into(),
            expires_at_ms: 1_000_000_000_000 + mins * 60_000,
            subscription: sub.into(),
        }
    }

    #[test]
    fn parses_keychain_json_and_rejects_empty_tokens() {
        let c = parse(
            ITEM,
            r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":5,"subscriptionType":"max","refreshToken":"r"}}"#,
        );
        assert_eq!(
            c,
            Some(Credential { item: ITEM.into(), token: "abc".into(), expires_at_ms: 5, subscription: "max".into() })
        );
        assert_eq!(parse(ITEM, r#"{"claudeAiOauth":{"accessToken":"","expiresAt":0}}"#), None);
        assert_eq!(parse(ITEM, "not json"), None);
    }

    #[test]
    fn lists_claude_items_from_keychain_dump() {
        let dump = "    \"svce\"<blob>=\"Claude Code-credentials-0a\"\n    \"svce\"<blob>=\"Other\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n    \"svce\"<blob>=\"Claude Code-credentials-0a\"\n    \"svce\"<blob>=\"Claude Code-credentialsX\"";
        assert_eq!(keychain_items(dump), [ITEM.to_string(), format!("{ITEM}-0a")]);
    }

    #[test]
    fn prefers_plain_item_then_freshest_max() {
        let now = 1_000_000_000_000;
        let pick = |v: Vec<Credential>| choose(v, now).map(|c| c.item);
        assert_eq!(
            pick(vec![cred("Claude Code-credentials-a", 300, "max"), cred(ITEM, 90, "max")]).as_deref(),
            Some(ITEM)
        );
        assert_eq!(
            pick(vec![
                cred(ITEM, 30, "max"),
                cred("x-pro", 500, "pro"),
                cred("x-max", 400, "max"),
                cred("y-max", 200, "max")
            ])
            .as_deref(),
            Some("x-max")
        );
        assert_eq!(pick(vec![cred(ITEM, 59, "max")]), None);
    }

    #[test]
    fn payload_has_no_refresh_token() {
        let p = secret_payload(&cred(ITEM, 100, "max"));
        assert!(p.contains("accessToken") && p.contains("expiresAt") && !p.contains("refresh"));
    }

    #[test]
    fn plist_runs_login_every_ten_minutes() {
        let p = plist(Path::new("/u/.local/bin/hydra"), Path::new("/r/.secrets/kubeconfig"), Path::new("/u/l.log"));
        assert!(p.contains("<string>/u/.local/bin/hydra</string><string>login</string><string>--kubeconfig</string><string>/r/.secrets/kubeconfig</string>"));
        assert!(p.contains("<key>StartInterval</key><integer>600</integer>"));
        assert!(p.contains(LABEL));
    }
}
