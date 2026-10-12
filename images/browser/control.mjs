// Root-side control endpoint of hydra-browser, only in tunnel mode:
//   :8081  POST /exit {"exit": "<name>", "conf": "<wg0.conf>", "label"?: "<display name>"} -> switch the exit in place
// Accepts connections only from outside the pod (the panel): anything local
// (the browser, its renderers, loopback or the pod's own address) is refused,
// so a compromised page can never repoint the tunnel.
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import { execFile } from "node:child_process";

const NETDIR = "/var/lib/hydra/net";
const MOUNTED = "/etc/hydra/wg/wg0.conf";
const MAX_BODY = 8192;
let queue = Promise.resolve();

function plain(addr) {
  return (addr || "").replace(/^::ffff:/, "");
}

function isLocal(addr) {
  const a = plain(addr);
  if (a.startsWith("127.") || a === "::1") return true;
  return Object.values(os.networkInterfaces()).flat().some((i) => plain(i.address) === a);
}

function currentConf() {
  return fs.existsSync(`${NETDIR}/wg0.conf`) ? `${NETDIR}/wg0.conf` : MOUNTED;
}

function swap(exit, conf, label) {
  const next = `${NETDIR}/wg0.conf.new`;
  fs.writeFileSync(next, conf, { mode: 0o600 });
  return new Promise((resolve, reject) => {
    execFile("/usr/local/bin/net-up.sh", ["--swap", next, currentConf()], (err, _out, stderr) => {
      if (err) return reject(new Error(stderr.trim() || err.message));
      fs.renameSync(next, `${NETDIR}/wg0.conf`);
      fs.writeFileSync(`${NETDIR}/exit`, exit);
      fs.writeFileSync(`${NETDIR}/label`, label);
      fs.writeFileSync("/run/hydra/exit", exit, { mode: 0o644 });
      fs.writeFileSync("/run/hydra/exit-label", label, { mode: 0o644 });
      fs.writeFileSync("/run/hydra/net-gen", String(Date.now()), { mode: 0o644 });
      process.stderr.write(stderr);
      resolve();
    });
  });
}

function reply(res, code, body) {
  res.writeHead(code, { "content-type": "application/json" }).end(JSON.stringify(body));
}

http
  .createServer((req, res) => {
    if (isLocal(req.socket.remoteAddress)) return reply(res, 403, { error: "local clients may not switch the exit" });
    if (req.method !== "POST" || req.url !== "/exit") return reply(res, 404, { error: "not found" });
    let body = "";
    req.on("data", (c) => {
      body += c;
      if (body.length > MAX_BODY) req.destroy();
    });
    req.on("end", () => {
      let exit, conf, label;
      try {
        ({ exit, conf, label } = JSON.parse(body));
      } catch {
        return reply(res, 400, { error: "expected JSON {exit, conf}" });
      }
      if (!/^[a-z0-9-]{1,32}$/.test(exit || "") || typeof conf !== "string" || !/\[Interface\][\s\S]*\[Peer\]/.test(conf)) {
        return reply(res, 400, { error: "invalid exit name or wg0.conf" });
      }
      label = typeof label === "string" ? label.replace(/[^\p{L}\p{N} ,.'()-]/gu, "").slice(0, 64) : "";
      // One switch at a time.
      queue = queue.then(() => swap(exit, conf, label)).then(
        () => reply(res, 200, { ok: true, exit }),
        (e) => reply(res, 500, { error: e.message }),
      );
    });
  })
  .listen(8081, "0.0.0.0");
