# hydra-browser

Yksi image (`images/browser/`), ei kube-riippuvuutta: sama image toimii
`docker run`illa ja kube-podina. Pohja `node:24-trixie-slim` + Debianin
`chromium` (154) + `@playwright/mcp` 0.0.83 + noVNC 1.7.0.

## Prosessit

`tini` -> `start.sh`:

1. Jos `/etc/hydra/wg/wg0.conf` on olemassa: `net-up.sh` rootina
   (killswitch, DNS, WireGuard). Muuten suora ulosmeno.
2. Loput ajetaan käyttäjänä `hydra` ilman capabilityjä
   (`setpriv --inh-caps=-all --no-new-privs`): Xvfb, x11vnc (vain
   localhost), Chromium, Playwright MCP ja `server.mjs` (Node: status :8080
   sekä noVNC-tiedostot ja websocket-silta x11vnc:hen :6080; ei Pythonin
   websockifyä, jonka numpy-riippuvuus kaatui amd64-ristikäännöksessä).
   Vaikka renderöijä murrettaisiin, se ei voi koskea palomuuriin.
3. Jos jokin prosessi kuolee, kontti päättyy (restart-politiikka hoitaa).

Chromium on headful (Xvfb, oletus 1366x768), `--disable-blink-features=AutomationControlled`
(`navigator.webdriver` on false), `--test-type` piilottaa `--no-sandbox`-varoituspalkin.
CDP kuuntelee vain `127.0.0.1:9222`.

Evästebannerit: imagessa on "I still don't care about cookies" -laajennus (ylläpidetty avoimen lähdekoodin haara, github.com/OhMyGuus/I-Still-Dont-Care-About-Cookies), kiinnitetty julkaisuun 1.1.9 ja SHA-256-tarkistettu, ladattu `--load-extension`illa ja `--disable-extensions-except`illa (Debianin Chromium tarvitsee lisäksi `--disable-features=DisableLoadExtensionCommandLineSwitch`). Laajennuksen hakemisto `/opt/isdcac` kuuluu käyttäjälle `hydra`, koska Chromium kirjoittaa sinne declarativeNetRequest-sääntöjen indeksin; ilman kirjoitusoikeutta lataus kaatuu modaaliseen virheeseen eikä selain käynnisty. Tarkistettu Playwright MCP:n kautta: hs.fi, yle.fi, iltalehti.fi, ilmatieteenlaitos.fi ja kaalimato.com ilman evästebanneria.

## microVM (Kata Containers)

Sama image toimii sellaisenaan Kata-podina (n100-1:llä jokainen selain on oma Cloud Hypervisor -VM). Jos kernelissä ei ole WireGuardia (Katan vieraskernel), `net-up.sh` luo TUN-laitteen ja käyttää wireguard-go:ta; killswitch ja vaihto toimivat samoin. x11vnc ajetaan `ulimit -n 4096`:llä, koska valtava `nofile`-raja saa sen pyörimään 100 % CPU:lla. Mittaukset ja asennus: [infra.md](infra.md).

## Portit ja ympäristö

| Portti | Mitä |
|---|---|
| 6080 | noVNC `/vnc.html`, websocket `/websockify`, ei salasanaa (vain sisäverkkoon) |
| 8931 | Playwright MCP, Streamable HTTP `/mcp` (myös vanha SSE `/sse`) |
| 8080 | `GET /status` -> `{"ready","exit","egress":{ip,city,region,country,org,timezone}}` |
| 8080 | `GET /ip`: iso IP-sivu (IP, lippu, kaupunki, operaattori, WebRTC/STUN, DNS-resolveri, sormenjälkirivit), panelin IP-testi |
| 8080 | `GET /replay.mp4`: näytön viimeiset noin 10 s |
| 8081 | vain tunnelitilassa: `POST /exit`, exitin vaihto lennosta (root, vain podin ulkopuolelta) |

| Muuttuja | Oletus | Mitä |
|---|---|---|
| `HYDRA_EXIT` | - | Exitin nimi statukseen |
| `HYDRA_LOCAL_CIDRS` | `10.0.0.0/8,172.16.0.0/12,192.168.0.0/16` | Verkot, joista saa avata yhteyden palveluportteihin ja joihin vastaukset menevät tunnelin ohi |
| `HYDRA_SCREEN` | `1366x768` | Näytön ja ikkunan koko |
| `TZ`, `LANG` | UTC, en_US | Aikavyöhyke ja kieli (`--lang`, `--accept-lang`); valitse exitin sijainnin mukaan |
| `HYDRA_WG_USERSPACE` | 0 | 1 = pakota wireguard-go |
| `PLAYWRIGHT_MCP_*` | - | Suoraan Playwright MCP:lle, esim. `PLAYWRIGHT_MCP_CAPS=storage,pdf` |

