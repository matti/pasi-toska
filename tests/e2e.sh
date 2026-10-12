#!/usr/bin/env bash
# Local end-to-end test, docker only. The "exit host" is this machine's
# docker (tests/fake-ssh), kube is recorded by tests/fake-kubectl.
#
#   1. hydra exit add (twice: idempotent) -> hydra-exit container + peer confs + Secrets manifest
#   2. hydra-browser with peer 0 behind the exit: status, noVNC, CDP localhost-only
#   3. MCP use case A (search, open top result by ref, extract) and B (read a page)
#   4. tunnel proof (exit wg counters), killswitch (eth0 bypass, unprivileged
#      user cannot touch iptables, exit stopped -> no egress, MCP navigation fails)
#   4b. exit switch to a second exit container while traffic runs: tcpdump on
#      eth0 must see nothing but WireGuard and service-port packets
#   5. direct browser without exit
# Everything it starts is removed at the end. The exit runs on this machine,
# so the egress IP equals the host's IP here; that is expected.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
T=$(mktemp -d)
export HYDRA_STATE=$T/state
hydra=$root/cli/target/debug/hydra
mcp() { node "$root/tests/mcp-client/mcp.mjs" "$@"; }
B=hydra-e2e-b
D=hydra-e2e-direct

X2=hydra-e2e-exit2
DUMP=hydra-e2e-dump
cleanup_before() {
  docker rm -f "$B" "$D" hydra-exit "$X2" "$DUMP" "$DUMP-wg" >/dev/null 2>&1 || true
  docker volume rm hydra-exit "$X2" >/dev/null 2>&1 || true
}
cleanup() {
  cleanup_before
  rm -rf "$T"
}
trap cleanup EXIT

