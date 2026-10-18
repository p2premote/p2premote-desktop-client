import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

import run as regression


def configuration():
    return {"schema_version": 1, "timeout_seconds": 3, "poll_seconds": 1,
            "peers": {name: {"host": "vm-" + name, "ssh_user": "tester",
                "cli_path": "C:/Regression/p2premote-cli.exe", "dedicated_test_machine": True,
                "expected_service_version": "1.14.0-test",
                "identifier_env": "TEST_USER", "password_env": "TEST_PASSWORD"} for name in ("a", "b")}}


ENV = {"TEST_USER": "automation-account", "TEST_PASSWORD": "secret-value-123"}


class Clock:
    def __init__(self):
        self.value = 0
    def now(self):
        return self.value
    def sleep(self, seconds):
        self.value += seconds


class FakeTransport:
    def __init__(self):
        self.online = {"a": True, "b": True}
        self.states = {}
        self.commands = []
        self.bad_speed = False
        self.freeze_counters = False
        self.refuse_cleanup = False
        self.network_actions = []
        self.network_created = True
        self.network_restore_ok = True
        for index, name in enumerate(("a", "b"), 1):
            self.states[name] = {"logged_in": True, "ws_connected": True, "device_id": index,
                "device_uuid": "uuid-" + name, "service_session_id": "session-" + name,
                "current_device": {"client_version": "1.14.0-test"},
                "wgvpn_sessions": [], "active_tunnel_jobs": [], "wgvpn_jobs": [], "tunnel_lifecycles": []}

    def clear(self):
        if not self.refuse_cleanup:
            for state in self.states.values():
                for key in ("wgvpn_sessions", "active_tunnel_jobs", "wgvpn_jobs", "tunnel_lifecycles"):
                    state[key] = []

    def connect(self, source):
        target = "b" if source == "a" else "a"
        for name, peer in ((source, target), (target, source)):
            self.states[name]["wgvpn_sessions"] = [{"peer_device_id": self.states[peer]["device_id"],
                "is_active": name == source, "approval_pending": False, "health_state": "connected",
                "latency_ms": 1,
                "virtual_ip": "10.90.0." + str(self.states[name]["device_id"]),
                "peer_virtual_ip": "10.90.0." + str(self.states[peer]["device_id"]),
                "received_bytes": 100, "transmitted_bytes": 100}]

    def call(self, name, operation, **fields):
        if operation == "network":
            self.network_actions.append(fields["action"])
            return {"ok": self.network_restore_ok if fields["action"] == "restore" else True,
                    "created": self.network_created}
        if operation == "inventory":
            return {"ok": True, "cli_sha256": "a" * 64}
        if operation == "service":
            action = fields["action"]
            self.online[name] = action != "stop"
            if action in ("restart", "stop"):
                self.clear()
            if action == "restart":
                self.states[name]["service_session_id"] += "-new"
            return {"ok": True}
        cmd = fields["command"]
        self.commands.append(copy.deepcopy(cmd))
        if not self.online[name]:
            return {"ok": False, "error_code": "automation.request.failed"}
        kind = cmd["t"]
        if kind == "StartActiveTunnelJob":
            self.connect(name)
        elif kind in ("StopActiveTunnelJob", "StopActiveTunnel", "StopTunnel", "DisconnectActiveTunnel"):
            self.clear()
        elif kind == "TestTunnelSpeed":
            if not self.freeze_counters:
                for state in self.states.values():
                    for session in state["wgvpn_sessions"]:
                        session["received_bytes"] += 131072
                        session["transmitted_bytes"] += 131072
            return {"ok": True, "data": {"upload_mbps": 0 if self.bad_speed else 10,
                                         "download_mbps": 20}}
        return {"ok": True, "status": copy.deepcopy(self.states[name]), "data": None}


