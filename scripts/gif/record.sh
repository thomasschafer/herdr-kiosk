#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HK_ROOT=${HK_GIF_HOME:-/tmp/hk-gif}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-gif
LAST_SCREEN="$HK_ROOT/last-screen.txt"
FRAMES="$HK_ROOT/frames"
CAST="$HK_ROOT/preview.cast"
RENDERED_GIF="$HK_ROOT/preview.gif"
COLS=120
ROWS=32
PINNED_PATH=/Users/tomschafer/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin
RUNTIME_PATH=/etc/profiles/per-user/tomschafer/bin:$PINNED_PATH
TMUX_BIN=/etc/profiles/per-user/tomschafer/bin/tmux
AGG_BIN=/opt/homebrew/bin/agg
FFPROBE_BIN=/opt/homebrew/bin/ffprobe
AGG_THEME=24273a,cad3f5,494d64,ed8796,a6da95,eed49f,8aadf4,c6a0f6,8bd5ca,b8c0e0,5b6078,ed8796,a6da95,eed49f,8aadf4,f5bde6,8bd5ca,a5adcb
export PATH="$RUNTIME_PATH"

case "$HK_ROOT" in
    /tmp/* | /private/tmp/*) ;;
    *)
        printf 'HK_GIF_HOME must be below /tmp or /private/tmp: %s\n' "$HK_ROOT" >&2
        exit 2
        ;;
esac

for executable in "$HERDR_BIN" "$TMUX_BIN" "$AGG_BIN" "$FFPROBE_BIN"; do
    if [ ! -x "$executable" ]; then
        printf 'Required executable is not available: %s\n' "$executable" >&2
        exit 2
    fi
done

h() {
    env HOME="$HK_HOME_DIR" "$HERDR_BIN" "$@"
}

t() {
    "$TMUX_BIN" -S "$TMUX_SOCKET" "$@"
}

capture() {
    t capture-pane -p -t "$SESSION" >"$LAST_SCREEN"
}

fail() {
    printf 'gif recording failed: %s\n' "$1" >&2
    if t has-session -t "$SESSION" 2>/dev/null; then
        capture || true
        sed -n '1,160p' "$LAST_SCREEN" >&2 || true
        h plugin log list --plugin thomasschafer.herdr-kiosk >&2 || true
    fi
    exit 1
}

cleanup() {
    h server stop >/dev/null 2>&1 || true
    t kill-server >/dev/null 2>&1 || true
    rm -rf -- "$HK_ROOT"
}

wait_screen_contains() {
    local pattern=$1
    local attempts=${2:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        capture
        if grep -Fq "$pattern" "$LAST_SCREEN"; then
            return 0
        fi
        sleep 0.1
    done
    fail "screen did not contain: $pattern"
}

wait_screen_absent() {
    local pattern=$1
    local attempts=${2:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        capture
        if ! grep -Fq "$pattern" "$LAST_SCREEN"; then
            return 0
        fi
        sleep 0.1
    done
    fail "screen still contained: $pattern"
}

wait_path_exists() {
    local path=$1
    local attempts=${2:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        if [ -e "$path" ]; then
            return 0
        fi
        sleep 0.1
    done
    fail "path did not appear: $path"
}

make_repo() {
    local path=$1
    local name=${path##*/}
    mkdir -p "$path"
    git -C "$path" init -q -b master
    printf '# %s\n\nDemo fixture for the herdr-kiosk preview.\n' "$name" >"$path/README.md"
    git -C "$path" add README.md
    git -C "$path" -c user.name=Demo -c user.email=demo@example.invalid commit -qm initial
}

capture_loop() {
    local ms
    while t has-session -t "$SESSION" 2>/dev/null; do
        ms=$(/usr/bin/perl -MTime::HiRes=time -e 'printf "%d", time()*1000')
        t display -p -t "$SESSION" '@CURSOR #{cursor_x} #{cursor_y}' \; \
            capture-pane -e -p -t "$SESSION" >"$FRAMES/f_${ms}.txt" 2>/dev/null || true
        sleep 0.08
    done
}

send() {
    t send-keys -t "$SESSION" "$@"
}

