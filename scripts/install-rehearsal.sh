#!/usr/bin/env bash
set -euo pipefail

# This deliberately installs from the real public GitHub repository, not the
# working tree. Run only after pushing and publishing the matching release.
if [ "${HK_RUN_INSTALL_REHEARSAL:-}" != 1 ]; then
    printf '%s\n' \
        'install rehearsal is opt-in and requires network access.' \
        'Run with HK_RUN_INSTALL_REHEARSAL=1 after the release is public.' >&2
    exit 2
fi

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=scripts/harness-env.sh
source "$PROJECT_ROOT/scripts/harness-env.sh"
HK_ROOT=${HK_INSTALL_REHEARSAL_HOME:-/tmp/hk-install}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-install
LAST_SCREEN="$HK_ROOT/last-screen.txt"
CARGO_PATH=$HARNESS_CARGO_BIN:$HARNESS_PROFILE_BIN:/usr/bin:/bin:/usr/sbin:/sbin
export PATH="$CARGO_PATH"
TMUX_BIN=${HK_TMUX:-$(command -v tmux || true)}

# Herdr gets a sandbox HOME, but rustup's cargo shim still needs the invoking
# user's toolchain homes. Resolve them before h() applies the HOME override and
# pass through only directories that actually exist.
INVOKING_HOME=${HOME:-}
INVOKING_CARGO_HOME=${CARGO_HOME:-"${INVOKING_HOME:+$INVOKING_HOME/.cargo}"}
INVOKING_RUSTUP_HOME=${RUSTUP_HOME:-"${INVOKING_HOME:+$INVOKING_HOME/.rustup}"}
unset CARGO_HOME RUSTUP_HOME
if [ -n "$INVOKING_CARGO_HOME" ] && [ -d "$INVOKING_CARGO_HOME" ]; then
    export CARGO_HOME="$INVOKING_CARGO_HOME"
fi
if [ -n "$INVOKING_RUSTUP_HOME" ] && [ -d "$INVOKING_RUSTUP_HOME" ]; then
    export RUSTUP_HOME="$INVOKING_RUSTUP_HOME"
fi

case "$HK_ROOT" in
    /tmp/* | /private/tmp/*) ;;
    *)
        printf 'HK_INSTALL_REHEARSAL_HOME must be below /tmp or /private/tmp: %s\n' \
            "$HK_ROOT" >&2
        exit 2
        ;;
esac
if [ ! -x "$HERDR_BIN" ]; then
    printf 'Herdr binary is not executable: %s\n' "$HERDR_BIN" >&2
    exit 2
fi
if [ ! -x "$TMUX_BIN" ]; then
    printf 'tmux binary is not executable: %s\n' "$TMUX_BIN" >&2
    exit 2
fi

# shellcheck source=scripts/e2e-helpers.sh
source "$PROJECT_ROOT/scripts/e2e-helpers.sh"
trap cleanup EXIT

cleanup
rm -rf -- "$HK_ROOT"
write_herdr_config

make_repo "$HK_ROOT/repos/rehearsal-repo"

printf 'installing released plugin from GitHub...\n'
h plugin install thomasschafer/herdr-kiosk --yes
PLUGIN_CONFIG_DIR=$(h plugin config-dir "$PLUGIN_ID")
mkdir -p "$PLUGIN_CONFIG_DIR"
cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [{ path = "$HK_ROOT/repos", depth = 1 }]
EOF

printf 'starting herdr...\n'
start_herdr 160 45

invoke_picker
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "rehearsal-repo"
type_text rehearsal-repo
wait_screen_contains "1 of 1 repos"
press Enter
wait_screen_absent "herdr-kiosk — select repo"
wait_focused_checkout "$HK_ROOT/repos/rehearsal-repo"

printf 'real marketplace install, picker render, and repo open: PASS\n'
