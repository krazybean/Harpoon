#!/usr/bin/env python3
import importlib.util
import importlib.machinery
import json
import os
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
AGENT = ROOT / "tools/guest-builder/src/harpoon-mgmt"
CLI = ROOT / "harpoon/Sources/HarpoonCLI.swift"

def run_agent(request):
    with tempfile.TemporaryDirectory() as tmp:
        marker_path = pathlib.Path(tmp) / "markers"
        env = os.environ | {"HARPOON_MGMT_MARKER_PATH": str(marker_path)}
        result = subprocess.run([sys.executable, str(AGENT)], input=request, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, check=False)
        return result, marker_path.read_text()

def require(condition, message):
    if not condition:
        raise AssertionError(message)

valid = json.dumps({"op": "exec", "argv": ["/usr/bin/printf", "secret-output"]}).encode() + b"\n"
result, markers = run_agent(valid)
require(result.returncode == 0 and b'"exit": 0' in result.stdout, "valid exec failed")
for marker in ("CHILD_START", "REQUEST_RECEIVED", "REQUEST_PARSED op=exec", "EXEC_START argc=2", "RESPONSE_SENT status=0", "CHILD_EXIT status=0"):
    require(marker in markers, "missing marker " + marker)
require("secret-output" not in markers, "command output leaked into markers")

result, markers = run_agent(b'{"op":"exec"')
require(result.returncode == 0 and not result.stdout, "EOF-before-newline changed protocol")
require("REQUEST_EOF bytes=12 newline=no" in markers, "missing EOF marker")

result, markers = run_agent(b"not-json\n")
require(result.returncode == 1 and b"invalid request: not JSON" in result.stdout, "malformed JSON response changed")
require("REQUEST_PARSE_ERROR type=JSONDecodeError" in markers, "missing parse marker")

with tempfile.TemporaryDirectory() as tmp:
    marker_path = pathlib.Path(tmp) / "markers"
    os.environ["HARPOON_MGMT_MARKER_PATH"] = str(marker_path)
    spec = importlib.util.spec_from_loader("harpoon_mgmt", importlib.machinery.SourceFileLoader("harpoon_mgmt", str(AGENT)))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.main = lambda: (_ for _ in ()).throw(RuntimeError())
    require(module.run() == 1, "child exception was not handled")
    require("CHILD_ERROR type=RuntimeError" in marker_path.read_text(), "missing child exception marker")

cli = CLI.read_text()
require("func managementListenerReady()" in cli and "func managementReady() -> Bool {\n    isMgmtServiceReachable()" in cli, "listener and end-to-end readiness are not separated")
print("harpoon-mgmt diagnostics PASS")
