# Runs on the exit host (POSIX sh, fed over ssh by `hydra exit add`; can be
# run by hand with the same env). Idempotent: (re)creates the hydra-exit
# container only when image, port or pool size changed. Keys stay in the
# hydra-exit volume. Prints HYDRA_PUBLIC_IP=<ip> and then the peer JSON.
#
#   HYDRA_IMAGE=hydra-exit:amd64 HYDRA_PORT=51820 HYDRA_PEERS=8 \
#     HYDRA_ENDPOINT=203.0.113.7:51820 sh remote-up.sh
set -eu
spec="$(docker image inspect -f '{{.Id}}' "$HYDRA_IMAGE"):$HYDRA_PORT:$HYDRA_PEERS"
have=$(docker inspect -f '{{index .Config.Labels "hydra.spec"}} {{.State.Running}}' hydra-exit 2>/dev/null || true)
if [ "$have" != "$spec true" ]; then
  echo "-- (re)creating hydra-exit container" >&2
  docker rm -f hydra-exit >/dev/null 2>&1 || true
  tun=""
  [ -e /dev/net/tun ] && tun="--device /dev/net/tun"
  # shellcheck disable=SC2086
  docker run -d --name hydra-exit --restart unless-stopped --label "hydra.spec=$spec" \
    --cap-add NET_ADMIN --sysctl net.ipv4.ip_forward=1 $tun \
    -p "$HYDRA_PORT:51820/udp" -e "HYDRA_PEERS=$HYDRA_PEERS" \
    -v hydra-exit:/etc/wireguard "$HYDRA_IMAGE" >/dev/null
fi
i=0
until docker exec hydra-exit test -f /run/hydra-exit-ready 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -ge 150 ] || [ "$(docker inspect -f '{{.State.Running}}' hydra-exit)" != true ]; then
    docker logs --tail 20 hydra-exit >&2
    exit 1
  fi
  sleep 0.1
done
echo "HYDRA_PUBLIC_IP=$(docker exec hydra-exit curl -4 -fsS --max-time 10 https://api.ipify.org || echo unknown)"
docker exec hydra-exit hydra-exit-peers "$HYDRA_ENDPOINT"
