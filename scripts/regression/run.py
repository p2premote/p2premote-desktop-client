"""Two-Windows regression runner. Standard library only; transport is injectable for unit tests."""
from __future__ import annotations

import argparse
import base64
import hashlib
import gzip
import ipaddress
import html
import json
import math
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

SECRET_KEYS = {"password", "temporary_password", "invite_temporary_password", "access_token",
               "refresh_token", "token", "secret", "access_grant", "punch_token", "private_key",
               "preshared_key", "security_code", "web_admin_security_code", "connect_code"}
DEFAULT_CASES = ("a_to_b", "b_to_a", "reconnect", "passive_disconnect",
                 "cancel_disconnect", "service_restart", "peer_service_stop")


class RegressionError(RuntimeError):
    pass


def sanitize(value, secrets=()):
    if isinstance(value, dict):
        return {k: sanitize(v, secrets) for k, v in value.items()
                if k.lower() not in SECRET_KEYS}
    if isinstance(value, list):
        return [sanitize(v, secrets) for v in value]
    if isinstance(value, str):
        for secret in secrets:
            if secret:
                value = value.replace(secret, "<redacted>")
    return value


def command(kind, **fields):
    return {"t": kind, **({"c": fields} if fields else {})}


def validate_config(config):
    def contains_inline_secret(value):
        if isinstance(value, dict):
            return any(k.lower() in SECRET_KEYS or contains_inline_secret(v) for k, v in value.items())
        return isinstance(value, list) and any(contains_inline_secret(v) for v in value)
    if contains_inline_secret(config):
        raise RegressionError("inline secrets are forbidden")
    if config.get("schema_version") != 1:
        raise RegressionError("schema_version must be 1")
    peers = config.get("peers", {})
    if set(peers) != {"a", "b"}:
        raise RegressionError("exactly two peers named a and b are required")
    if peers["a"].get("host", "").lower() == peers["b"].get("host", "").lower():
        raise RegressionError("peer hosts must be different")
    for peer in peers.values():
        if peer.get("dedicated_test_machine") is not True:
            raise RegressionError("dedicated_test_machine must explicitly be true")
        for key in ("host", "ssh_user"):
            if not re.fullmatch(r"[A-Za-z0-9_.:-]+", peer.get(key, "")) or peer[key].startswith("-"):
                raise RegressionError("invalid SSH endpoint")
        if not 1 <= int(peer.get("ssh_port", 22)) <= 65535:
            raise RegressionError("invalid SSH port")
        if not PureWindowsPath(peer.get("cli_path", "")).is_absolute():
            raise RegressionError("cli_path must be an absolute Windows path")
        if not peer.get("expected_service_version"):
            raise RegressionError("expected_service_version must pin the tested service build")
        if peer.get("temporary_gateway"):
            try:
                gateway = ipaddress.IPv4Address(peer["temporary_gateway"])
                address = ipaddress.IPv4Address(peer["host"])
            except ipaddress.AddressValueError:
                raise RegressionError("temporary_gateway requires explicit IPv4 host and gateway") from None
            if not gateway.is_private or gateway.is_loopback or gateway.is_multicast or gateway == address:
                raise RegressionError("invalid temporary gateway")
        for key in ("identifier_env", "password_env"):
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", peer.get(key, "")):
                raise RegressionError("credentials must reference environment variable names")
    if any(key.lower() in SECRET_KEYS for peer in peers.values() for key in peer):
        raise RegressionError("inline secrets are forbidden")
    timeout = config.get("timeout_seconds", 150)
    interval = config.get("poll_seconds", 1)
    if not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or not 1 <= timeout <= 600:
        raise RegressionError("timeout_seconds must be between 1 and 600")
    if not isinstance(interval, (int, float)) or not math.isfinite(interval) or not 0 < interval <= timeout:
        raise RegressionError("invalid poll_seconds")
    minimum = config.get("minimum_probe_bytes", 65536)
    if type(minimum) is not int or not 1024 <= minimum <= 1048576:
        raise RegressionError("minimum_probe_bytes must be between 1024 and 1048576")
    return config


