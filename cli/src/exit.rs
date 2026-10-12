//! `hydra exit add`: build the hydra-exit image for the host's arch, ship it
//! over ssh only if the remote copy differs, run it (cli/remote-up.sh), and
//! store the peer configs under the state dir (and as kube Secrets).

use crate::{capture, kube, print_table};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REMOTE_UP: &str = include_str!("../remote-up.sh");
const EXIT_IMAGE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../images/exit");

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Meta {
    pub name: String,
    pub ssh_host: String,
    pub endpoint: String,
    pub public_ip: String,
    /// UI display name, e.g. "Kulloo, Finland" (annotation hydra/label).
    #[serde(default)]
    pub label: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub tz: Option<String>,
    pub lang: Option<String>,
    pub peers: u8,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct Peer {
    pub peer: u8,
    pub conf: String,
}

/// `HYDRA_STATE`, else `.secrets/` at the repo root (gitignored).
pub fn state_dir() -> PathBuf {
    std::env::var_os("HYDRA_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.secrets")))
}

/// Exit names end up in kube object names (`hydra-exit-<name>-<n>`).
pub fn validate_name(name: &str) -> Result<()> {
    let ok = (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if !ok || name == "direct" {
        bail!("invalid exit name '{name}': 1-32 of a-z, 0-9, '-' (not at the ends), not 'direct'");
    }
    Ok(())
}

fn with_port(endpoint: &str, port: u16) -> Result<String> {
    if endpoint.is_empty() || !endpoint.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b)) {
        bail!("invalid --endpoint '{endpoint}': host or IPv4, optional :port");
    }
    Ok(if endpoint.contains(':') { endpoint.to_string() } else { format!("{endpoint}:{port}") })
}

fn docker_platform(uname_m: &str) -> Result<&'static str> {
    match uname_m.trim() {
        "x86_64" | "amd64" => Ok("linux/amd64"),
        "aarch64" | "arm64" => Ok("linux/arm64"),
        other => bail!("unsupported exit host architecture '{other}'"),
    }
}

/// `$HYDRA_SSH` (e.g. "ssh -F cfg") or plain ssh, with `args` appended.
fn ssh(host: &str, args: &[&str]) -> Command {
    let spec = std::env::var("HYDRA_SSH").unwrap_or_else(|_| "ssh".into());
    let mut parts = spec.split_whitespace();
    let mut cmd = Command::new(parts.next().unwrap_or("ssh"));
    cmd.args(parts).arg(host).args(args);
    cmd
}

fn remote_env(image: &str, port: u16, peers: u8, endpoint: &str) -> String {
    format!("HYDRA_IMAGE={image} HYDRA_PORT={port} HYDRA_PEERS={peers} HYDRA_ENDPOINT={endpoint} sh -s")
}

/// Splits remote-up.sh output into (public IP, peers).
fn parse_remote(out: &str) -> Result<(String, Vec<Peer>)> {
    let ip = out.lines().find_map(|l| l.strip_prefix("HYDRA_PUBLIC_IP=")).context("no HYDRA_PUBLIC_IP in output")?;
    let json = out.lines().find(|l| l.starts_with('[')).context("no peer JSON in output")?;
    let peers: Vec<Peer> = serde_json::from_str(json).context("parse peer JSON")?;
    if peers.is_empty() || peers.iter().any(|p| !p.conf.contains("[Peer]")) {
        bail!("peer JSON has no usable configs");
    }
    Ok((ip.trim().to_string(), peers))
}

/// Builds the exit image for `platform` and copies it to `host` unless the
/// remote already has the same image id.
fn ship_image(host: &str, platform: &str) -> Result<String> {
    let tag = format!("hydra-exit:{}", platform.trim_start_matches("linux/"));
    eprintln!("-- building {tag} ({platform})");
    let mut build = Command::new("docker");
    // No provenance/SBOM: they make the image id change on every build, and
    // the id decides whether the image is copied and the container recreated.
    build.args(["buildx", "build", "-q", "--provenance=false", "--sbom=false", "--platform", platform, "--load"]);
    build.args(["-t", &tag, EXIT_IMAGE_DIR]);
    capture(build, None)?;
    let mut inspect = Command::new("docker");
    inspect.args(["image", "inspect", "-f", "{{.Id}}", &tag]);
    let local_id = capture(inspect, None)?;
    let remote_id =
        capture(ssh(host, &["docker", "image", "inspect", "-f", "'{{.Id}}'", &tag]), None).unwrap_or_default();
    if local_id.trim() == remote_id.trim() {
        eprintln!("-- {host} already has {tag}");
        return Ok(tag);
    }
    eprintln!("-- copying {tag} to {host}");
    let mut save = Command::new("docker").args(["save", &tag]).stdout(Stdio::piped()).spawn()?;
    let load = ssh(host, &["docker", "load"])
        .stdin(save.stdout.take().context("docker save stdout")?)
        .stdout(Stdio::null())
        .status()?;
    if !save.wait()?.success() || !load.success() {
        bail!("copying {tag} to {host} failed");
    }
    Ok(tag)
}

