# AWS-exit Kapkaupungissa (kertakäyttöinen, 2026-10-12)

Demoexit `capetown` AWS:n Kapkaupungin alueessa. Kaikki on kertakäyttöistä ja
poistetaan myöhemmin. Muihin donutlab-tileihin tai OU:ihin ei koskettu.

## Mitä luotiin

| Mikä | Arvo |
|---|---|
| Organisaatio | donutlab, management-tili 980855401112 (profiili `donutlab-mgmt`) |
| OU | `hydra-test-can-delete`, `ou-aqi8-eaxh5kq0` (rootin `r-aqi8` alla) |
| Jäsentili | `hydra-test-can-delete`, 012631426689, `nocode+hydra-test-can-delete@donutlab.com` |
| Pääsy | `OrganizationAccountAccessRole`, profiili `hydra-test-can-delete` tiedostossa `~/.aws/config` |
| Alue | `af-south-1` (Cape Town, opt-in, aktivoitu `aws account enable-region`illa) |
| Instanssi | `i-089ee82daf6d2bc21`, t4g.micro (arm64), Debian 13, AZ af-south-1a |
| Julkinen IP | 15.240.66.186 (auto-assign, vaihtuu jos instanssi pysäytetään) |
| Levy | `vol-07162d603c382dd15`, 8 GB gp3, poistuu instanssin mukana |
| Security group | `sg-0e8ca4a9697368389` (`hydra-exit`): UDP 51820 ja TCP 22 vain 212.90.93.49/32 |
| Avainpari | `hydra-exit` (`key-0e5fcdeaa8f0fb28d`, = `~/.ssh/id_ed25519.pub`), ssh-käyttäjä `admin` |
| Tagit | `Name=hydra-exit`, `Project=hydra-test-can-delete` |
| Exit | `hydra exit add capetown`, kube Secretit `hydra-exit-capetown-0..7` (namespace `hydra`), `.secrets/exits/capetown/` |

212.90.93.49 on sekä n100-1:n että tämän Macin julkinen IP (sama liittymä).
Jos IP vaihtuu, päivitä security groupin säännöt.

Exit luotiin näin (Docker = Debianin `docker.io`, asennettu user-datalla):

```bash
cd cli && ./target/release/hydra exit add capetown admin@15.240.66.186 --endpoint 15.240.66.186 \
  --country ZA --city "Cape Town" --tz Africa/Johannesburg --lang en_ZA.UTF-8 \
  --kubeconfig ../.secrets/kubeconfig
```

Tarkistus panelista: exit näkyy valikossa, ja `capetown`-selaimen
`/b/<id>/status` antoi `15.240.66.186`, Cape Town, Western Cape, ZA,
`AS16509 Amazon.com`, `Africa/Johannesburg`.

## Kustannusarvio (on-demand, AWS Pricing API 2026-10-12)

- t4g.micro 0,0108 $/h, noin 7,9 $/kk
- julkinen IPv4 0,005 $/h, noin 3,7 $/kk
- 8 GB gp3 0,1047 $/GB-kk, noin 0,8 $/kk
- Yhteensä noin 12,4 $/kk + ulosmenevä liikenne (af-south-1 on kalliimpi kuin EU-alueet).

## Purku

```bash
export AWS_PROFILE=hydra-test-can-delete AWS_REGION=af-south-1
aws ec2 terminate-instances --instance-ids i-089ee82daf6d2bc21
aws ec2 wait instance-terminated --instance-ids i-089ee82daf6d2bc21
aws ec2 delete-security-group --group-id sg-0e8ca4a9697368389
aws ec2 delete-key-pair --key-name hydra-exit

# kube ja paikalliset tiedostot (sulje ensin capetown-selaimet panelista)
kubectl --kubeconfig .secrets/kubeconfig -n hydra delete secret -l hydra/exit=capetown
rm -r .secrets/exits/capetown
ssh-keygen -R 15.240.66.186

# tili ja OU (management-tililtä)
export AWS_PROFILE=donutlab-mgmt; unset AWS_REGION
aws organizations move-account --account-id 012631426689 \
  --source-parent-id ou-aqi8-eaxh5kq0 --destination-parent-id r-aqi8
aws organizations close-account --account-id 012631426689
aws organizations delete-organizational-unit --organizational-unit-id ou-aqi8-eaxh5kq0
```

Lopuksi poista profiili `hydra-test-can-delete` tiedostosta `~/.aws/config`.

Tilin sulkemisen erikoisuudet:

- Suljettu tili jää organisaatioon tilaan `SUSPENDED` 90 päiväksi (vrt.
  `esox-hydra`), ja se lasketaan organisaation tilikiintiöön koko ajan.
  Tämän jälkeen AWS poistaa sen pysyvästi. 90 päivän aikana tilin voi avata
  uudelleen AWS Supportin kautta.
- Siksi tili siirretään ensin rootiin: OU:n poisto vaatii tyhjän OU:n.
- `close-account` on rajoitettu: kerrallaan enintään 10 % jäsentileistä
  30 päivän aikana. Sähköpostiosoitetta ei voi käyttää uuteen tiliin.
- Ennen sulkemista varmista, ettei tilillä ole enää maksullisia resursseja
  (instanssi, IP, levy): ne laskutetaan sulkemiseen asti.