def encode_agent(source):
    # Windows OpenSSH commonly launches cmd.exe, whose command line is limited to 8191 chars.
    compressed = base64.b64encode(gzip.compress(source.encode("utf-8"), mtime=0)).decode("ascii")
    bootstrap = ("$g=[IO.Compression.GZipStream]::new([IO.MemoryStream]::new([Convert]::FromBase64String('"
                 + compressed + "')),[IO.Compression.CompressionMode]::Decompress);"
                 "&([scriptblock]::Create([IO.StreamReader]::new($g,[Text.Encoding]::UTF8).ReadToEnd()))")
    encoded = base64.b64encode(bootstrap.encode("utf-16-le")).decode("ascii")
    if len(encoded) > 7500:
        raise RegressionError("compressed Windows agent exceeds the remote shell command limit")
    return encoded


class SshTransport:
    def __init__(self, peers, executable="ssh"):
        self.peers = peers
        self.executable = executable
        source = Path(__file__).with_name("windows_agent.ps1").read_text(encoding="utf-8")
        self.encoded_agent = encode_agent(source)

    def deploy(self, name, environ):
        peer = self.peers[name]
        installer = Path(environ.get(peer.get("installer_env", ""), ""))
        expected = environ.get(peer.get("installer_sha256_env", ""), "").lower()
        if not installer.is_file() or installer.suffix.lower() != ".exe" or not re.fullmatch("[0-9a-f]{64}", expected):
            raise RegressionError("installer path and SHA-256 environment variables are required for deployment")
        with installer.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if digest != expected:
            raise RegressionError("local installer hash mismatch")
        stage = self.call(name, "stage", sha256=digest)
        remote_path = "C:/ProgramData/P2PRemoteRegression/staging/" + digest + ".exe"
        if stage.get("ok") is not True or stage.get("path") != remote_path:
            raise RegressionError("invalid remote staging location")
        host = peer["host"] if ":" not in peer["host"] else "[" + peer["host"] + "]"
        argv = ["scp", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                "-o", "ConnectTimeout=10", "-P", str(peer.get("ssh_port", 22)),
                str(installer.resolve()), peer["ssh_user"] + "@" + host + ":" + remote_path]
        try:
            result = subprocess.run(argv, capture_output=True, timeout=180)
        except (OSError, subprocess.TimeoutExpired):
            raise RegressionError("installer upload failed") from None
        if result.returncode:
            raise RegressionError("installer upload failed")
        result = self.call(name, "install", sha256=digest, timeout_secs=180)
        if result.get("ok") is not True or result.get("installer_sha256") != digest:
            raise RegressionError("remote installer failed")
        return {"ok": True, "installer_sha256": digest}

    def call(self, name, operation, **fields):
        peer = self.peers[name]
        payload = {"operation": operation, "cli_path": peer["cli_path"], **fields}
        remote = "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand " + self.encoded_agent
        argv = [self.executable, "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                "-o", "ConnectTimeout=10", "-p", str(peer.get("ssh_port", 22)),
                "-l", peer["ssh_user"], peer["host"], remote]
        try:
            completed = subprocess.run(argv, input=json.dumps(payload), text=True,
                                       encoding="utf-8", errors="replace", capture_output=True,
                                       timeout=fields.get("timeout_secs", 90) + 20)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise RegressionError("SSH execution failed or timed out") from None
        try:
            response = json.loads(completed.stdout.lstrip("\ufeff"))
        except (ValueError, TypeError):
            raise RegressionError("remote agent returned invalid JSON") from None
        if not isinstance(response, dict) or type(response.get("ok")) is not bool:
            raise RegressionError("remote agent returned an invalid response envelope")
        if completed.returncode and response.get("ok"):
            raise RegressionError("SSH process failed despite a successful response")
        return response


def session_for(status, peer_id):
    matches = [s for s in status.get("wgvpn_sessions", []) if s.get("peer_device_id") == peer_id]
    if len(matches) > 1:
        raise RegressionError("duplicate peer sessions")
    return matches[0] if matches else None


