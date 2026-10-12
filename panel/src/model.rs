//! Pure view logic: kube objects + pod statuses -> the JSON snapshot the UI renders.

use std::collections::{BTreeMap, HashMap, HashSet};

use k8s_openapi::api::core::v1::{Pod, Secret};
use serde::{Deserialize, Serialize};

pub const APP_LABEL: &str = "app";
pub const APP: &str = "hydra-browser";
pub const ID_LABEL: &str = "hydra/id";
pub const EXIT_LABEL: &str = "hydra/exit";
pub const PEER_LABEL: &str = "hydra/peer";
pub const DIRECT: &str = "direct";

const ANN_COUNTRY: &str = "hydra/country";
const ANN_CITY: &str = "hydra/city";
const ANN_TZ: &str = "hydra/tz";
const ANN_LANG: &str = "hydra/lang";
const ANN_IP: &str = "hydra/ip";
/// Display name shown in the UI instead of the exit name and ipinfo's city.
const ANN_LABEL: &str = "hydra/label";

/// Public egress as reported by ipinfo-style lookups.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Egress {
    pub ip: String,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Body of the browser pod's `GET :8080/status`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PodStatus {
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub exit: Option<String>,
    #[serde(default)]
    pub egress: Option<Egress>,
}

/// One exit with its peer pool, built from the `hydra/exit` Secrets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Exit {
    pub name: String,
    pub label: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub tz: Option<String>,
    pub lang: Option<String>,
    pub ip: Option<String>,
    /// peer number -> Secret name
    #[serde(skip)]
    pub peers: BTreeMap<u32, String>,
    pub peers_total: usize,
    pub peers_free: usize,
}

impl Exit {
    /// Lowest peer not held by any pod.
    pub fn free_peer(&self, used: &HashSet<(String, u32)>) -> Option<(u32, &str)> {
        self.peers
            .iter()
            .find(|(n, _)| !used.contains(&(self.name.clone(), **n)))
            .map(|(n, s)| (*n, s.as_str()))
    }
}

fn label<'a>(meta: &'a k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta, key: &str) -> Option<&'a str> {
    meta.labels.as_ref()?.get(key).map(String::as_str)
}

fn annotation(meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta, key: &str) -> Option<String> {
    meta.annotations.as_ref()?.get(key).filter(|v| !v.is_empty()).cloned()
}

/// (exit, peer) pairs held by existing pods, terminating ones included: a peer
/// must not be reused before the old tunnel is gone.
pub fn used_peers<'a>(pods: impl IntoIterator<Item = &'a Pod>) -> HashSet<(String, u32)> {
    pods.into_iter()
        .filter_map(|p| {
            let exit = label(&p.metadata, EXIT_LABEL)?;
            let peer = label(&p.metadata, PEER_LABEL)?.parse().ok()?;
            Some((exit.to_string(), peer))
        })
        .collect()
}

