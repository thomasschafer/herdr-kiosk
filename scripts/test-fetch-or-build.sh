#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=${HK_FETCH_TEST_ROOT:-/tmp/hk-fetch-or-build-test}

case "$TEST_ROOT" in
    /tmp/* | /private/tmp/*) ;;
    *)
        printf 'HK_FETCH_TEST_ROOT must be below /tmp or /private/tmp: %s\n' "$TEST_ROOT" >&2
        exit 2
        ;;
esac

cleanup() {
    rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT
cleanup
mkdir -p "$TEST_ROOT/bin" "$TEST_ROOT/repo/scripts" "$TEST_ROOT/releases"
cp "$PROJECT_ROOT/scripts/fetch-or-build.sh" "$TEST_ROOT/repo/scripts/"
cp "$PROJECT_ROOT/herdr-plugin.toml" "$TEST_ROOT/repo/"

cat >"$TEST_ROOT/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in
    -s) printf 'Linux\n' ;;
    -m) printf 'x86_64\n' ;;
    *) exit 2 ;;
esac
EOF

cat >"$TEST_ROOT/bin/cargo" <<'EOF'
#!/bin/sh
printf 'cargo %s\n' "$*" >>"$HK_TEST_LOG"
mkdir -p "$HK_REPO_ROOT/target/release"
printf 'built from source\n' >"$HK_REPO_ROOT/target/release/herdr-kiosk"
chmod +x "$HK_REPO_ROOT/target/release/herdr-kiosk"
EOF

cat >"$TEST_ROOT/bin/curl" <<'EOF'
#!/bin/sh
destination=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) destination=$2; shift 2 ;;
        --*) shift ;;
        *) url=$1; shift ;;
    esac
done
source=${url#file://}
[ -f "$source" ] || exit 22
cp "$source" "$destination"
EOF
chmod +x "$TEST_ROOT/bin/uname" "$TEST_ROOT/bin/cargo" "$TEST_ROOT/bin/curl"

VERSION=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$PROJECT_ROOT/herdr-plugin.toml")
ASSET="herdr-kiosk-v${VERSION}-x86_64-unknown-linux-gnu"
BASE="$TEST_ROOT/releases/v$VERSION"
mkdir -p "$BASE"

run_installer() {
    env \
        HOME="$TEST_ROOT/home" \
        HK_REPO_ROOT="$TEST_ROOT/repo" \
        HK_OUT="$TEST_ROOT/out/herdr-kiosk" \
        HK_BASE_URL="file://$BASE" \
        HK_UNAME="$TEST_ROOT/bin/uname" \
        HK_CURL="$TEST_ROOT/bin/curl" \
        HK_WGET="$TEST_ROOT/bin/missing-wget" \
        HK_CARGO="$TEST_ROOT/bin/cargo" \
        HK_TEST_LOG="$TEST_ROOT/cargo.log" \
        "$TEST_ROOT/repo/scripts/fetch-or-build.sh"
}

rm -f "$BASE/$ASSET" "$TEST_ROOT/cargo.log"
printf '%064d  %s\n' 0 "$ASSET" >"$BASE/SHA256SUMS"
run_installer 2>"$TEST_ROOT/no-asset.err"
grep -Fq "could not download $ASSET" "$TEST_ROOT/no-asset.err"
grep -Fq 'building from source' "$TEST_ROOT/no-asset.err"
grep -Fq 'cargo build --locked --release' "$TEST_ROOT/cargo.log"
printf 'fallback when no asset: ok\n'

printf 'downloaded but corrupt\n' >"$BASE/$ASSET"
printf '%064d  %s\n' 0 "$ASSET" >"$BASE/SHA256SUMS"
rm -f "$TEST_ROOT/cargo.log" "$TEST_ROOT/out/herdr-kiosk"
run_installer 2>"$TEST_ROOT/mismatch.err"
grep -Fq 'checksum mismatch' "$TEST_ROOT/mismatch.err"
grep -Fq 'cargo build --locked --release' "$TEST_ROOT/cargo.log"
printf 'checksum mismatch falls back: ok\n'

printf '#!/bin/sh\nprintf "stubbed release binary\\n"\n' >"$BASE/$ASSET"
if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$BASE/$ASSET" | awk '{ print $1 }')
else
    checksum=$(shasum -a 256 "$BASE/$ASSET" | awk '{ print $1 }')
fi
printf '%s  %s\n' "$checksum" "$ASSET" >"$BASE/SHA256SUMS"
rm -f "$TEST_ROOT/cargo.log" "$TEST_ROOT/out/herdr-kiosk"
run_installer >"$TEST_ROOT/success.out" 2>"$TEST_ROOT/success.err"
[ -x "$TEST_ROOT/out/herdr-kiosk" ]
cmp "$BASE/$ASSET" "$TEST_ROOT/out/herdr-kiosk"
[ ! -e "$TEST_ROOT/cargo.log" ]
grep -Fq 'installed verified' "$TEST_ROOT/success.out"
printf 'successful stubbed download: ok\n'

printf 'fetch-or-build tests: PASS\n'
