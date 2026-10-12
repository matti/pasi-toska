//! Kube side: watched pods and exit Secrets, launching and stopping browsers,
//! and the loop that turns it all into the UI snapshot.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use futures::{Stream, StreamExt};
use k8s_openapi::api::core::v1::{Pod, Secret};
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::runtime::reflector::{self, Store};
use kube::runtime::{WatchStreamExt, watcher};
use kube::{Api, Client};
use tokio::sync::{Notify, watch};

use crate::config::Config;
use crate::model::{self, ClusterInfo, DIRECT, Egress, EXIT_LABEL, PEER_LABEL, PodStatus, Snapshot};
use crate::podspec::{self, CONTROL_PORT, LaunchSpec, Route, STATUS_PORT};
use crate::web::ApiError;
use crate::id;

const BROWSER_SELECTOR: &str = "app=hydra-browser";

#[derive(Clone, Debug, PartialEq)]
enum Sync {
    Syncing,
    Ok,
    Failed(String),
}

pub struct Cluster {
    pods_api: Api<Pod>,
    pods: Store<Pod>,
    secrets: Store<Secret>,
    /// Watch health of [pods, secrets].
    sync: Mutex<[Sync; 2]>,
    /// Fired on every watch event.
    changed: Notify,
    /// Serializes launches so two cannot pick the same peer.
    launch_lock: tokio::sync::Mutex<()>,
}

impl From<kube::Error> for ApiError {
    fn from(e: kube::Error) -> Self {
        ApiError(StatusCode::BAD_GATEWAY, format!("Kube API: {e}"))
    }
}

