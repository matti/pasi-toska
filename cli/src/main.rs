//! hydra: puts a hydra-exit (WireGuard egress container) on a docker host
//! over ssh and registers its peers locally and as kube Secrets.
//! Browsers themselves are plain containers (images/browser); see docs/.

mod exit;
mod kube;
mod login;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "hydra", version, about = "Exit machines and Claude login for hydra browsers")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage exits
    #[command(subcommand)]
    Exit(ExitCmd),
    /// Copy the Claude subscription token from the macOS Keychain into Secret hydra-claude
    Login {
        /// kubeconfig (default: .secrets/kubeconfig)
        #[arg(long, env = "KUBECONFIG")]
        kubeconfig: Option<PathBuf>,
        /// Keychain item to use (default: "Claude Code-credentials", else the freshest Max one)
        #[arg(long)]
        item: Option<String>,
        /// Also install a launchd agent that repeats this every 10 minutes
        #[arg(long)]
        install: bool,
    },
}

#[derive(Subcommand)]
enum ExitCmd {
    /// Ship and run hydra-exit on <ssh-host>, store its peer configs (idempotent)
    Add {
        name: String,
        /// ssh destination with docker access, e.g. root@purkki
        ssh_host: String,
        /// Host[:port] browsers connect to, e.g. a public or Tailscale IP
        #[arg(long)]
        endpoint: String,
        /// UDP port published on the exit host
        #[arg(long, default_value_t = 51820)]
        port: u16,
        /// Peer pool size = concurrent browsers
        #[arg(long, default_value_t = 8)]
        peers: u8,
        /// UI display name, e.g. "Kulloo, Finland"
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        country: Option<String>,
        #[arg(long)]
        city: Option<String>,
        /// Browser timezone matching the exit, e.g. Europe/Helsinki
        #[arg(long)]
        tz: Option<String>,
        /// Browser locale matching the exit, e.g. fi_FI.UTF-8
        #[arg(long)]
        lang: Option<String>,
        /// Write kube Secrets (namespace hydra) with this kubeconfig
        #[arg(long, env = "KUBECONFIG")]
        kubeconfig: Option<PathBuf>,
    },
    /// List exits stored locally
    Ls,
}

/// Runs `cmd` (optionally feeding `input`), returns stdout; fails with stderr.
pub fn capture(mut cmd: Command, input: Option<&[u8]>) -> Result<String> {
    let name = format!("{:?}", cmd.get_program());
    let mut child = cmd
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {name}"))?;
    if let Some(input) = input {
        child.stdin.take().context("stdin")?.write_all(input)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("{name} failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.len());
        }
    }
    let line = |cells: Vec<&str>| {
        let padded: Vec<String> = cells.iter().zip(&widths).map(|(c, w)| format!("{c:<w$}")).collect();
        println!("{}", padded.join("  ").trim_end());
    };
    line(headers.to_vec());
    for row in rows {
        line(row.iter().map(String::as_str).collect());
    }
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Exit(ExitCmd::Add {
            name,
            ssh_host,
            endpoint,
            port,
            peers,
            label,
            country,
            city,
            tz,
            lang,
            kubeconfig,
        }) => {
            let meta = exit::Meta {
                name,
                ssh_host,
                endpoint,
                public_ip: String::new(),
                label,
                country,
                city,
                tz,
                lang,
                peers,
            };
            exit::add(meta, port, kubeconfig)
        }
        Cmd::Exit(ExitCmd::Ls) => exit::ls(),
        Cmd::Login { kubeconfig, item, install } => login::login(kubeconfig, item, install),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_add_parses_with_defaults() {
        let cli = Cli::try_parse_from(["hydra", "exit", "add", "purkki", "root@purkki", "--endpoint", "100.111.221.2"])
            .unwrap();
        let Cmd::Exit(ExitCmd::Add { port, peers, endpoint, .. }) = cli.cmd else { panic!("not exit add") };
        assert_eq!((port, peers, endpoint.as_str()), (51820, 8, "100.111.221.2"));
    }

    #[test]
    fn endpoint_is_required() {
        assert!(Cli::try_parse_from(["hydra", "exit", "add", "purkki", "root@purkki"]).is_err());
    }

    #[test]
    fn capture_returns_stdout_and_feeds_stdin() {
        let out = capture(Command::new("cat"), Some(b"hello")).unwrap();
        assert_eq!(out, "hello");
        assert!(capture(Command::new("false"), None).is_err());
    }
}
