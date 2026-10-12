# Test stand-in for hydra-browser (see stub.py). Not shipped.
FROM alpine:3
RUN apk add --no-cache xvfb x11vnc novnc websockify python3 xterm xsetroot font-dejavu procps
COPY stub.py start.sh /
ENTRYPOINT ["/start.sh"]
