# Rajapintasopimus (osien väliset liitokset)

Tämä tiedosto on lähde, jota kaikki osat noudattavat. Muutos tänne ensin, sitten koodiin.

## Kokonaiskuva

```
selain (ihminen) ──> hydra-panel (axum, k0s Deployment, NodePort 30080)
                      ├─ /                 UI: exit-valikko, selainkortit, live noVNC
                      ├─ /api/...          JSON
                      ├─ /b/<id>/vnc/...   noVNC-proxy (http + websocket) -> pod:6080
                      ├─ /b/<id>/mcp       MCP-proxy (Streamable HTTP, SSE läpi) -> pod:8931/mcp
                      ├─ /b/<id>/status    -> pod:8080/status
                      └─ /b/<id>/replay.mp4 -> pod:8080/replay.mp4
                             │ exitin vaihto: POST pod:8081/exit (suoraan, ei proxyn kautta)
                             │ kube API (kube-rs)
                             v
                      Pod hydra-b-<id> (image hydra-browser)
                        Chromium (headful, Xvfb) + noVNC + MCP + status
                        WireGuard + killswitch, jos /etc/hydra/wg/wg0.conf on olemassa
                             │ UDP 51820 (tunneli)
                             v
                      exit-kone: kontti hydra-exit (WireGuard-palvelin + NAT)
```

## Image `hydra-browser`

- Yksi image, ei kube-riippuvuutta: toimii `docker run`illa ja podina sellaisenaan.
- Portit (kaikki 0.0.0.0 kontin sisällä):
  - `6080` noVNC: `/vnc.html` ja websocket `/websockify` (VNC ilman salasanaa tai vakio, vain kontin sisäverkossa).
  - `8931` MCP Streamable HTTP polussa `/mcp` (valittu ohjaustyökalu, kiinnitetty Chromiumiin CDP:llä).
  - `8080` status: `GET /status` -> `{"ready":bool,"exit":string|null,"exit_label":string|null,"egress":{"ip","city","region","country","org","timezone"}|null}`. `egress` haetaan tunnelin läpi (esim. ipinfo.io), välimuisti ~60 s. `ready` = Chromium + MCP vastaavat ja (jos tunneli) wg0 ylhäällä.
  - `8080` lisäksi: `GET /ip` (iso IP-sivu ja sormenjälkirivit, panelin IP-testi), `GET /replay.mp4` (näytön viimeiset ~10 s, 503 jos ei vielä tallennetta).
  - `8081` vain tunnelitilassa, root-prosessi: `POST /exit {"exit": "<nimi>", "conf": "<wg0.conf>", "label": "<näyttönimi>"}` vaihtaa exitin lennosta (wg0 pysyy ylhäällä, killswitch ei aukea hetkeksikään). Hylkää kaikki paikalliset asiakkaat (403: loopback ja podin omat osoitteet, eli selain itse); vastaa `{"ok":true,"exit":...}` tai `{"error":...}`. Statuksen `exit` ja `egress` päivittyvät heti.
  - CDP (9222) ei koskaan kuuntele muuta kuin 127.0.0.1.
- Konfiguraatio:
  - Valinnainen tiedosto `/etc/hydra/wg/wg0.conf` (kube Secret -mount tai docker -v). Jos olemassa: killswitch ensin, sitten tunneli. Vaatii `NET_ADMIN`.
  - `HYDRA_EXIT` exitin nimi (näytetään statuksessa), `HYDRA_EXIT_LABEL` sen näyttönimi (statuksen `exit_label`, IP-sivu näyttää sen kaupungin sijaan).
  - `HYDRA_LOCAL_CIDRS` oletus `10.0.0.0/8,172.16.0.0/12,192.168.0.0/16`: näihin vain vastaukset sisääntuleviin yhteyksiin, eivät tunneliin.
  - `TZ`, `LANG` (esim. `Europe/Helsinki`, `fi_FI.UTF-8`): selaimen aikavyöhyke ja kieli vastaamaan exitin sijaintia.
- `/dev/shm`: kube-podissa emptyDir medium Memory, dockerissa `--shm-size 1g`.

## Image `hydra-exit`

