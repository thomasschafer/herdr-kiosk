HERDR := env_var_or_default("HERDR", "herdr")

build:
    cargo build --release

lint:
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets -- -D warnings

test:
    cargo test --locked --workspace --all-targets

readme:
    cargo xtask readme

readme-check:
    cargo --locked xtask readme --check

e2e:
    ./scripts/e2e.sh

gif:
    ./scripts/gif/record.sh

link: build
    "{{HERDR}}" plugin link "{{justfile_directory()}}"

unlink:
    "{{HERDR}}" plugin unlink thomasschafer.herdr-kiosk