step() { printf '\n=== %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }
port() { docker port "$1" "$2" | head -n 1 | sed 's/.*://'; }
transfer() { docker exec hydra-exit wg show wg0 transfer | awk '{n += $2 + $3} END {print n + 0}'; }
wait_ready() { # <container>: poll /status until ready
  local p; p=$(port "$1" 8080)
  for _ in $(seq 300); do
    curl -sf "127.0.0.1:$p/status" | grep -q '"ready":true' && return 0
    sleep 0.1
  done
  docker logs --tail 30 "$1"; fail "$1 not ready"
}

step "build"
cleanup_before
(cd "$root/cli" && cargo build -q)
docker build -q -t hydra-browser "$root/images/browser" >/dev/null
printf 'FROM alpine:3.24\nRUN apk add --no-cache tcpdump\n' | docker build -q -t hydra-e2e-tcpdump - >/dev/null
[ -d "$root/tests/mcp-client/node_modules" ] || (cd "$root/tests/mcp-client" && npm ci -q)

step "hydra exit add (fake ssh = local docker, fake kubectl)"
export HYDRA_SSH=$root/tests/fake-ssh HYDRA_KUBECTL=$root/tests/fake-kubectl
export KUBECONFIG=$T/kubeconfig HYDRA_KUBECTL_OUT=$T/secrets.json
add() {
  "$hydra" exit add fake localhost --endpoint 172.17.0.1 --peers 2 \
    --country FI --city Espoo --tz Europe/Helsinki --lang fi_FI.UTF-8
}
add
jq -r '.items[] | "\(.kind) \(.metadata.name) \(.metadata.labels // {} | tostring) \(.metadata.annotations // {} | tostring)"' "$T/secrets.json"
[ "$(jq '[.items[] | select(.kind == "Secret" and (.stringData["wg0.conf"] | contains("[Peer]")))] | length' "$T/secrets.json")" = 2 ] ||
  fail "expected 2 Secrets with wg0.conf"
step "hydra exit add again (idempotent: no image copy, container kept)"
add 2>&1 | tee "$T/add2.log"
! grep -q 're)creating\|copying' "$T/add2.log" || fail "second add recreated or copied"
"$hydra" exit ls
ls -l "$HYDRA_STATE/exits/fake"

step "hydra-browser behind the exit (peer 0)"
docker create --name "$B" --cap-add NET_ADMIN --shm-size 1g \
  -p 127.0.0.1::6080 -p 127.0.0.1::8080 -p 127.0.0.1::8081 -p 127.0.0.1::8931 \
  -e HYDRA_EXIT=fake -e TZ=Europe/Helsinki -e LANG=fi_FI.UTF-8 \
  -e PLAYWRIGHT_MCP_TIMEOUT_NAVIGATION=15000 hydra-browser >/dev/null
docker cp "$HYDRA_STATE/exits/fake/peer-0.conf" "$B:/etc/hydra/wg/wg0.conf"
docker start "$B" >/dev/null
wait_ready "$B"
M=http://127.0.0.1:$(port "$B" 8931)/mcp
docker logs "$B" 2>&1 | grep -E 'hydra-(net|browser)'
echo "status: $(curl -s "127.0.0.1:$(port "$B" 8080)/status")"
docker exec "$B" curl -sf 127.0.0.1:8080/ip | grep -q '<title>PASI TOSKA IP</title>' && echo "IP page: 127.0.0.1:8080/ip served"
curl -sf "127.0.0.1:$(port "$B" 6080)/vnc.html" | grep -qi novnc && echo "noVNC: http://127.0.0.1:$(port "$B" 6080)/vnc.html"
docker exec "$B" ss -ltn | grep -E ':(9222|8931|8080|6080) '
docker exec "$B" ss -ltn | grep -q '127.0.0.1:9222' || fail "CDP not on 127.0.0.1"
! docker exec "$B" ss -ltn | grep -E '(0.0.0.0|\*|\[::\]):9222' || fail "CDP exposed"
docker exec "$B" cat /etc/resolv.conf
docker exec "$B" sh -c 'echo "TZ=$TZ date=$(date +%Z) LANG=$LANG"'

step "MCP tools"
mcp "$M" tools | tr '\n' ' '
echo
echo "browser fingerprint basics (should match the exit: fi-FI, Europe/Helsinki, no webdriver):"
mcp "$M" call browser_evaluate '{"function":"() => ({webdriver: navigator.webdriver, language: navigator.language, languages: navigator.languages, timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone, userAgent: navigator.userAgent})"}' | sed -n '/### Result/,/^###/p' | sed '1d;$d'
before=$(transfer)

step "use case A: search (DuckDuckGo html), open top result by snapshot ref, extract"
mcp "$M" search ddg "wireguard kill switch iptables" | cut -c1-160 | head -n 22

step "use case B: read a given page"
mcp "$M" read https://fi.wikipedia.org/wiki/WireGuard | cut -c1-160 | head -n 12

step "tunnel: exit-side WireGuard counters"
after=$(transfer)
docker exec hydra-exit wg show wg0 | sed -n '/peer/,/transfer/p' | head -n 6
echo "exit wg bytes: before $before, after $after"
[ "$after" -gt $((before + 100000)) ] || fail "exit counters did not grow"

step "killswitch: no egress around wg0, browser user cannot change rules"
if docker exec "$B" curl -4 -sS --max-time 3 --interface eth0 -o /dev/null http://1.1.1.1; then fail "eth0 egress worked"; fi
if docker exec -u hydra "$B" iptables -F 2>/dev/null; then fail "user hydra flushed iptables"; fi
echo "eth0 bypass blocked; user hydra cannot touch iptables"
docker exec "$B" iptables -S OUTPUT

step "killswitch: exit stopped -> no egress at all, no fallback"
docker stop -t 1 hydra-exit >/dev/null
if docker exec "$B" curl -4 -sS --max-time 5 -o /dev/null https://api.ipify.org; then fail "egress with exit down"; fi
if docker exec "$B" curl -4 -sS --max-time 3 -o /dev/null http://1.1.1.1; then fail "IP literal with exit down"; fi
if mcp "$M" read https://example.org 2>"$T/mcp.err"; then fail "MCP navigation worked with exit down"; fi
head -n 3 "$T/mcp.err"
echo "no egress with the exit down (curl DNS, IP literal and MCP navigation all failed)"

step "exit switch on the fly: to a second exit, no restart, no leak"
img=$(docker inspect -f '{{.Config.Image}}' hydra-exit)
docker run -d --name "$X2" --cap-add NET_ADMIN --sysctl net.ipv4.ip_forward=1 \
  -p 51821:51820/udp -v "$X2:/etc/wireguard" -e HYDRA_PEERS=2 "$img" >/dev/null
for _ in $(seq 100); do docker exec "$X2" test -f /run/hydra-exit-ready 2>/dev/null && break; sleep 0.1; done
conf2=$(docker exec "$X2" hydra-exit-peers 172.17.0.1:51821 | jq -r '.[0].conf')
code=$(docker exec "$B" curl -s -o /dev/null -w '%{http_code}' -X POST 127.0.0.1:8081/exit -d '{}')
[ "$code" = 403 ] || fail "local client could reach the exit switch ($code)"
echo "exit switch refuses local clients (HTTP $code)"
# Everything leaving eth0 that is not WireGuard UDP or the service ports is a
# leak. Outbound only (-Q out): stale host conntrack from earlier direct
# containers can deliver unsolicited inbound packets, which INPUT drops anyway.
docker run -d --name "$DUMP" --net "container:$B" --cap-add NET_RAW --cap-add NET_ADMIN hydra-e2e-tcpdump \
  tcpdump -i eth0 -Q out -n -l 'not arp and not (udp and (port 51820 or port 51821)) and not (tcp and (port 6080 or port 8080 or port 8081 or port 8931))' >/dev/null
# Sanity: the same capture point must see the WireGuard packets to the new exit.
docker run -d --name "$DUMP-wg" --net "container:$B" --cap-add NET_RAW --cap-add NET_ADMIN hydra-e2e-tcpdump \
  tcpdump -i eth0 -Q out -n -l 'udp and port 51821' >/dev/null
for d in "$DUMP" "$DUMP-wg"; do
  for _ in $(seq 50); do docker logs "$d" 2>&1 | grep -q listening && break; sleep 0.1; done
done
docker exec -d "$B" sh -c 'for i in $(seq 30); do curl -s -o /dev/null --max-time 2 https://example.com; getent ahostsv4 example.org >/dev/null; done'
curl -sf -X POST "127.0.0.1:$(port "$B" 8081)/exit" -d "$(jq -n --arg c "$conf2" '{exit: "fake2", conf: $c}')"
echo
for _ in $(seq 100); do curl -sf "127.0.0.1:$(port "$B" 8080)/status" | grep -q '"exit":"fake2".*"ip"' && break; sleep 0.1; done
echo "status after switch: $(curl -s "127.0.0.1:$(port "$B" 8080)/status")"
docker exec "$B" curl -4 -sS --max-time 10 -o /dev/null https://example.com || fail "no egress after the switch"
docker exec "$X2" wg show wg0 transfer | awk '$2 > 0 {ok = 1} END {exit !ok}' || fail "second exit saw no traffic"
for _ in $(seq 200); do docker exec "$B" pgrep -f 'seq 30' >/dev/null || break; sleep 0.1; done
leaks=$(docker logs "$DUMP" 2>/dev/null | wc -l | tr -d ' ')
wgpk=$(docker logs "$DUMP-wg" 2>/dev/null | wc -l | tr -d ' ')
[ "$wgpk" -gt 0 ] || fail "capture saw no WireGuard packets (capture not working?)"
[ "$leaks" = 0 ] || { docker logs "$DUMP" 2>/dev/null | head -n 5; fail "$leaks packets left eth0 outside WireGuard during the switch"; }
docker rm -f "$DUMP" "$DUMP-wg" >/dev/null
echo "switched to fake2 while traffic ran: $wgpk WireGuard packets to the new exit, 0 packets outside WireGuard on eth0"

step "direct browser (no wg0.conf, no NET_ADMIN)"
docker run -d --name "$D" --shm-size 1g -p 127.0.0.1::8080 -p 127.0.0.1::8931 hydra-browser >/dev/null
wait_ready "$D"
echo "status: $(curl -s "127.0.0.1:$(port "$D" 8080)/status")"
mcp "http://127.0.0.1:$(port "$D" 8931)/mcp" read https://example.com | head -n 3

echo
echo "E2E OK"