- WireGuard-palvelin + NAT konttina: `docker run -d --name hydra-exit --restart unless-stopped --cap-add NET_ADMIN --sysctl net.ipv4.ip_forward=1 -p 51820:51820/udp -v hydra-exit:/etc/wireguard hydra-exit`.
- Ensimmäisellä käynnistyksellä luo palvelinavaimen ja peer-poolin (oletus 8 peeriä), tilan volumeen. Uudelleenkäynnistys on idempotentti.
- `docker exec hydra-exit hydra-exit-peers <endpoint-host>` tulostaa JSONin: `[{"peer":0,"conf":"<wg0.conf sisältö>"},...]`. Client-confissa `AllowedIPs = 0.0.0.0/0, ::/0`, DNS tunnelin kautta, MTU sopii myös Tailscalen yli ajettuna (1280-pohjainen polku -> wg MTU 1200).
- Exit-kone voi olla NATin takana: endpointiksi käy sen Tailscale-IP (purkki: 100.111.221.2).

## Exit-rekisteri kubessa (namespace `hydra`)

- Yksi Secret per exit-peer: nimi `hydra-exit-<exit>-<n>`, data-avain `wg0.conf`, labelit `hydra/exit=<exit>`, `hydra/peer=<n>`; annotaatiot `hydra/label` (UI:n näyttönimi, esim. `Kulloo, Finland`; korvaa UI:ssa exitin nimen ja ipinfo.io:n kaupunkiarvauksen), `hydra/country`, `hydra/city`, `hydra/tz`, `hydra/lang`, `hydra/ip` (exitin julkinen IP). Sisäinen nimi (Secretit, labelit, kone) pysyy, esim. `purkki`.
- Panel varaa vapaan peerin: peer on varattu, jos jollain podilla on labelit `hydra/exit=<exit>,hydra/peer=<n>`.
- Exit `direct` (ei Secretiä) = suora ulosmeno solmun omasta IP:stä.

## Selainpod (panel luo)

- Nimi `hydra-b-<id>` (id = 6 merkkiä [a-z0-9]), labelit `app=hydra-browser`, `hydra/id`, `hydra/exit`, `hydra/peer`.
- Secret mountattuna `/etc/hydra/wg` (vain kun exit != direct), `capabilities.add: [NET_ADMIN]` vain silloin, ja emptyDir `/var/lib/hydra/net` (vaihdetun exitin wg0.conf säilyy kontin uudelleenkäynnistyksen yli).
- Exitin vaihto: panel siirtää labelit `hydra/exit` ja `hydra/peer` uuteen peeriin (patch), sitten POST pod:8081/exit; epäonnistuessa labelit palautetaan. Vain tunneliselaimet ja vain exitistä toiseen (ei suoraan yhteyteen). Käynnissä olevan Chromiumin `TZ`/`LANG` eivät vaihdu.
- Resurssit: requests cpu 500m / mem 768Mi, limits mem 2Gi (`HYDRA_BROWSER_MEMORY_LIMIT`, CPU-limit `HYDRA_BROWSER_CPU_LIMIT` valinnainen).
- `runtimeClassName` valinnainen (`HYDRA_RUNTIME_CLASS`), n100-1:llä `kata`: jokainen selain omassa microVM:ssä (Kata + Cloud Hypervisor). Image ei muutu; docker run toimii kuten ennen. Max samanaikaiset selaimet konfiguroitava (oletus 2, n100-1:llä on vähän muistia).

## Agentin näkökulma

- MCP-osoite: `http://<panel>/b/<id>/mcp`, esim. `claude mcp add --transport http hydra-<id> http://n100-1:30080/b/<id>/mcp`.

## Panelin API (lisäykset)

- `POST /api/browsers/<id>/exit {"exit": "<nimi>"}` -> 204 / virhe `{"error"}`.
- `POST /api/browsers/<id>/fptest` avaa botintunnistustestin (`HYDRA_FP_TEST_URL`) selaimessa.
- `POST /api/browsers/<id>/agent/resume` jatkaa `ask_human`-tauolla odottavaa agenttiajoa (404 jos mikään ei odota). Agentin NDJSON-virrassa uudet askeleet `{"type":"ask_human","call","reason"}` ja `{"type":"resumed"}`.
