// PASI TOSKA browser panel (internal name hydra-panel): renders /api/events snapshots, launches and stops browsers.

const $ = (sel, root = document) => root.querySelector(sel);
const DIRECT = "direct";

const ICONS = {
  server: '<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="7" rx="2" fill="none" stroke="currentColor" stroke-width="1.7"/><rect x="4" y="13" width="16" height="7" rx="2" fill="none" stroke="currentColor" stroke-width="1.7"/><circle cx="8" cy="7.5" r="1.1" fill="currentColor"/><circle cx="8" cy="16.5" r="1.1" fill="currentColor"/></svg>',
  eye: '<svg viewBox="0 0 20 20"><path d="M2 10s3-5.5 8-5.5S18 10 18 10s-3 5.5-8 5.5S2 10 2 10z" fill="none" stroke="currentColor" stroke-width="1.6"/><circle cx="10" cy="10" r="2.4" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>',
  pointer: '<svg viewBox="0 0 20 20"><path d="M5 3l10 6.5-4.3 1.1 2.6 4.6-1.8 1-2.6-4.6L5 15z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/></svg>',
};

const VERDICTS = {
  tunnel: "Tunneli toimii: IP on exitin oma",
  leak: "VUOTO: liikenne näkyy solmun omasta IP:stä, vaikka exit on valittu",
  direct: "Suora yhteys solmun omasta IP:stä",
  unverified: "IP ei vastaa exitin ilmoitettua IP:tä",
  unknown: "Haetaan IP:tä…",
};

const ui = {
  snap: null,
  selected: localStorage.getItem("hydra.exit"),
  cards: new Map(),
  scrollTo: null,
};

// ---------- helpers ----------

function flag(cc) {
  if (!cc || !/^[A-Za-z]{2}$/.test(cc)) return "";
  return String.fromCodePoint(...[...cc.toUpperCase()].map((c) => 0x1f1a5 + c.charCodeAt(0)));
}

function el(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else if (k === "html") e.innerHTML = v;
    else e.setAttribute(k, v);
  }
  for (const k of kids) if (k != null) e.append(k);
  return e;
}

function setText(node, text) {
  if (node.textContent !== text) node.textContent = text;
}

function toast(msg, kind = "") {
  const t = el("div", { class: `toast ${kind}` }, msg);
  $("#toasts").append(t);
  setTimeout(() => {
    t.classList.add("out");
    setTimeout(() => t.remove(), 300);
  }, kind === "error" ? 6000 : 3000);
}

async function copy(text, btn) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Plain http (not a secure context) has no async clipboard.
    const ta = el("textarea", { style: "position:fixed;opacity:0" });
    ta.value = text;
    document.body.append(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
  btn.classList.add("done");
  setTimeout(() => btn.classList.remove("done"), 1200);
}

async function api(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: body ? { "content-type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    let msg = `${res.status} ${res.statusText}`;
    try {
      msg = (await res.json()).error || msg;
    } catch {}
    throw new Error(msg);
  }
  return res.status === 204 ? null : res.json();
}

async function busy(btn, fn) {
  btn.classList.add("busy");
  try {
    await fn();
  } catch (e) {
    toast(e.message, "error");
  } finally {
    btn.classList.remove("busy");
  }
}

// ---------- header ----------

function renderHeader(snap) {
  const c = snap.cluster;
  const node = $("#node");
  node.className = `node ${c.ok ? "ok" : c.error ? "bad" : ""}`;
  setText($(".node-ip", node), c.node?.ip || "…");
  const where = [c.node?.city, c.node?.country].filter(Boolean).join(", ");
  node.title = c.error ? `Kube API: ${c.error}` : `Solmun oma IP${where ? ` (${where})` : ""}`;
}

function renderClaude(snap) {
  const m = snap.cluster.agent_minutes;
  const c = $("#claude");
  c.hidden = m == null;
  if (m == null) return;
  setText(c, m <= 0 ? "Claude: vanhentunut" : `Claude ${m >= 60 ? `${Math.floor(m / 60)} h ` : ""}${m % 60} min`);
  c.className = `claude ${m < 10 ? "bad" : m < 30 ? "warn" : ""}`;
  c.title = "Claude-tilauksen tunnuksen voimassaolo (päivitetään Macilta: hydra login)";
  $("#task").hidden = false;
}

function setDisconnected() {
  $("#node").className = "node bad";
  $("#node").title = "Paneeliin ei yhteyttä, yhdistetään…";
}

// ---------- exit picker ----------

