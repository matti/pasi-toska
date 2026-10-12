// Small HTTP side of hydra-browser (docs/contract.md), one process:
//   :8080  GET /status -> {"ready":bool,"exit":string|null,"egress":{ip,city,region,country,org,timezone}|null}
//          GET /ip     -> big-font IP page (ip.html; the panel's IP test opens it)
//          GET /replay.mp4 -> the last ~10 s of the screen (ffmpeg segment ring in /dev/shm/replay)
//   :6080  noVNC static files (/vnc.html) + WebSocket bridge (any path, e.g. /websockify) to x11vnc on 127.0.0.1:5900
// Runs inside the browser's network namespace, so the egress lookup goes
// through the tunnel exactly like the browser's own traffic.
import http from "node:http";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { execFile } from "node:child_process";
import { WebSocketServer } from "ws";

const NOVNC_DIR = "/opt/novnc";
const IP_PAGE = fs.readFileSync(new URL("./ip.html", import.meta.url));
// Written by start.sh / control.mjs (root) in tunnel mode; present = tunneled.
const EXIT_FILE = "/run/hydra/exit";
const NET_GEN_FILE = "/run/hydra/net-gen";
const tunneled = fs.existsSync(EXIT_FILE);
const EGRESS_TTL_MS = 60_000;
const EGRESS_RETRY_MS = 10_000;
const egress = { value: null, until: 0, pending: null, gen: null };
const REPLAY_DIR = "/dev/shm/replay";
const REPLAY_SEGMENTS = 6; // 2 s each: the last one is still being written

const read = (f) => {
  try {
    return fs.readFileSync(f, "utf8").trim();
  } catch {
    return null;
  }
};

async function responds(url) {
  try {
    await fetch(url, { signal: AbortSignal.timeout(2000) });
    return true;
  } catch {
    return false;
  }
}

async function ready() {
  const [cdp, mcp] = await Promise.all([responds("http://127.0.0.1:9222/json/version"), responds("http://127.0.0.1:8931/")]);
  return cdp && mcp && (!tunneled || fs.existsSync("/sys/class/net/wg0"));
}

async function lookupEgress() {
  try {
    const r = await fetch("https://ipinfo.io/json", { signal: AbortSignal.timeout(5000) });
    const j = await r.json();
    egress.value = { ip: j.ip, city: j.city, region: j.region, country: j.country, org: j.org, timezone: j.timezone };
    egress.until = Date.now() + EGRESS_TTL_MS;
  } catch {
    egress.value = null;
    egress.until = Date.now() + EGRESS_RETRY_MS;
  }
}

async function currentEgress() {
  // An exit switch bumps net-gen: look the egress up again right away.
  const gen = read(NET_GEN_FILE);
  if (gen !== egress.gen) {
    egress.gen = gen;
    egress.until = 0;
  }
  if (Date.now() >= egress.until) {
    egress.pending ??= lookupEgress().finally(() => (egress.pending = null));
    await egress.pending;
  }
  return egress.value;
}

http
  .createServer(async (req, res) => {
    const route = req.method === "GET" ? req.url.split("?")[0] : "";
    if (route === "/ip") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
      return void res.end(IP_PAGE);
    }
    if (route === "/replay.mp4") return void replay(res);
    if (route !== "/status") return void res.writeHead(404).end();
    const [isReady, eg] = await Promise.all([ready(), currentEgress()]);
    const exit = tunneled ? read(EXIT_FILE) || process.env.HYDRA_EXIT : null;
    const label = tunneled ? read("/run/hydra/exit-label") : null;
    res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
    res.end(JSON.stringify({ ready: isReady, exit: exit || null, exit_label: label || null, egress: eg }));
  })
  .listen(8080, "0.0.0.0");

/** Joins the newest ring segments (stream copy, no re-encode) into one mp4. */
function replay(res) {
  let segs = [];
  try {
    segs = fs
      .readdirSync(REPLAY_DIR)
      .filter((f) => f.endsWith(".ts"))
      .map((f) => ({ f: `${REPLAY_DIR}/${f}`, t: fs.statSync(`${REPLAY_DIR}/${f}`).mtimeMs }))
      .sort((a, b) => a.t - b.t)
      .slice(-REPLAY_SEGMENTS)
      .map((s) => s.f);
  } catch {}
  if (!segs.length) return void res.writeHead(503).end("replay not available yet");
  const out = `/dev/shm/replay-${process.pid}-${Date.now()}.mp4`;
  const args = ["-nostdin", "-loglevel", "error", "-y", "-i", `concat:${segs.join("|")}`,
    "-c", "copy", "-avoid_negative_ts", "make_zero", "-movflags", "+faststart", out];
  execFile("ffmpeg", args, { timeout: 10_000 }, (err) => {
    if (err) {
      fs.rm(out, { force: true }, () => {});
      return void res.writeHead(500).end(String(err.message));
    }
    res.writeHead(200, { "content-type": "video/mp4", "cache-control": "no-store", "content-length": fs.statSync(out).size });
    fs.createReadStream(out).on("close", () => fs.rm(out, { force: true }, () => {})).pipe(res);
  });
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml",
  ".png": "image/png", ".ico": "image/x-icon", ".json": "application/json", ".oga": "audio/ogg", ".mp3": "audio/mpeg" };

const vnc = http.createServer((req, res) => {
  const rel = decodeURIComponent(req.url.split("?")[0]);
  const file = path.join(NOVNC_DIR, path.normalize(rel === "/" ? "/vnc.html" : rel));
  if (!file.startsWith(NOVNC_DIR + "/")) return void res.writeHead(403).end();
  fs.readFile(file, (err, data) => {
    if (err) return void res.writeHead(404).end();
    res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream" });
    res.end(data);
  });
});
const wss = new WebSocketServer({ noServer: true });
vnc.on("upgrade", (req, socket, head) => {
  wss.handleUpgrade(req, socket, head, (ws) => {
    const tcp = net.connect(5900, "127.0.0.1");
    ws.on("message", (data) => tcp.write(data));
    tcp.on("data", (data) => ws.send(data));
    ws.on("close", () => tcp.destroy());
    tcp.on("close", () => ws.close());
    tcp.on("error", () => ws.close());
  });
});
vnc.listen(6080, "0.0.0.0");
