"""Exercise the real Windows PowerShell 5.1 agent against a tiny local CLI stub."""
import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import run as regression


@unittest.skipUnless(os.name == "nt", "Windows PowerShell agent test")
class WindowsAgentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.executable = Path(cls.temp.name) / "fake-cli.exe"
        source = r"""
using System;
using System.Text;
public static class AgentSmokeProgram {
    public static int Main(string[] args) {
        if (args.Length == 2 && args[0] == "service") return 0;
        Console.InputEncoding = new UTF8Encoding(false);
        Console.OutputEncoding = new UTF8Encoding(false);
        string input = Console.In.ReadToEnd();
        if (input.Contains("simulate-failure")) {
            Console.WriteLine("{\"ok\":false,\"error_code\":\"test.request.failed\"}");
            return 1;
        }
        bool valid = input.Contains("中文") && input.Contains("test-password-$");
        Console.WriteLine(valid ? "{\"ok\":true,\"data\":{\"utf8\":true}}" : "{\"ok\":false}");
        return input.Contains("simulate-bad-exit") ? 1 : 0;
    }
}
"""
        script = "Add-Type -TypeDefinition @'\n" + source + "\n'@ -OutputType ConsoleApplication -OutputAssembly '" + str(cls.executable).replace("'", "''") + "'"
        cls.powershell(script)
        cls.agent = Path(__file__).with_name("windows_agent.ps1").read_text(encoding="utf-8")

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    @staticmethod
    def powershell(source, payload=None):
        encoded = base64.b64encode(source.encode("utf-16-le")).decode("ascii")
        return subprocess.run(["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive",
                               "-EncodedCommand", encoded], input=payload, text=True,
                              encoding="utf-8", capture_output=True, timeout=20, check=True)

    def invoke(self, kind):
        request = {"operation": "request", "cli_path": str(self.executable), "timeout_secs": 2,
                   "command": {"t": kind, "c": {"identifier": "中文", "password": "test-password-$"}}}
        encoded = regression.encode_agent(self.agent)
        self.assertLess(len(encoded), 7501)
        result = subprocess.run(["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive",
                                 "-EncodedCommand", encoded], input=json.dumps(request, ensure_ascii=False),
                                text=True, encoding="utf-8", capture_output=True, timeout=20)
        self.assertNotIn("test-password-$", result.stdout + result.stderr)
        return result.returncode, json.loads(result.stdout)

    def test_utf8_credentials_survive_real_powershell_51_stdin(self):
        code, response = self.invoke("Login")
        self.assertEqual(code, 0)
        self.assertTrue(response["ok"])
        self.assertTrue(response["data"]["utf8"])

    def test_cli_business_failure_remains_structured(self):
        code, response = self.invoke("simulate-failure")
        self.assertEqual(code, 0)
        self.assertFalse(response["ok"])
        self.assertEqual(response["error_code"], "test.request.failed")

    def test_cli_exit_code_cannot_contradict_success_json(self):
        code, response = self.invoke("simulate-bad-exit")
        self.assertNotEqual(code, 0)
        self.assertFalse(response["ok"])
        self.assertEqual(response["error_code"], "regression.agent.failed")

    def test_service_operations_wait_for_actual_windows_state(self):
        for action, expected in (("stop", "Stopped"), ("start", "Running"), ("restart", "Running")):
            with self.subTest(action=action):
                prefix = r"""
function Get-Service {
    param($Name)
    if ($Name -ne 'p2premote-service') { throw 'wrong service' }
    $mock = New-Object PSObject
    $mock | Add-Member ScriptMethod WaitForStatus {
        param($state, $timeout)
        if ($timeout.TotalSeconds -ne 30) { throw 'unbounded wait' }
        $global:observedState = [string]$state
    }
    return $mock
}
"""
                script = prefix + "\n& {\n" + self.agent + "\n}\n" + (
                    "if ($global:observedState -ne '" + expected + "') { throw 'state was not awaited' }")
                request = {"operation": "service", "action": action, "cli_path": str(self.executable)}
                result = self.powershell(script, json.dumps(request))
                self.assertTrue(json.loads(result.stdout)["ok"])

    def test_service_state_wait_failure_is_not_success(self):
        prefix = "function Get-Service { $m=New-Object PSObject; $m | Add-Member ScriptMethod WaitForStatus { throw 'still stopping' }; return $m }\n"
        encoded = base64.b64encode((prefix + self.agent).encode("utf-16-le")).decode("ascii")
        request = {"operation": "service", "action": "stop", "cli_path": str(self.executable)}
        result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
                                input=json.dumps(request), text=True, encoding="utf-8", capture_output=True, timeout=20)
        self.assertNotEqual(result.returncode, 0)
        response = json.loads(result.stdout)
        self.assertFalse(response["ok"])
        self.assertEqual(response["error_code"], "regression.agent.failed")
