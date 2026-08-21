Name:           p2premote-headless
Version:        %{pkg_version}
Release:        %{pkg_release}
Summary:        p2pRemote headless remote access service
License:        Proprietary
BuildArch:      %{pkg_arch}
Requires:       systemd

%description
p2pRemote service, command-line tools, WireGuard userspace components,
and browser-based management interface.

%install
mkdir -p %{buildroot}
cp -a %{payload_root}/. %{buildroot}/

%files
/opt/p2premote/resources
/usr/lib/systemd/system/p2premote-service.service

%pre
if command -v systemctl >/dev/null 2>&1; then
  systemctl stop p2premote-service.service >/dev/null 2>&1 || true
fi

%post
/opt/p2premote/resources/configure-installation
if [ -f /etc/systemd/system/p2premote-service.service ] && \
   cmp -s /etc/systemd/system/p2premote-service.service \
     /usr/lib/systemd/system/p2premote-service.service; then
  rm -f /etc/systemd/system/p2premote-service.service
fi
systemctl daemon-reload
systemctl enable p2premote-service.service >/dev/null 2>&1 || true
systemctl restart p2premote-service.service

%preun
if [ "$1" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  systemctl stop p2premote-service.service >/dev/null 2>&1 || true
  systemctl disable p2premote-service.service >/dev/null 2>&1 || true
fi

%postun
if [ "$1" -eq 0 ]; then
  rm -f /etc/systemd/system/p2premote-service.service
  rm -rf /opt/p2premote
fi
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 || true
  systemctl reset-failed p2premote-service.service >/dev/null 2>&1 || true
fi
