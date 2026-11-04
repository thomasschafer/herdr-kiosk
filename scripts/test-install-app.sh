#!/usr/bin/env bash
# Tests install-app.sh against a fake release and a fake app, from a checkout and from a
# release fetched with git.
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=${HK_INSTALL_TEST_ROOT:-/tmp/hk-install-app-test}

case "$TEST_ROOT" in
    /tmp/* | /private/tmp/*) ;;
    *)
        printf 'HK_INSTALL_TEST_ROOT must be below /tmp or /private/tmp: %s\n' "$TEST_ROOT" >&2
        exit 2
        ;;
esac

cleanup() {
    rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT
[[ -n ${HK_INSTALL_TEST_KEEP:-} ]] && trap - EXIT
cleanup

VERSION=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$PROJECT_ROOT/herdr-plugin.toml")
repo=$TEST_ROOT/repo
mkdir -p "$TEST_ROOT/bin" "$TEST_ROOT/releases" "$repo/scripts" "$repo/herdr-app/kiosk"
cp "$PROJECT_ROOT/scripts/install-app.sh" "$PROJECT_ROOT/scripts/fetch-or-build.sh" "$repo/scripts/"
cp "$PROJECT_ROOT/herdr-plugin.toml" "$repo/"
cp "$PROJECT_ROOT/herdr-app/kiosk/main.js" "$PROJECT_ROOT/herdr-app/kiosk/mod.json" "$repo/herdr-app/kiosk/"

cat >"$TEST_ROOT/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in
    -s) printf 'Linux\n' ;;
    -m) printf 'x86_64\n' ;;
    *) exit 2 ;;
esac
EOF
cat >"$TEST_ROOT/bin/herdr-app" <<EOF
#!/bin/sh
printf '%s %s\n' "\$HERDR_APP_HOME" "\$*" >>"$TEST_ROOT/herdr-app-calls"
EOF
chmod +x "$TEST_ROOT/bin/uname" "$TEST_ROOT/bin/herdr-app"

asset=herdr-kiosk-v$VERSION-x86_64-unknown-linux-gnu
printf '#!/bin/sh\necho released\n' >"$TEST_ROOT/releases/$asset"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$TEST_ROOT/releases" && sha256sum "$asset" >SHA256SUMS)
else
    (cd "$TEST_ROOT/releases" && shasum -a 256 "$asset" >SHA256SUMS)
fi

# Every run gets an app home of its own here, never the real one.
export PATH="$TEST_ROOT/bin:$PATH" HK_BASE_URL="file://$TEST_ROOT/releases" HK_UNAME="$TEST_ROOT/bin/uname" \
    HERDR_APP_HOME="$TEST_ROOT/default-home"

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

assert_installed() {
    local mod=$1/mods/kiosk
    [[ -x $mod/bin/herdr-kiosk ]] || fail "no executable binary in $mod"
    cmp -s "$mod/bin/herdr-kiosk" "$TEST_ROOT/releases/$asset" || fail "$mod has a different binary"
    cmp -s "$mod/main.js" "$PROJECT_ROOT/herdr-app/kiosk/main.js" || fail "$mod has a different main.js"
    cmp -s "$mod/mod.json" "$PROJECT_ROOT/herdr-app/kiosk/mod.json" || fail "$mod has a different mod.json"
    grep -qx "$1 check $mod" "$TEST_ROOT/herdr-app-calls" || fail "the app did not check $mod"
}

printf 'test: installs from a checkout with its verified release binary\n'
home=$TEST_ROOT/home
HERDR_APP_HOME=$home sh "$repo/scripts/install-app.sh" >"$TEST_ROOT/out" 2>&1
assert_installed "$home"
grep -q 'installed verified' "$TEST_ROOT/out" || fail "the binary was not verified: $(cat "$TEST_ROOT/out")"

printf 'test: reinstalling replaces the whole folder\n'
touch "$home/mods/kiosk/stale.js"
HERDR_APP_HOME=$home sh "$repo/scripts/install-app.sh" >/dev/null 2>&1
[[ ! -e $home/mods/kiosk/stale.js ]] || fail "a stale file survived"
[[ ! -e $home/mods/kiosk.old ]] || fail "the old install was left behind"
assert_installed "$home"

printf 'test: the app home follows XDG_CONFIG_HOME\n'
env -u HERDR_APP_HOME XDG_CONFIG_HOME="$TEST_ROOT/xdg" sh "$repo/scripts/install-app.sh" >/dev/null 2>&1
assert_installed "$TEST_ROOT/xdg/herdr-app"

printf 'test: a linked development install is left alone\n'
linked=$TEST_ROOT/linked
mkdir -p "$linked/mods"
ln -s "$repo/herdr-app/kiosk" "$linked/mods/kiosk"
if HERDR_APP_HOME=$linked sh "$repo/scripts/install-app.sh" 2>"$TEST_ROOT/out"; then
    fail "installed over a link"
fi
grep -q 'is a link' "$TEST_ROOT/out" || fail "unclear refusal: $(cat "$TEST_ROOT/out")"
[[ ! -e $repo/herdr-app/kiosk/bin ]] || fail "wrote into the linked checkout"

printf 'test: run on its own, it fetches the newest release tag\n'
# Independent of the user's git config, which may sign commits and tags.
g() { git -C "$repo" -c user.email=test@example.com -c user.name=Test -c commit.gpgSign=false -c tag.gpgSign=false -c tag.forceSignAnnotated=false "$@"; }
g init -q
g add -A
g commit -q -m release
for tag in v0.0.9 v0.0.10 "v$VERSION" v0.0.1-rc.1; do g tag "$tag"; done
mkdir "$TEST_ROOT/elsewhere"
cp "$repo/scripts/install-app.sh" "$TEST_ROOT/elsewhere/"
released=$TEST_ROOT/released
HERDR_APP_HOME=$released KIOSK_INSTALL_REPO="file://$repo" sh "$TEST_ROOT/elsewhere/install-app.sh" 2>"$TEST_ROOT/out"
grep -q "fetching herdr-kiosk v$VERSION" "$TEST_ROOT/out" || fail "did not pick v$VERSION: $(cat "$TEST_ROOT/out")"
assert_installed "$released"

printf 'test: options are checked\n'
if sh "$repo/scripts/install-app.sh" --frobnicate 2>"$TEST_ROOT/out"; then fail "took an unknown option"; fi
grep -q 'unknown option --frobnicate' "$TEST_ROOT/out" || fail "unclear error: $(cat "$TEST_ROOT/out")"
if sh "$TEST_ROOT/elsewhere/install-app.sh" --from-source 2>"$TEST_ROOT/out"; then fail "built without a checkout"; fi
grep -q 'needs a checkout' "$TEST_ROOT/out" || fail "unclear error: $(cat "$TEST_ROOT/out")"

printf 'install-app tests passed\n'
