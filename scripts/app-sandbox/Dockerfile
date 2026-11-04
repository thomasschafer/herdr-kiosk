# A Linux desktop with the released Herdr app, for trying the kiosk mod with real input
# and recording it without touching anyone's screen. See README.md beside this file.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb xdotool x11-utils mesa-vulkan-drivers libvulkan1 libgl1 libegl1 \
    libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libx11-xcb1 libxcb1 libfontconfig1 libfreetype6 \
    fonts-dejavu-core fonts-noto-core ffmpeg jq git curl ca-certificates build-essential pkg-config procps xz-utils tar \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -s /bin/bash dev
USER dev
WORKDIR /home/dev
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
ENV PATH=/home/dev/.cargo/bin:/home/dev/.local/bin:$PATH
RUN curl -fsSL https://releases.herdr.dev/app/install.sh | sh