impl Cluster {
    pub fn start(client: Client, namespace: &str) -> Arc<Self> {
        let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);
        let secrets_api: Api<Secret> = Api::namespaced(client, namespace);
        let (pods, pods_writer) = reflector::store();
        let (secrets, secrets_writer) = reflector::store();
        let this = Arc::new(Self {
            pods_api: pods_api.clone(),
            pods,
            secrets,
            sync: Mutex::new([Sync::Syncing, Sync::Syncing]),
            changed: Notify::new(),
            launch_lock: Default::default(),
        });
        let cfg = |labels| watcher::Config::default().labels(labels);
        let pod_events = reflector::reflector(pods_writer, watcher(pods_api, cfg(BROWSER_SELECTOR))).default_backoff();
        let secret_events = reflector::reflector(secrets_writer, watcher(secrets_api, cfg(EXIT_LABEL))).default_backoff();
        tokio::spawn(this.clone().drive(0, pod_events));
        tokio::spawn(this.clone().drive(1, secret_events));
        this
    }

    async fn drive<K>(self: Arc<Self>, slot: usize, events: impl Stream<Item = Result<watcher::Event<K>, watcher::Error>>) {
        let mut events = std::pin::pin!(events);
        while let Some(ev) = events.next().await {
            let state = match ev {
                Ok(watcher::Event::Init | watcher::Event::InitApply(_)) => None,
                Ok(_) => Some(Sync::Ok),
                Err(e) => {
                    tracing::warn!(slot, "watch: {e}");
                    Some(Sync::Failed(e.to_string()))
                }
            };
            if let Some(s) = state {
                self.sync.lock().unwrap()[slot] = s;
            }
            self.changed.notify_one();
        }
    }

    fn health(&self) -> (bool, Option<String>) {
        let sync = self.sync.lock().unwrap();
        let error = sync.iter().find_map(|s| match s {
            Sync::Failed(e) => Some(e.clone()),
            _ => None,
        });
        (sync.iter().all(|s| *s == Sync::Ok), error)
    }

    pub fn pod_ip(&self, id: &str) -> Option<IpAddr> {
        let pod = self.pods.find(|p| model::pod_id(p) == Some(id))?;
        pod.status.as_ref()?.pod_ip.as_ref()?.parse().ok()
    }

    pub async fn launch(&self, cfg: &Config, exit: &str) -> Result<String, ApiError> {
        let _guard = self.launch_lock.lock().await;
        // Fresh list, not the watch cache: a pod created a moment ago must count.
        let pods = self.pods_api.list(&ListParams::default().labels(BROWSER_SELECTOR)).await?.items;
        let active = pods.iter().filter(|p| p.metadata.deletion_timestamp.is_none()).count();
        if active >= cfg.max_browsers {
            return Err(ApiError(
                StatusCode::CONFLICT,
                format!("Enintään {} selainta voi olla käynnissä yhtä aikaa. Sulje ensin jokin niistä.", cfg.max_browsers),
            ));
        }
        let (route, tz, lang) = if exit == DIRECT {
            (Route::Direct, cfg.direct_tz.clone(), cfg.direct_lang.clone())
        } else {
            let used = model::used_peers(&pods);
            let secrets = self.secrets.state();
            let exits = model::exits(secrets.iter().map(AsRef::as_ref), &used);
            let e = exits
                .iter()
                .find(|e| e.name == exit)
                .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("Exitiä \"{exit}\" ei löydy.")))?;
            let (peer, secret) = e
                .free_peer(&used)
                .ok_or_else(|| ApiError(StatusCode::CONFLICT, format!("Exitin \"{exit}\" kaikki peerit ovat käytössä.")))?;
            let route = Route::Tunnel { exit: exit.to_string(), peer, secret: secret.to_string(), label: e.label.clone() };
            (route, e.tz.clone().unwrap_or_else(|| cfg.direct_tz.clone()), e.lang.clone().unwrap_or_else(|| cfg.direct_lang.clone()))
        };
        let taken: HashSet<&str> = pods.iter().filter_map(model::pod_id).collect();
        let id = id::generate(|c| taken.contains(c));
        let pod = podspec::build(&LaunchSpec { id: id.clone(), image: cfg.image.clone(), pull_policy: cfg.pull_policy.clone(), route, tz, lang });
        self.pods_api.create(&PostParams::default(), &pod).await?;
        tracing::info!(id, exit, "launched browser");
        Ok(id)
    }

    /// Moves a tunnel browser to another exit without a restart: reserves a
    /// free peer on `exit` (pod labels), then hands its wg0.conf to the pod's
    /// control port, which swaps the tunnel in place. On failure the labels
    /// go back, so the old peer stays reserved.
    pub async fn switch_exit(&self, http: &reqwest::Client, id: &str, exit: &str) -> Result<(), ApiError> {
        if !id::is_valid(id) {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Virheellinen tunniste.".into()));
        }
        if exit == DIRECT {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Tunnelista ei voi vaihtaa suoraan yhteyteen (killswitch pysyy).".into()));
        }
        let _guard = self.launch_lock.lock().await;
        let pods = self.pods_api.list(&ListParams::default().labels(BROWSER_SELECTOR)).await?.items;
        let pod = pods
            .iter()
            .find(|p| model::pod_id(p) == Some(id))
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Selainta ei löydy.".into()))?;
        let labels = pod.metadata.labels.clone().unwrap_or_default();
        let (old_exit, old_peer) = (labels.get(EXIT_LABEL).cloned().unwrap_or_default(), labels.get(PEER_LABEL).cloned());
        if old_exit == DIRECT || old_peer.is_none() {
            return Err(ApiError(StatusCode::CONFLICT, "Suoralla selaimella ei ole tunnelia, jonka exitin voisi vaihtaa.".into()));
        }
        if old_exit == exit {
            return Ok(());
        }
        let ip: IpAddr = pod
            .status
            .as_ref()
            .and_then(|s| s.pod_ip.as_ref()?.parse().ok())
            .ok_or_else(|| ApiError(StatusCode::CONFLICT, "Selaimella ei vielä ole IP-osoitetta.".into()))?;
        let used = model::used_peers(&pods);
        let secrets = self.secrets.state();
        let exits = model::exits(secrets.iter().map(AsRef::as_ref), &used);
        let e = exits
            .iter()
            .find(|e| e.name == exit)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("Exitiä \"{exit}\" ei löydy.")))?;
        let (peer, secret_name) = e
            .free_peer(&used)
            .ok_or_else(|| ApiError(StatusCode::CONFLICT, format!("Exitin \"{exit}\" kaikki peerit ovat käytössä.")))?;
        let conf = secrets
            .iter()
            .find(|s| s.metadata.name.as_deref() == Some(secret_name))
            .and_then(|s| s.data.as_ref()?.get("wg0.conf"))
            .map(|b| String::from_utf8_lossy(&b.0).into_owned())
            .ok_or_else(|| ApiError(StatusCode::BAD_GATEWAY, format!("Secretistä {secret_name} puuttuu wg0.conf.")))?;

        let pod_name = podspec::pod_name(id);
        let relabel = |exit: &str, peer: &str| {
            Patch::Merge(serde_json::json!({ "metadata": { "labels": { (EXIT_LABEL): exit, (PEER_LABEL): peer } } }))
        };
        self.pods_api.patch(&pod_name, &PatchParams::default(), &relabel(exit, &peer.to_string())).await?;
        let res = http
            .post(format!("http://{ip}:{CONTROL_PORT}/exit"))
            .json(&serde_json::json!({ "exit": exit, "conf": conf, "label": e.label }))
            .timeout(Duration::from_secs(20))
            .send()
            .await;
        let failure = match res {
            Ok(r) if r.status().is_success() => None,
            Ok(r) => Some(format!("{}: {}", r.status(), r.text().await.unwrap_or_default())),
            Err(e) => Some(e.to_string()),
        };
        if let Some(msg) = failure {
            let back = relabel(&old_exit, old_peer.as_deref().unwrap_or_default());
            if let Err(e) = self.pods_api.patch(&pod_name, &PatchParams::default(), &back).await {
                tracing::warn!(id, "restoring labels after a failed exit switch: {e}");
            }
            return Err(ApiError(StatusCode::BAD_GATEWAY, format!("Exitin vaihto epäonnistui: {msg}")));
        }
        tracing::info!(id, from = old_exit, to = exit, peer, "switched exit");
        self.changed.notify_one();
        Ok(())
    }

    pub async fn stop(&self, id: &str) -> Result<(), ApiError> {
        if !id::is_valid(id) {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Virheellinen tunniste.".into()));
        }
        let dp = DeleteParams { grace_period_seconds: Some(1), ..Default::default() };
        match self.pods_api.delete(&podspec::pod_name(id), &dp).await {
            Ok(_) => {
                tracing::info!(id, "stopped browser");
                Ok(())
            }
            Err(kube::Error::Api(s)) if s.code == 404 => Err(ApiError(StatusCode::NOT_FOUND, "Selainta ei löydy.".into())),
            Err(e) => Err(e.into()),
        }
    }

    fn snapshot(&self, cfg: &Config, statuses: &HashMap<String, PodStatus>, node: Option<Egress>) -> Snapshot {
        let pods = self.pods.state();
        let refs: Vec<&Pod> = pods.iter().map(AsRef::as_ref).collect();
        let used = model::used_peers(refs.iter().copied());
        let secrets = self.secrets.state();
        let exits = model::exits(secrets.iter().map(AsRef::as_ref), &used);
        let browsers = model::browsers(&refs, &exits, statuses, node.as_ref().map(|n| n.ip.as_str()));
        let (ok, error) = self.health();
        Snapshot {
            cluster: ClusterInfo {
                ok,
                error,
                namespace: cfg.namespace.clone(),
                node,
                max_browsers: cfg.max_browsers,
                agent_minutes: crate::agent::credential_minutes(&cfg.claude_credentials),
                ip_test: cfg.ip_test_url.clone(),
                fp_test: cfg.fp_test_url.clone(),
            },
            exits,
            browsers,
        }
    }

    /// `GET :8080/status` of every running pod, concurrently. Unreachable pods
    /// keep their previous status so the UI does not flicker.
    async fn poll_statuses(&self, http: &reqwest::Client, prev: &HashMap<String, PodStatus>) -> HashMap<String, PodStatus> {
        let targets: Vec<(String, IpAddr)> = self
            .pods
            .state()
            .iter()
            .filter(|p| p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running"))
            .filter_map(|p| Some((model::pod_id(p)?.to_string(), p.status.as_ref()?.pod_ip.as_ref()?.parse().ok()?)))
            .collect();
        let fetches = targets.into_iter().map(|(id, ip)| async move {
            let res = http.get(format!("http://{ip}:{STATUS_PORT}/status")).timeout(Duration::from_millis(1500)).send().await;
            let status = match res {
                Ok(r) => r.json::<PodStatus>().await.ok(),
                Err(_) => None,
            };
            (id, status)
        });
        futures::future::join_all(fetches)
            .await
            .into_iter()
            .filter_map(|(id, s)| {
                let s = s.or_else(|| prev.get(&id).cloned())?;
                Some((id, s))
            })
            .collect()
    }

    /// Keeps `tx` current: rebuilds on every watch event and polls pod statuses every second.
    pub async fn run_snapshots(
        self: Arc<Self>,
        cfg: Arc<Config>,
        http: reqwest::Client,
        mut node: watch::Receiver<Option<Egress>>,
        tx: watch::Sender<Snapshot>,
    ) {
        let mut statuses = HashMap::new();
        let mut seen_ready = None;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let poll = tokio::select! {
                _ = tick.tick() => true,
                _ = self.changed.notified() => false,
                _ = node.changed() => false,
            };
            if poll {
                statuses = self.poll_statuses(&http, &statuses).await;
                for id in newly_ready(&mut seen_ready, &statuses) {
                    let Some(ip) = self.pod_ip(&id) else { continue };
                    let (http, url) = (http.clone(), cfg.ip_test_url.clone());
                    tokio::spawn(async move {
                        if let Err(e) = crate::mcp::navigate(http, ip, url).await {
                            tracing::warn!(id, "opening IP test page: {e:#}");
                        }
                    });
                }
            }
            let snap = self.snapshot(&cfg, &statuses, node.borrow().clone());
            tx.send_if_modified(|cur| {
                let changed = *cur != snap;
                if changed {
                    *cur = snap;
                }
                changed
            });
        }
    }
}

