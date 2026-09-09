FROM ubuntu:18.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_TOOLCHAIN=1.77.2 \
    PATH=/root/.cargo/bin:$PATH

RUN sed -i 's|\(archive\|security\).ubuntu.com/ubuntu|old-releases.ubuntu.com/ubuntu|g' /etc/apt/sources.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl file git patchelf pkg-config \
      libappindicator3-dev librsvg2-dev libssl-dev libwebkit2gtk-4.0-dev \
 && rm -rf /var/lib/apt/lists/* \
 && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.77.2 \
 && cargo install tauri-cli --version 1.6.3 --locked

WORKDIR /workspace
CMD ["bash"]