Profiili on `/data/profile` (VOLUME): kirjautumiset säilyvät, jos sen
mounttaa. `/dev/shm`: dockerissa `--shm-size 1g`, kubessa emptyDir Memory.

## Tunneli ja killswitch (`net-up.sh`)

Järjestys on tärkeä: killswitch ensin, tunneli vasta sitten.

1. iptables: INPUT, OUTPUT ja FORWARD oletuksena DROP. Ulos sallitaan vain
   `lo`, `wg0` ja UDP exitin endpointiin. Sisään sallitaan vastaukset ja
   uudet TCP-yhteydet porteihin 6080, 8080, 8931 vain `HYDRA_LOCAL_CIDRS`-verkoista.
   Ulos eth0:aan sallitaan muuten vain ESTABLISHED/RELATED-vastaukset
   `HYDRA_LOCAL_CIDRS`-verkkoihin. IPv6: kaikki DROP paitsi loopback.
2. DNS: `/etc/resolv.conf` = konfiguraation `DNS` (oletus 1.1.1.1, 9.9.9.9),
   kyselyt kulkevat tunnelin läpi. Klusterin tai Dockerin DNS ei ole käytössä.
3. WireGuard: kernel, tai wireguard-go jos kernelin moduulia ei ole
   (vaatii `--device /dev/net/tun`). Endpoint resolvoidaan ennen killswitchiä.
4. Reititys kuten wg-quick (fwmark 51820 + oma taulu, oletusreitti wg0).
   `HYDRA_LOCAL_CIDRS` saavat main-tauluun reitin alkuperäisen
   yhdyskäytävän kautta, jotta vastaukset dockerin/kuben asiakkaille menevät
   eth0:aan eivätkä tunneliin.

Vaatii `NET_ADMIN`-capabilityn (vain kun tunneli on käytössä).
Kubernetes-klusterin on sallittava se podille (PodSecurity `baseline` ei
salli, `privileged` sallii, tai oma poikkeus).

Asiakkaan IP:n on oltava `HYDRA_LOCAL_CIDRS`-verkossa. Docker-portin
julkaisussa paikallinen asiakas näkyy Dockerin yhdyskäytävänä (172.17.0.1),
mutta ulkoinen asiakas omalla IP:llään: esimerkiksi Tailscale-asiakkaille
lisää `100.64.0.0/10`.

## Exitin vaihto lennosta (`control.mjs`, `net-up.sh --swap`)

Tunnelitilassa root-prosessi kuuntelee porttia 8081: `POST /exit {"exit","conf"}` vaihtaa exitin ilman selaimen uudelleenkäynnistystä. Paikalliset asiakkaat (loopback ja podin omat osoitteet, eli selain ja sen sivut) saavat 403, joten vain panel tai muu `HYDRA_LOCAL_CIDRS`-verkon asiakas voi vaihtaa exitin.

Vaihdon aikana killswitch ei aukea hetkeksikään: OUTPUT-oletus pysyy DROP ja wg0 pysyy oletusreittinä. Uuden exitin UDP sallitaan ensin, sitten avain ja peer vaihdetaan yhdellä `wg set` -kutsulla, osoite lisätään ennen vanhan poistoa, DNS kirjoitetaan uusiksi ja vanhan exitin UDP poistetaan lopuksi. Välissä lähetetyt paketit menevät wg0:aan vanhalle tai uudelle peerille, eivät koskaan eth0:aan. Nykyinen konfiguraatio tallennetaan `/var/lib/hydra/net/wg0.conf` (kubessa emptyDir), joten kontin uudelleenkäynnistys pitää vaihdetun exitin. Status (`exit`, `egress`) päivittyy heti.

Ei vaihdu: käynnissä olevan Chromiumin `TZ` ja `LANG` (ne asetetaan käynnistyksessä), joten vaihdon jälkeen aikavyöhyke ja kieli voivat poiketa uudesta exitistä (näkyy IP-sivun sormenjälkiriveillä punaisena). Suorasta yhteydestä tunneliin tai takaisin ei vaihdeta.

