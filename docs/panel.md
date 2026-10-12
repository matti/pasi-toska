# hydra-panel

Näkyvä nimi on **PASI TOSKA browser** (otsikko, `<title>`, selaimen IP-sivu); sisäiset nimet (imaget, kube-resurssit, koodi, MCP-nimet `hydra-<id>`, `hydra`-CLI, AWS) ovat hydra. Asettelu toimii myös kapealla näytöllä (testattu 390 ja 320 px): otsikkorivi rivittyy, tehtävärivin painike siirtyy omalle rivilleen, korteissa `min-width: 0`, eikä mikään vieritä sivua vaakasuunnassa; syötteen fonttikoko on mobiilissa 16 px, ettei iOS Safari zoomaa sivua kohdistettaessa. Asettelun tarkistus: `node tests/mcp-client/mcp.mjs <mcp-url> layout <panel-url> 390x844 "Aja kaikissa"`.

Web-käyttöliittymä ja proxy selainpodeille. Rust-crate hakemistossa `panel/` (oma `[workspace]`), ajetaan k0s:ssä Deploymenttina, NodePort 30080. Rajapinnat: `docs/contract.md`.

## Mitä se tekee

- **UI** (`/`): yläpalkissa logo ja solmun oma IP (piste: kube-yhteys). Käynnistysrivi: exit-valikko (lippu, nimi, kaupunki, IP, vapaat peerit; lisäksi "Suora") + Käynnistä. Raja-ilmoitus näkyy vain, kun raja tulee vastaan (toast). Staattiset tiedostot (`panel/static/`, vanilla JS/CSS) upotetaan binääriin `rust-embed`illä; debug-buildi lukee ne levyltä.
- **Kortti**: lippu + exitin nimi, ulospäin näkyvä IP isolla ja sen alla kaupunki ja AS yhdellä rivillä. Tilapiste IP:n vieressä (selitys tooltipissä):
  - vihreä: egress-IP = exitin annotaatio `hydra/ip` (tunneli toimii)
  - punainen, vilkkuu, kortin reuna punainen: exit valittu, mutta egress-IP = solmun oma IP (vuoto)
  - sininen: suora yhteys; keltainen: IP ei vastaa kumpaakaan; harmaa: haetaan

  Live noVNC on kortin pääosa. Ikonipalkki: vain katselu / ohjaus, IP-testi, koko näyttö, oma välilehti, kopioi MCP-URL (shift-klikkaus kopioi koko `claude mcp add` -komennon), sulje.
- **IP-testi**: kun selain on ensimmäistä kertaa valmis, panel avaa siihen `HYDRA_IP_TEST_URL`in (oletus `http://127.0.0.1:8080/ip`, hydra-browserin oma sivu) MCP:n `browser_navigate`lla. Sivu näyttää julkisen IP:n koko näytön levyisenä, lipun, kaupungin ja maan, operaattorin, WebRTC-osoitteen (STUN, vihreä jos sama kuin IP, punainen jos eri) ja DNS-resolverin; tiedot haetaan selaimesta tunnelin läpi (ipinfo.io, Googlen ja Cloudflaren STUN, edns.ip-api.com) ja päivitetään 30 s välein. Isot fontit, jotta IP näkyy kortin live-näkymässä. Painike `POST /api/browsers/<id>/iptest` avaa sen uudelleen. Panelin käynnistyessä jo valmiita selaimia ei ladata uudelleen.
- **Elävä tila**: `GET /api/events` (SSE) lähettää koko tilannekuvan aina kun se muuttuu. Podit ja exit-Secretit tulevat kube-rs-reflektoreista (watch), podien `/status` kysytään sekunnin välein (aikakatkaisu 1,5 s). Solmun oma IP haetaan panelista `https://ipinfo.io/json` (10 min välein; ohitus `HYDRA_NODE_IP`).
- **Käynnistys** (`POST /api/browsers {"exit": "<nimi>"|"direct"}`): sarjallistettu lukolla, tuore pod-lista API:sta (ei välimuistista). Raja `HYDRA_MAX_BROWSERS` lasketaan podeista, joilla ei ole `deletionTimestamp`ia. Peeriksi valitaan pienin vapaa; myös sammuva podi pitää peerinsä. Podin rakenne: `panel/src/podspec.rs` (sopimuksen mukainen, lisäksi `automountServiceAccountToken: false`, `terminationGracePeriodSeconds: 2`).
- **Sulkeminen**: `DELETE /api/browsers/<id>` (grace 1 s).