function options(snap) {
  const opts = snap.exits.map((e) => ({
    value: e.name,
    icon: flag(e.country) || "🌐",
    name: e.label || e.name,
    meta: e.label ? "" : e.city,
    ip: e.ip,
    free: e.peers_free,
    total: e.peers_total,
    disabled: e.peers_free === 0,
  }));
  opts.push({ value: DIRECT, iconHtml: ICONS.server, name: "Suora", meta: "solmun IP", ip: snap.cluster.node?.ip });
  return opts;
}

function optionNode(o, tag = "div") {
  const icon = el("span", { class: "opt-flag" });
  if (o.iconHtml) icon.innerHTML = o.iconHtml;
  else icon.textContent = o.icon;
  const meta = el("span", { class: "opt-meta" }, o.meta || "");
  if (o.ip) meta.append(o.meta ? " · " : "", el("span", { class: "mono" }, o.ip));
  const node = el(tag, { class: "opt" }, icon, el("span", { class: "opt-name" }, o.name), meta);
  if (o.total != null) {
    node.append(el("span", { class: `free ${o.free ? "" : "none"}`, title: "Vapaat peerit" }, `${o.free}/${o.total}`));
  }
  return node;
}

function renderPicker(snap) {
  const opts = options(snap);
  if (!opts.find((o) => o.value === ui.selected && !o.disabled)) {
    ui.selected = (opts.find((o) => !o.disabled) || opts[opts.length - 1]).value;
  }
  $("#picker-value").replaceChildren(optionNode(opts.find((o) => o.value === ui.selected)));

  const list = $("#picker-list");
  const activeValue = list.querySelector(".opt.active")?.dataset.value;
  list.replaceChildren(
    ...opts.map((o) => {
      const li = optionNode(o, "li");
      li.setAttribute("role", "option");
      li.dataset.value = o.value;
      li.setAttribute("aria-selected", String(o.value === ui.selected));
      if (o.disabled) li.setAttribute("aria-disabled", "true");
      if (o.value === activeValue) li.classList.add("active");
      return li;
    }),
  );
}

function openPicker(open) {
  const list = $("#picker-list");
  list.hidden = !open;
  $("#picker-btn").setAttribute("aria-expanded", String(open));
  if (open) list.querySelectorAll(".opt").forEach((o) => o.classList.toggle("active", o.dataset.value === ui.selected));
}

function choose(value) {
  ui.selected = value;
  localStorage.setItem("hydra.exit", value);
  openPicker(false);
  if (ui.snap) renderPicker(ui.snap);
}

function initPicker() {
  const btn = $("#picker-btn");
  const list = $("#picker-list");
  btn.addEventListener("click", () => openPicker(list.hidden));
  list.addEventListener("click", (e) => {
    const li = e.target.closest(".opt");
    if (li && li.getAttribute("aria-disabled") !== "true") choose(li.dataset.value);
  });
  document.addEventListener("click", (e) => {
    if (!e.target.closest("#picker")) openPicker(false);
  });
  btn.addEventListener("keydown", (e) => {
    const opts = [...list.querySelectorAll(".opt:not([aria-disabled='true'])")];
    if (!opts.length) return;
    let i = opts.findIndex((o) => o.classList.contains("active"));
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (list.hidden) openPicker(true);
      i = (i + (e.key === "ArrowDown" ? 1 : -1) + opts.length) % opts.length;
      opts.forEach((o, j) => o.classList.toggle("active", j === i));
    } else if (e.key === "Enter" && !list.hidden) {
      e.preventDefault();
      if (i >= 0) choose(opts[i].dataset.value);
    } else if (e.key === "Escape") {
      openPicker(false);
    }
  });
}

// ---------- launch ----------

async function launch() {
  await busy($("#launch-btn"), async () => {
    const { id } = await api("POST", "/api/browsers", { exit: ui.selected });
    ui.scrollTo = id;
  });
}

// ---------- cards ----------

function vncSrc(b, viewOnly) {
  // Explicit host/port: noVNC >= 1.5 otherwise resolves `path` relative to vnc.html.
  const https = location.protocol === "https:";
  const port = location.port || (https ? "443" : "80");
  return `${b.vnc}&host=${location.hostname}&port=${port}&encrypt=${https ? 1 : 0}&view_only=${viewOnly ? 1 : 0}`;
}

function mcpUrl(b) {
  return `${location.origin}${b.mcp}`;
}

function mcpCmd(b) {
  return `claude mcp add --transport http hydra-${b.id} ${mcpUrl(b)}`;
}

