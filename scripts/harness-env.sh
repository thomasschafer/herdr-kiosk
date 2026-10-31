#!/usr/bin/env bash

# Environment shared by the tmux-driven harness scripts. Source this before
# pinning PATH or starting the sandboxed Herdr server.

# A harness launched from inside a Herdr pane inherits HERDR_SOCKET_PATH and
# related variables, which would point sandboxed `herdr` commands (including
# `server stop` during cleanup) at the caller's live server. The XDG base
# directories would likewise take precedence over the sandbox HOME.
while IFS= read -r name; do
    case "$name" in
        HERDR_* | XDG_*_HOME | XDG_RUNTIME_DIR) unset "$name" ;;
    esac
done < <(compgen -e)

# Consumed by the sourcing scripts when they pin PATH.
# shellcheck disable=SC2034
HARNESS_CARGO_BIN=${CARGO_HOME:-"$HOME/.cargo"}/bin
# shellcheck disable=SC2034
HARNESS_PROFILE_BIN=/etc/profiles/per-user/${USER:-$(id -un)}/bin
