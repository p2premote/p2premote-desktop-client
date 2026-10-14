# Service access policy

Settings > Security > Allowed services configures this device's ingress policy.
The desktop GUI and authenticated Web UI use the same service IPC commands:
`GetServiceAccessPolicy` and `SaveServiceAccessPolicy`.

The service persists `service_access` in machine configuration. Missing policy
fields at the machine-config level select the legacy unrestricted default.
An explicitly present malformed policy fails configuration loading; service
startup must not silently substitute an unrestricted policy.

```json
{
  "service_access": {
    "mode": "allowlist",
    "tcp": [{ "start": 3389, "end": 3389 }],
    "udp": [{ "start": 1000, "end": 2000 }],
    "allow_ping": false
  }
}
```

Modes are `all`, `allowlist`, and `deny`. An empty whitelist denies application
traffic. Port ranges are inclusive, within 1–65535, validated by the service,
sorted, and merged. Each protocol accepts at most 256 input ranges. Switching
modes preserves the configured whitelist for later use.

The policy applies to both tunnel roles and all authenticated peers. Userspace
enforcement covers delivery to the local virtual IP, before native TUN writes.
LAN proxy destinations are governed separately by LAN exposure settings. In
particular, exposing a LAN subnet can expose services via the device's LAN IP;
the local-virtual-IP whitelist does not restrict that separately authorized path.

TCP port 41119 remains reserved for the service's peer-validated tunnel health
and disconnect controls. The service refuses startup if it cannot own that
listener. The optional legacy listener at 48082 is not automatically exempted.
Application ports, including remote desktop, management, file transfers and
speed tests, receive no automatic exemptions. Ping controls ICMP echo requests,
not WireGuard keepalives or the internal TCP health channel.

## Enforcement

- Windows/macOS: `p2premote-wg-ffi` checks decrypted packets before native TUN
  delivery. Outbound-origin TCP/UDP/echo tuples allow their corresponding replies.
  Inbound-origin tuples cannot become outbound exemptions from server replies.
  Every inbound packet is checked against the current policy, so removing a port
  also blocks subsequent traffic on existing connections. Related ICMP errors
  for locally initiated TCP/UDP flows remain available for PMTU/error handling.
  TCP flow records expire after 24 hours; UDP flow records retain the two-minute
  expiry. Matching replies renew the corresponding flow record.
- Linux: dedicated `P2PREMOTE_SERVICES` filter chain, hooked at the start of
  `INPUT` for `wg0`. `iptables-restore`/`ip6tables-restore --noflush` update only
  this chain. Conntrack exemptions require `--ctdir REPLY`, preventing existing
  inbound connections from retaining removed access. Existing system firewall
  rules still apply to permitted traffic. `FORWARD` and NAT remain unchanged.
  Each update removes all previous INPUT hooks and recreates one at the head,
  including when another firewall operation has displaced the old hook.

The userspace filter parses IPv4 and bounded IPv6 extension chains. Restricted
userspace traffic rejects IP fragments and malformed transport headers; IPv6
parser support does not add IPv6 addressing to the existing WG engine. Linux
uses the kernel's normal packet parsing/reassembly and installs IPv4 and IPv6
rules. Restrictive Linux policies require firewall tools and privileges for both
families; failures are surfaced instead of marking the policy applied.

Policy saves write configuration only, without rewriting `state.json`.
Policy updates apply before persistence and roll back on failure. If rollback
also fails, the service attempts an emergency deny policy, tears down tunnels
if that cannot be installed, and prevents new tunnel jobs until a policy has
been applied successfully. UI drafts remain available after a failed save.
Configuration reload also reapplies the policy. Socket objects are not forcibly
closed by port removal; further disallowed inbound traffic is dropped.

## Build and validation

The client and WG library require ABI 3, which adds `SetServiceAccessPolicy`.
Rebuild and ship the client/service and WG DLL/dylib together. The WG library
starts with a deny policy until service initialization supplies configuration.

Focused checks:

```bash
# p2premote-wg-ffi (Windows)
GOTOOLCHAIN=go1.20.14 go test -tags wgonly ./punchffi

# p2premote-desktop-client
cargo test -p p2premote-core service_access::tests --lib
npx playwright test --grep '服务访问规则' --reporter=line
npm run build
```

The tests cover inclusive port boundaries, protocol isolation, invalid ranges,
empty whitelist, live policy tightening, outbound replies, Ping, malformed
packets/fragments, related ICMP, peer flow cleanup, Linux rule generation,
the Windows DLL JSON ABI, and UI saves/failure handling. Linux/macOS live
peer traffic still requires validation on those systems before release. Linux
compilation and IPv4/IPv6 chain installation, mode switching and preservation
of unrelated rules were also verified inside an isolated WSL network namespace.
The ignored `linux_backend_installs_and_updates_only_its_own_chain` test refuses
to run in the host network namespace and must be launched with `unshare --net`.
