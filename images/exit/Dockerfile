# hydra-exit: WireGuard server + NAT as a container on any docker host.
#   docker run -d --name hydra-exit --restart unless-stopped --cap-add NET_ADMIN \
#     --sysctl net.ipv4.ip_forward=1 -p 51820:51820/udp -v hydra-exit:/etc/wireguard hydra-exit
#   docker exec hydra-exit hydra-exit-peers <endpoint-host>[:port]
FROM alpine:3.24
RUN apk add --no-cache wireguard-tools-wg wireguard-go iptables iproute2 jq curl
COPY hydra-exit-start hydra-exit-peers /usr/local/bin/
RUN chmod 755 /usr/local/bin/hydra-exit-start /usr/local/bin/hydra-exit-peers
VOLUME /etc/wireguard
EXPOSE 51820/udp
ENTRYPOINT ["/usr/local/bin/hydra-exit-start"]
