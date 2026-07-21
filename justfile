HERDR := env_var_or_default("HERDR", "herdr")

build:
    cargo build --release

lint:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings

test:
    cargo test

link: build
    "{{HERDR}}" plugin link "{{justfile_directory()}}"

unlink:
    "{{HERDR}}" plugin unlink thomasschafer.herdr-kiosk
