# Agentti: Claude-tilaus selainten ohjaimena

Panelin agentti ajaa saman tehtävän rinnakkain jokaisessa käynnissä olevassa selaimessa (esim. purkki ja toinen exit) ja näyttää jokaisen kortin alla askeleet ja vastauksen. Ihminen näkee työn live-näkymästä.

## Tunnus: `hydra login`

Agentti käyttää käyttäjän Claude-tilausta, ei API-avainta. Tunnus on Claude Coden OAuth-token macOS:n Keychainissa.

```bash
cd cli && cargo build --release
./target/release/hydra login              # Keychain -> Secret hydra-claude (namespace hydra)
./target/release/hydra login --install    # + launchd-agentti, ajaa saman 10 min välein
```

- Lukee Keychain-kohteen `Claude Code-credentials`. Jos se on tyhjä tai vanhenemassa, käytetään tuoreinta `Claude Code-credentials-<hash>`-kohdetta (Max-tilaus ensin). Kohteen voi valita: `--item`.
- Vaatii vähintään 60 min voimassaoloa, muuten virhe (aja `claude` kerran, se uusii tokenin).
- Secretiin menee vain `accessToken`, `expiresAt` ja `subscriptionType`, ei refresh-tokenia. Token ei tulostu mihinkään. Kirjoitus `kubectl apply --server-side`, jolloin Secret ei kopioidu `last-applied-configuration`-annotaatioon.
- `--install` kopioi binäärin `~/.local/bin/hydra`, kirjoittaa `~/Library/LaunchAgents/dev.hydra.login.plist` (`StartInterval` 600 s, loki `~/Library/Logs/hydra-login.log`) ja lataa sen `launchctl bootstrap`illa. Poisto: `launchctl bootout gui/$(id -u)/dev.hydra.login`.
- kubeconfig: `--kubeconfig`, `KUBECONFIG` tai `.secrets/kubeconfig`.

## Ajo

- Panel lukee Secretin tiedostosta joka ajossa. Jos tunnus vanhenee alle 10 min kuluttua, ajo estetään suomenkielisellä viestillä. Otsikkorivi näyttää jäljellä olevan ajan.
- Ohjeet mallille: hakuun DuckDuckGo html tai Bing (ei Googlea), sivujen lukuun `browser_snapshot` tai `browser_evaluate` (kuvakaappaukset vain tarvittaessa), noin 25 työkalukutsua, edistymisviestit ja lyhyt vastaus suomeksi lähdelinkkeineen.
- Tilauksen raja (HTTP 429) pysäyttää ajon viestiin "tilauksen raja tuli vastaan", uudelleenyrityksiä ei tehdä.

## Ihmisen apu: `ask_human`

Agentilla on panelin oma työkalu `ask_human(reason)` CAPTCHAa, kirjautumista, kaksivaiheista tunnistusta, maksua ja muuta ihmisen asiaa varten. Kun malli kutsuu sitä, ajo pysähtyy: kortille tulee syy ja "Jatka"-painike, ja noVNC-näkymä vaihtuu ohjattavaksi. Jatka (`POST /api/browsers/<id>/agent/resume`) palauttaa mallille tiedon, että ihminen on valmis, ja näkymä palaa katselutilaan. Pysäytä toimii myös tauon aikana. Ilman jatkoa ajo päättyy 15 minuutin jälkeen. Testattu reCAPTCHA-demolla: purkki-selain pysähtyi syyllä "Sivu pyytää CAPTCHAa...", Jatka-painalluksen jälkeen ajo jatkui.

## Testattu 2026-10-12 (n100-1, purkki + capetown rinnakkain)

| Tehtävä | purkki (193.211.54.84) | capetown (15.240.66.186) |
|---|---|---|
| B: Hae lokakuun tarjoukset kaalimato.com | "Liukas lokakuu", -20 % 17.10. asti, tuotteet hintoineen, lähteet | sama kampanja ja hinnat, lähteet |
| A: maailman vanhin yhä toimiva ravintola | Sobrino de Botín, Madrid, 1725 (Guinness); huomio St. Peter Stiftskulinariumista (803) ja Wikipedian lähteistä | sama, lähteinä botin.es, HowStuffWorks, Wikipedia |
