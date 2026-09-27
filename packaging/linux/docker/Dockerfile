# p2premote Linux 客户端 Docker 镜像
#
# 复用 build-linux-headless.sh 产出的 GNU/glibc tgz 产物，避免在 Docker
# builder 里重新装配 Rust + Node + musl 工具链。仅支持 host 网络模式：
# bridge 模式下 Web Admin 看到的来源 IP 会塌缩为 docker0 网关地址，导致
# web_admin_allowed_ip 白名单依赖容器看到的真实来源地址。
#
# 构建上下文需放置 tgz 产物，默认文件名 p2premote-headless.tar.gz，
# 可通过 --build-arg P2P_HEADLESS_TGZ=<文件名> 覆盖。建议使用
# scripts/build-linux.sh 自动完成编译、四格式打包与上下文准备。

FROM ubuntu:22.04

ARG http_proxy
ARG https_proxy
ARG HTTP_PROXY
ARG HTTPS_PROXY
ENV http_proxy=${http_proxy} \
    https_proxy=${https_proxy} \
    HTTP_PROXY=${HTTP_PROXY} \
    HTTPS_PROXY=${HTTPS_PROXY}

# 运行时依赖：
#   iproute2  提供 `ip` 命令，service 在创建/删除 WireGuard 接口时依赖
#   ca-certificates  service 访问 server API 时的 TLS 根证书
#   tzdata  日志按本地时间滚动
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        iproute2 \
        ca-certificates \
        tzdata \
    && rm -rf /var/lib/apt/lists/*

ENV http_proxy= \
    https_proxy= \
    HTTP_PROXY= \
    HTTPS_PROXY=

ENV TZ=Asia/Shanghai

WORKDIR /opt/p2premote

# 解压 headless tgz 到 /opt/p2premote，保留 resources/、install-service.sh、
# uninstall-service.sh 原始目录结构，避免 core 内相对路径解析出现偏差。
ARG P2P_HEADLESS_TGZ=p2premote-headless.tar.gz
COPY ${P2P_HEADLESS_TGZ} /tmp/p2premote-headless.tar.gz
RUN set -eux; \
    mkdir -p /opt/p2premote; \
    tar -xzf /tmp/p2premote-headless.tar.gz -C /tmp; \
    cp -a /tmp/p2premote-headless/. /opt/p2premote/; \
    rm -rf /tmp/p2premote-headless /tmp/p2premote-headless.tar.gz; \
    # 必需二进制缺失应当让 build 失败，而不是吞掉错误
    chmod 755 \
        /opt/p2premote/resources/p2premote-service \
        /opt/p2premote/resources/p2premote-cli \
        /opt/p2premote/resources/wireguard-go \
        /opt/p2premote/resources/wg; \
    chmod 755 \
        /opt/p2premote/install-service.sh \
        /opt/p2premote/uninstall-service.sh; \
    mkdir -p /opt/p2premote/data /opt/p2premote/logs /opt/p2premote/run

# 固定安装根目录，保证 core 的资源、配置、日志、run 目录解析稳定。
# 参考 core/src/config.rs 的 linux_install_root_dir。
ENV P2PREMOTE_INSTALL_ROOT=/opt/p2premote

# Docker 与直接安装的 Headless 包使用同一二进制，通过运行时目标区分版本策略。
ARG P2PREMOTE_UPDATE_TARGET=linux-docker-x64
ENV P2PREMOTE_UPDATE_TARGET=${P2PREMOTE_UPDATE_TARGET}

# 首次信任（TOFU）引导：无 GUI 的 NAS 场景没有本机浏览器可用，全新部署
# （未配置安全码且未配置白名单）时 Web Admin 直接监听 0.0.0.0，首个远程
# 客户端获得引导访问权并被要求设置安全码；设置完成后白名单锁定到该来源
# IP，其余来源一律拒绝。容器界面创建后无需查看 stdout——引导直接在浏览器
# 里完成。开关仅在本镜像注入，裸机安装保持 loopback-only 默认。
ENV P2PREMOTE_WEB_ADMIN_FIRST_TRUST=1

# Web Admin 默认监听端口。host 网络模式下宿主机直接暴露该端口；
# 外部访问该端口的源 IP 即真实客户端 IP，是 web_admin_allowed_ip 生效的前提。
EXPOSE 48083

# 容器内不跑 systemd，service --foreground 模式专为容器/前台运行设计。
# log-dir 固定指向卷挂载路径，便于宿主侧查看。
CMD ["/opt/p2premote/resources/p2premote-service", "--foreground", "--log-dir", "/opt/p2premote/logs"]
