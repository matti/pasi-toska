# CLAUDE.md

- hydra = AI-agent browsers that egress through chosen WireGuard exits. Interfaces between parts: `docs/contract.md` (change there first). Docs in Finnish: `docs/README.md`, `docs/browser.md`, `docs/exit.md`.
- Agent usage: `skills/hydra-browser/SKILL.md`. Panel agent on the user's Claude subscription (`hydra login`, Secret `hydra-claude`, never print tokens): `docs/agent.md`.
- Layout: `images/browser/` (Chromium + noVNC + Playwright MCP + status + killswitch), `images/exit/` (WireGuard server + NAT), `cli/` (Rust, `hydra exit add|ls`, own `[workspace]`), `panel/` (separate crate, other owner), `tests/` (docker e2e, fake ssh/kubectl, MCP client).
- From scratch: no web2 / web-mcp code or images.
- User-visible UI brand is "PASI TOSKA browser" (panel title, IP page); everything internal stays hydra (images, k8s, code, MCP names, CLI, AWS).
- Browser control = Playwright MCP over Streamable HTTP on :8931 attached via CDP; CDP never leaves 127.0.0.1.
- Killswitch is set before the tunnel exists; never add a path that lets the browser netns egress outside `wg0` (DNS and IPv6 included). Browser processes run as unprivileged `hydra` without caps.
- Secrets only under `.secrets/` (or `$HYDRA_STATE`), files 0600. Never commit, never print private keys.
- Build exit/browser images with `--provenance=false --sbom=false`, otherwise image ids change every build and `exit add` recopies.
- No em-dash or en-dash characters anywhere. User-facing docs in Finnish.
- Tests: `(cd cli && cargo test)` (< 1 s), `tests/e2e.sh` (~40 s, docker only, cleans up). No fixed sleeps: poll with short interval + deadline.
- Never create cloud servers; touch remote hosts only when the user asked.
- Keep docs in `docs/` in sync with behavior; this file stays a terse list.
- n100-1 runs browser pods as Kata microVMs (RuntimeClass `kata`, Cloud Hypervisor; guest has no WireGuard module, wireguard-go is used). Setup, numbers, rollback: `docs/infra.md`.
- Infra (k0s on n100-1, kubeconfig `.secrets/kubeconfig`, image import, purkki exit host): `docs/infra.md`.
- Panel (UI, proxy, agent, deploy `panel/deploy/panel.yaml` + `panel/ship.sh`): `docs/panel.md`.
- AWS exit `capetown` (af-south-1) purettu 2026-10-12, tili suljettu: `docs/aws-exit.md`.
- Team presentation (Finnish HTML deck + PDF): `presentation/index.html`, `presentation/pasi-toska.pdf`.
