FROM ubuntu:18.04

ARG GO_VERSION=1.25.13

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_TOOLCHAIN=1.77.2 \
    PATH=/root/.cargo/bin:/usr/local/go/bin:$PATH

RUN sed -i \
      -e 's|archive.ubuntu.com|mirrors.aliyun.com|g' \
      -e 's|security.ubuntu.com|mirrors.aliyun.com|g' \
      /etc/apt/sources.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl file git musl-tools patchelf pkg-config \
      libappindicator3-dev librsvg2-dev libssl-dev libwebkit2gtk-4.0-dev \
 && rm -rf /var/lib/apt/lists/* \
 && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.77.2 \
 && cargo install tauri-cli --version 1.6.3 --locked

RUN curl --proto '=https' --tlsv1.2 --fail --location --retry 5 \
      "https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz" -o /tmp/go.tar.gz \
 && tar -C /usr/local -xzf /tmp/go.tar.gz \
 && rm /tmp/go.tar.gz \
 && ln -s "$(command -v musl-gcc)" /usr/local/bin/x86_64-linux-musl-gcc \
 && go version \
 && x86_64-linux-musl-gcc -dumpmachine

RUN apt-get update \
 && apt-get install -y --no-install-recommends dbus-x11 systemd wget xauth xvfb \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
CMD ["bash"]
