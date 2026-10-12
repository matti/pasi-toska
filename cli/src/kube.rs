//! Exit registry in kube (docs/contract.md): one Secret per peer in
//! namespace `hydra`, applied with kubectl (`$HYDRA_KUBECTL` overrides).

use crate::capture;
use crate::exit::{Meta, Peer};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::process::Command;

pub const NAMESPACE: &str = "hydra";

pub fn manifest(meta: &Meta, peers: &[Peer]) -> Value {
    let mut annotations = Map::new();
    annotations.insert("hydra/ip".into(), meta.public_ip.clone().into());
    for (key, value) in [
        ("label", &meta.label),
        ("country", &meta.country),
        ("city", &meta.city),
        ("tz", &meta.tz),
        ("lang", &meta.lang),
    ] {
        if let Some(v) = value {
            annotations.insert(format!("hydra/{key}"), v.clone().into());
        }
    }
    let mut items = vec![json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}})];
    items.extend(peers.iter().map(|p| {
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "type": "Opaque",
            "metadata": {
                "name": format!("hydra-exit-{}-{}", meta.name, p.peer),
                "namespace": NAMESPACE,
                "labels": {"hydra/exit": meta.name, "hydra/peer": p.peer.to_string()},
                "annotations": annotations,
            },
            "stringData": {"wg0.conf": p.conf},
        })
    }));
    json!({"apiVersion": "v1", "kind": "List", "items": items})
}

pub fn apply_secrets(kubeconfig: &Path, meta: &Meta, peers: &[Peer]) -> Result<()> {
    apply(kubeconfig, &manifest(meta, peers))
}

/// Secret `hydra-claude` (key `credentials.json`): the Claude subscription
/// token the panel's agent uses.
pub fn claude_manifest(credentials_json: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": "Opaque",
        "metadata": {"name": "hydra-claude", "namespace": NAMESPACE},
        "stringData": {"credentials.json": credentials_json},
    })
}

/// `kubectl apply --server-side`: client-side apply would copy the whole
/// Secret, keys included, into the last-applied-configuration annotation.
pub fn apply(kubeconfig: &Path, manifest: &Value) -> Result<()> {
    let kubectl = std::env::var("HYDRA_KUBECTL").unwrap_or_else(|_| "kubectl".into());
    let mut cmd = Command::new(kubectl);
    cmd.arg("--kubeconfig").arg(kubeconfig);
    cmd.args(["apply", "--server-side", "--field-manager=hydra", "--force-conflicts", "-f", "-"]);
    let out = capture(cmd, Some(manifest.to_string().as_bytes()))?;
    eprint!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_follow_the_contract() {
        let meta = Meta {
            name: "purkki".into(),
            ssh_host: "root@purkki".into(),
            endpoint: "100.111.221.2:51820".into(),
            public_ip: "203.0.113.7".into(),
            label: Some("Kulloo, Finland".into()),
            country: Some("FI".into()),
            city: Some("Helsinki".into()),
            tz: Some("Europe/Helsinki".into()),
            lang: None,
            peers: 2,
        };
        let peers = vec![Peer { peer: 0, conf: "A".into() }, Peer { peer: 1, conf: "B".into() }];
        let m = manifest(&meta, &peers);
        let items = m["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["kind"], "Namespace");
        let s = &items[2];
        assert_eq!(s["metadata"]["name"], "hydra-exit-purkki-1");
        assert_eq!(s["metadata"]["namespace"], "hydra");
        assert_eq!(s["metadata"]["labels"]["hydra/exit"], "purkki");
        assert_eq!(s["metadata"]["labels"]["hydra/peer"], "1");
        let ann = &s["metadata"]["annotations"];
        assert_eq!(ann["hydra/ip"], "203.0.113.7");
        assert_eq!(ann["hydra/country"], "FI");
        assert_eq!(ann["hydra/label"], "Kulloo, Finland");
        assert_eq!(ann["hydra/tz"], "Europe/Helsinki");
        assert!(ann.get("hydra/lang").is_none());
        assert_eq!(s["stringData"]["wg0.conf"], "B");
    }
}
