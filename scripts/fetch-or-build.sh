#!/bin/sh
set -eu

REPO_ROOT=${HK_REPO_ROOT:-"$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"}
OUT=${HK_OUT:-"$REPO_ROOT/target/release/herdr-kiosk"}
UNAME=${HK_UNAME:-uname}
CURL=${HK_CURL:-curl}
WGET=${HK_WGET:-wget}
CARGO=${HK_CARGO:-cargo}
MANIFEST="$REPO_ROOT/herdr-plugin.toml"

version_lines=$(sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)"[[:space:]]*$/\1/p' "$MANIFEST" 2>/dev/null || true)
VERSION=$(printf '%s\n' "$version_lines" | sed -n '1p')
version_count=$(printf '%s\n' "$version_lines" | awk 'NF { count++ } END { print count + 0 }')
case "$VERSION" in
    '' | *[!0-9A-Za-z.+-]*)
        printf 'fetch-or-build: could not parse a valid version from %s\n' "$MANIFEST" >&2
        exit 1
        ;;
esac
if [ "$version_count" -ne 1 ]; then
    printf 'fetch-or-build: expected exactly one version in %s, found %s\n' \
        "$MANIFEST" "$version_count" >&2
    exit 1
fi

BASE_URL=${HK_BASE_URL:-"https://github.com/thomasschafer/herdr-kiosk/releases/download/v$VERSION"}

fallback() {
    reason=$1
    printf 'fetch-or-build: prebuilt binary unavailable (%s); building from source.\n' "$reason" >&2

    if [ -f "${HOME:-}/.cargo/env" ]; then
        # shellcheck disable=SC1091
        . "${HOME}/.cargo/env"
    fi
    if ! command -v "$CARGO" >/dev/null 2>&1; then
        printf '%s\n' \
            'fetch-or-build: cargo is required for the source-build fallback.' \
            'Install Rust with rustup: https://rustup.rs/' >&2
        exit 1
    fi

    (cd "$REPO_ROOT" && "$CARGO" build --locked --release)
    built_out=${CARGO_TARGET_DIR:-"$REPO_ROOT/target"}/release/herdr-kiosk
    if [ "$built_out" != "$OUT" ]; then
        if [ ! -f "$built_out" ]; then
            printf 'fetch-or-build: cargo succeeded but did not produce %s\n' "$built_out" >&2
            exit 1
        fi
        mkdir -p "$(dirname -- "$OUT")"
        install -m 755 "$built_out" "$OUT"
    fi
}

download() {
    url=$1
    destination=$2
    if command -v "$CURL" >/dev/null 2>&1; then
        "$CURL" --fail --location --silent --show-error --output "$destination" "$url"
    elif command -v "$WGET" >/dev/null 2>&1; then
        "$WGET" --quiet --output-document="$destination" "$url"
    else
        return 127
    fi
}

sha256() {
    file=$1
    if [ -n "${HK_SHA256:-}" ] && command -v "$HK_SHA256" >/dev/null 2>&1; then
        "$HK_SHA256" "$file" | awk '{ print $1 }'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{ print $1 }'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file" | awk '{ print $NF }'
    else
        return 127
    fi
}

os=$($UNAME -s 2>/dev/null || true)
arch=$($UNAME -m 2>/dev/null || true)
case "$os:$arch" in
    Linux:x86_64 | Linux:amd64) target=x86_64-unknown-linux-gnu ;;
    Linux:aarch64 | Linux:arm64) target=aarch64-unknown-linux-gnu ;;
    Darwin:x86_64 | Darwin:amd64) target=x86_64-apple-darwin ;;
    Darwin:arm64 | Darwin:aarch64) target=aarch64-apple-darwin ;;
    *)
        fallback "unmapped platform $os/$arch"
        exit 0
        ;;
esac

asset="herdr-kiosk-v$VERSION-$target"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/herdr-kiosk-fetch.XXXXXX") || {
    fallback 'could not create a temporary directory'
    exit 0
}
trap 'rm -rf -- "$tmp_dir"' EXIT HUP INT TERM

if ! download "$BASE_URL/SHA256SUMS" "$tmp_dir/SHA256SUMS"; then
    fallback 'could not download SHA256SUMS'
    exit 0
fi
if ! download "$BASE_URL/$asset" "$tmp_dir/$asset"; then
    fallback "could not download $asset"
    exit 0
fi

expected=$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1 }' \
    "$tmp_dir/SHA256SUMS" | sed -n '1p')
case "$expected" in
    '' | *[!0-9A-Fa-f]*)
        fallback "SHA256SUMS has no valid entry for $asset"
        exit 0
        ;;
esac
if [ "${#expected}" -ne 64 ]; then
    fallback "SHA256SUMS has no valid entry for $asset"
    exit 0
fi
actual=$(sha256 "$tmp_dir/$asset" 2>/dev/null || true)
if [ -z "$actual" ]; then
    fallback 'no SHA-256 tool is available'
    exit 0
fi
if [ "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" != "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" ]; then
    fallback "checksum mismatch for $asset"
    exit 0
fi

mkdir -p "$(dirname -- "$OUT")"
install -m 755 "$tmp_dir/$asset" "$OUT"
printf 'fetch-or-build: installed verified %s at %s\n' "$asset" "$OUT"
