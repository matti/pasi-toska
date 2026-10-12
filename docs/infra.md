# Infra: k0s n100-1:llä, exit-kone purkki

Tila 2026-10-11. Komennot ajetaan Macilta repon juuresta.

## n100-1 (Proxmox VE 8, Debian 12, kernel 6.8.12-9-pve)

- k0s **v1.36.4+k0s.1** (Kubernetes v1.36.4, containerd 2.3.5), yksisolmuinen: `k0s install controller --single --config /etc/k0s/k0s.yaml --kubelet-extra-args=--node-ip=192.168.9.101`. Palvelu `k0scontroller` (systemd, enabled).
- Binääri `/usr/local/bin/k0s`, data `/var/lib/k0s` (~0,9 Gt), konfiguraatio `/etc/k0s/k0s.yaml`:
  - `api.address: 192.168.9.101` (oletus olisi valinnut linkdown-tilassa olevan `enp4s0` 10.99.99.1).
  - `api.sans`: 192.168.9.101, 100.121.196.77, n100-1, n100-1.local.
  - `storage.type: kine` (SQLite, kevyempi kuin etcd), CNI kube-router, kube-proxy iptables-tilassa, telemetria pois.
  - podCIDR 10.244.0.0/16, serviceCIDR 10.96.0.0/12. Ei päällekkäisyyttä hostin verkkojen kanssa (192.168.9.0/24 vmbr0, 10.0.0.0/24 vmbr99, 10.99.99.0/24 enp4s0, 100.64.0.0/10 tailscale0).
- Namespace `hydra` luotu.
- Muistinkäyttö: k0s-prosessit yhteensä ~1 Gt RSS. Hostilla jäljellä ~2,9 Gt available (VM:t 100, 103 ja CT 102 vievät loput). Kubelet näkee koko 15 Gt:n, joten selainpodien määrää rajoittaa vain panelin max (oletus 2). Burstable-podit ovat OOM-tilanteessa VM:iä edellä tapettavina.
- Levy: `/` 77 % käytössä (8,8 Gt vapaana). Kubeletin image GC alkaa 85 %:ssa ja poistaa käyttämättömät imaget, siksi tuodut imaget kannattaa pinnata (alla).

### Kubeconfig

- `.secrets/kubeconfig` (0600), server `https://100.121.196.77:6443` (Tailscale). Uudelleenluonti:
  `ssh root@n100-1 k0s kubeconfig admin | sed -E 's#server: https://[^ ]+:6443#server: https://100.121.196.77:6443#' > .secrets/kubeconfig && chmod 600 .secrets/kubeconfig`
- Käyttö: `KUBECONFIG=.secrets/kubeconfig kubectl get nodes`.

### Imagen tuonti (ei registryä)

```
docker build --platform linux/amd64 -t hydra-browser:dev .
docker save --platform linux/amd64 hydra-browser:dev | ssh root@n100-1 k0s ctr -n k8s.io images import -
ssh root@n100-1 k0s ctr -n k8s.io images label docker.io/library/hydra-browser:dev io.cri-containerd.pinned=pinned
```

- Mac on arm64: rakenna aina `linux/amd64`.
- containerd normalisoi nimen: `hydra-browser:dev` -> `docker.io/library/hydra-browser:dev`, `foo/bar:x` -> `docker.io/foo/bar:x`. Podissa riittää lyhyt nimi + `imagePullPolicy: Never`.
- Pinnattua imagea kubelet ei siivoa GC:ssä. Listaus: `ssh root@n100-1 k0s ctr -n k8s.io images ls -q | grep -v sha256`.

### Proxmox-vierasverkko ja k0s

k0s (kube-proxy, kube-router) lataa `br_netfilter`in ja asettaa `net.bridge.bridge-nf-call-iptables=1` (ennen 0). Tämän jälkeen vierasliikenne siltojen yli kulkee iptablesin FORWARD-ketjun läpi, jossa kube-proxyn `KUBE-FORWARD` pudottaa `ctstate INVALID` -paketit (todettu: myös IPv6). kube-proxyn `--conntrack-tcp-be-liberal` ei toimi k0s:ssä, koska kube-proxy ajetaan `--config`-tiedostolla.

Suoja: `pve-forward-guard` pitää FORWARD-ketjun alussa säännöt (iptables ja ip6tables):

```
-A FORWARD -i fwbr+ -m physdev --physdev-is-bridged -m comment --comment pve-guest-bridged -j ACCEPT
-A FORWARD -i vmbr+ -m physdev --physdev-is-bridged -m comment --comment pve-guest-bridged -j ACCEPT
```

