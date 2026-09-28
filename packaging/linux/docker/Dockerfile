# p2premote Linux 客户端 Docker 镜像（多阶段构建 + distroless 运行时）
#
# 阶段 1（alpine）负责解包 headless tgz 并装配静态 busybox 命令集（含 ip
# 与 sh）；阶段 2（distroless cc-debian13）只保留运行时必需文件，基底不含
# shell 与包管理器，常用命令由静态 busybox 提供，体积较 ubuntu:22.04
# 单阶段方案（~140MB）减少约六成。
#
# 复用 _build-linux-headless.sh 产出的 GNU/glibc tgz 产物，避免在 Docker
# builder 里重新装配 Rust + Node + musl 工具链。仅支持 host 网络模式：
# bridge 模式下 Web Admin 看到的来源 IP 会塌缩为 docker0 网关地址，导致
# web_admin_allowed_ip 白名单依赖容器看到的真实来源地址。
#
# 构建上下文需放置 tgz 产物，默认文件名 p2premote-headless.tar.gz，
# 可通过 --build-arg P2P_HEADLESS_TGZ=<文件名> 覆盖。建议使用
# scripts/build-linux.sh 自动完成编译、四格式打包与上下文准备。

# 阶段 1：解包与装配。产物为 /opt/p2premote 目录树与静态 busybox。
FROM alpine:3.20 AS assemble

ARG http_proxy
ARG https_proxy
ARG HTTP_PROXY
ARG HTTPS_PROXY

# busybox-static 提供静态链接的多用户调用（multi-call）busybox，在镜像内
# 暴露为完整命令集：service 创建/删除 WireGuard 接口依赖 PATH 中的 `ip`，
# distroless 基底无 shell 与常用命令，一并由它满足（容器内运维调试可用
# sh/ls/cat 等）；`wg`/`wireguard-go` 均使用安装包内自带的静态二进制，
# 不依赖系统。
RUN apk add --no-cache busybox-static

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
    mkdir -p /staged-bin; \
    cp /bin/busybox.static /staged-bin/busybox; \
    # 按完整 applet 清单建相对符号链接（含 ip 与 sh）。不用 busybox --install：
    # 它依赖 /proc/self/exe，aarch64 QEMU 模拟构建下会指向 qemu 路径
    for applet in $(/staged-bin/busybox --list); do ln -s busybox "/staged-bin/${applet}"; done; \
    mkdir -p /opt/p2premote/data /opt/p2premote/logs /opt/p2premote/run

# 阶段 2：distroless 运行时。基底无 shell、无包管理器，shell 与常用命令
# 由阶段 1 的静态 busybox 命令集提供；CMD 必须使用 exec 形式。
# tgz 内的 service/cli/wireguard-go/wg 均为静态链接二进制，不依赖基础镜像
# 的 glibc；TLS 根证书、时区数据与 /etc/nsswitch.conf 由 distroless 提供。
FROM gcr.io/distroless/cc-debian13

COPY --from=assemble /opt/p2premote /opt/p2premote
COPY --from=assemble /staged-bin/ /usr/local/bin/

ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ENV TZ=Asia/Shanghai

WORKDIR /opt/p2premote

# 固定安装根目录，保证 core 的资源、配置、日志、run 目录解析稳定。
# 参考 core/src/config.rs 的 linux_install_root_dir。
ENV P2PREMOTE_INSTALL_ROOT=/opt/p2premote

# Docker 与直接安装的 Headless 包使用同一二进制，通过运行时目标区分版本策略。
ARG P2PREMOTE_UPDATE_TARGET=linux-docker-x64
ENV P2PREMOTE_UPDATE_TARGET=${P2PREMOTE_UPDATE_TARGET}

# 首次信任（TOFU）引导：无 GUI 的 NAS 场景没有本机浏览器可用，全新部署
# （未配置安全码且未配置白名单）时 Web Admin 直接监听 0.0.0.0，首个远程
# 客户端获得引导访问权，并在页面中明确填写安全码与允许访问的 IP；提交后
# 才保存这两个设置。容器界面创建后无需查看 stdout——引导直接在浏览器
# 里完成。开关仅在本镜像注入，裸机安装保持 loopback-only 默认。
ENV P2PREMOTE_WEB_ADMIN_FIRST_TRUST=1

# Web Admin 默认监听端口。host 网络模式下宿主机直接暴露该端口；
# 外部访问该端口的源 IP 即真实客户端 IP，是 web_admin_allowed_ip 生效的前提。
EXPOSE 48083

# 容器内不跑 systemd，service --foreground 模式专为容器/前台运行设计。
# log-dir 固定指向卷挂载路径，便于宿主侧查看。
CMD ["/opt/p2premote/resources/p2premote-service", "--foreground", "--log-dir", "/opt/p2premote/logs"]