pub fn exits<'a>(secrets: impl IntoIterator<Item = &'a Secret>, used: &HashSet<(String, u32)>) -> Vec<Exit> {
    let mut by_name: BTreeMap<String, Exit> = BTreeMap::new();
    for s in secrets {
        let m = &s.metadata;
        let (Some(name), Some(secret)) = (label(m, EXIT_LABEL), m.name.as_ref()) else { continue };
        let Some(peer) = label(m, PEER_LABEL).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if name == DIRECT {
            continue;
        }
        let e = by_name.entry(name.to_string()).or_insert_with(|| Exit { name: name.to_string(), ..Default::default() });
        for (field, key) in [
            (&mut e.label, ANN_LABEL),
            (&mut e.country, ANN_COUNTRY),
            (&mut e.city, ANN_CITY),
            (&mut e.tz, ANN_TZ),
            (&mut e.lang, ANN_LANG),
            (&mut e.ip, ANN_IP),
        ] {
            if field.is_none() {
                *field = annotation(m, key);
            }
        }
        e.peers.insert(peer, secret.clone());
    }
    by_name
        .into_values()
        .map(|mut e| {
            e.peers_total = e.peers.len();
            e.peers_free = e.peers.keys().filter(|n| !used.contains(&(e.name.clone(), **n))).count();
            e
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Pending,
    Starting,
    Ready,
    Stopping,
    Error,
}

/// Lifecycle phase from the pod object and (when reachable) its `/status`.
pub fn phase(pod: &Pod, status: Option<&PodStatus>) -> (Phase, Option<String>) {
    if pod.metadata.deletion_timestamp.is_some() {
        return (Phase::Stopping, None);
    }
    let st = pod.status.as_ref();
    let container = st.and_then(|s| s.container_statuses.as_ref()).and_then(|c| c.first());
    let waiting = container.and_then(|c| c.state.as_ref()).and_then(|s| s.waiting.as_ref());
    if let Some(w) = waiting {
        let reason = w.reason.clone().unwrap_or_default();
        if matches!(
            reason.as_str(),
            "ErrImagePull" | "ImagePullBackOff" | "ErrImageNeverPull" | "InvalidImageName" | "CrashLoopBackOff" | "CreateContainerConfigError" | "CreateContainerError"
        ) {
            let msg = w.message.as_deref().map(|m| format!("{reason}: {m}")).unwrap_or(reason);
            return (Phase::Error, Some(msg));
        }
    }
    match st.and_then(|s| s.phase.as_deref()) {
        Some("Running") if status.is_some_and(|s| s.ready) => (Phase::Ready, None),
        Some("Running") => (Phase::Starting, None),
        Some("Failed") | Some("Succeeded") => {
            let term = container.and_then(|c| c.state.as_ref()).and_then(|s| s.terminated.as_ref());
            let reason = term.and_then(|t| t.reason.clone()).or_else(|| st.and_then(|s| s.reason.clone()));
            (Phase::Error, Some(reason.unwrap_or_else(|| "Pod päättyi".into())))
        }
        _ => {
            let unschedulable = st
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "PodScheduled" && c.status == "False"))
                .and_then(|c| c.message.clone());
            let detail = unschedulable.or_else(|| waiting.and_then(|w| w.reason.clone()));
            (Phase::Pending, detail)
        }
    }
}

/// Does the observed egress prove the tunnel works?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Egress equals the exit's public IP.
    Tunnel,
    /// Exit chosen but egress equals the node's own IP.
    Leak,
    /// Direct browser, egress is the node's IP by design.
    Direct,
    /// Egress known but matches neither IP (exit IP not annotated, or changed).
    Unverified,
    /// No egress yet.
    Unknown,
}