`tests/e2e.sh` vaihtaa exitin toiseen exit-konttiin liikenteen aikana ja tarkistaa tcpdumpilla, ettei eth0:lta lähde mitään muuta kuin WireGuard- ja palveluporttiliikennettä (satoja WireGuard-paketteja uudelle exitille, 0 muuta pakettia).

## Uusinta (`GET /replay.mp4`)

ffmpeg tallentaa Xvfb-näyttöä jatkuvasti: x11grab 10 fps, skaalaus 1024 leveyteen, libx264 ultrafast, 2 s segmentit, 7 tiedoston rengas `/dev/shm/replay`. `GET :8080/replay.mp4` liittää uusimmat 6 segmenttiä ilman uudelleenkoodausta (noin 10 s, 0,2 s, noin 400 kt). Mitattu n100-1:llä: noin 8-9 % yhdestä N100-ytimestä ja 74 Mt muistia per selain. Jos ffmpeg kaatuu, se käynnistetään uudelleen eikä kontti kaadu. Video on H.264: toimii Chromessa, Safarissa ja Debianin Chromiumissa, ei Playwrightin oman Chromiumin kanssa.

## IP-sivun sormenjälkirivit

`/ip` näyttää IP:n lisäksi: `navigator.webdriver`, aikavyöhyke exitin aikavyöhykkeeseen verrattuna, kielet exitin maahan verrattuna (vihreä = täsmää, punainen = ei), alusta, WebGL-valmistaja ja user agent.

## Vuodot

| Vuoto | Suojaus |
|---|---|
| Liikenne tunnelin ohi | OUTPUT DROP muualle kuin wg0:aan; testattu `curl --interface eth0` |
| Exit kaatuu | Ei paluuta suoraan: testattu, DNS, IP-osoite ja MCP-navigointi epäonnistuvat |
| DNS | Vain tunnelin takaisiin resolvereihin |
| WebRTC/QUIC | Kaikki UDP wg0:n kautta, STUN näkee exitin IP:n |
| IPv6 | Estetty kokonaan |
| Selain klusterin sisäverkkoon | Uudet yhteydet `HYDRA_LOCAL_CIDRS`-verkkoihin estetty (vain vastaukset) |
| Selain exitin lähiverkkoon | Exit pudottaa tunnelista yksityisiin verkkoihin menevät paketit |

## Sormenjälki: mitä nykyinen asetelma vuotaa (mitattu 2026-10-12, ei vielä korjattu)

| Testi | Tulos |
|---|---|
| deviceandbrowserinfo.com/are_you_a_bot | "You are a bot": `isAutomatedWithCDP`, `isAutomatedWithCDPInWebWorker` (Playwright MCP:n CDP-yhteys näkyy sivulle), `hasInconsistentTimingResolution` |
| CreepJS | 0 % headless, 0 % stealth, 44 % "like headless"; ei WebGL:ää, WebGPU ei tuettu |
| BrowserScan bot detection | "Normal" (webdriver, CDP, user agent, navigator kunnossa) |
| oma `/ip` | webdriver false; aikavyöhyke ja kieli vastaavat exitiä; alusta Linux x86_64; WebGL puuttuu kokonaan |

Suurimmat vuodot: CDP-automaation tunnistus ja puuttuva WebGL (ohjelmistorenderöinti ei ole päällä). Lisäksi kaikki selaimet ovat samaa Linux-Chromiumia samalla näytön koolla.

## Mitä ei ole katettu

- Selaimen sormenjälki: kaikki hydra-selaimet ovat samaa Chromium-versiota
  samalla Linuxilla, fonteilla ja näytön koolla. Aikavyöhyke ja kieli
  asetetaan exitin mukaan (`TZ`, `LANG`), mutta esimerkiksi user agent
  kertoo Linuxin.
- Captchat: hakukoneista Google antaa captchan automaatiolle; ohjeissa
  käytetään DuckDuckGo HTML:ää, Bingiä ja Brave Searchia.
- Useampi agentti samassa selaimessa: MCP-istunnot jakavat saman selaimen
  välilehdet. Rinnakkaistyöhön käytetään useampaa selainta.
- `browser_file_upload` lukee tiedostot selainkontista, ei agentin koneelta.
