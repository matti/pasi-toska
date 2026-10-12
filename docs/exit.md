# Exit: hydra-exit ja `hydra exit add`

Exit on mikä tahansa kone, jossa on Docker ja ulospääsy internetiin:
pilvi-VM, kotona oleva kone tai NATin takana oleva kone (silloin endpointiksi
sen Tailscale-IP). Exit on yksi kontti, `hydra-exit` (`images/exit/`).

## hydra-exit-kontti

```bash
docker run -d --name hydra-exit --restart unless-stopped --cap-add NET_ADMIN \
  --sysctl net.ipv4.ip_forward=1 -p 51820:51820/udp -v hydra-exit:/etc/wireguard hydra-exit
docker exec hydra-exit hydra-exit-peers 100.111.221.2      # [{"peer":0,"conf":"..."},...]
```

- Ensimmäinen käynnistys luo palvelinavaimen ja peer-poolin (`HYDRA_PEERS`,
  oletus 8) volyymiin. Uudelleenkäynnistys käyttää samoja avaimia.
- Tunneliverkko `10.77.0.0/24` (`HYDRA_SUBNET`), palvelin `.1`, peer n on `.(n+2)`.
- MTU 1200 (`HYDRA_MTU`): toimii myös Tailscalen yli (polku-MTU 1280).
- NAT oletusreitin rajapinnasta ulos. Tunnelista ei pääse yksityisiin
  verkkoihin (10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10).
- Kernelin WireGuard, tai wireguard-go jos moduulia ei ole (`--device /dev/net/tun`).
- Asiakaskonfiguraatio: `DNS = 1.1.1.1, 9.9.9.9` (`HYDRA_DNS`),
  `AllowedIPs = 0.0.0.0/0, ::/0`, `PersistentKeepalive = 25`.

## `hydra exit add`

```bash
cd cli && cargo build --release
./target/release/hydra exit add <nimi> <ssh-kohde> --endpoint <host>[:portti] \
  [--peers 8] [--port 51820] [--label "Kulloo, Finland"] [--country FI] [--city Helsinki] \
  [--tz Europe/Helsinki] [--lang fi_FI.UTF-8] [--kubeconfig .secrets/kubeconfig]
./target/release/hydra exit ls
```

1. `ssh <host> uname -m` -> rakentaa `hydra-exit`-imagen oikealle
   arkkitehtuurille (`docker buildx`, tagi `hydra-exit:amd64` tai `:arm64`).
2. Kopioi imagen (`docker save | ssh docker load`) vain, jos etäkoneella ei
   ole samaa image-id:tä.
3. Ajaa `cli/remote-up.sh`:n etäkoneella: luo kontin uudelleen vain jos
   image, portti tai poolin koko muuttui. Tulostaa exitin julkisen IP:n ja
   peer-JSONin.
4. Tallentaa `.secrets/exits/<nimi>/peer-<n>.conf` ja `exit.json` (0600).
5. Jos `--kubeconfig` tai `KUBECONFIG` on annettu: `kubectl apply --server-side` namespaceen
   `hydra` Secretit `hydra-exit-<nimi>-<n>` (`wg0.conf`, labelit ja
   annotaatiot [contract.md](contract.md):n mukaan). Server-side apply, koska
   tavallinen apply kopioisi Secretin yksityisine avaimineen
   `last-applied-configuration`-annotaatioon.

Ajon voi toistaa milloin tahansa. ssh-käyttäjällä on oltava Docker-oikeus.
Oma ssh-komento: `HYDRA_SSH="ssh -F polku/ssh_config"`.

## Vaihtoehdot exit-koneeksi

- **Kone kotona tai toimistolla, NATin takana**: Tailscale molempiin
  päihin ja endpointiksi Tailscale-IP (purkki: `100.111.221.2`). Ei vaadi
  porttiohjausta. Ilman Tailscalea reitittimeen porttiohjaus UDP 51820.
