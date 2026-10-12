#!/bin/bash
# WireGuard egress with a killswitch for the whole container network
# namespace (in kube: the whole pod). Runs as root.
#
#   net-up.sh <wg0.conf>                    killswitch first, then the tunnel
#   net-up.sh --swap <new.conf> <old.conf>  switch exit in place: wg0 stays up
#                                           and the killswitch is never lifted
#
#   HYDRA_LOCAL_CIDRS     networks that stay off the tunnel, replies only
#                         (default RFC1918: docker, pod and LAN clients)
#   HYDRA_WG_USERSPACE=1  force wireguard-go instead of kernel WireGuard
#
# The only ways out are: wg0, the encrypted UDP to the exit endpoint, and
# replies to connections that LOCAL_CIDRS clients opened to the service
# ports. DNS goes to the resolvers in the config, through wg0.
set -euo pipefail
LOCAL_CIDRS=${HYDRA_LOCAL_CIDRS:-10.0.0.0/8,172.16.0.0/12,192.168.0.0/16}
PORTS=6080,8080,8081,8931
MARK=51820
log() { echo "hydra-net: $*" >&2; }

field() { sed -n "s/^[[:space:]]*$2[[:space:]]*=[[:space:]]*//p" "$1" | head -n 1; }

# parse <conf> <prefix>: sets <prefix>_address, _dns, _mtu, _key, _pub, _ip, _port.
parse() {
  local conf=$1 p=$2 endpoint host ip
  endpoint=$(field "$conf" Endpoint)
  host=${endpoint%:*}
  # Resolved up front: the killswitch needs a literal address.
  ip=$(getent ahostsv4 "$host" | awk 'NR == 1 {print $1}')
  declare -g "${p}_address=$(field "$conf" Address | cut -d, -f1)" "${p}_dns=$(field "$conf" DNS)" \
    "${p}_mtu=$(field "$conf" MTU)" "${p}_key=$(field "$conf" PrivateKey)" "${p}_pub=$(field "$conf" PublicKey)" \
    "${p}_ip=$ip" "${p}_port=${endpoint##*:}"
  local a="${p}_address" d="${p}_dns" k="${p}_key" u="${p}_pub"
  [ -n "${!a}" ] && [ -n "${!d}" ] && [ -n "${!k}" ] && [ -n "${!u}" ] && [ -n "$ip" ] ||
    { log "$conf: Address, DNS, PrivateKey, PublicKey and a resolvable Endpoint are required"; exit 1; }
}

write_dns() { echo "$1" | tr ',' ' ' | xargs -n 1 printf 'nameserver %s\n' > /etc/resolv.conf; }

allow_endpoint() { # <-I|-D> <ip> <port>
  iptables "$1" OUTPUT -p udp -d "$2" --dport "$3" -j ACCEPT
  iptables "$1" INPUT -p udp -s "$2" --sport "$3" -j ACCEPT
}

up() {
  parse "$1" c
  local gw dev
  gw=$(ip -4 route show default | awk '{print $3; exit}')
  dev=$(ip -4 route show default | awk '{print $5; exit}')

  # 1. Killswitch before the tunnel exists.
  iptables -F
  iptables -P INPUT DROP
  iptables -P FORWARD DROP
  iptables -P OUTPUT DROP
  iptables -A INPUT -i lo -j ACCEPT
  iptables -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  iptables -A OUTPUT -o lo -j ACCEPT
  iptables -A OUTPUT -o wg0 -j ACCEPT
  allow_endpoint -A "$c_ip" "$c_port"
  for cidr in ${LOCAL_CIDRS//,/ }; do
    iptables -A INPUT ! -i wg0 -s "$cidr" -p tcp -m multiport --dports "$PORTS" -j ACCEPT
    iptables -A OUTPUT ! -o wg0 -d "$cidr" -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  done
  # IPv6 is not tunneled: nothing but loopback.
  if [ -e /proc/net/if_inet6 ]; then
    for chain in INPUT FORWARD OUTPUT; do ip6tables -P "$chain" DROP; done
    ip6tables -A INPUT -i lo -j ACCEPT
    ip6tables -A OUTPUT -o lo -j ACCEPT
  fi

  # 2. DNS through the tunnel only.
  write_dns "$c_dns"

  # 3. Tunnel: kernel WireGuard, else userspace wireguard-go (needs /dev/net/tun).
  if [ "${HYDRA_WG_USERSPACE:-0}" = 1 ] || ! ip link add wg0 type wireguard 2>/dev/null; then
    log "using userspace wireguard-go"
    WG_PROCESS_FOREGROUND=0 wireguard-go wg0
  fi
  grep -Ev '^[[:space:]]*(Address|DNS|MTU|Endpoint)[[:space:]]*=' "$1" \
    | sed "/^\[Peer\]/a Endpoint = $c_ip:$c_port" > /run/wg0.conf
  wg setconf wg0 /run/wg0.conf
  rm -f /run/wg0.conf
  wg set wg0 fwmark "$MARK"
  ip -4 address add "$c_address" dev wg0
  ip link set mtu "${c_mtu:-1420}" up dev wg0

  # 4. Routing like wg-quick: everything but the tunnel's own packets goes to
  #    a table whose default is wg0. LOCAL_CIDRS keep a main-table route via
  #    the original gateway, so replies to docker/pod/LAN clients go back out
  #    eth0 (the killswitch allows only replies there).
  for cidr in ${LOCAL_CIDRS//,/ }; do
    ip -4 route replace "$cidr" via "$gw" dev "$dev"
  done
  ip -4 route add default dev wg0 table "$MARK"
  ip -4 rule add not fwmark "$MARK" table "$MARK"
  ip -4 rule add table main suppress_prefixlength 0
  log "tunnel up: $c_address via $c_ip:$c_port, local $LOCAL_CIDRS"
}

# Exit switch without a gap: the OUTPUT policy stays DROP and wg0 stays the
# default route throughout. The new endpoint is allowed before the peer
# changes, the old one removed after; packets sent in between go into wg0
# (to the old or the new peer), never out of eth0.
swap() {
  parse "$1" n
  parse "$2" o
  local changed=0
  [ "$n_ip:$n_port" = "$o_ip:$o_port" ] || changed=1
  [ "$changed" = 0 ] || allow_endpoint -I "$n_ip" "$n_port"

  install -m 600 /dev/null /run/wg.key
  printf '%s\n' "$n_key" > /run/wg.key
  local args=(set wg0 private-key /run/wg.key)
  [ "$n_pub" = "$o_pub" ] || args+=(peer "$o_pub" remove)
  args+=(peer "$n_pub" endpoint "$n_ip:$n_port" allowed-ips 0.0.0.0/0,::/0 persistent-keepalive 25)
  wg "${args[@]}"
  rm -f /run/wg.key

  if [ "$n_address" != "$o_address" ]; then
    ip -4 address add "$n_address" dev wg0
    ip -4 address del "$o_address" dev wg0
  fi
  ip link set mtu "${n_mtu:-1420}" dev wg0
  write_dns "$n_dns"
  [ "$changed" = 0 ] || allow_endpoint -D "$o_ip" "$o_port"
  log "exit switched: $o_ip:$o_port -> $n_ip:$n_port ($n_address)"
}

case "${1:-}" in
  --swap) swap "$2" "$3" ;;
  *) up "$1" ;;
esac
