# hydra: pikaopas

hydra antaa AI-agentille oikean selaimen, jonka liikenne lähtee internetiin
valitusta "exitistä" (toinen kone, toinen julkinen IP) WireGuard-tunnelilla.
Agentti ohjaa selainta MCP:llä verkon yli; ihminen voi katsoa noVNC:llä.

- [contract.md](contract.md): osien väliset rajapinnat (portit, Secretit, labelit).
- [browser.md](browser.md): `hydra-browser`-image, killswitch, vuodot, rajoitukset.
- [exit.md](exit.md): `hydra-exit`-image ja `hydra exit add`.
- [infra.md](infra.md): n100-1 (k0s) ja purkki.
- [panel.md](panel.md) ja [agent.md](agent.md): web-UI ja Claude-agentti (`hydra login`).
- Agentin ohje: [../skills/hydra-browser/SKILL.md](../skills/hydra-browser/SKILL.md).

## Osat

| Osa | Missä | Mitä |
|---|---|---|
| `hydra-browser` | `images/browser/` | Chromium (headful, Xvfb), noVNC :6080, Playwright MCP :8931 `/mcp`, status :8080, valinnainen WireGuard + killswitch |
| `hydra-exit` | `images/exit/` | WireGuard-palvelin + NAT konttina, peer-pooli, `hydra-exit-peers` |
| `hydra` CLI | `cli/` | `hydra exit add` (exit-kontti ssh:lla mille tahansa docker-hostille, peerit paikallisesti ja kube-Secreteiksi), `hydra exit ls` |
| panel | `panel/` | Selainten käynnistys kubessa ja web-UI (oma dokumentaationsa) |

## Ohjaustyökalun valinta

Valittu **Playwright MCP** (`@playwright/mcp` 0.0.83, tarkistettu npm:stä
2026-10-11). Perusteet:

- Streamable HTTP suoraan (`--port`, `--host`, `--allowed-hosts`): agentti
  voi olla eri koneella tai kube-podissa ja saa pelkän URL:n.
- Kiinnittyy olemassa olevaan Chromiumiin CDP:llä (`--cdp-endpoint`), joten
  selain on meidän: headful, oma profiili, tunneli ja killswitch.
- Laajin työkalusarja: saavutettavuussnapshot ref-viitteillä, click, type,
  fill_form, select, evaluate, screenshot, tabs, file_upload, wait_for,
  network_requests, dialogit, `run_code_unsafe` kaikkeen muuhun.
- Vertailu: chrome-devtools-mcp toimii README:n mukaan stdio:lla ja tukee
  virallisesti vain Chromea (ei Chromiumia), ja se kerää käyttötilastoja
  oletuksena. agent-browser on Bash-CLI ilman MCP-palvelinta, eli se ei sovi
  verkon yli ohjattavaksi.

## Pikakäyttö paikallisesti (docker)

```bash
docker build -t hydra-browser images/browser
docker build -t hydra-exit images/exit

# Suora ulosmeno (ei exitiä)
docker run -d --name b1 --shm-size 1g \
  -p 127.0.0.1:8931:8931 -p 127.0.0.1:6080:6080 -p 127.0.0.1:8080:8080 \
  -e TZ=Europe/Helsinki -e LANG=fi_FI.UTF-8 hydra-browser
curl -s 127.0.0.1:8080/status
claude mcp add --transport http hydra-b1 http://127.0.0.1:8931/mcp
open http://127.0.0.1:6080/vnc.html     # katselu

# Exitin kautta: peer-konfiguraatio mountattuna, NET_ADMIN
docker run -d --name b2 --shm-size 1g --cap-add NET_ADMIN \
  -v "$PWD/.secrets/exits/purkki/peer-7.conf:/etc/hydra/wg/wg0.conf:ro" \
  -e HYDRA_EXIT=purkki -e TZ=Europe/Helsinki -e LANG=fi_FI.UTF-8 \
  -p 127.0.0.1:18931:8931 -p 127.0.0.1:16080:6080 -p 127.0.0.1:18080:8080 hydra-browser
```

Exitin pystytys: [exit.md](exit.md).

## Testit

```bash
(cd cli && cargo test)     # CLI:n puhdas logiikka, alle sekunti
tests/e2e.sh               # docker-e2e, n. 40 s, siivoaa jälkensä
```

`tests/e2e.sh`: `hydra exit add` paikallista dockeria vastaan
(`tests/fake-ssh`, `tests/fake-kubectl`), selain exitin takana, MCP:llä
käyttötapaus A (DuckDuckGo-haku, tuloksen avaus snapshot-refillä, teksti) ja
B (annetun sivun luku), tunnelin laskurit exitillä, killswitch (eth0-ohitus,
käyttäjä `hydra` ei voi muuttaa iptablesia, exit alas = ei mitään
ulosmenoa) ja suora selain. Exit on samalla koneella, joten julkinen IP on
testissä sama kuin hostilla. MCP-asiakas: `tests/mcp-client/mcp.mjs`
(`tools`, `call`, `search`, `read`).
