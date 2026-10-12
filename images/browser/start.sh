#!/bin/bash
# hydra-browser entrypoint (under tini). Network first (root), then the
# browser stack as the unprivileged user `hydra`, so a compromised renderer
# can never touch the killswitch. If any process dies the container exits.
#
#   HYDRA_EXIT      exit name shown in /status
#   HYDRA_SCREEN    Xvfb/window size, default 1366x768
#   TZ, LANG        timezone and locale, e.g. Europe/Helsinki, fi_FI.UTF-8
#   PLAYWRIGHT_MCP_*  passed through to Playwright MCP (e.g. PLAYWRIGHT_MCP_CAPS)
set -euo pipefail
log() { echo "hydra-browser: $*" >&2; }

# Network state owned by root: the current wg0.conf after an exit switch
# (an emptyDir in kube, so a container restart keeps the switched exit) and
# world-readable markers for the status server.
NETDIR=/var/lib/hydra/net
install -d -m 700 -o root -g root "$NETDIR"
install -d -m 755 -o root -g root /run/hydra
conf=$NETDIR/wg0.conf
[ -f "$conf" ] || conf=/etc/hydra/wg/wg0.conf
if [ -f "$conf" ]; then
  net-up.sh "$conf"
  exit_name=$(cat "$NETDIR/exit" 2>/dev/null || echo "${HYDRA_EXIT:-}")
  printf '%s' "$exit_name" > /run/hydra/exit
  # Display label of the exit (e.g. "Kulloo, Finland"), shown instead of ipinfo's city.
  cat "$NETDIR/label" 2>/dev/null > /run/hydra/exit-label || printf '%s' "${HYDRA_EXIT_LABEL:-}" > /run/hydra/exit-label
  node /opt/hydra/control.mjs &   # :8081, exit switch (root, panel only)
else
  log "no /etc/hydra/wg/wg0.conf: direct egress"
fi

screen=${HYDRA_SCREEN:-1366x768}
lang_tag=$(echo "${LANG:-en_US.UTF-8}" | sed -E 's/[.@].*//; s/_/-/')
case "$lang_tag" in C|POSIX|"") lang_tag=en-US ;; esac
accept_lang="$lang_tag,${lang_tag%%-*}"
[ "${lang_tag%%-*}" = en ] || accept_lang="$accept_lang,en-US,en"

# A restarted container keeps its filesystem: drop stale locks.
rm -rf /tmp/.X99-lock /tmp/.X11-unix /data/profile/Singleton*
mkdir -p /data/profile && chown hydra:hydra /data/profile

as_hydra() {
  setpriv --reuid=hydra --regid=hydra --init-groups --inh-caps=-all --no-new-privs \
    env HOME=/home/hydra DISPLAY=:99 "$@"
}
wait_for() { # <what> <command...>: poll every 100 ms, up to 30 s
  local what=$1; shift
  for _ in $(seq 300); do "$@" >/dev/null 2>&1 && return 0; sleep 0.1; done
  log "$what did not come up"; exit 1
}

as_hydra Xvfb :99 -screen 0 "${screen}x24" -nolisten tcp -ac &
wait_for Xvfb test -S /tmp/.X11-unix/X99
as_hydra x11vnc -display :99 -forever -shared -nopw -localhost -rfbport 5900 -quiet >/dev/null 2>&1 &

# Headful Chromium. --test-type hides the --no-sandbox infobar (the sandbox
# needs privileges a container does not have; the user is unprivileged and
# has no capabilities instead). CDP binds to 127.0.0.1 only.
as_hydra chromium \
  --no-sandbox --test-type \
  --user-data-dir=/data/profile \
  --remote-debugging-address=127.0.0.1 --remote-debugging-port=9222 \
  --disable-blink-features=AutomationControlled \
  --no-first-run --no-default-browser-check --disable-default-apps \
  --disable-features=Translate,MediaRouter,DisableLoadExtensionCommandLineSwitch --password-store=basic \
  --load-extension=/opt/isdcac --disable-extensions-except=/opt/isdcac \
  --disable-background-timer-throttling --disable-renderer-backgrounding \
  --disable-backgrounding-occluded-windows \
  --lang="$lang_tag" --accept-lang="$accept_lang" \
  --window-position=0,0 --window-size="${screen/x/,}" \
  about:blank >/dev/null 2>&1 &
wait_for Chromium curl -sf http://127.0.0.1:9222/json/version

# Instant replay ring: 2 s segments of the screen, 7 files in /dev/shm.
# Low fps, scaled down, ultrafast: a few % of one core. Restarted if it dies,
# never takes the container down.
install -d -m 755 -o hydra -g hydra /dev/shm/replay
as_hydra bash -c 'while :; do
  ffmpeg -nostdin -loglevel error -f x11grab -framerate 10 -video_size "$1" -i :99 \
    -vf scale=1024:-2 -c:v libx264 -preset ultrafast -tune zerolatency -crf 30 -g 20 -pix_fmt yuv420p \
    -f segment -segment_time 2 -segment_wrap 7 /dev/shm/replay/seg%d.ts
  sleep 2
done' replay "$screen" </dev/null >/dev/null 2>&1 &
disown

as_hydra playwright-mcp \
  --cdp-endpoint http://127.0.0.1:9222 \
  --host 0.0.0.0 --port 8931 --allowed-hosts '*' \
  --output-dir /tmp/mcp-output &
as_hydra node /opt/hydra/server.mjs &   # status :8080, noVNC :6080
log "ready: noVNC :6080/vnc.html, MCP :8931/mcp, status :8080/status (lang $lang_tag, TZ ${TZ:-UTC})"

wait -n
log "a process exited, stopping"
exit 1