function setViewOnly(card, on) {
  const btn = $(".viewonly", card);
  btn.classList.toggle("on", on);
  btn.innerHTML = on ? ICONS.eye : ICONS.pointer;
  btn.title = on ? "Vain katselu (klikkaa ohjataksesi)" : "Ohjaus päällä (klikkaa vain katseluun)";
}

/** View-only on/off, reloading the noVNC iframe so the mode takes effect. */
function applyViewOnly(card, on) {
  setViewOnly(card, on);
  const iframe = $("iframe", card);
  if (iframe.getAttribute("src")) {
    $(".screen", card).classList.remove("live");
    iframe.src = vncSrc(card.b, on);
  }
}

async function showReplay(card) {
  const res = await fetch(`/b/${card.b.id}/replay.mp4?t=${Date.now()}`);
  if (!res.ok) throw new Error(res.status === 503 ? "Uusintaa ei vielä ole." : `Uusinta: ${res.status}`);
  const view = $(".replay-view", card);
  const video = $("video", view);
  if (video.src) URL.revokeObjectURL(video.src);
  video.src = URL.createObjectURL(await res.blob());
  view.hidden = false;
  video.play().catch(() => {});
}

function hideReplay(card) {
  const view = $(".replay-view", card);
  const video = $("video", view);
  video.pause();
  if (video.src) URL.revokeObjectURL(video.src);
  video.removeAttribute("src");
  view.hidden = true;
}

function closeMenus() {
  for (const m of document.querySelectorAll(".menu")) m.hidden = true;
}

/** Exit switch menu: the other exits with a free peer. */
function toggleExitMenu(card) {
  const menu = $(".menu", card);
  const open = menu.hidden;
  closeMenus();
  if (!open) return;
  const exits = ui.snap.exits.filter((e) => e.name !== card.b.exit);
  menu.replaceChildren(
    ...exits.map((e) => {
      const li = optionNode({ icon: flag(e.country) || "🌐", name: e.label || e.name, meta: e.label ? "" : e.city, ip: e.ip }, "li");
      if (!e.peers_free) li.setAttribute("aria-disabled", "true");
      li.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (!e.peers_free) return;
        closeMenus();
        busy($(".switch", card), async () => {
          await api("POST", `/api/browsers/${card.b.id}/exit`, { exit: e.name });
          toast(`Exit vaihdettu: ${e.name}`);
        });
      });
      return li;
    }),
  );
  if (!exits.length) menu.append(el("li", { class: "opt", "aria-disabled": "true" }, "Ei muita exitejä"));
  menu.hidden = false;
}

function createCard(b) {
  const card = $("#card-tpl").content.firstElementChild.cloneNode(true);
  card.dataset.id = b.id;
  card.b = b;
  const iframe = $("iframe", card);
  const screen = $(".screen", card);

  iframe.addEventListener("load", () => {
    if (!iframe.getAttribute("src")) return;
    try {
      // Same origin through the proxy: hide noVNC's own chrome.
      const d = iframe.contentDocument;
      const st = d.createElement("style");
      st.textContent = "#noVNC_control_bar_anchor,#noVNC_status{display:none!important}body{background:#000!important}";
      d.head.append(st);
    } catch {}
    screen.classList.add("live");
  });

  setViewOnly(card, true);
  $(".viewonly", card).addEventListener("click", (e) => applyViewOnly(card, !e.currentTarget.classList.contains("on")));
  $(".fullscreen", card).addEventListener("click", () => screen.requestFullscreen?.());
  $(".iptest", card).addEventListener("click", (e) => busy(e.currentTarget, () => api("POST", `/api/browsers/${card.b.id}/iptest`)));
  $(".fptest", card).addEventListener("click", (e) => busy(e.currentTarget, () => api("POST", `/api/browsers/${card.b.id}/fptest`)));
  $(".replay", card).addEventListener("click", (e) => busy(e.currentTarget, () => showReplay(card)));
  $(".replay-close", card).addEventListener("click", () => hideReplay(card));
  $(".switch", card).addEventListener("click", (e) => {
    e.stopPropagation();
    toggleExitMenu(card);
  });
  $(".ask-go", card).addEventListener("click", (e) => busy(e.currentTarget, () => api("POST", `/api/browsers/${card.b.id}/agent/resume`)));
  $(".copy-mcp", card).addEventListener("click", (e) => copy(e.shiftKey ? mcpCmd(card.b) : mcpUrl(card.b), e.currentTarget));
  $(".stop", card).addEventListener("click", (e) =>
    busy(e.currentTarget, async () => {
      await api("DELETE", `/api/browsers/${card.b.id}`);
      card.classList.add("stopping");
    }),
  );
  return card;
}