## Korttien työkalurivi

Ikonit vasemmalta: katselu/ohjaus, IP-testi, sormenjälkitesti (avaa `HYDRA_FP_TEST_URL`, oletus deviceandbrowserinfo.com/are_you_a_bot, koska se antaa selvän tuomion syineen; BrowserScan ja CreepJS testattiin myös), uusinta (viimeiset 10 s videona kortin päällä), exitin vaihto lennosta (valikko muista exiteistä, joilla on vapaa peer; piilossa suorilla selaimilla), koko näyttö, avaa välilehdessä, kopioi MCP-URL, sulje.

- Exitin vaihto: `POST /api/browsers/<id>/exit {"exit"}`. Panel siirtää podin `hydra/exit`- ja `hydra/peer`-labelit uuteen peeriin (vanha vapautuu), lähettää uuden wg0.conf:n podin porttiin 8081 ja avaa IP-sivun, jolloin live-näkymä näyttää uuden IP:n. Epäonnistuessa labelit palautetaan. Roolissa tarvitaan `patch` podeille. Mitattu: vaihto 0,15 s, uusi IP statuksessa 1,4 s.
- Uusinta: `GET /b/<id>/replay.mp4` -> pod:8080/replay.mp4.
- Sormenjälkitesti: `POST /api/browsers/<id>/fptest`.

## Proxy

Vain panelin portti näkyy ulos. Podin IP haetaan reflektorista labelilla `hydra/id`.

| Polku | Kohde |
|---|---|
| `/b/<id>/vnc/*` | `pod:6080/*` (HTTP + websocket-upgrade) |
| `/b/<id>/mcp`, `/b/<id>/mcp/*` | `pod:8931/mcp...` (metodi, otsakkeet ml. `Mcp-Session-Id`, SSE virtaa puskuroimatta) |
| `/b/<id>/status` | `pod:8080/status` |

- Tavalliset pyynnöt kulkevat reqwestillä virtana molempiin suuntiin (ei kokonaisaikakatkaisua, ei uudelleenohjausten seurantaa, hop-by-hop-otsakkeet pois, Host = podin osoite).
- Upgrade-pyynnöt (noVNC `/websockify`) tunneloidaan tavu tavulta hyperin raa'an HTTP/1-yhteyden yli (`copy_bidirectional`), joten aliprotokollat kulkevat sellaisenaan.
- noVNC-iframe: `vnc.html?...&path=b/<id>/vnc/websockify&host=<selaimen host>&port=<portti>`. Host ja portti annetaan aina, koska noVNC 1.5+ tulkitsee muuten `path`in suhteessa sivun osoitteeseen. Iframe on samaa alkuperää, joten UI piilottaa noVNC:n oman työkalupalkin.

## Agentti

Claude ohjaa selaimia niiden MCP:n kautta käyttäjän Claude-tilauksella (ei API-avainta), ks. [agent.md](agent.md). Näkyy, kun Secret `hydra-claude` on olemassa (mountattu `/etc/hydra/claude`, `optional: true`, luetaan joka ajossa, ei uudelleenkäynnistystä).

