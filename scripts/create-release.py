#!/usr/bin/env python3
"""
Makes a RustMode release zip with the precompiled engine for every platform.

    scripts/create-release.py 1.2.0 --publish   the usual way (see below)
    scripts/create-release.py [version]         everything here, then upload by hand
    scripts/create-release.py [version] --local zip what's in cache/ now

--publish: builds the jar, zips the mode folder (with no caches, so it's
quick) as RustMode-<version>.zip and publishes it as GitHub release
v<version>. Publishing starts .github/workflows/release.yml, which builds
the precompiled engine on Linux, macOS (Intel and Apple Silicon) and
Windows, for CPU and libprocessing, and puts every one into that zip on the
release. Push the commit first and run this from a clean checkout of it:
the caches are built from the tag, and RustMode only uses a cache built
from the same engine sources as the mode in the zip.

Without --publish:
1. Runs the build-cache workflow on GitHub and waits for it.
2. Downloads each platform's cache into cache/<platform>/<renderer>/.
3. Builds the mode jar and zips the mode folder as RustMode[-version].zip
   next to the RustMode folder.
--local skips 1 and 2 and zips whatever is in cache/ now (for example a
cache you built yourself with build-cache.py).

Needs the GitHub CLI (gh), logged in. The repository is RUSTMODE_REPO, or
else the one gh finds for this folder (its git remote).
"""
import json, os, shutil, subprocess, sys, tempfile, time, zipfile, datetime
from pathlib import Path

def find_repo():
    if os.environ.get("RUSTMODE_REPO"):
        return os.environ["RUSTMODE_REPO"]
    r = subprocess.run(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"],
                       cwd=Path(__file__).resolve().parent.parent, capture_output=True, text=True)
    return r.stdout.strip() or None


REPO = find_repo()
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


def make_zip(version, with_cache=True):
    name = f"RustMode-{version}.zip" if version else "RustMode.zip"
    path = ROOT.parent / name
    if path.exists():
        path.unlink()
    n = 0
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for dirpath, dirs, files in os.walk(ROOT):
            rel = Path(dirpath).relative_to(ROOT)
            dirs[:] = [d for d in dirs if not (rel == Path(".") and (d in SKIP_DIRS or (d == "cache" and not with_cache)))]
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
    return path


def publish(version, zip_path):
    tag = f"v{version}"
    r = gh("release", "create", tag, str(zip_path), "--repo", REPO, "--title", f"RustMode {version}",
           "--generate-notes", "--target", subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                                                          capture_output=True, text=True).stdout.strip())
    if r.returncode != 0:
        sys.exit(f"couldn't publish {tag}: {r.stderr.strip()}")
    print(f"published {tag}: {r.stdout.strip()}")
    print("the release workflow is now adding every platform's cache to the zip (about an hour);")
    print(f"watch it with: gh run watch --repo {REPO} $(gh run list --repo {REPO} --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    version = args[0] if args else None
    publishing = "--publish" in sys.argv
    if (publishing or "--local" not in sys.argv) and not REPO:
        sys.exit("no GitHub repository: set RUSTMODE_REPO=owner/RustMode, or add the git remote")
    if publishing and not version:
        sys.exit("--publish needs a version: scripts/create-release.py 1.2.0 --publish")
    if publishing:
        changed = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no", "--", "src", "Cargo.toml",
                                  "Cargo.lock", "build.rs"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
        if changed:
            sys.exit("the engine has uncommitted changes; commit and push them first, "
                     "since the release's caches are built from what's on GitHub")
    if version:
        bump(version)
    if "--local" not in sys.argv and not publishing:
        download(run_workflow())
    build_jar()
    # published zips get their caches from the release workflow
    path = make_zip(version, with_cache=not publishing)
    if publishing:
        publish(version, path)
