#!/bin/sh
set -eu

exec "$HERDR_BIN_PATH" plugin pane open \
  --plugin thomasschafer.herdr-kiosk \
  --entrypoint picker
