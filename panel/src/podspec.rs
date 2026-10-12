//! Browser Pod manifest, per docs/contract.md ("Selainpod").

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{
    Capabilities, Container, ContainerPort, EmptyDirVolumeSource, EnvVar, KeyToPath, Pod, PodSpec, ResourceRequirements,
    SecretVolumeSource, SecurityContext, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::model::{APP, APP_LABEL, DIRECT, EXIT_LABEL, ID_LABEL, PEER_LABEL};

pub const VNC_PORT: u16 = 6080;
pub const MCP_PORT: u16 = 8931;
pub const STATUS_PORT: u16 = 8080;
/// Root-side exit switch in tunnel pods; accepts only non-local clients (the panel).
pub const CONTROL_PORT: u16 = 8081;

pub enum Route {
    Direct,
    Tunnel { exit: String, peer: u32, secret: String, label: Option<String> },
}

pub struct LaunchSpec {
    pub id: String,
    pub image: String,
    pub pull_policy: String,
    pub route: Route,
    pub tz: String,
    pub lang: String,
}

pub fn pod_name(id: &str) -> String {
    format!("hydra-b-{id}")
}

pub fn build(spec: &LaunchSpec) -> Pod {
    let (exit, peer) = match &spec.route {
        Route::Direct => (DIRECT, None),
        Route::Tunnel { exit, peer, .. } => (exit.as_str(), Some(*peer)),
    };
    let mut labels = BTreeMap::from([
        (APP_LABEL.to_string(), APP.to_string()),
        (ID_LABEL.to_string(), spec.id.clone()),
        (EXIT_LABEL.to_string(), exit.to_string()),
    ]);
    if let Some(p) = peer {
        labels.insert(PEER_LABEL.to_string(), p.to_string());
    }

    let label = match &spec.route {
        Route::Tunnel { label: Some(l), .. } => l.as_str(),
        _ => "",
    };
    let env = [("HYDRA_EXIT", exit), ("HYDRA_EXIT_LABEL", label), ("TZ", &spec.tz), ("LANG", &spec.lang)]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(name, value)| EnvVar { name: name.into(), value: Some(value.into()), ..Default::default() })
        .collect();
    let ports = [("vnc", VNC_PORT), ("mcp", MCP_PORT), ("status", STATUS_PORT)]
        .into_iter()
        .map(|(name, port)| ContainerPort { name: Some(name.into()), container_port: port.into(), ..Default::default() })
        .collect();

    let mut volumes = vec![Volume {
        name: "shm".into(),
        empty_dir: Some(EmptyDirVolumeSource { medium: Some("Memory".into()), size_limit: Some(Quantity("1Gi".into())) }),
        ..Default::default()
    }];
    let mut mounts = vec![VolumeMount { name: "shm".into(), mount_path: "/dev/shm".into(), ..Default::default() }];
    let mut security = None;
    if let Route::Tunnel { secret, .. } = &spec.route {
        volumes.push(Volume {
            name: "wg".into(),
            secret: Some(SecretVolumeSource {
                secret_name: Some(secret.clone()),
                items: Some(vec![KeyToPath { key: "wg0.conf".into(), path: "wg0.conf".into(), mode: Some(0o400) }]),
                ..Default::default()
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount { name: "wg".into(), mount_path: "/etc/hydra/wg".into(), read_only: Some(true), ..Default::default() });
        // The current wg0.conf after an exit switch: survives a container
        // restart, so the pod does not fall back to the (freed) mounted peer.
        volumes.push(Volume { name: "net".into(), empty_dir: Some(EmptyDirVolumeSource::default()), ..Default::default() });
        mounts.push(VolumeMount { name: "net".into(), mount_path: "/var/lib/hydra/net".into(), ..Default::default() });
        security = Some(SecurityContext {
            capabilities: Some(Capabilities { add: Some(vec!["NET_ADMIN".into()]), ..Default::default() }),
            ..Default::default()
        });
    }

    let q = |s: &str| Quantity(s.into());
    Pod {
        metadata: ObjectMeta { name: Some(pod_name(&spec.id)), labels: Some(labels), ..Default::default() },
        spec: Some(PodSpec {
            containers: vec![Container {
                name: "browser".into(),
                image: Some(spec.image.clone()),
                image_pull_policy: Some(spec.pull_policy.clone()),
                env: Some(env),
                ports: Some(ports),
                volume_mounts: Some(mounts),
                security_context: security,
                resources: Some(ResourceRequirements {
                    requests: Some(BTreeMap::from([("cpu".into(), q("500m")), ("memory".into(), q("768Mi"))])),
                    limits: Some(BTreeMap::from([("memory".into(), q("2Gi"))])),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            volumes: Some(volumes),
            // The browser never talks to the kube API.
            automount_service_account_token: Some(false),
            enable_service_links: Some(false),
            termination_grace_period_seconds: Some(2),
            ..Default::default()
        }),
        status: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(route: Route) -> LaunchSpec {
        LaunchSpec {
            id: "abc123".into(),
            image: "hydra-browser:latest".into(),
            pull_policy: "IfNotPresent".into(),
            route,
            tz: "Europe/Helsinki".into(),
            lang: "fi_FI.UTF-8".into(),
        }
    }

    fn container(p: &Pod) -> &Container {
        &p.spec.as_ref().unwrap().containers[0]
    }

    fn env(p: &Pod) -> Vec<(String, String)> {
        container(p).env.as_ref().unwrap().iter().map(|e| (e.name.clone(), e.value.clone().unwrap())).collect()
    }

    #[test]
    fn tunnel_pod_mounts_secret_and_gets_net_admin() {
        let p = build(&spec(Route::Tunnel { exit: "purkki".into(), peer: 3, secret: "hydra-exit-purkki-3".into(), label: Some("Kulloo, Finland".into()) }));
        assert_eq!(p.metadata.name.as_deref(), Some("hydra-b-abc123"));
        let labels = p.metadata.labels.as_ref().unwrap();
        assert_eq!(labels[APP_LABEL], "hydra-browser");
        assert_eq!(labels[ID_LABEL], "abc123");
        assert_eq!(labels[EXIT_LABEL], "purkki");
        assert_eq!(labels[PEER_LABEL], "3");

        let c = container(&p);
        assert_eq!(c.image.as_deref(), Some("hydra-browser:latest"));
        assert_eq!(c.image_pull_policy.as_deref(), Some("IfNotPresent"));
        let caps = c.security_context.as_ref().unwrap().capabilities.as_ref().unwrap();
        assert_eq!(caps.add.as_deref(), Some(&["NET_ADMIN".to_string()][..]));
        let wg = c.volume_mounts.as_ref().unwrap().iter().find(|m| m.name == "wg").unwrap();
        assert_eq!((wg.mount_path.as_str(), wg.read_only), ("/etc/hydra/wg", Some(true)));
        let net = c.volume_mounts.as_ref().unwrap().iter().find(|m| m.name == "net").unwrap();
        assert_eq!(net.mount_path, "/var/lib/hydra/net");
        let vol = p.spec.as_ref().unwrap().volumes.as_ref().unwrap().iter().find(|v| v.name == "wg").unwrap();
        assert_eq!(vol.secret.as_ref().unwrap().secret_name.as_deref(), Some("hydra-exit-purkki-3"));
        assert!(env(&p).contains(&("HYDRA_EXIT".into(), "purkki".into())));
        assert!(env(&p).contains(&("TZ".into(), "Europe/Helsinki".into())));
        assert!(env(&p).contains(&("LANG".into(), "fi_FI.UTF-8".into())));
    }

    #[test]
    fn direct_pod_has_no_secret_and_no_capabilities() {
        let p = build(&spec(Route::Direct));
        let labels = p.metadata.labels.as_ref().unwrap();
        assert_eq!(labels[EXIT_LABEL], DIRECT);
        assert!(!labels.contains_key(PEER_LABEL));
        let c = container(&p);
        assert!(c.security_context.is_none());
        assert!(c.volume_mounts.as_ref().unwrap().iter().all(|m| m.name != "wg"));
        assert_eq!(p.spec.as_ref().unwrap().volumes.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn shm_resources_and_ports_follow_contract() {
        let p = build(&spec(Route::Direct));
        let s = p.spec.as_ref().unwrap();
        let shm = s.volumes.as_ref().unwrap().iter().find(|v| v.name == "shm").unwrap();
        assert_eq!(shm.empty_dir.as_ref().unwrap().medium.as_deref(), Some("Memory"));
        let c = container(&p);
        assert!(c.volume_mounts.as_ref().unwrap().iter().any(|m| m.mount_path == "/dev/shm"));
        let r = c.resources.as_ref().unwrap();
        assert_eq!(r.requests.as_ref().unwrap()["cpu"].0, "500m");
        assert_eq!(r.requests.as_ref().unwrap()["memory"].0, "768Mi");
        assert_eq!(r.limits.as_ref().unwrap()["memory"].0, "2Gi");
        let ports: Vec<i32> = c.ports.as_ref().unwrap().iter().map(|p| p.container_port).collect();
        assert_eq!(ports, [6080, 8931, 8080]);
        assert_eq!(s.automount_service_account_token, Some(false));
    }
}
