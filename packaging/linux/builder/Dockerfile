#
# Native Linux headless builder ABI baseline: Debian 10 (buster) / glibc 2.28.
# Node 22 official binaries require GLIBC_2.28+ (Ubuntu 18.04 / glibc 2.27 is
# NOT enough — verified empirically), so buster is the lowest Debian/Ubuntu
# base on which the whole toolchain (Node 22 + Rust + Go + musl) runs natively
# and the complete client, including the web UI, builds inside the image.
# Build each target platform natively or through Buildx QEMU; do not copy host
# toolchains into this image, otherwise their glibc baseline leaks into output.
FROM debian:10

ARG DEBIAN_FRONTEND=noninteractive
ARG http_proxy
ARG https_proxy
ARG HTTP_PROXY
ARG HTTPS_PROXY
ARG GO_VERSION=1.25.13
ARG NODE_VERSION=22.22.1
ARG RUST_VERSION=1.94.1
ARG GO_DOWNLOAD_BASE=https://go.dev/dl
ARG NODE_DOWNLOAD_BASE=https://nodejs.org/dist

# p2premote-punch 的 go.mod 要求 go 1.25，镜像必须内置 ≥1.25 的工具链，
# 否则 go build 会在运行期临时下载 toolchain（网络不稳定时构建直接失败）。
ENV http_proxy=${http_proxy} \
    https_proxy=${https_proxy} \
    HTTP_PROXY=${HTTP_PROXY} \
    HTTPS_PROXY=${HTTPS_PROXY} \
    GOPROXY=https://goproxy.cn,direct \
    CARGO_HOME=/opt/rust/cargo \
    RUSTUP_HOME=/opt/rust/rustup \
    PATH=/opt/rust/cargo/bin:/usr/local/go/bin:${PATH}

# buster left LTS support, so its packages live on the Debian archive
# (mirrored by Aliyun for mainland China). The archive is frozen and its
# Release metadata outdates, hence the relaxed Valid-Until check.
COPY sources.list /tmp/sources.list
RUN cp /tmp/sources.list /etc/apt/sources.list \
    && echo 'Acquire::Check-Valid-Until "false";' > /etc/apt/apt.conf.d/99-debian-archive \
    && apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        build-essential \
        linux-libc-dev \
        musl-tools \
        pkg-config \
        cmake \
        curl \
        file \
        git \
        make \
        perl \
        rpm \
        xz-utils \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# musl-tools exposes only the generic musl-gcc name. Publish an explicit
# target-architecture name so build scripts never fall back to a host compiler.
RUN case "$(uname -m)" in \
        x86_64) ln -s "$(command -v musl-gcc)" /usr/local/bin/x86_64-linux-musl-gcc ;; \
        aarch64) ln -s "$(command -v musl-gcc)" /usr/local/bin/aarch64-linux-musl-gcc ;; \
        *) echo "Unsupported target architecture: $(uname -m)" >&2; exit 1 ;; \
    esac

# Go and Node.js come from upstream tarballs into /usr/local; the frozen
# archive packages are far below the toolchain versions this project needs.
RUN case "$(uname -m)" in \
        x86_64) go_arch=amd64; node_arch=x64 ;; \
        aarch64) go_arch=arm64; node_arch=arm64 ;; \
        *) echo "Unsupported target architecture: $(uname -m)" >&2; exit 1 ;; \
    esac \
    && curl --http1.1 --fail --location --retry 10 --retry-delay 5 \
        --retry-connrefused --connect-timeout 30 --speed-limit 1024 --speed-time 60 \
        --continue-at - \
        "${GO_DOWNLOAD_BASE}/go${GO_VERSION}.linux-${go_arch}.tar.gz" \
        -o /tmp/go.tar.gz \
    && tar -C /usr/local -xzf /tmp/go.tar.gz \
    && rm /tmp/go.tar.gz \
    && curl --http1.1 --fail --location --retry 10 --retry-delay 5 \
        --retry-connrefused --connect-timeout 30 --speed-limit 1024 --speed-time 60 \
        --continue-at - \
        "${NODE_DOWNLOAD_BASE}/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" \
        -o /tmp/node.tar.xz \
    && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
    && rm /tmp/node.tar.xz \
    && go version \
    && node --version \
    && npm --version

RUN curl --http1.1 --fail --location --retry 10 --retry-delay 5 \
        --retry-connrefused --connect-timeout 30 --speed-limit 1024 --speed-time 60 \
        https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal \
        --default-toolchain "${RUST_VERSION}" \
        --component rustfmt \
    && rm /tmp/rustup-init.sh \
    && rustc --version \
    && cargo --version \
    && rustfmt --version

# 构建期通过 --build-arg http_proxy 等传入的代理只用于下载工具链，
# 不固化进运行期环境，保证镜像在任意网络下都可复用。
ENV http_proxy= https_proxy= HTTP_PROXY= HTTPS_PROXY=

WORKDIR /workspace