/// Browsers that turned ready since the last call. The first call only
/// records the current state, so a panel restart does not reload pages.
fn newly_ready(seen: &mut Option<HashSet<String>>, statuses: &HashMap<String, PodStatus>) -> Vec<String> {
    let ready = statuses.iter().filter(|(_, s)| s.ready).map(|(id, _)| id.clone());
    let Some(seen) = seen else {
        *seen = Some(ready.collect());
        return Vec::new();
    };
    seen.retain(|id| statuses.contains_key(id));
    let mut fresh: Vec<String> = ready.filter(|id| seen.insert(id.clone())).collect();
    fresh.sort();
    fresh
}

/// Public IP of the node itself (the panel egresses directly), refreshed every 10 min.
pub async fn run_node_egress(http: reqwest::Client, fixed_ip: Option<String>, tx: watch::Sender<Option<Egress>>) {
    loop {
        let res = async { http.get("https://ipinfo.io/json").timeout(Duration::from_secs(5)).send().await?.error_for_status()?.json::<Egress>().await }.await;
        let wait = match res {
            Ok(mut e) => {
                if let Some(ip) = &fixed_ip
                    && *ip != e.ip
                {
                    e = Egress { ip: ip.clone(), ..Default::default() };
                }
                tx.send_replace(Some(e));
                600
            }
            Err(err) => {
                tracing::warn!("node egress lookup: {err}");
                if let Some(ip) = &fixed_ip {
                    tx.send_replace(Some(Egress { ip: ip.clone(), ..Default::default() }));
                }
                30
            }
        };
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(pairs: &[(&str, bool)]) -> HashMap<String, PodStatus> {
        pairs.iter().map(|(id, ready)| (id.to_string(), PodStatus { ready: *ready, ..Default::default() })).collect()
    }

    #[test]
    fn newly_ready_skips_startup_state_and_reports_each_browser_once() {
        let mut seen = None;
        assert!(newly_ready(&mut seen, &st(&[("old", true), ("boot", false)])).is_empty());
        assert_eq!(newly_ready(&mut seen, &st(&[("old", true), ("boot", true), ("new", true)])), ["boot", "new"]);
        assert!(newly_ready(&mut seen, &st(&[("old", true), ("boot", true), ("new", true)])).is_empty());
        // Gone, then an unrelated pod: state is pruned, nothing re-fires.
        assert!(newly_ready(&mut seen, &st(&[("new", true)])).is_empty());
        assert_eq!(seen.as_ref().unwrap().len(), 1);
    }
}
