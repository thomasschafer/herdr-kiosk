#!/bin/sh
# Installs kiosk's Herdr app mod: the mod files and the herdr-kiosk binary, together in
# <app home>/mods/kiosk, where the mod looks for them.
#
#   curl -fsSL https://raw.githubusercontent.com/thomasschafer/herdr-kiosk/main/scripts/install-app.sh | sh
#   sh scripts/install-app.sh [--from-source] [--version <tag>]
#
# Run on its own, it fetches the latest release (or --version) and installs its verified
# prebuilt binary, building from source only where there is none. Run from a checkout, it
# installs that checkout's mod with that checkout's release binary, or with --from-source,
# a binary built from the checkout. The app home is $HERDR_APP_HOME, else
# $XDG_CONFIG_HOME/herdr-app, else ~/.config/herdr-app, as the app has it.
set -eu

REPO_URL=${KIOSK_INSTALL_REPO:-https://github.com/thomasschafer/herdr-kiosk.git}
MOD_NAME=kiosk

say() { printf 'install-app: %s\n' "$*" >&2; }
fail() {
    say "$*"
    exit 1
}

from_source=false
version=
while [ $# -gt 0 ]; do
    case $1 in
        --from-source) from_source=true ;;
        --version)
            [ $# -ge 2 ] || fail '--version needs a tag, such as v0.3.0'
            version=$2
            shift
            ;;
        -h | --help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) fail "unknown option $1 (see --help)" ;;
    esac
    shift
done

if [ -n "${HERDR_APP_HOME:-}" ]; then
    app_home=$HERDR_APP_HOME
elif [ -n "${XDG_CONFIG_HOME:-}" ]; then
    app_home=$XDG_CONFIG_HOME/herdr-app
else
    [ -n "${HOME:-}" ] || fail 'HOME is not set, so the app home is unknown; set HERDR_APP_HOME'
    app_home=$HOME/.config/herdr-app
fi
mod_dir=$app_home/mods/$MOD_NAME

tmp=$(mktemp -d "${TMPDIR:-/tmp}/herdr-kiosk-install.XXXXXX") || fail 'could not create a temporary directory'
trap 'rm -rf -- "$tmp"' EXIT HUP INT TERM

# The checkout this script is in, if it is in one and no other version was asked for.
script_dir=
if dir=$(CDPATH='' cd -- "$(dirname -- "$0")" 2>/dev/null && pwd); then
    script_dir=$dir
fi
checkout=
if [ -z "$version" ] && [ -n "$script_dir" ] && [ -f "$script_dir/../herdr-plugin.toml" ] &&
    [ -f "$script_dir/../herdr-app/kiosk/main.js" ]; then
    checkout=$(CDPATH='' cd -- "$script_dir/.." && pwd)
fi

if [ -z "$checkout" ] && $from_source; then
    fail '--from-source needs a checkout: clone the repository and run scripts/install-app.sh from it'
fi

# A linked checkout is a development install: writing into it would put the binary in the
# checkout. It keeps working as it is.
if [ -L "$mod_dir" ]; then
    fail "$mod_dir is a link (to $(readlink "$mod_dir")); remove it to install a copy instead"
fi

if [ -z "$checkout" ]; then
    command -v git >/dev/null 2>&1 || fail 'git is needed to fetch the release'
    if [ -z "$version" ]; then
        version=$(git ls-remote --tags --refs "$REPO_URL" 'v*' | sed 's|.*refs/tags/||' |
            grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' |
            awk -F. '{ tag = $0; major = substr($1, 2); printf "%09d%09d%09d %s\n", major, $2, $3, tag }' |
            sort | tail -n 1 | cut -d' ' -f2)
        [ -n "$version" ] || fail "no release tags found at $REPO_URL"
    fi
    say "fetching herdr-kiosk $version"
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$version" "$REPO_URL" "$tmp/checkout" ||
        fail "could not fetch $version from $REPO_URL"
    checkout=$tmp/checkout
fi

binary=$tmp/herdr-kiosk
if $from_source; then
    say "building $checkout"
    command -v cargo >/dev/null 2>&1 || fail 'cargo is needed to build from source: https://rustup.rs/'
    (cd "$checkout" && cargo build --locked --release) || fail 'the build failed'
    install -m 755 "${CARGO_TARGET_DIR:-$checkout/target}/release/herdr-kiosk" "$binary"
else
    HK_REPO_ROOT=$checkout HK_OUT=$binary sh "$checkout/scripts/fetch-or-build.sh" ||
        fail 'could not get the herdr-kiosk binary'
fi

staged=$tmp/$MOD_NAME
mkdir -p "$staged/bin"
cp "$checkout/herdr-app/kiosk/main.js" "$checkout/herdr-app/kiosk/mod.json" "$staged/"
install -m 755 "$binary" "$staged/bin/herdr-kiosk"

# Swapped in whole, so the app never loads half of an update. A running picker keeps the
# old binary it has open.
mkdir -p "$app_home/mods"
if [ -e "$mod_dir" ]; then
    rm -rf -- "$mod_dir.old"
    mv -- "$mod_dir" "$mod_dir.old"
fi
mv -- "$staged" "$mod_dir"
rm -rf -- "$mod_dir.old"
say "installed $mod_dir"

if command -v herdr-app >/dev/null 2>&1; then
    HERDR_APP_HOME=$app_home herdr-app check "$mod_dir" >&2 || fail "the app could not load $mod_dir"
fi
if [ "$(uname -s 2>/dev/null || true)" = Darwin ]; then
    shortcut=cmd-p
else
    shortcut=ctrl-shift-r
fi
cat >&2 <<EOF
install-app: bind a key to open it in $app_home/keys.json, for example:
  { "global": { "$shortcut": "mod.open:kiosk" } }
EOF
