#!/usr/bin/env python3
"""Builds scripts/ci/smoke_main.rs against cache/<platform>/<renderer> the way
RustMode does (one rustc call, see RustBuild.cachedBuild) and runs it. Fails if
it doesn't build, or doesn't print "smoke ok" within 60 seconds.

    python3 scripts/ci/smoke.py cpu|libprocessing
"""
import json, os, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import importlib.util
spec = importlib.util.spec_from_file_location("bc", ROOT / "scripts" / "build-cache.py")
bc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bc)

rend = sys.argv[1]
cache = ROOT / "cache" / bc.platform_dir() / rend
meta = json.loads((cache / "cache.json").read_text())
out = Path(tempfile.mkdtemp()) / ("smoke.exe" if os.name == "nt" else "smoke")
cmd = ["rustc", "--edition", "2021", "--crate-name", "smoke", "--crate-type", "bin", "-C", "opt-level=1",
       "-L", f"dependency={cache / 'deps'}", "-L", f"native={cache / 'native'}",
       "--extern", f"processing={cache / 'deps' / meta['engine']}", "-o", str(out)]
for p in meta.get("link_search", []):
    if Path(p).is_dir():
        cmd += ["-L", f"native={p}"]
cmd.append(str(ROOT / "scripts" / "ci" / "smoke_main.rs"))
print(" ".join(cmd))
if subprocess.run(cmd).returncode != 0:
    sys.exit("smoke: the sketch didn't build against the cache")

env = dict(os.environ)
nat = str(cache / "native")
for var in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "PATH"):
    env[var] = nat + os.pathsep + env.get(var, "")
if rend == "libprocessing":
    env["PROCESSING_RENDERER"] = "libprocessing"
try:
    p = subprocess.run([str(out)], env=env, capture_output=True, text=True, timeout=60)
except subprocess.TimeoutExpired:
    sys.exit("smoke: the sketch didn't finish within 60 seconds")
print(p.stdout[-2000:])
print(p.stderr[-4000:], file=sys.stderr)
if "smoke ok" not in p.stdout:
    sys.exit(f"smoke: the sketch didn't run (exit {p.returncode})")
print("smoke: ok")
