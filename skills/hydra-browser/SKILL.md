---
name: hydra-browser
description: Drive a real headful Chromium (hydra-browser) over MCP whose traffic leaves from a chosen exit IP through WireGuard. Use for web research (search, open results, extract), reading a given page "right now" from a specific country/IP, or any multi-step browsing (forms, logins, tabs, screenshots) that must not come from the agent's own IP.
---

# hydra browser

A hydra browser is one container: headful Chromium + Playwright MCP
(Streamable HTTP, port 8931 `/mcp`) + noVNC (6080) + status (8080). With an
exit configured, ALL its traffic (HTTP, DNS, WebRTC) goes through WireGuard;
if the exit is down it has no internet at all, it never falls back.

## 1. Get an MCP URL

- Through the hydra panel: `http://<panel>/b/<id>/mcp` (status `/b/<id>/status`,
  live view `/b/<id>/vnc/vnc.html`).
- Or a container you start yourself (see docs/README.md):
  `http://127.0.0.1:8931/mcp`, status `http://127.0.0.1:8080/status`.

Register it once per browser (any number of browsers, one name each):

```bash
claude mcp add --transport http hydra-<id> http://<panel>/b/<id>/mcp
```

Tools then appear as `mcp__hydra-<id>__browser_*`.

## 2. Check where you are before browsing

```bash
curl -s http://<panel>/b/<id>/status
# {"ready":true,"exit":"purkki","egress":{"ip":"...","city":"...","country":"FI",...}}
```

- `ready` must be true. `egress.ip` is the IP websites see; `egress` null
  means no internet (tunnel down, killswitch working): report it, do not
  look for a way around it.
- The browser's language and timezone are set to match the exit.

## 3. Core loop

1. `browser_navigate {url}`: the response links the snapshot but does not
   inline it.
2. `browser_snapshot`: accessibility tree; every interactive element has a
   ref like `[ref=e19]`. This is how you see the page.
3. Act with the ref as `target`: `browser_click {target: "e19", element: "Sign in button"}`,
   `browser_type {target, text, submit: true}`, `browser_fill_form`,
   `browser_select_option`, `browser_press_key {key: "Enter"}`, `browser_hover`.
4. `browser_snapshot` again (refs change after navigation), repeat.

Extracting data: `browser_evaluate {function: "() => ..."}` returns JSON. It
is the most token-efficient way to pull lists, tables or the main text, e.g.
`() => document.querySelector('main')?.innerText.slice(0, 4000)`.

Other tools: `browser_tabs {action: list|new|select|close}`,
`browser_wait_for {text}` / `{textGone}` (prefer these to `{time}`),
`browser_take_screenshot` (image returned inline; use only to look, act via
snapshot refs), `browser_navigate_back`, `browser_handle_dialog`,
`browser_network_requests` / `browser_network_request` (what the page
loaded: find JSON APIs behind a page), `browser_console_messages`,
`browser_find`, `browser_run_code_unsafe` (a Playwright snippet for things
no other tool covers). `browser_file_upload` paths are inside the browser
container, not on your machine.

## 4. Web research (search engines)

Do NOT use Google: it serves captchas to automated and datacenter traffic.
Use, in this order:

1. DuckDuckGo HTML: `https://html.duckduckgo.com/html/?q=<urlencoded query>`.
   Results are `a.result__a` links; extract all at once:
   `() => [...document.querySelectorAll('a.result__a')].map(a => ({title: a.textContent.trim(), href: a.href}))`
   (hrefs are duckduckgo.com/l/?uddg= redirects; clicking or navigating them works).
2. Bing: `https://www.bing.com/search?q=...`, results `li.b_algo h2 a`.
3. Brave Search: `https://search.brave.com/search?q=...`, results `.result-content > a`.

Mojeek and Startpage answered with a challenge or a redirect loop in tests.
If an engine shows a captcha or "unusual traffic" page, switch engines; do
not try to solve captchas.

Typical flow: search, pick 2-5 promising results, open each
(`browser_click` on its ref, or `browser_navigate` to the href, or
`browser_tabs {action: "new", url}`), extract with `browser_evaluate`,
then answer with the source URLs.

## 5. Reading a given page

`browser_navigate` to it, dismiss the cookie banner if it covers content
(snapshot, click the accept/reject button by ref), then `browser_evaluate`
for the text or the specific data (prices, dates, offers). For shops, also
check `browser_network_requests`: product data often comes from a JSON API.

## Notes

- The profile persists in the container (cookies, logins) until the browser
  is deleted. A human can watch or take over via noVNC at any time.
- One browser = one tab set shared by all MCP sessions to it; do not run two
  agents on the same browser at once. For parallel work, use several browsers.