- **Hetzner Cloud -VM**: pienin jaettu tyyppi riittää (hetzner.com
  2026-10-11: CX23 5,99 €/kk, CPX02 6,49 €/kk, sis. IPv4, alv 0 %; useat
  tyypit merkitty tilapäisesti "unavailable"). Asenna Docker, sitten
  `hydra exit add hel root@<ip> --endpoint <ip> --country FI --city Helsinki --tz Europe/Helsinki --lang fi_FI.UTF-8`.
  Hetzner-palomuurissa salli UDP 51820. Datakeskus-IP saa hakukoneilta
  helpommin captchoja kuin koti- tai yritysliittymä.

## Oikea ympäristö: purkki ja n100-1 (testattu 2026-10-11)

Exit `purkki` (kotiliittymä, Telia, NATin takana) ajaa `hydra-exit`-konttia,
endpoint on sen Tailscale-IP. Selaimet ajetaan n100-1:n k0s:ssä
(ks. [infra.md](infra.md)); podeista purkkiin kuljetaan Tailscalen
DERP-releen kautta (polku-MTU 1280, wg MTU 1200).

```bash
cd cli && cargo build --release
HYDRA_SSH="ssh -o BatchMode=yes" ./target/release/hydra exit add purkki root@purkki \
  --endpoint 100.111.221.2 --country FI --city Helsinki --tz Europe/Helsinki \
  --lang fi_FI.UTF-8 --kubeconfig ../.secrets/kubeconfig
# -> exit 'purkki' ready: public IP 193.211.54.84, endpoint 100.111.221.2:51820, 8 peers

# Selainimage n100-1:lle (containerd, k8s.io), tagi hydra-browser:latest, pinnattu
ssh root@n100-1 'k0s ctr -n k8s.io images import - && \
  k0s ctr -n k8s.io images tag --force docker.io/library/hydra-browser:amd64 docker.io/library/hydra-browser:latest && \
  k0s ctr -n k8s.io images label docker.io/library/hydra-browser:latest io.cri-containerd.pinned=pinned' \
  < dist/hydra-browser-amd64.tar
```

Testipodit (purkki-peer 7 + `NET_ADMIN`, ja suora ilman niitä), tulokset:

| Testi | Tulos |
|---|---|
| `/status` purkin kautta | `193.211.54.84`, Helsinki, `AS1759 Telia Finland Oyj` |
| `/status` suora | `212.90.93.49`, `AS16086 DNA Oyj` |
| Käyttötapaus A (MCP) | DuckDuckGo html: 5 tulosta, ylin avattu snapshot-refillä, teksti poimittu (8,6 s) |
| Käyttötapaus B (MCP) | kaalimato.com: navigaatiosta "Tarjoukset" -> tarjoussivu, tuotteet, alennukset ja hinnat poimittu |
| `docker stop hydra-exit` purkilla | podista: DNS-nimi, IP-osoite ja eth0-ohitus aikakatkaisu, MCP `net::ERR_NAME_NOT_RESOLVED` |
| `docker start hydra-exit` | ulosmeno palasi 4 s:ssa, podia ei käynnistetty uudelleen (restarts=0) |
| noVNC port-forwardin kautta | `vnc.html` 200 |

Podin määrittely (vastaa panelin luomaa):

```yaml
spec:
  containers:
  - name: browser
    image: hydra-browser:latest
    imagePullPolicy: Never
    env: [{name: HYDRA_EXIT, value: purkki}, {name: TZ, value: Europe/Helsinki}, {name: LANG, value: fi_FI.UTF-8}]
    securityContext: {capabilities: {add: [NET_ADMIN]}}
    resources: {requests: {cpu: 500m, memory: 768Mi}, limits: {memory: 2Gi}}
    volumeMounts: [{name: wg, mountPath: /etc/hydra/wg, readOnly: true}, {name: shm, mountPath: /dev/shm}]
  volumes:
  - {name: wg, secret: {secretName: hydra-exit-purkki-7, defaultMode: 0400}}
  - {name: shm, emptyDir: {medium: Memory, sizeLimit: 1Gi}}
```

Huom: n100-1:n levy on 83 % täynnä hydra-browser-imagen jälkeen
(kubeletin image GC alkaa 85 %:ssa; image on pinnattu).

## Vianetsintä

```bash
ssh <host> docker logs hydra-exit
ssh <host> docker exec hydra-exit wg show wg0      # kättelyt ja tavut per peer
docker logs <selain> | grep hydra-net              # selaimen tunnelin tila
```