type_slow() {
    local value=$1
    local index
    for ((index = 0; index < ${#value}; index++)); do
        send -l "${value:index:1}"
        sleep 0.08
    done
}

trap cleanup EXIT
cleanup
mkdir -p "$HK_HOME_DIR/.config/herdr" "$HK_ROOT/repos" "$HK_ROOT/worktrees" "$FRAMES"

cat >"$HK_HOME_DIR/.config/herdr/config.toml" <<EOF
[worktrees]
directory = "$HK_ROOT/worktrees"

[theme]
name = "catppuccin"

[theme.custom]
panel_bg = "#24273a"
surface0 = "#363a4f"
surface1 = "#494d64"
surface_dim = "#1e2030"
overlay0 = "#6e738d"
overlay1 = "#8087a2"
text = "#cad3f5"
subtext0 = "#a5adcb"
accent = "#8aadf4"
mauve = "#c6a0f6"
green = "#a6da95"
yellow = "#eed49f"
red = "#ed8796"
blue = "#8aadf4"
teal = "#8bd5ca"
peach = "#f5a97f"
EOF

cat >"$HK_HOME_DIR/.zshrc" <<'EOF'
autoload -Uz add-zsh-hook
herdr_kiosk_prompt() {
    local branch
    branch=$(git branch --show-current 2>/dev/null)
    PROMPT='%F{magenta}%1~%f'
    if [ -n "$branch" ]; then
        PROMPT+=" %F{cyan}${branch}%f"
    fi
    PROMPT+=' %F{green}❯%f '
}
add-zsh-hook precmd herdr_kiosk_prompt
EOF

cat >"$HK_HOME_DIR/.bashrc" <<'EOF'
herdr_kiosk_prompt() {
    local branch
    branch=$(git branch --show-current 2>/dev/null)
    PS1='\[\e[35m\]\W\[\e[0m\]'
    if [ -n "$branch" ]; then
        PS1+=" \[\e[36m\]${branch}\[\e[0m\]"
    fi
    PS1+=' \[\e[32m\]❯\[\e[0m\] '
}
PROMPT_COMMAND=herdr_kiosk_prompt
EOF

cat >"$HK_HOME_DIR/.bash_profile" <<'EOF'
. "$HOME/.bashrc"
EOF

for repo in dotfiles web-app api-server notes herdr-kiosk infra design-system mobile-app; do
    make_repo "$HK_ROOT/repos/$repo"
done

BRANCH_REPO="$HK_ROOT/repos/herdr-kiosk"
git -C "$BRANCH_REPO" branch docs/readme-demo
git -C "$BRANCH_REPO" branch feature/quick-switch
git -C "$BRANCH_REPO" branch fix/picker-focus

REMOTE="$HK_ROOT/kiosk-remote.git"
git init -q --bare "$REMOTE"
make_repo "$HK_ROOT/remote-seed"
git -C "$HK_ROOT/remote-seed" branch experiment/sidebar
git -C "$HK_ROOT/remote-seed" remote add origin "$REMOTE"
git -C "$HK_ROOT/remote-seed" push -q origin master experiment/sidebar
git -C "$BRANCH_REPO" remote add origin "$REMOTE"

printf 'Building plugin release binary...\n'
(cd "$PROJECT_ROOT" && env PATH="$PINNED_PATH" cargo build --release --quiet)
h plugin link "$PROJECT_ROOT" >/dev/null
PLUGIN_CONFIG_DIR=$(h plugin config-dir thomasschafer.herdr-kiosk)
mkdir -p "$PLUGIN_CONFIG_DIR"
cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [{ path = "$HK_ROOT/repos", depth = 1 }]
EOF

printf 'Starting Herdr and recording demo...\n'
t new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" \
    "env HOME='$HK_HOME_DIR' '$HERDR_BIN'"
sleep 2
send Enter
sleep 0.5
send Escape
sleep 0.5

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "8 of 8 repos"
capture_loop &
CAPTURE_PID=$!
sleep 0.1
sleep 1.2
type_slow app
wait_screen_contains "2 of 8 repos"
sleep 1.2
send Escape
sleep 0.5
type_slow kiosk
wait_screen_contains "1 of 8 repos"
sleep 1.2
kill "$CAPTURE_PID" 2>/dev/null || true
wait "$CAPTURE_PID" 2>/dev/null || true
send Tab
wait_screen_contains "5 of 5 branches"
wait_screen_contains "experiment/sidebar (remote)"
capture_loop &
CAPTURE_PID=$!
sleep 0.1
sleep 1.8
type_slow docs
wait_screen_contains "1 of 5 branches"
sleep 1.2
send Enter
wait_screen_absent "herdr-kiosk — select branch"
wait_path_exists "$HK_ROOT/worktrees/herdr-kiosk/docs-readme-demo"
sleep 1.8

t kill-server >/dev/null 2>&1 || true
wait "$CAPTURE_PID" 2>/dev/null || true

/usr/bin/python3 "$PROJECT_ROOT/scripts/gif/build_cast.py" "$FRAMES" "$CAST" "$COLS" "$ROWS"
"$AGG_BIN" --theme "$AGG_THEME" --font-size 13 --fps-cap 15 \
    --last-frame-duration 1.2 --quiet \
    "$CAST" "$RENDERED_GIF"
mkdir -p "$PROJECT_ROOT/media"
mv "$RENDERED_GIF" "$PROJECT_ROOT/media/preview.gif"

GIF_STATS=$("$FFPROBE_BIN" -v error -select_streams v:0 \
    -show_entries stream=width,height,nb_frames -of csv=p=0 \
    "$PROJECT_ROOT/media/preview.gif")
GIF_BYTES=$(wc -c <"$PROJECT_ROOT/media/preview.gif" | tr -d ' ')
printf 'gif: %s (width,height,frames)\n' "$GIF_STATS"
printf 'output: %s (%s bytes)\n' "$PROJECT_ROOT/media/preview.gif" "$GIF_BYTES"