class RegressionTests(unittest.TestCase):
    def network_suite(self):
        config = configuration()
        config["peers"]["a"].update(host="10.10.100.200", temporary_gateway="10.10.100.1")
        config["peers"]["b"]["host"] = "10.10.100.52"
        fake, clock = FakeTransport(), Clock()
        return regression.Suite(config, fake, ENV, clock.now, clock.sleep), fake

    def test_temporary_network_is_restored_after_success_and_failed_preflight(self):
        for broken in (False, True):
            suite, fake = self.network_suite()
            if broken:
                fake.states["a"]["current_device"]["client_version"] = "wrong"
            report = suite.run(("a_to_b",))
            self.assertEqual(report["ok"], not broken)
            self.assertEqual(fake.network_actions, ["prepare", "restore"])

    def test_existing_gateway_is_not_removed_and_failed_restore_fails_suite(self):
        suite, fake = self.network_suite()
        fake.network_created = False
        self.assertTrue(suite.run(("a_to_b",))["ok"])
        self.assertEqual(fake.network_actions, ["prepare"])
        suite, fake = self.network_suite()
        fake.network_restore_ok = False
        report = suite.run(("a_to_b",))
        self.assertFalse(report["ok"])
        self.assertEqual(report["results"][-1]["name"], "network_cleanup:a")

    def test_unexpected_runner_exception_still_restores_network(self):
        suite, fake = self.network_suite()
        suite.network_leases.append("a")
        with patch.object(suite, "_run", side_effect=RuntimeError("unexpected")):
            with self.assertRaises(RuntimeError):
                suite.run(("a_to_b",))
        self.assertEqual(fake.network_actions, ["restore"])
    def suite(self, fake=None):
        clock = Clock()
        fake = FakeTransport() if fake is None else fake
        return regression.Suite(configuration(), fake, ENV, clock.now, clock.sleep), fake

    def test_all_scenarios_repeat_without_gui_and_reports_have_no_credentials(self):
        suite, fake = self.suite()
        report = suite.run(repeat=2)
        self.assertTrue(report["ok"], report["results"])
        self.assertEqual(len(report["results"]), 14)
        self.assertEqual(sum(cmd["t"] == "TestTunnelSpeed" for cmd in fake.commands), 16)
        self.assertEqual(sum(cmd["t"] == "StartActiveTunnelJob" for cmd in fake.commands), 18)
        self.assertNotIn(ENV["TEST_PASSWORD"], json.dumps(report))
        self.assertNotIn(ENV["TEST_USER"], json.dumps(report))
        self.assertTrue(suite.clean_pair())

    def test_configuration_rejects_same_host_untrusted_endpoints_and_inline_secrets(self):
        for update in (
            {"host": "vm-a"}, {"host": "-oProxyCommand=bad"},
            {"ssh_user": "user; command"}, {"dedicated_test_machine": False},
            {"cli_path": "relative.exe"}, {"expected_service_version": ""},
            {"password": "forbidden"}, {"password_env": "invalid env"}, {"ssh_port": 0},
        ):
            config = configuration()
            config["peers"]["b"].update(update)
            with self.subTest(update=update), self.assertRaises(regression.RegressionError):
                regression.validate_config(config)
        for timeout in (0, 601, float("nan"), "10"):
            config = configuration()
            config["timeout_seconds"] = timeout
            with self.assertRaises(regression.RegressionError):
                regression.validate_config(config)
        with self.assertRaises(regression.RegressionError):
            regression.Suite(configuration(), FakeTransport(), {})

    def test_wrong_service_build_and_duplicate_device_identity_fail_preflight(self):
        for broken in ("version", "identity", "schema"):
            suite, fake = self.suite()
            if broken == "version":
                fake.states["a"]["current_device"]["client_version"] = "old"
            elif broken == "identity":
                fake.states["b"]["device_id"] = 1
            else:
                del fake.states["a"]["active_tunnel_jobs"]
            report = suite.run(("a_to_b",))
            self.assertFalse(report["ok"])
            self.assertEqual(report["results"][0]["name"], "preflight")

    def test_connected_flags_without_bidirectional_data_are_not_a_pass(self):
        suite, fake = self.suite()
        fake.bad_speed = True
        report = suite.run(("a_to_b",))
        self.assertFalse(report["ok"])
        self.assertIn("bidirectional", report["results"][0]["error"])
        self.assertTrue(suite.clean_pair())

    def test_failed_prerequisite_marks_later_cases_skipped_in_junit(self):
        suite, fake = self.suite()
        fake.bad_speed = True
        report = suite.run()
        self.assertFalse(report["ok"])
        self.assertEqual(len(report["not_run"]), 6)
        with tempfile.TemporaryDirectory() as path:
            regression.write_reports(report, Path(path))
            root = ET.parse(Path(path) / "junit.xml").getroot()
            self.assertEqual(root.get("tests"), "7")
            self.assertEqual(root.get("failures"), "1")
            self.assertEqual(root.get("skipped"), "6")
            self.assertEqual(len(root.findall("testcase/skipped")), 6)

    def test_frozen_byte_counters_fail_despite_positive_speed_numbers(self):
        suite, fake = self.suite()
        fake.freeze_counters = True
        report = suite.run(("a_to_b",))
        self.assertFalse(report["ok"])
        self.assertIn("traffic counters", report["results"][0]["error"])

    def test_small_health_only_counter_growth_does_not_count_as_data_transfer(self):
        suite, fake = self.suite()
        original = fake.call
        def call(name, operation, **fields):
            result = original(name, operation, **fields)
            if fields.get("command", {}).get("t") == "TestTunnelSpeed":
                for state in fake.states.values():
                    for session in state["wgvpn_sessions"]:
                        session["received_bytes"] = 101
                        session["transmitted_bytes"] = 101
            return result
        fake.call = call
        report = suite.run(("a_to_b",))
        self.assertFalse(report["ok"])
        self.assertIn("traffic counters", report["results"][0]["error"])

    def test_terminal_connect_failure_is_reported_without_waiting_for_deadline(self):
        suite, fake = self.suite()
        suite.prepare()
        fake.states["a"]["active_tunnel_jobs"] = [{"peer_device_id": 2,
            "state": "failed", "error_code": "connection.compatibility.client_upgrade_required"}]
        with self.assertRaisesRegex(regression.RegressionError, "client_upgrade_required"):
            suite.connected("a", "b")

    def test_failed_cleanup_cannot_leave_a_green_result(self):
        suite, fake = self.suite()
        original = fake.connect
        def connect(source):
            original(source)
            fake.refuse_cleanup = True
        fake.connect = connect
        report = suite.run(("a_to_b",))
        self.assertFalse(report["ok"])
        self.assertIn("cleanup", report["results"][0]["error"])

    def test_mismatched_address_pairing_and_missing_counters_are_rejected(self):
        for problem in ("address", "counter"):
            suite, fake = self.suite()
            suite.prepare()
            suite.start("a", "b")
            if problem == "address":
                fake.states["a"]["wgvpn_sessions"][0]["peer_virtual_ip"] = "10.0.0.99"
            else:
                del fake.states["b"]["wgvpn_sessions"][0]["received_bytes"]
                fake.freeze_counters = True
            with self.assertRaises(regression.RegressionError):
                suite.verify_data("a", "b")
            suite.cleanup()

    def test_pending_approval_is_not_connected_and_stale_jobs_are_not_clean(self):
        suite, fake = self.suite()
        suite.prepare()
        suite.start("a", "b")
        fake.states["b"]["wgvpn_sessions"][0]["approval_pending"] = True
        self.assertIsNone(suite.connected("a", "b"))
        suite.cleanup()
        fake.states["a"]["active_tunnel_jobs"] = [{"peer_device_id": 2, "state": "running"}]
        self.assertFalse(regression.is_clean(fake.states["a"], 2))
        fake.states["a"]["active_tunnel_jobs"] = []
        fake.states["a"]["tunnel_lifecycles"] = [{"peer_device_id": 2, "state": "recovering"}]
        self.assertFalse(regression.is_clean(fake.states["a"], 2))

    def test_initial_connected_enum_without_health_reply_does_not_pass_readiness(self):
        suite, fake = self.suite()
        suite.prepare()
        suite.start("a", "b")
        fake.states["a"]["wgvpn_sessions"][0]["latency_ms"] = None
        self.assertIsNone(suite.connected("a", "b"))
        fake.states["a"]["wgvpn_sessions"][0]["latency_ms"] = 1
        self.assertIsNotNone(suite.connected("a", "b"))

    def test_duplicate_sessions_are_rejected(self):
        with self.assertRaises(regression.RegressionError):
            regression.session_for({"wgvpn_sessions": [{"peer_device_id": 2}, {"peer_device_id": 2}]}, 2)

    def test_speed_values_must_be_finite_positive_numbers(self):
        for value in (0, -1, float("nan"), float("inf"), True, "10", None):
            suite, fake = self.suite()
            original = fake.call
            def call(name, operation, **fields):
                if fields.get("command", {}).get("t") == "TestTunnelSpeed":
                    return {"ok": True, "data": {"upload_mbps": value, "download_mbps": 1}}
                return original(name, operation, **fields)
            fake.call = call
            report = suite.run(("a_to_b",))
            self.assertFalse(report["ok"], value)

    def test_json_and_junit_outputs_preserve_failure(self):
        report = {"ok": False, "results": [{"name": "failure", "ok": False,
                  "error": "<invalid>", "seconds": 1}], "events": []}
        with tempfile.TemporaryDirectory() as path:
            regression.write_reports(report, Path(path))
            self.assertFalse(json.loads((Path(path) / "results.json").read_text())["ok"])
            root = ET.parse(Path(path) / "junit.xml").getroot()
            self.assertEqual(root.get("failures"), "1")
            self.assertEqual(root.find("testcase/failure").get("message"), "<invalid>")
            self.assertIn("FAIL", (Path(path) / "summary.md").read_text())

    def test_aborted_run_invalidates_previous_success_report(self):
        with tempfile.TemporaryDirectory() as path:
            output = Path(path)
            config_path = output / "config.json"
            config_path.write_text(json.dumps(configuration()), encoding="utf-8")
            regression.write_reports({"ok":True,"results":[],"events":[]}, output)
            with patch.object(regression.sys, "argv", ["run.py", "--config", str(config_path),
                    "--output", str(output)]), patch.dict(regression.os.environ, ENV), patch.object(
                    regression.Suite, "run", side_effect=KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    regression.main()
            marker = json.loads((output / "results.json").read_text())
            self.assertFalse(marker["ok"])
            self.assertEqual(marker["results"][0]["name"], "run_in_progress")

    def test_ssh_payload_uses_stdin_and_strict_host_verification(self):
        transport = regression.SshTransport(configuration()["peers"])
        with patch.object(regression.subprocess, "run", return_value=SimpleNamespace(
                returncode=0, stdout='\ufeff{"ok":true}')) as execute:
            self.assertTrue(transport.call("a", "request", command=regression.command(
                "Login", identifier=ENV["TEST_USER"], password=ENV["TEST_PASSWORD"]))["ok"])
            args, kwargs = execute.call_args
            self.assertNotIn(ENV["TEST_PASSWORD"], str(args))
            self.assertIn("StrictHostKeyChecking=yes", args[0])
            self.assertIn(ENV["TEST_PASSWORD"], kwargs["input"])
            self.assertNotIn("shell", kwargs)

    def test_ssh_errors_and_invalid_response_shapes_do_not_leak_output(self):
        transport = regression.SshTransport(configuration()["peers"])
        for code, output in ((0, "secret-value-123"), (0, '[]'), (0, '{"ok":"true"}'), (1, '{"ok":true}')):
            with patch.object(regression.subprocess, "run", return_value=SimpleNamespace(
                    returncode=code, stdout=output)):
                with self.assertRaises(regression.RegressionError) as raised:
                    transport.call("a", "inventory")
                self.assertNotIn(ENV["TEST_PASSWORD"], str(raised.exception))
        with patch.object(regression.subprocess, "run", side_effect=subprocess.TimeoutExpired("ssh", 1)):
            with self.assertRaises(regression.RegressionError):
                transport.call("a", "inventory")

    def test_installer_is_hashed_locally_and_remotely_before_execution(self):
        config = configuration()
        config["peers"]["a"].update(installer_env="INSTALLER", installer_sha256_env="INSTALLER_HASH")
        transport = regression.SshTransport(config["peers"])
        with tempfile.TemporaryDirectory() as path:
            installer = Path(path) / "setup.exe"
            installer.write_bytes(b"test package bytes")
            digest = hashlib.sha256(installer.read_bytes()).hexdigest()
            environment = {"INSTALLER": str(installer), "INSTALLER_HASH": digest}
            stage = "C:/ProgramData/P2PRemoteRegression/staging/" + digest + ".exe"
            with patch.object(transport, "call", side_effect=[{"ok": True, "path": stage},
                    {"ok": True, "installer_sha256": digest}]) as call, patch.object(
                    regression.subprocess, "run", return_value=SimpleNamespace(returncode=0)) as upload:
                self.assertEqual(transport.deploy("a", environment)["installer_sha256"], digest)
                self.assertEqual(call.call_args_list[-1].kwargs["sha256"], digest)
                self.assertIn("StrictHostKeyChecking=yes", upload.call_args.args[0])
            environment["INSTALLER_HASH"] = "0" * 64
            with patch.object(transport, "call") as call:
                with self.assertRaises(regression.RegressionError):
                    transport.deploy("a", environment)
                call.assert_not_called()

    def test_installer_cannot_choose_an_arbitrary_remote_path(self):
        config = configuration()
        config["peers"]["a"].update(installer_env="INSTALLER", installer_sha256_env="INSTALLER_HASH")
        transport = regression.SshTransport(config["peers"])
        with tempfile.TemporaryDirectory() as path:
            installer = Path(path) / "setup.exe"
            installer.write_bytes(b"package")
            environment = {"INSTALLER": str(installer), "INSTALLER_HASH": hashlib.sha256(b"package").hexdigest()}
            with patch.object(transport, "call", return_value={"ok": True, "path": "C:/other.exe"}), patch.object(
                    regression.subprocess, "run") as upload:
                with self.assertRaises(regression.RegressionError):
                    transport.deploy("a", environment)
                upload.assert_not_called()


if __name__ == "__main__":
    unittest.main()
