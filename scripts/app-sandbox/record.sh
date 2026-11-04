#!/usr/bin/env bash
# Records the README's preview of the kiosk mod in the app sandbox: frames the app renders
# itself, taken while a script types, then turned into media/app-preview.gif. Run through
# `run.sh record`; look at the result before committing it.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
sb() { "$here/sandbox" "$@"; }
out=${RECORD_OUT:-/src/media/app-preview.gif}
fps=12
frames=$(mktemp -d)
export TYPE_DELAY=110

sb down >/dev/null 2>&1 || true
sb fixtures
SANDBOX_APPEARANCE=dark sb up
sb install >/dev/null
sb "do" space.new "{\"folder\":\"$HOME/code/api\"}" >/dev/null
sleep 2
# The app's note that it was updated, which a fresh home shows once.
sb click 1244 787
sleep 1

capture() {
  local n=0 next
  next=$(date +%s%N)
  while [[ ! -f $frames/stop ]]; do
    sb shot "$(printf '%s/%05d.png' "$frames" "$n")" >/dev/null
    n=$((n + 1))
    next=$((next + 1000000000 / fps))
    local now
    now=$(date +%s%N)
    ((next > now)) && sleep "$(printf '0.%09d' $((next - now)))"
  done
}
capture &
capturing=$!

pause() { sleep "$1"; }
pause 1
sb open
pause 1.4
sb type web
pause 0.9
sb key Tab
pause 1.4
sb type dark
pause 1.1
sb key Return
pause 2.2
sb open
pause 1.2
sb type api
pause 0.7
sb key Tab
pause 1.2
sb type feat/login
pause 1.3
sb key Return
pause 1.6
sb key Return
pause 2.4

touch "$frames/stop"
wait "$capturing"
scale="scale=1000:-2:flags=lanczos"
ffmpeg -hide_banner -loglevel error -y -framerate "$fps" -i "$frames/%05d.png" \
  -vf "$scale,palettegen=max_colors=192:stats_mode=full" "$frames/palette.png"
ffmpeg -hide_banner -loglevel error -y -framerate "$fps" -i "$frames/%05d.png" -i "$frames/palette.png" \
  -lavfi "${scale}[frames];[frames][1:v]paletteuse=dither=sierra2_4a" -loop 0 "$out"
[[ -n ${RECORD_KEEP:-} ]] && cp -r "$frames" "$RECORD_KEEP"
rm -rf "$frames"
echo "$out ($(du -h "$out" | cut -f1))"