function overlayText(b) {
  switch (b.phase) {
    case "pending":
      return b.detail ? `Odotetaan podia: ${b.detail}` : "Odotetaan podia…";
    case "starting":
      return b.exit === DIRECT ? "Käynnistetään…" : "Avataan tunnelia…";
    case "stopping":
      return "Suljetaan…";
    case "error":
      return b.detail || "Selain ei käynnistynyt.";
    default:
      return "Yhdistetään…";
  }
}

function updateCard(card, b, snap) {
  card.b = b;
  const direct = b.exit === DIRECT;
  card.className = `card ${b.phase} ${b.verdict}${card.agentRun ? " working" : ""}`;

  const fl = $(".flag", card);
  if (direct) {
    if (!fl.querySelector("svg")) fl.innerHTML = ICONS.server;
  } else setText(fl, flag(b.exit_country) || "🌐");
  fl.title = direct ? "Suora" : b.exit_label || [b.exit_city, b.exit_country].filter(Boolean).join(", ");
  setText($(".exit-name", card), direct ? "Suora" : b.exit_label || b.exit);
  $(".exit-name", card).title = `#${b.id}${b.exit_ip ? ` · exit ${b.exit_ip}` : ""}`;

  const ip = $(".ip", card);
  ip.classList.toggle("placeholder", !b.egress);
  setText(ip, b.egress ? b.egress.ip : b.phase === "error" ? "virhe" : "…");
  const e = b.egress;
  // An exit's display label replaces ipinfo's guess of the city (e.g. Helsinki for Kulloo).
  const place = b.exit_label || (e ? [e.city, e.country].filter(Boolean).join(", ") : "");
  setText($(".geo", card), e ? [place, e.org].filter(Boolean).join(" · ") : "");
  const dot = $(".vdot", card);
  dot.className = `vdot ${b.verdict}`;
  dot.title = VERDICTS[b.verdict] || "";

  const screen = $(".screen", card);
  const iframe = $("iframe", card);
  screen.classList.toggle("failed", b.phase === "error");
  setText($(".overlay-text", card), overlayText(b));
  if (b.phase === "ready" && !iframe.getAttribute("src")) {
    iframe.src = vncSrc(b, $(".viewonly", card).classList.contains("on"));
  }
  $(".newwin", card).href = vncSrc(b, false);
  $(".switch", card).closest(".menu-wrap").hidden = direct;
  $(".copy-mcp", card).title = `Kopioi MCP-URL\nShift-klikkaus: ${mcpCmd(b)}`;
  $(".iptest", card).title = `IP-testi (${new URL(snap.cluster.ip_test).host})`;

  $(".agent", card).hidden = !card.agentUsed;
}

function renderCards(snap) {
  const grid = $("#grid");
  const seen = new Set();
  for (const b of snap.browsers) {
    seen.add(b.id);
    let card = ui.cards.get(b.id);
    if (!card) {
      card = createCard(b);
      ui.cards.set(b.id, card);
      grid.append(card);
    }
    updateCard(card, b, snap);
    if (ui.scrollTo === b.id) {
      ui.scrollTo = null;
      card.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
  }
  for (const [id, card] of ui.cards) {
    if (seen.has(id)) continue;
    ui.cards.delete(id);
    card.agentRun?.abort();
    card.classList.add("gone");
    setTimeout(() => card.remove(), 300);
  }
  $("#empty").hidden = snap.browsers.length > 0;
}

function render(snap) {
  ui.snap = snap;
  renderHeader(snap);
  renderClaude(snap);
  renderPicker(snap);
  $("#launch-btn").disabled = !snap.cluster.ok;
  renderCards(snap);
}

// ---------- agent ----------

const LOG_LINES = 5;

/** One short line per tool call: "navigate html.duckduckgo.com/html/?q=..." */
function stepLine(ev) {
  const name = ev.name.replace(/^browser_/, "");
  const i = ev.input || {};
  let arg = "";
  if (i.url) arg = i.url.replace(/^https?:\/\/(www\.)?/, "");
  else if (i.element) arg = `"${i.element}"`;
  else if (i.text) arg = `"${i.text}"`;
  else if (i.action) arg = i.action;
  else if (i.key) arg = i.key;
  else if (i.function) arg = "js";
  const line = arg ? `${name} ${arg}` : name;
  return line.length > 70 ? `${line.slice(0, 69)}…` : line;
}

function escapeHtml(t) {
  return t.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

/** Final answer: escaped text with markdown links, bare URLs, bold and line breaks. */
function answerHtml(text) {
  return escapeHtml(text)
    .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, '$1<a href="$2" target="_blank" rel="noopener">$2</a>')
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/\n/g, "<br>");
}

