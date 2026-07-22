HERDR := env_var_or_default("HERDR", "herdr")

build:
    cargo build --release

lint:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings

test:
    cargo test

readme:
    cargo run --example update-readme

e2e:
    ./scripts/e2e.sh

gif:
    ./scripts/gif/record.sh

link: build
    "{{HERDR}}" plugin link "{{justfile_directory()}}"

unlink:
    "{{HERDR}}" plugin unlink thomasschafer.herdr-kiosk
