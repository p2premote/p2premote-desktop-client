#
# Native Linux headless builder ABI baseline: Ubuntu 16.04 / glibc 2.23.
# Build each target platform natively or through Buildx QEMU; do not copy host
# toolchains into this image, otherwise their glibc baseline leaks into output.
FROM ubuntu:16.04

ARG DEBIAN_FRONTEND=noninteractive
ARG http_proxy
ARG https_proxy
ARG HTTP_PROXY
ARG HTTPS_PROXY
ARG GO_VERSION=1.24.4

ENV http_proxy=${http_proxy} \
    https_proxy=${https_proxy} \
    HTTP_PROXY=${HTTP_PROXY} \
    HTTPS_PROXY=${HTTPS_PROXY} \
    CARGO_HOME=/opt/rust/cargo \
    RUSTUP_HOME=/opt/rust/rustup \
    PATH=/opt/rust/cargo/bin:/usr/local/go/bin:${PATH}

COPY sources.list /tmp/sources.list

# Xenial's base APT image has no HTTPS transport. Bootstrap it from the same
# Aliyun mirror over HTTP, then restore the HTTPS-only source list above.
# Aliyun hosts ARM64 packages under ubuntu-ports rather than ubuntu.
RUN case "$(uname -m)" in \
        x86_64) cp /tmp/sources.list /tmp/selected-sources.list ;; \
        aarch64) sed 's|mirrors.aliyun.com/ubuntu/|mirrors.aliyun.com/ubuntu-ports/|g' \
            /tmp/sources.list > /tmp/selected-sources.list ;; \
        *) echo "Unsupported target architecture: $(uname -m)" >&2; exit 1 ;; \
    esac \
    && sed 's|https://|http://|g' /tmp/selected-sources.list > /etc/apt/sources.list \
    && apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        apt-transport-https \
        ca-certificates \
    && cp /tmp/selected-sources.list /etc/apt/sources.list

RUN apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        build-essential \
        linux-libc-dev \
        musl-tools \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

# musl-tools exposes only the generic musl-gcc name. Publish an explicit
# target-architecture name so build scripts never fall back to a host compiler.
RUN case "$(uname -m)" in \
        x86_64) ln -s "$(command -v musl-gcc)" /usr/local/bin/x86_64-linux-musl-gcc ;; \
        aarch64) ln -s "$(command -v musl-gcc)" /usr/local/bin/aarch64-linux-musl-gcc ;; \
        *) echo "Unsupported target architecture: $(uname -m)" >&2; exit 1 ;; \
    esac

RUN apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        cmake \
        curl \
        file \
        git \
        make \
        perl \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN case "$(uname -m)" in \
        x86_64) go_arch=amd64 ;; \
        aarch64) go_arch=arm64 ;; \
        *) echo "Unsupported target architecture: $(uname -m)" >&2; exit 1 ;; \
    esac \
    && curl --fail --location --retry 3 \
        "https://go.dev/dl/go${GO_VERSION}.linux-${go_arch}.tar.gz" \
        -o /tmp/go.tar.gz \
    && tar -C /usr/local -xzf /tmp/go.tar.gz \
    && rm /tmp/go.tar.gz \
    && go version

RUN curl --fail --location --retry 3 https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal \
    && rm /tmp/rustup-init.sh \
    && rustc --version \
    && cargo --version

WORKDIR /workspace