- UI: korttien yläpuolella yksi tehtävärivi (syöte, "Aja kaikissa" ajaa saman tehtävän rinnakkain kaikissa valmiissa selaimissa, sama nappi pysäyttää; x-painike tyhjentää syötteen ja palauttaa kohdistuksen siihen, näkyy vain kun syötteessä on tekstiä) ja kaksi esivalintaa (A: tutkimus hakukoneesta alkaen, B: lokakuun tarjoukset kaalimato.com). Kortin alla tiivis askelloki (yksi rivi per työkalukutsu, 5 viimeisintä) ja lopullinen vastaus linkkeineen. Otsikkorivillä pienellä Claude-tunnuksen jäljellä oleva aika.
- `POST /api/browsers/<id>/agent {"task": ...}` palauttaa NDJSON-virran (`text`, `tool`, `tool_result`, `final`, `error`); 409, jos tunnusta ei ole tai se vanhenee alle 10 minuutissa. Pyynnön katkaisu lopettaa ajon.
- MCP-asiakas: `rmcp` (Streamable HTTP) suoraan podiin, `tools/list` -> Clauden client-työkalut, `tool_use` -> `tools/call`.
- Messages API raakana HTTP:nä: `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20,...`, ensimmäinen system-lohko on tilaustunnuksen vaatima Claude Code -tunniste. Malli `claude-opus-5-5` (`HYDRA_AGENT_MODEL`), `thinking: adaptive` + `display: "updates"` (edistymisviestit lokiin), `effort: medium`, `fallbacks: "default"`, automaattinen prompt-välimuisti, vanhat työkalutulokset siivotaan palvelimella (`clear_tool_uses`, yli 60k syötetokenia), enintään 25 kierrosta. 429 = "tilauksen raja tuli vastaan", ei uudelleenyrityksiä.

## Ympäristömuuttujat

| Muuttuja | Oletus |
|---|---|
| `HYDRA_LISTEN` | `0.0.0.0:8080` |
| `HYDRA_NAMESPACE` | `hydra` |
| `HYDRA_MAX_BROWSERS` | `2` |
| `HYDRA_BROWSER_IMAGE` | `hydra-browser:latest` |
| `HYDRA_RUNTIME_CLASS` | ei asetettu (klusterin oletus); n100-1:llä `kata` (microVM) |
| `HYDRA_BROWSER_MEMORY_LIMIT`, `HYDRA_BROWSER_CPU_LIMIT` | `2Gi`, ei CPU-limittiä; n100-1:llä `1280Mi`, `2` (Kata-VM:n koko) |
| `HYDRA_BROWSER_PULL_POLICY` | `IfNotPresent` (manifestissa `Never`) |
| `HYDRA_DIRECT_TZ`, `HYDRA_DIRECT_LANG` | `Europe/Helsinki`, `fi_FI.UTF-8` (suorat selaimet ja exitit ilman annotaatiota) |
| `HYDRA_NODE_IP` | haetaan ipinfo.io:sta |
| `HYDRA_IP_TEST_URL` | `http://127.0.0.1:8080/ip` |
| `HYDRA_FP_TEST_URL` | `https://deviceandbrowserinfo.com/are_you_a_bot` |
| `HYDRA_CLAUDE_CREDENTIALS`, `HYDRA_AGENT_MODEL` | `/etc/hydra/claude/credentials.json`, `claude-opus-5-5` |
| `RUST_LOG` | `info` |

Kube-yhteys: in-cluster, muuten `KUBECONFIG` / `~/.kube/config`.

## Käyttöönotto

```
kubectl apply -f panel/deploy/panel.yaml   # SA, Role, RoleBinding, Deployment, Service
panel/ship.sh                              # build linux/amd64, import + pin n100-1:lle, rollout restart
```

- Role: pods get/list/watch/create/delete, secrets get/list/watch (watch tarvitaan exit-reflektoriin).
- Dockerfile ristikääntää natiivisti (ei emulointia), `rust:1-slim-trixie` -> `distroless/cc-debian13:nonroot`. Puhdas build ~50 s, image ~18 Mt.
- Manifesti ei sisällä namespacea (`kubectl create namespace hydra` erikseen), joten `kubectl delete -f panel/deploy/panel.yaml` ei koske exit-Secretteihin.

## Testit

- `cd panel && cargo test`: puhdas logiikka (pod-spec, peerien varaus, tila, tuomio, id:t, proxy-polut, otsakkeet, MCP->Claude-muunnos), alle sekunti.
- `HYDRA_MCP_PROBE=host:port cargo test mcp_probe -- --ignored --nocapture`: MCP-asiakas oikeaa hydra-browseria vasten (esim. `docker run -p 127.0.0.1:18931:8931 hydra-browser`).
- `panel/stub/`: kevyt korvike-image (oikea noVNC + Xvfb, `/status`, minimaalinen MCP) UI:n ja proxyjen testaamiseen ilman Chromiumia: `HYDRA_BROWSER_IMAGE=hydra-browser-stub:latest`.