def is_clean(status, peer_id):
    if session_for(status, peer_id) is not None:
        return False
    for key in ("active_tunnel_jobs", "wgvpn_jobs"):
        if any(j.get("peer_device_id") == peer_id and j.get("state") in ("running", "waiting")
               for j in status.get(key, [])):
            return False
    return not any(row.get("peer_device_id") == peer_id and row.get("state") in
                   ("connected", "connecting", "recovering", "awaiting_approval")
                   for row in status.get("tunnel_lifecycles", []))


class Suite:
    def __init__(self, config, transport, environ=None, clock=time.monotonic, sleep=time.sleep, deploy=False, progress=None):
        self.config = validate_config(config)
        self.transport = transport
        self.deploy = deploy
        self.progress = progress or (lambda phase: None)
        self.environ = os.environ if environ is None else environ
        self.clock, self.sleep = clock, sleep
        self.timeout = config.get("timeout_seconds", 150)
        self.interval = config.get("poll_seconds", 1)
        self.identities = {}
        self.network_leases = []
        self.events = []
        self.secrets = []
        for peer in config["peers"].values():
            password = self.environ.get(peer["password_env"], "")
            identifier = self.environ.get(peer["identifier_env"], "")
            if not password or not identifier:
                raise RegressionError("test credential environment variables are missing")
            self.secrets.extend((password, identifier))

    def call(self, name, operation, **fields):
        response = self.transport.call(name, operation, **fields)
        self.events.append(sanitize({"peer": name, "operation": operation,
            "command": fields.get("command", {}).get("t"), "response": response}, self.secrets))
        return response

    def request(self, name, kind, allow_failure=False, **fields):
        seconds = 90 if kind == "TestTunnelSpeed" else 30
        response = self.call(name, "request", command=command(kind, **fields), timeout_secs=seconds)
        if response.get("ok") is not True and not allow_failure:
            raise RegressionError(f"{name}: {kind} failed: {response.get('error_code', 'missing_code')}")
        return response

    def status(self, name):
        response = self.request(name, "RefreshTunnelStatus")
        status = response.get("status")
        required = ("logged_in", "ws_connected", "device_id", "device_uuid",
                    "wgvpn_sessions", "active_tunnel_jobs", "wgvpn_jobs", "tunnel_lifecycles")
        if not isinstance(status, dict) or any(k not in status for k in required):
            raise RegressionError("service status schema is incomplete; no empty-state fallback")
        return status

    def wait(self, description, predicate):
        deadline = self.clock() + self.timeout
        while True:
            result = predicate()
            if result:
                return result
            if self.clock() >= deadline:
                raise RegressionError("timed out waiting for " + description)
            self.sleep(min(self.interval, max(0, deadline - self.clock())))

    def service(self, name, action):
        response = self.call(name, "service", action=action)
        if response.get("ok") is not True:
            raise RegressionError(f"{name}: service {action} failed")

    def login(self, name):
        peer = self.config["peers"][name]
        self.wait(name + " IPC ready", lambda: self.request(name, "Status", allow_failure=True).get("ok") is True)
        self.request(name, "Login", identifier=self.environ[peer["identifier_env"]],
                     password=self.environ[peer["password_env"]])
        self.request(name, "RegisterDevice")
        def ready():
            state = self.status(name)
            return state if state["logged_in"] and state["ws_connected"] and state["device_id"] and state["device_uuid"] else None
        state = self.wait(name + " online", ready)
        version = (state.get("current_device") or {}).get("client_version")
        if version != peer["expected_service_version"]:
            raise RegressionError(f"{name}: service version does not match pinned build")
        self.identities[name] = {"device_id": state["device_id"], "device_uuid": state["device_uuid"]}
        return state

    def prepare(self):
        for name in ("a", "b"):
            peer = self.config["peers"][name]
            if peer.get("temporary_gateway"):
                self.progress("network:" + name)
                prepared = self.call(name, "network", action="prepare", interface_ip=peer["host"], gateway=peer["temporary_gateway"])
                if prepared.get("ok") is not True:
                    raise RegressionError("temporary network preparation failed")
                if prepared.get("created") is True:
                    self.network_leases.append(name)
            if self.deploy:
                self.progress("deploy:" + name)
                deployed = self.transport.deploy(name, self.environ)
                self.events.append({"peer": name, "operation": "deploy", "response": deployed})
            self.service(name, "start")
            self.progress("login:" + name)
            inventory = self.call(name, "inventory")
            if not inventory.get("ok") or not re.fullmatch("[0-9a-f]{64}", inventory.get("cli_sha256", "")):
                raise RegressionError("missing CLI build fingerprint")
            self.login(name)
        if self.identities["a"]["device_id"] == self.identities["b"]["device_id"]:
            raise RegressionError("the VMs registered the same device identity")
        for source, target in (("a", "b"), ("b", "a")):
            self.request(source, "SetAutoKeep", target_device_id=self.identities[target]["device_id"], enabled=False)
        self.cleanup()

    def start(self, source, target):
        peer = self.identities[target]
        self.request(source, "StartActiveTunnelJob", peer_device_id=peer["device_id"],
                     peer_device_uuid=peer["device_uuid"], lan_cidrs=[])

    def connected(self, source, target):
        left, right = self.status(source), self.status(target)
        for job in left["active_tunnel_jobs"]:
            if job.get("peer_device_id") == self.identities[target]["device_id"] and job.get("state") in ("failed", "cancelled"):
                raise RegressionError("connection terminated: " + job.get("error_code", "missing_code"))
        for status, name in ((left, target), (right, source)):
            session = session_for(status, self.identities[name]["device_id"])
            if session is None or session.get("approval_pending", False):
                return None
            if session.get("health_state") != "connected":
                return None
        a = session_for(left, self.identities[target]["device_id"])
        b = session_for(right, self.identities[source]["device_id"])
        if a.get("latency_ms") is None:
            return None  # Connected is also the initial enum value; require an actual health reply.
        if not a.get("is_active") or b.get("is_active"):
            raise RegressionError("session roles are inconsistent")
        if not a.get("virtual_ip") or a["virtual_ip"] != b.get("peer_virtual_ip") or a.get("peer_virtual_ip") != b.get("virtual_ip"):
            raise RegressionError("tunnel address pairing is inconsistent")
        return left, right

    def verify_data(self, source, target):
        before = self.connected(source, target)
        if before is None:
            raise RegressionError("cannot probe a disconnected pair")
        result = self.request(source, "TestTunnelSpeed", peer_device_id=self.identities[target]["device_id"]).get("data")
        if not isinstance(result, dict):
            raise RegressionError("missing real tunnel speed result")
        for key in ("upload_mbps", "download_mbps"):
            value = result.get(key)
            if type(value) not in (int, float) or not math.isfinite(value) or value <= 0:
                raise RegressionError("bidirectional tunnel data did not flow")
        def counters_advanced():
            after = self.connected(source, target)
            if after is None:
                return False
            for old, new, peer in ((before[0], after[0], target), (before[1], after[1], source)):
                old_session = session_for(old, self.identities[peer]["device_id"])
                new_session = session_for(new, self.identities[peer]["device_id"])
                for key in ("received_bytes", "transmitted_bytes"):
                    if key not in old_session or key not in new_session:
                        raise RegressionError("missing traffic counters")
                    if new_session[key] - old_session[key] < self.config.get("minimum_probe_bytes", 65536):
                        return False
            return True
        self.wait("traffic counters on both endpoints", counters_advanced)

    def connect(self, source="a", target="b"):
        self.start(source, target)
        self.wait("both tunnel sessions", lambda: self.connected(source, target))
        self.verify_data(source, target)

    def clean_pair(self):
        return all(is_clean(self.status(name), self.identities[other]["device_id"])
                   for name, other in (("a", "b"), ("b", "a")))

    def cleanup(self):
        for name, other in (("a", "b"), ("b", "a")):
            if name not in self.identities or other not in self.identities:
                continue
            peer_id = self.identities[other]["device_id"]
            self.request(name, "StopActiveTunnelJob", allow_failure=True, peer_device_id=peer_id)
            self.request(name, "StopActiveTunnel", allow_failure=True, peer_device_id=peer_id)
            self.request(name, "StopTunnel", allow_failure=True, source_device_id=peer_id)
        if len(self.identities) == 2:
            self.wait("complete session and job cleanup", self.clean_pair)

    def scenario(self, name):
        if name == "a_to_b":
            self.connect("a", "b")
        elif name == "b_to_a":
            self.connect("b", "a")
        elif name == "reconnect":
            self.connect()
            self.cleanup()
            self.connect()
        elif name == "passive_disconnect":
            self.connect()
            self.request("b", "StopTunnel", source_device_id=self.identities["a"]["device_id"])
            self.wait("passive disconnect propagation", self.clean_pair)
        elif name == "cancel_disconnect":
            self.start("a", "b")
            self.request("a", "DisconnectActiveTunnel", peer_device_id=self.identities["b"]["device_id"], auto_keep_enabled=False)
            self.wait("cancel/disconnect termination", self.clean_pair)
        elif name == "service_restart":
            self.connect()
            old = self.status("b").get("service_session_id")
            if not old:
                raise RegressionError("missing service session identity")
            self.service("b", "restart")
            state = self.login("b")
            if not state.get("service_session_id") or state["service_session_id"] == old:
                raise RegressionError("service did not restart")
            self.cleanup()
            self.connect()
        elif name == "peer_service_stop":
            self.connect()
            try:
                self.service("b", "stop")
                response = self.request("b", "Status", allow_failure=True)
                if response.get("ok") is True:
                    raise RegressionError("stopped service still answers IPC")
                def loss_detected():
                    state = self.status("a")
                    session = session_for(state, self.identities["b"]["device_id"])
                    return session is None or session.get("health_state") == "degraded"
                self.wait("peer loss detection", loss_detected)
            finally:
                self.service("b", "start")
                self.login("b")
        else:
            raise RegressionError("unknown scenario: " + name)

    def run(self, cases=DEFAULT_CASES, repeat=1):
        report = {"ok":False, "results":[], "events":[]}
        try:
            report = self._run(cases, repeat)
        finally:
            self._restore_network(report)
        report["events"] = self.events
        report["planned"] = len(cases) * repeat
        completed = {row["name"] for row in report["results"]}
        report["not_run"] = [f"{case}[{iteration + 1}]" for iteration in range(repeat)
                             for case in cases if f"{case}[{iteration + 1}]" not in completed]
        return sanitize(report, self.secrets)

    def _restore_network(self, report):
        for name in reversed(self.network_leases):
            peer = self.config["peers"][name]
            try:
                restored = self.call(name, "network", action="restore", interface_ip=peer["host"], gateway=peer["temporary_gateway"])
                if restored.get("ok") is not True:
                    raise RegressionError("route restoration rejected")
            except Exception:
                report["ok"] = False
                report["results"].append({"name":"network_cleanup:" + name, "ok":False,
                                          "error":"temporary route restoration failed", "seconds":0})
        self.network_leases.clear()

    def _run(self, cases=DEFAULT_CASES, repeat=1):
        results = []
        try:
            self.prepare()
        except Exception as error:
            results.append({"name": "preflight", "ok": False, "error": str(error), "seconds": 0})
            # Recover partially prepared VMs without treating a failed preflight as a successful run.
            if len(self.identities) == 2:
                try:
                    self.cleanup()
                except Exception:
                    pass
            return sanitize({"ok": False, "results": results, "events": self.events}, self.secrets)
        for iteration in range(repeat):
            for case in cases:
                self.progress("scenario:" + case)
                started = self.clock()
                errors = []
                try:
                    self.scenario(case)
                except Exception as error:
                    errors.append(str(error))
                finally:
                    try:
                        self.cleanup()
                    except Exception as error:
                        errors.append("cleanup: " + str(error))
                results.append({"name": f"{case}[{iteration + 1}]", "ok": not errors,
                                "error": "; ".join(errors), "seconds": self.clock() - started})
                if errors:
                    return sanitize({"ok": False, "results": results, "events": self.events}, self.secrets)
        return sanitize({"ok": True, "results": results, "events": self.events}, self.secrets)


