#!/usr/bin/env python3
"""
Makes a RustMode release zip with the precompiled engine for every platform.

    scripts/create-release.py [version]

1. Runs the build-cache workflow on GitHub (Linux, Windows, macOS; CPU and
   libprocessing) and waits for it.
2. Downloads each platform's cache into cache/<platform>/<renderer>/.
3. Builds the mode jar and zips the mode folder as RustMode[-version].zip
   next to the RustMode folder.

Needs the GitHub CLI (gh), logged in. Nothing is pushed: the workflow runs
on what is already on GitHub. --local skips GitHub and zips whatever is in
cache/ now (for example a cache you built yourself with build-cache.py).
"""
import json, os, shutil, subprocess, sys, tempfile, time, zipfile, datetime
from pathlib import Path

REPO = os.environ.get("RUSTMODE_REPO", "pepc84/RustMode")
ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = "build-cache.yml"
PLATFORMS = ["linux-x86-64", "macos-x86-64", "macos-arm64", "windows-x86-64"]
RENDERERS = ["cpu", "libprocessing"]
SKIP_DIRS = {".git", ".github", "target", "build", ".gradle"}


def gh(*args):
    return subprocess.run(["gh", *args], capture_output=True, text=True)


def run_workflow():
    start = time.time()
    r = gh("workflow", "run", WORKFLOW, "--repo", REPO)
    if r.returncode != 0:
        sys.exit(f"couldn't start {WORKFLOW}: {r.stderr.strip()}")
    run_id = None
    for _ in range(24):
        time.sleep(5)
        r = gh("run", "list", "--repo", REPO, "--workflow", WORKFLOW, "--limit", "5", "--json", "databaseId,createdAt")
        for run in json.loads(r.stdout or "[]"):
            created = datetime.datetime.fromisoformat(run["createdAt"].replace("Z", "+00:00")).timestamp()
            if created >= start - 10:
                run_id = run["databaseId"]
                break
        if run_id:
            break
    if not run_id:
        sys.exit("couldn't find the workflow run")
    print(f"{WORKFLOW}: run {run_id}")
    while True:
        r = gh("run", "view", str(run_id), "--repo", REPO, "--json", "status,conclusion")
        if r.returncode == 0:
            d = json.loads(r.stdout)
            if d["status"] == "completed":
                print(f"{WORKFLOW}: {d['conclusion']}")
                return run_id
            print(f"{WORKFLOW}: {d['status']}, checking again in 30s")
        time.sleep(30)


def download(run_id):
    for plat in PLATFORMS:
        for rend in RENDERERS:
            name = f"cache-{plat}-{rend}"
            dest = ROOT / "cache" / plat / rend
            with tempfile.TemporaryDirectory() as tmp:
                r = gh("run", "download", str(run_id), "--repo", REPO, "--name", name, "--dir", tmp)
                if r.returncode != 0:
                    print(f"  {name}: not there ({r.stderr.strip()[:80]}), keeping what's in cache/")
                    continue
                if dest.exists():
                    shutil.rmtree(dest)
                shutil.copytree(tmp, dest)
                print(f"  {name}: ok")


def build_jar():
    r = subprocess.run(["gradle", "jar", "-q"], cwd=ROOT)
    if r.returncode != 0:
        sys.exit("gradle jar failed")


def bump(version):
    p = ROOT / "mode.properties"
    if not p.exists():
        return
    out = []
    for line in p.read_text().splitlines():
        if line.startswith("version="):
            try:
                line = f"version={int(line.split('=', 1)[1]) + 1}"
            except ValueError:
                pass
        elif line.startswith("prettyVersion="):
            line = f"prettyVersion={version}"
        out.append(line)
    p.write_text("\n".join(out) + "\n")


def make_zip(version):
    name = f"RustMode-{version}.zip" if version else "RustMode.zip"
    path = ROOT.parent / name
    if path.exists():
        path.unlink()
    n = 0
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for dirpath, dirs, files in os.walk(ROOT):
            rel = Path(dirpath).relative_to(ROOT)
            dirs[:] = [d for d in dirs if not (rel == Path(".") and d in SKIP_DIRS)]
            for f in files:
                full = Path(dirpath) / f
                if f.endswith((".log", ".bak")) or f.startswith("hs_err_pid"):
                    continue
                z.write(full, Path("RustMode") / full.relative_to(ROOT))
                n += 1
    print(f"\n{path}  ({path.stat().st_size // (1024 * 1024)} MB, {n} files)")
    for plat in PLATFORMS:
        have = [r for r in RENDERERS if (ROOT / "cache" / plat / r / "cache.json").exists()]
        print(f"  {plat}: {', '.join(have) if have else 'no cache (cargo builds there)'}")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    version = args[0] if args else None
    if version:
        bump(version)
    if "--local" not in sys.argv:
        download(run_workflow())
    build_jar()
    make_zip(version)
