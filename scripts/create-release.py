#!/usr/bin/env python3
"""
Makes RustMode-<version>.zip, ready to upload as a release, next to the
RustMode folder. One command, nothing pushed, nothing changed in the repo:

    scripts/create-release.py

The version is prettyVersion in mode.properties (0.1.0); change it there
when you want a new one.

1. Precompiled engine for this machine's platform (cache/<platform>/cpu and
   .../libprocessing), built only if it's missing or out of date. Builds
   share one target folder (see build-cache.py), so Bevy compiles once and
   later builds only redo what changed. libprocessing is the commit pinned
   in Cargo.toml, what users get, even when ~/lp-fork is there.
2. Builds the mode jar (gradle jar).
3. Zips the mode folder. Caches for other platforms that are already in
   cache/ go in too if they match the engine; stale ones are left out.

On platforms without a cache in the zip, sketches build with cargo the
first time (slower, then fast). Caches for the other platforms can come
from the build-cache workflow on GitHub later: --ci runs it and downloads
them into cache/ before zipping (needs the code on GitHub and gh logged in).

    --skip-cache   zip what's in cache/ now, build nothing
    --ci           also fetch every platform's cache from GitHub first
"""
import importlib.util, json, os, shutil, subprocess, sys, tempfile, time, zipfile, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLATFORMS = ["linux-x86-64", "linux-arm64", "macos-x86-64", "macos-arm64", "windows-x86-64"]
RENDERERS = ["cpu", "libprocessing"]
SKIP_DIRS = {".git", ".github", "target", "build", ".gradle", "__pycache__"}
WORKFLOW = "build-cache.yml"

_spec = importlib.util.spec_from_file_location("build_cache", ROOT / "scripts" / "build-cache.py")
bc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bc)


def cache_ok(plat, rend):
    """A cache RustMode would use: same engine sources and Rust version."""
    meta = ROOT / "cache" / plat / rend / "cache.json"
    if not meta.is_file():
        return False
    try:
        m = json.loads(meta.read_text())
    except ValueError:
        return False
    if m.get("engine_hash") != bc.engine_hash(ROOT):
        return False
    if m.get("rustc") != bc.toolchain() and not str(m.get("rustc", "")).startswith(bc.toolchain()):
        return False
    return True


def local_caches():
    plat = bc.platform_dir()
    for rend in RENDERERS:
        meta = ROOT / "cache" / plat / rend / "cache.json"
        pinned = rend == "cpu" or (meta.is_file() and json.loads(meta.read_text()).get("libprocessing") == "pinned")
        if cache_ok(plat, rend) and pinned:
            print(f"{plat}/{rend}: up to date")
            continue
        print(f"{plat}/{rend}: building (the first time takes a while; later only the engine recompiles)")
        t = time.time()
        r = subprocess.run([sys.executable, str(ROOT / "scripts" / "build-cache.py"), rend, "--upstream"])
        if r.returncode != 0:
            sys.exit(f"{plat}/{rend}: the cache build failed")
        print(f"{plat}/{rend}: done in {int(time.time() - t) // 60} min")


def gh(*args):
    return subprocess.run(["gh", *args], capture_output=True, text=True)


def ci_caches():
    repo = os.environ.get("RUSTMODE_REPO") or gh("repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner").stdout.strip()
    if not repo:
        sys.exit("--ci: no GitHub repository (set RUSTMODE_REPO=owner/RustMode or add the git remote)")
    start = time.time()
    r = gh("workflow", "run", WORKFLOW, "--repo", repo)
    if r.returncode != 0:
        sys.exit(f"couldn't start {WORKFLOW}: {r.stderr.strip()}")
    run_id = None
    for _ in range(24):
        time.sleep(5)
        r = gh("run", "list", "--repo", repo, "--workflow", WORKFLOW, "--limit", "5", "--json", "databaseId,createdAt")
        for run in json.loads(r.stdout or "[]"):
            created = datetime.datetime.fromisoformat(run["createdAt"].replace("Z", "+00:00")).timestamp()
            if created >= start - 10:
                run_id = run["databaseId"]
                break
        if run_id:
            break
    if not run_id:
        sys.exit("couldn't find the workflow run")
    print(f"{WORKFLOW}: run {run_id}, checking every 30s")
    while True:
        d = json.loads(gh("run", "view", str(run_id), "--repo", repo, "--json", "status,conclusion").stdout or "{}")
        if d.get("status") == "completed":
            print(f"{WORKFLOW}: {d.get('conclusion')}")
            break
        time.sleep(30)
    for plat in PLATFORMS:
        for rend in RENDERERS:
            name = f"cache-{plat}-{rend}"
            with tempfile.TemporaryDirectory() as tmp:
                if gh("run", "download", str(run_id), "--repo", repo, "--name", name, "--dir", tmp).returncode != 0:
                    continue
                dest = ROOT / "cache" / plat / rend
                if dest.exists():
                    shutil.rmtree(dest)
                shutil.copytree(tmp, dest)
                print(f"  {name}: downloaded")


def mode_version():
    p = ROOT / "mode.properties"
    for line in p.read_text().splitlines() if p.exists() else []:
        if line.startswith("prettyVersion="):
            return line.split("=", 1)[1].strip()
    return None


def build_jar():
    r = subprocess.run(["gradle", "jar", "-q"], cwd=ROOT)
    if r.returncode != 0:
        sys.exit("gradle jar failed")


def make_zip(version):
    good = {(p, r) for p in PLATFORMS for r in RENDERERS if cache_ok(p, r)}
    name = f"RustMode-{version}.zip" if version else "RustMode.zip"
    path = ROOT.parent / name
    if path.exists():
        path.unlink()
    n = 0
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for dirpath, dirs, files in os.walk(ROOT):
            rel = Path(dirpath).relative_to(ROOT)
            if rel == Path("."):
                dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
            dirs[:] = [d for d in dirs if d != "__pycache__"]
            # cache/<platform>/<renderer>: only the ones RustMode would use
            parts = rel.parts
            if len(parts) == 2 and parts[0] == "cache":
                dirs[:] = [d for d in dirs if (parts[1], d) in good]
            for f in files:
                full = Path(dirpath) / f
                if f.endswith((".log", ".bak")) or f.startswith("hs_err_pid"):
                    continue
                z.write(full, Path("RustMode") / full.relative_to(ROOT))
                n += 1
    print(f"\n{path}  ({path.stat().st_size // (1024 * 1024)} MB, {n} files)")
    for plat in PLATFORMS:
        have = [r for r in RENDERERS if (plat, r) in good]
        print(f"  {plat}: {', '.join(have) if have else 'no cache (sketches build with cargo the first time)'}")
    return path


if __name__ == "__main__":
    version = mode_version()
    if "--ci" in sys.argv:
        ci_caches()
    if "--skip-cache" not in sys.argv:
        local_caches()
    print(f"RustMode {version or '(no prettyVersion in mode.properties)'}")
    build_jar()
    make_zip(version)
