#!/bin/sh
# Build hydra-panel for n100-1 (amd64), import it into k0s containerd and restart the Deployment.
# Usage: panel/ship.sh [node]   (needs ssh root@<node>; KUBECONFIG defaults to .secrets/kubeconfig, see docs/infra.md)
set -eu
node="${1:-n100-1}"
cd "$(dirname "$0")"
export KUBECONFIG="${KUBECONFIG:-$(cd .. && pwd)/.secrets/kubeconfig}"
docker build --platform linux/amd64 -t hydra-panel:latest .
docker save --platform linux/amd64 hydra-panel:latest | ssh "root@$node" k0s ctr -n k8s.io images import -
ssh "root@$node" k0s ctr -n k8s.io images label docker.io/library/hydra-panel:latest io.cri-containerd.pinned=pinned >/dev/null
if kubectl -n hydra get deploy/hydra-panel >/dev/null 2>&1; then
  kubectl -n hydra rollout restart deploy/hydra-panel
  kubectl -n hydra rollout status deploy/hydra-panel --timeout=60s
else
  echo "Deployment puuttuu: kubectl apply -f panel/deploy/panel.yaml"
fi