- Skripti `/usr/local/sbin/pve-forward-guard.sh` (idempotentti, siirtää säännöt `KUBE-FORWARD`-hypyn yläpuolelle), `pve-forward-guard.service` + `pve-forward-guard.timer` (30 s bootista, sitten minuutin välein).
- Koskee vain sillattua liikennettä. Reititetty liikenne (NodePort, pod-egress, Tailscale) kulkee k8s-sääntöjen kautta kuten ennenkin.

Tarkistus ennen ja jälkeen asennuksen (sama tulos molemmilla kerroilla):

| Testi | Ennen | Jälkeen |
|---|---|---|
| host -> VM 103 (192.168.9.185), CT 102 (192.168.9.158), OPNsense vmbr99 (10.0.0.1), 1.1.1.1 ping | OK | OK |
| host -> OPNsense 192.168.9.2 ping | ei vastaa (ICMP estetty WANissa) | sama |
| host -> 192.168.9.2:443/22, 10.0.0.1:443/22 TCP | auki | auki |
| CT 102 -> 1.1.1.1, -> VM 103 ping; -> 192.168.9.2:443 TCP | OK | OK |
| VM 103 (guest agent) -> 1.1.1.1, -> CT 102, -> purkki 100.111.221.2 ping; https google 200 | OK | OK |
| `pve-firewall status` | disabled/running | sama |
| iptables FORWARD policy | ACCEPT | ACCEPT |

### Testatut toiminnot

- Podi `capabilities.add: [NET_ADMIN]`: `ip link add wg0 type wireguard` onnistuu (wireguard-moduuli kernelissä).
- Podista purkkiin Tailscalen yli: ICMP OK (~42 ms, Tailscale-polku kulkee DERP(hel)-releen kautta, suoraa yhteyttä ei muodostunut), UDP 51820 perillä purkin kontissa. Podin egress SNATataan hostin osoitteeseen (internetiin 212.90.93.49, Tailscaleen 100.121.196.77).
- Polku-MTU n100-1 -> purkki: 1280 (ping `-M do -s 1252` menee läpi), joten wg MTU 1200 sopii.
- NodePort 30080 vastaa Macilta `http://100.121.196.77:30080` ja `http://n100-1:30080`. Huom: heti Servicen luonnin jälkeen kube-proxy voi olla vielä ohjelmoimatta sääntöjä (ensimmäinen yritys: connection refused), joten pollaa.
- NodePortit kuuntelevat kaikilla hostin osoitteilla (myös LAN 192.168.9.101).

### k0s:n poisto

```
ssh root@n100-1 'k0s stop && k0s reset && systemctl disable --now pve-forward-guard.timer && rm /etc/systemd/system/pve-forward-guard.* /usr/local/sbin/pve-forward-guard.sh && systemctl daemon-reload && sysctl -w net.bridge.bridge-nf-call-iptables=0 net.bridge.bridge-nf-call-ip6tables=0'
```

- `k0s reset` jättää iptables-sääntöjä (KUBE-*, KUBE-ROUTER-*) ja ipsetit. k0s-dokumentaatio suosittelee rebootia; Proxmox-hostilla tämä tarkoittaa VM:ien katkoa, joten vaihtoehtona siivoa ketjut käsin (`iptables-save | grep -v KUBE` -tyyppisesti) ja tarkista vierasverkko yllä olevilla testeillä.
- Lopuksi `rm -rf /etc/k0s /usr/local/bin/k0s`.

## purkki (Proxmox VE, kernel 6.8.12-5-pve, exit-kone)

Vain tarkistettu, ei pysyviä muutoksia:

- Docker 29.2.1 (client ja server). Ajossa ennestään kontti `foxhole`, VM 101 `asdf`.
- `pve-firewall status`: disabled/running. iptables FORWARD policy DROP (Docker), `bridge-nf-call-iptables=0`.
- Wireguard: `docker run --cap-add NET_ADMIN alpine:3` -kontissa `ip link add wg0 type wireguard` onnistuu (ei vaadi `--privileged`). Moduuli latautui automaattisesti ja jää ladatuksi rebootiin asti.
- UDP 51820 vapaa. Testikuuntelija `-p 51820:51820/udp` vastaanotti n100-1:n podista Tailscalen yli lähetetyn paketin. Testikontti ja alpine-image poistettu.
- Exit-kontin endpoint: `100.111.221.2:51820`.