pub fn verdict(exit: &str, exit_ip: Option<&str>, node_ip: Option<&str>, egress_ip: Option<&str>) -> Verdict {
    let Some(egress) = egress_ip else { return Verdict::Unknown };
    if exit == DIRECT {
        Verdict::Direct
    } else if node_ip == Some(egress) {
        Verdict::Leak
    } else if exit_ip == Some(egress) {
        Verdict::Tunnel
    } else {
        Verdict::Unverified
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Browser {
    pub id: String,
    pub exit: String,
    pub peer: Option<u32>,
    pub exit_label: Option<String>,
    pub exit_country: Option<String>,
    pub exit_city: Option<String>,
    pub exit_ip: Option<String>,
    /// Kube node the pod runs on.
    pub node: Option<String>,
    pub phase: Phase,
    pub detail: Option<String>,
    /// RFC 3339 creation time; the UI computes uptime from it.
    pub created: Option<String>,
    pub egress: Option<Egress>,
    pub verdict: Verdict,
    /// noVNC page through the panel proxy.
    pub vnc: String,
    /// MCP endpoint path through the panel proxy.
    pub mcp: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClusterInfo {
    pub ok: bool,
    pub error: Option<String>,
    pub namespace: String,
    pub node: Option<Egress>,
    pub max_browsers: usize,
    /// Minutes left on the Claude subscription token; None without one (agent hidden).
    pub agent_minutes: Option<i64>,
    /// Page opened by the "IP-testi" button.
    pub ip_test: String,
    /// Bot-detection page opened by the "Sormenjälkitesti" button.
    pub fp_test: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub cluster: ClusterInfo,
    pub exits: Vec<Exit>,
    pub browsers: Vec<Browser>,
}

pub fn pod_id(pod: &Pod) -> Option<&str> {
    label(&pod.metadata, ID_LABEL)
}

pub fn browsers(pods: &[&Pod], exits: &[Exit], statuses: &HashMap<String, PodStatus>, node_ip: Option<&str>) -> Vec<Browser> {
    let mut out: Vec<Browser> = pods
        .iter()
        .filter_map(|pod| {
            let id = pod_id(pod)?.to_string();
            let exit = label(&pod.metadata, EXIT_LABEL).unwrap_or(DIRECT).to_string();
            let info = exits.iter().find(|e| e.name == exit);
            let status = statuses.get(&id);
            let (phase, detail) = phase(pod, status);
            let egress = status.and_then(|s| s.egress.clone());
            let exit_ip = info.and_then(|e| e.ip.clone());
            let verdict = verdict(&exit, exit_ip.as_deref(), node_ip, egress.as_ref().map(|e| e.ip.as_str()));
            Some(Browser {
                peer: label(&pod.metadata, PEER_LABEL).and_then(|p| p.parse().ok()),
                exit_label: info.and_then(|e| e.label.clone()),
                exit_country: info.and_then(|e| e.country.clone()),
                exit_city: info.and_then(|e| e.city.clone()),
                exit_ip,
                node: pod.spec.as_ref().and_then(|s| s.node_name.clone()),
                created: pod.metadata.creation_timestamp.as_ref().map(|t| t.0.to_string()),
                vnc: crate::proxy::vnc_url(&id),
                mcp: format!("/b/{id}/mcp"),
                id,
                exit,
                phase,
                detail,
                egress,
                verdict,
            })
        })
        .collect();
    out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{ContainerState, ContainerStateWaiting, ContainerStatus, PodCondition, PodStatus as KPodStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

    fn map(kv: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
        Some(kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    pub fn secret(exit: &str, peer: u32, ip: &str) -> Secret {
        Secret {
            metadata: ObjectMeta {
                name: Some(format!("hydra-exit-{exit}-{peer}")),
                labels: map(&[(EXIT_LABEL, exit), (PEER_LABEL, &peer.to_string())]),
                annotations: map(&[(ANN_COUNTRY, "FI"), (ANN_CITY, "Helsinki"), (ANN_TZ, "Europe/Helsinki"), (ANN_LANG, "fi_FI.UTF-8"), (ANN_IP, ip)]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn pod(id: &str, exit: &str, peer: Option<u32>, phase: &str) -> Pod {
        let peer = peer.map(|p| p.to_string());
        let mut labels = vec![(APP_LABEL, APP), (ID_LABEL, id), (EXIT_LABEL, exit)];
        if let Some(p) = &peer {
            labels.push((PEER_LABEL, p));
        }
        Pod {
            metadata: ObjectMeta { name: Some(format!("hydra-b-{id}")), labels: map(&labels), ..Default::default() },
            spec: None,
            status: Some(KPodStatus { phase: Some(phase.into()), ..Default::default() }),
        }
    }

    #[test]
    fn exits_group_secrets_and_count_free_peers() {
        let secrets = [secret("purkki", 1, "1.2.3.4"), secret("purkki", 0, "1.2.3.4"), secret("hel", 0, "5.6.7.8")];
        let pods = [pod("aaaaaa", "purkki", Some(0), "Running"), pod("bbbbbb", DIRECT, None, "Running")];
        let used = used_peers(&pods);
        let ex = exits(&secrets, &used);
        assert_eq!(ex.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["hel", "purkki"]);
        let purkki = &ex[1];
        assert_eq!((purkki.peers_total, purkki.peers_free), (2, 1));
        assert_eq!(purkki.ip.as_deref(), Some("1.2.3.4"));
        assert_eq!(purkki.city.as_deref(), Some("Helsinki"));
        assert_eq!(purkki.free_peer(&used), Some((1, "hydra-exit-purkki-1")));
    }

    #[test]
    fn free_peer_is_lowest_unused_and_none_when_full() {
        let secrets: Vec<_> = (0..3).map(|n| secret("x", n, "1.1.1.1")).collect();
        let pods = [pod("aaaaaa", "x", Some(0), "Running"), pod("bbbbbb", "x", Some(2), "Running")];
        let used = used_peers(&pods);
        let e = &exits(&secrets, &used)[0];
        assert_eq!(e.free_peer(&used).map(|p| p.0), Some(1));
        let mut all = used.clone();
        all.insert(("x".into(), 1));
        assert_eq!(e.free_peer(&all), None);
    }

    #[test]
    fn terminating_pods_still_hold_their_peer() {
        let mut p = pod("aaaaaa", "x", Some(0), "Running");
        p.metadata.deletion_timestamp = Some(Time(Default::default()));
        assert!(used_peers([&p]).contains(&("x".into(), 0)));
    }

    #[test]
    fn secrets_without_peer_label_are_ignored() {
        let mut s = secret("x", 0, "1.1.1.1");
        s.metadata.labels.as_mut().unwrap().remove(PEER_LABEL);
        assert!(exits([&s], &HashSet::new()).is_empty());
    }

    #[test]
    fn verdicts() {
        use Verdict::*;
        let v = |exit, eip, nip, egr| verdict(exit, eip, nip, egr);
        assert_eq!(v("x", Some("1.1.1.1"), Some("9.9.9.9"), Some("1.1.1.1")), Tunnel);
        assert_eq!(v("x", Some("1.1.1.1"), Some("9.9.9.9"), Some("9.9.9.9")), Leak);
        assert_eq!(v("x", None, None, Some("2.2.2.2")), Unverified);
        assert_eq!(v("x", Some("1.1.1.1"), None, None), Unknown);
        assert_eq!(v(DIRECT, None, Some("9.9.9.9"), Some("9.9.9.9")), Direct);
    }

    #[test]
    fn phases() {
        let ready = PodStatus { ready: true, ..Default::default() };
        let p = pod("aaaaaa", "x", Some(0), "Running");
        assert_eq!(phase(&p, Some(&ready)).0, Phase::Ready);
        assert_eq!(phase(&p, Some(&PodStatus::default())).0, Phase::Starting);
        assert_eq!(phase(&p, None).0, Phase::Starting);
        assert_eq!(phase(&pod("aaaaaa", "x", None, "Pending"), None).0, Phase::Pending);
        assert_eq!(phase(&pod("aaaaaa", "x", None, "Failed"), None).0, Phase::Error);

        let mut pulling = pod("aaaaaa", "x", None, "Pending");
        let st = pulling.status.as_mut().unwrap();
        st.container_statuses = Some(vec![ContainerStatus {
            state: Some(ContainerState {
                waiting: Some(ContainerStateWaiting { reason: Some("ErrImageNeverPull".into()), message: Some("no image".into()) }),
                ..Default::default()
            }),
            ..Default::default()
        }]);
        assert_eq!(phase(&pulling, None), (Phase::Error, Some("ErrImageNeverPull: no image".into())));

        let mut unsched = pod("aaaaaa", "x", None, "Pending");
        unsched.status.as_mut().unwrap().conditions = Some(vec![PodCondition {
            type_: "PodScheduled".into(),
            status: "False".into(),
            message: Some("Insufficient memory".into()),
            ..Default::default()
        }]);
        assert_eq!(phase(&unsched, None), (Phase::Pending, Some("Insufficient memory".into())));

        let mut stopping = p.clone();
        stopping.metadata.deletion_timestamp = Some(Time(Default::default()));
        assert_eq!(phase(&stopping, Some(&ready)).0, Phase::Stopping);
    }

    #[test]
    fn browsers_join_pod_exit_and_status() {
        let secrets = [secret("purkki", 0, "1.2.3.4")];
        let pods = [pod("aaaaaa", "purkki", Some(0), "Running"), pod("bbbbbb", DIRECT, None, "Pending")];
        let ex = exits(&secrets, &used_peers(&pods));
        let status = PodStatus { ready: true, exit: Some("purkki".into()), egress: Some(Egress { ip: "1.2.3.4".into(), ..Default::default() }) };
        let statuses = HashMap::from([("aaaaaa".to_string(), status)]);
        let refs: Vec<&Pod> = pods.iter().collect();
        let b = browsers(&refs, &ex, &statuses, Some("9.9.9.9"));
        assert_eq!(b.len(), 2);
        let a = b.iter().find(|b| b.id == "aaaaaa").unwrap();
        assert_eq!((a.phase, a.verdict, a.peer), (Phase::Ready, Verdict::Tunnel, Some(0)));
        assert_eq!(a.exit_country.as_deref(), Some("FI"));
        let d = b.iter().find(|b| b.id == "bbbbbb").unwrap();
        assert_eq!((d.phase, d.verdict, d.exit.as_str()), (Phase::Pending, Verdict::Unknown, DIRECT));
    }

    #[test]
    fn status_json_parses_contract_shape() {
        let s: PodStatus = serde_json::from_str(
            r#"{"ready":true,"exit":"purkki","egress":{"ip":"1.2.3.4","city":"Helsinki","region":"Uusimaa","country":"FI","org":"AS1 X","timezone":"Europe/Helsinki"}}"#,
        )
        .unwrap();
        assert!(s.ready);
        assert_eq!(s.egress.unwrap().country.as_deref(), Some("FI"));
        let s: PodStatus = serde_json::from_str(r#"{"ready":false,"exit":null,"egress":null}"#).unwrap();
        assert_eq!(s, PodStatus::default());
    }
}
