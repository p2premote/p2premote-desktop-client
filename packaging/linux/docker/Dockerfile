# p2premote Linux 客户端 Docker 镜像
#
# 复用 build-linux-headless.sh 产出的 musl 静态 tgz 产物，避免在 Docker
# builder 里重新装配 Rust + Node + musl 工具链。仅支持 host 网络模式：
# bridge 模式下 Web Admin 看到的来源 IP 会塌缩为 docker0 网关地址，导致
# web_admin_allowed_ip 白名单失效。详细说明见 docs/docker-client.md。
#
# 构建上下文需放置 tgz 产物，默认文件名 p2premote-headless.tar.gz，
# 可通过 --build-arg P2P_HEADLESS_TGZ=<文件名> 覆盖。建议使用
# scripts/build-docker.sh 自动完成 tgz 构建与上下文准备。

FROM ubuntu:22.04

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
    # 可选脚本（裸机部署用）缺失不阻塞镜像构建
    for f in /opt/p2premote/install-service.sh /opt/p2premote/uninstall-service.sh; do \
        [ -f "$f" ] && chmod 755 "$f" || true; \
    done; \
    mkdir -p /opt/p2premote/data /opt/p2premote/logs /opt/p2premote/run

# 固定安装根目录，保证 core 的资源、配置、日志、run 目录解析稳定。
# 参考 core/src/config.rs 的 linux_install_root_dir。
ENV P2PREMOTE_INSTALL_ROOT=/opt/p2premote

# Web Admin 默认监听端口。host 网络模式下宿主机直接暴露该端口；
# 外部访问该端口的源 IP 即真实客户端 IP，是 web_admin_allowed_ip 生效的前提。
EXPOSE 48083

# 容器内不跑 systemd，service --foreground 模式专为容器/前台运行设计。
# log-dir 固定指向卷挂载路径，便于宿主侧查看。
CMD ["/opt/p2premote/resources/p2premote-service", "--foreground", "--log-dir", "/opt/p2premote/logs"]
