# wireguard-tools source archive

This directory vendors the upstream `wireguard-tools` source archive used by
the Linux headless build so that release builds do not depend on a network
download.

- Version: `1.0.20260223`
- Upstream: `https://git.zx2c4.com/wireguard-tools/`
- Archive: `wireguard-tools-1.0.20260223.tar.gz`
- SHA-256: `859f8af03702db5e5c43f8ece77f5ebef40a2f4627c3e03997a031d4940ea9bc`

When updating the archive, update `VERSION` and `SHA256` in
`scripts/build-wireguard-tools.sh` in the same commit.
