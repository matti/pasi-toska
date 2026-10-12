# hydra-browser: headful Chromium (Xvfb) + noVNC + Playwright MCP over
# Streamable HTTP + status endpoint, optionally egressing only through
# WireGuard with a killswitch. See docs/contract.md and docs/browser.md.
FROM node:24-trixie-slim

ARG PLAYWRIGHT_MCP_VERSION=0.0.83
ARG NOVNC_VERSION=1.7.0
ARG WS_VERSION=8.22.0
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends \
      chromium chromium-l10n xvfb x11vnc tini \
      fonts-liberation fonts-noto-core fonts-noto-color-emoji tzdata locales \
      wireguard-tools wireguard-go iptables iproute2 curl ca-certificates \
 && sed -i -E 's/^# *((en_US|fi_FI|sv_SE|de_DE|fr_FR|es_ES|nl_NL|en_GB)\.UTF-8)/\1/' /etc/locale.gen \
 && locale-gen >/dev/null \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g --omit=dev "@playwright/mcp@${PLAYWRIGHT_MCP_VERSION}" >/dev/null \
 && mkdir -p /opt/hydra && cd /opt/hydra && npm install --omit=dev "ws@${WS_VERSION}" >/dev/null \
 && curl -fsSL "https://github.com/novnc/noVNC/archive/refs/tags/v${NOVNC_VERSION}.tar.gz" | tar xz -C /opt \
 && mv "/opt/noVNC-${NOVNC_VERSION}" /opt/novnc \
 && npm cache clean --force >/dev/null \
 && useradd -m -u 1001 -s /bin/bash hydra \
 && mkdir -p /data/profile /etc/hydra/wg && chown hydra:hydra /data/profile

# Cookie banners away: "I still don't care about cookies" (maintained fork
# of IDCAC), pinned release, checksum-verified. Own layer so the big layer
# above stays shared with older images.
ARG ISDCAC_VERSION=1.1.9
ARG ISDCAC_SHA256=8f70ab947cb2d274f4022a970f5dd3cecd8ec02b060e05187bef9ee3cb18bbcb
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends unzip >/dev/null \
 && curl -fsSL -o /tmp/isdcac.zip \
      "https://github.com/OhMyGuus/I-Still-Dont-Care-About-Cookies/releases/download/v${ISDCAC_VERSION}/ISDCAC-chrome-source.zip" \
 && echo "${ISDCAC_SHA256}  /tmp/isdcac.zip" | sha256sum -c - >/dev/null \
 && unzip -q /tmp/isdcac.zip -d /opt/isdcac && rm /tmp/isdcac.zip \
 # Chromium writes the indexed declarativeNetRequest rules into the
 # extension dir; unwritable it fails with a modal error and never starts.
 && chown -R hydra:hydra /opt/isdcac \
 && apt-get purge -y -qq unzip >/dev/null && rm -rf /var/lib/apt/lists/*

# Instant replay (x11grab segment ring). Own layer, the big one above stays shared.
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ffmpeg >/dev/null \
 && rm -rf /var/lib/apt/lists/*

COPY start.sh net-up.sh /usr/local/bin/
COPY server.mjs control.mjs ip.html /opt/hydra/
RUN chmod 755 /usr/local/bin/start.sh /usr/local/bin/net-up.sh

# noVNC, status, MCP, exit control (tunnel mode, panel only). CDP (9222) stays on 127.0.0.1.
EXPOSE 6080 8080 8081 8931
VOLUME /data/profile
ENTRYPOINT ["tini", "--", "/usr/local/bin/start.sh"]
