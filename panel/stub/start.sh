#!/bin/sh
Xvfb :0 -screen 0 1280x800x24 &
sleep 1
DISPLAY=:0 xsetroot -solid '#1b2030'
DISPLAY=:0 xterm -geometry 120x40+40+40 -fa Monospace -fs 11 -bg '#0e1118' -fg '#43e5a0' -e top &
x11vnc -display :0 -forever -shared -nopw -quiet -rfbport 5900 &
websockify --web /usr/share/novnc 6080 localhost:5900 &
exec python3 /stub.py