def write_reports(report, output):
    output.mkdir(parents=True, exist_ok=True)
    (output / "results.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    not_run = report.get("not_run", [])
    suite = ET.Element("testsuite", name="windows-regression", tests=str(len(report["results"]) + len(not_run)),
                      failures=str(sum(not row["ok"] for row in report["results"])), skipped=str(len(not_run)))
    for row in report["results"]:
        case = ET.SubElement(suite, "testcase", name=row["name"], time=str(row["seconds"]))
        if not row["ok"]:
            ET.SubElement(case, "failure", message=row["error"]).text = row["error"]
    for name in not_run:
        case = ET.SubElement(suite, "testcase", name=name, time="0")
        ET.SubElement(case, "skipped", message="A previous prerequisite failed")
    ET.ElementTree(suite).write(output / "junit.xml", encoding="utf-8", xml_declaration=True)
    def cell(value):
        return html.escape(str(value)).replace("|", "\\|").replace("\n", " ")
    codes = set()
    for event in report.get("events", []):
        response = event.get("response", {})
        status = response.get("status") or {}
        for key in ("active_tunnel_jobs", "wgvpn_jobs", "tunnel_lifecycles"):
            codes.update(row["error_code"] for row in status.get(key, []) if row.get("error_code"))
    lines = ["# Windows regression", "", "Result: " + ("PASS" if report["ok"] else "FAIL"), "",
             "Planned scenarios: " + str(report.get("planned", "unknown")), "",
             "| Result item | Result | Seconds | Detail |", "| --- | --- | ---: | --- |"]
    for row in report["results"]:
        lines.append("| " + " | ".join(map(cell, (row["name"], "PASS" if row["ok"] else "FAIL",
                     round(row["seconds"], 2), row["error"]))) + " |")
    if codes:
        lines += ["", "Observed business error codes: " + ", ".join(map(cell, sorted(codes)))]
    if not_run:
        lines += ["", "Not run: " + ", ".join(map(cell, not_run))]
    lines += ["", "A failed prerequisite stops later dependent scenarios; unexecuted scenarios are not passes.", ""]
    (output / "summary.md").write_text("\n".join(lines), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cases", default=",".join(DEFAULT_CASES))
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--deploy", action="store_true", help="verify, upload and silently install pinned NSIS packages")
    parser.add_argument("--validate-only", action="store_true", help="validate configuration and credential references without connecting")
    args = parser.parse_args()
    if args.repeat < 1 or args.repeat > 100:
        parser.error("--repeat must be between 1 and 100")
    cases = tuple(args.cases.split(","))
    if not cases or any(case not in DEFAULT_CASES for case in cases):
        parser.error("unsupported regression cases")
    if not args.validate_only:
        # A killed/aborted run must not leave the previous run's green report behind.
        write_reports({"ok":False, "results":[{"name":"run_in_progress", "ok":False,
                       "error":"run did not complete", "seconds":0}], "events":[]}, args.output)
    try:
        config = validate_config(json.loads(args.config.read_text(encoding="utf-8-sig")))
        suite = Suite(config, SshTransport(config["peers"]), deploy=args.deploy,
                      progress=lambda phase: print(json.dumps({"phase":phase}), file=sys.stderr, flush=True))
        if args.validate_only:
            print('{"ok":true,"validated":true}')
            return 0
        report = suite.run(cases, args.repeat)
    except RegressionError as error:
        report = {"ok": False, "results": [{"name":"configuration", "ok":False,
                   "error":str(error), "seconds":0}], "events":[]}
    except Exception:
        # No exception dump: malformed configuration can itself contain secrets.
        report = {"ok": False, "results": [{"name": "configuration", "ok": False,
                   "error": "configuration or credential validation failed", "seconds": 0}], "events": []}
    write_reports(report, args.output)
    print(json.dumps({"ok": report["ok"], "results": len(report["results"])}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