pub fn write_secret(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(content.as_bytes())?;
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}

fn save_local(meta: &Meta, peers: &[Peer]) -> Result<PathBuf> {
    let dir = state_dir().join("exits").join(&meta.name);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    for p in peers {
        write_secret(&dir.join(format!("peer-{}.conf", p.peer)), &p.conf)?;
    }
    write_secret(&dir.join("exit.json"), &serde_json::to_string_pretty(meta)?)?;
    Ok(dir)
}

pub fn add(mut meta: Meta, port: u16, kubeconfig: Option<PathBuf>) -> Result<()> {
    validate_name(&meta.name)?;
    meta.endpoint = with_port(&meta.endpoint, port)?;
    if meta.peers == 0 {
        bail!("--peers must be at least 1");
    }
    let platform = docker_platform(&capture(ssh(&meta.ssh_host, &["uname", "-m"]), None)?)?;
    let image = ship_image(&meta.ssh_host, platform)?;
    eprintln!("-- starting hydra-exit on {}", meta.ssh_host);
    let remote = ssh(&meta.ssh_host, &[&remote_env(&image, port, meta.peers, &meta.endpoint)]);
    let (public_ip, peers) = parse_remote(&capture(remote, Some(REMOTE_UP.as_bytes()))?)?;
    meta.public_ip = public_ip;
    let dir = save_local(&meta, &peers)?;
    match kubeconfig {
        Some(kc) => kube::apply_secrets(&kc, &meta, &peers)?,
        None => eprintln!("-- no --kubeconfig/KUBECONFIG: kube Secrets skipped"),
    }
    println!(
        "exit '{}' ready: public IP {}, endpoint {}, {} peers, configs in {}",
        meta.name,
        meta.public_ip,
        meta.endpoint,
        peers.len(),
        dir.display()
    );
    Ok(())
}

pub fn ls() -> Result<()> {
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(state_dir().join("exits")) {
        for entry in entries {
            let path = entry?.path().join("exit.json");
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let m: Meta = serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
            let opt = |o: Option<String>| o.unwrap_or_else(|| "-".into());
            rows.push(vec![
                m.name,
                m.public_ip,
                m.endpoint,
                m.peers.to_string(),
                opt(m.country),
                opt(m.city),
                opt(m.tz),
                opt(m.lang),
                m.ssh_host,
            ]);
        }
    }
    rows.sort();
    print_table(&["EXIT", "PUBLIC IP", "ENDPOINT", "PEERS", "COUNTRY", "CITY", "TZ", "LANG", "SSH"], &rows);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["purkki", "hel-1", "a"] {
            validate_name(ok).unwrap();
        }
        for bad in ["", "Purkki", "a_b", "-a", "a-", "direct", &"x".repeat(33), "a;b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn endpoint_gets_default_port_and_rejects_junk() {
        assert_eq!(with_port("100.111.221.2", 51820).unwrap(), "100.111.221.2:51820");
        assert_eq!(with_port("exit.example.com:443", 51820).unwrap(), "exit.example.com:443");
        assert!(with_port("a b", 51820).is_err());
        assert!(with_port("x;rm -rf /", 51820).is_err());
        assert!(with_port("", 51820).is_err());
    }

    #[test]
    fn platforms() {
        assert_eq!(docker_platform("x86_64\n").unwrap(), "linux/amd64");
        assert_eq!(docker_platform("aarch64").unwrap(), "linux/arm64");
        assert!(docker_platform("riscv64").is_err());
    }

    #[test]
    fn remote_env_line() {
        assert_eq!(
            remote_env("hydra-exit:amd64", 51820, 8, "100.111.221.2:51820"),
            "HYDRA_IMAGE=hydra-exit:amd64 HYDRA_PORT=51820 HYDRA_PEERS=8 HYDRA_ENDPOINT=100.111.221.2:51820 sh -s"
        );
    }

    #[test]
    fn parses_remote_output() {
        let out = "HYDRA_PUBLIC_IP=203.0.113.7\n[{\"peer\":0,\"conf\":\"[Interface]\\n[Peer]\\n\"},{\"peer\":1,\"conf\":\"[Peer]\"}]\n";
        let (ip, peers) = parse_remote(out).unwrap();
        assert_eq!(ip, "203.0.113.7");
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[1], Peer { peer: 1, conf: "[Peer]".into() });
        assert!(parse_remote("HYDRA_PUBLIC_IP=1.2.3.4\n[]").is_err());
        assert!(parse_remote("[{\"peer\":0,\"conf\":\"[Peer]\"}]").is_err());
    }

    #[test]
    fn local_state_is_private() {
        let dir = std::env::temp_dir().join(format!("hydra-test-{}", std::process::id()));
        let file = dir.join("x.conf");
        std::fs::create_dir_all(&dir).unwrap();
        write_secret(&file, "secret").unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