function logLine(card, text, cls = "") {
  const log = $(".agent-log", card);
  const li = el("li", { class: cls }, text);
  log.append(li);
  while (log.children.length > LOG_LINES) log.firstElementChild.remove();
  return li;
}

async function runAgent(card, task) {
  const ctrl = new AbortController();
  card.agentRun = ctrl;
  card.agentUsed = true;
  card.classList.add("working");
  $(".agent", card).hidden = false;
  $(".agent-log", card).replaceChildren();
  const final = $(".agent-final", card);
  final.className = "agent-final";
  final.replaceChildren();
  const calls = new Map();
  try {
    const res = await fetch(`/api/browsers/${card.b.id}/agent`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ task }),
      signal: ctrl.signal,
    });
    if (!res.ok) {
      let msg = `${res.status}`;
      try {
        msg = (await res.json()).error || msg;
      } catch {}
      throw new Error(msg);
    }
    const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
    let buf = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += value;
      let nl;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line) continue;
        const ev = JSON.parse(line);
        if (ev.type === "tool") calls.set(ev.call, logLine(card, stepLine(ev), "tool"));
        else if (ev.type === "tool_result" && !ev.ok) calls.get(ev.call)?.classList.add("err");
        else if (ev.type === "text") logLine(card, ev.text.split("\n")[0], "note");
        else if (ev.type === "ask_human") askHuman(card, ev.reason);
        else if (ev.type === "resumed") endAsk(card);
        else if (ev.type === "final") {
          final.className = "agent-final done";
          final.innerHTML = answerHtml(ev.text);
        } else if (ev.type === "error") throw new Error(ev.message);
      }
    }
  } catch (err) {
    final.className = "agent-final error";
    final.textContent = err.name === "AbortError" ? "Pysäytetty." : err.message;
  } finally {
    endAsk(card);
    card.agentRun = null;
    card.classList.remove("working");
    updateTaskButton();
  }
}

/** The agent paused for the person: show why, make the view interactive. */
function askHuman(card, reason) {
  logLine(card, `Odottaa: ${reason}`, "note");
  $(".ask-reason", card).textContent = reason;
  $(".agent-ask", card).hidden = false;
  card.askRestoreViewOnly = $(".viewonly", card).classList.contains("on");
  if (card.askRestoreViewOnly) applyViewOnly(card, false);
}

function endAsk(card) {
  if ($(".agent-ask", card).hidden) return;
  $(".agent-ask", card).hidden = true;
  if (card.askRestoreViewOnly) applyViewOnly(card, true);
}

function updateTaskButton() {
  const running = [...ui.cards.values()].some((c) => c.agentRun);
  setText($("#task-run span"), running ? "Pysäytä" : "Aja kaikissa");
  $("#task-run").classList.toggle("stop", running);
}

function initTask() {
  $("#task-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const cards = [...ui.cards.values()];
    if (cards.some((c) => c.agentRun)) {
      for (const c of cards) c.agentRun?.abort();
      return;
    }
    const task = $("#task-input").value.trim();
    const ready = cards.filter((c) => c.b.phase === "ready");
    if (!task) return $("#task-input").focus();
    if (!ready.length) return toast("Ei valmiita selaimia.", "error");
    for (const c of ready) runAgent(c, task);
    updateTaskButton();
  });
  const input = $("#task-input");
  const clear = $("#task-clear");
  const sync = () => (clear.hidden = !input.value);
  input.addEventListener("input", sync);
  clear.addEventListener("click", () => {
    input.value = "";
    sync();
    input.focus();
  });
  for (const chip of document.querySelectorAll(".chip")) {
    chip.addEventListener("click", () => {
      input.value = chip.dataset.task;
      sync();
      input.focus();
    });
  }
}

// ---------- boot ----------

initPicker();
initTask();
document.addEventListener("click", closeMenus);
$("#launch-btn").addEventListener("click", launch);
const events = new EventSource("/api/events");
events.addEventListener("snapshot", (e) => render(JSON.parse(e.data)));
events.addEventListener("error", setDisconnected);
