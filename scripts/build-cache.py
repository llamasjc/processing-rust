#!/usr/bin/env python3
"""
Builds the precompiled engine for the current platform into
cache/<platform>/<renderer>/, so sketches compile in about a second instead
of building the engine and all its crates first.

    scripts/build-cache.py              the CPU renderer
    scripts/build-cache.py libprocessing
    scripts/build-cache.py all          both
    scripts/build-cache.py all --bg     both, in the background
    scripts/build-cache.py all --upstream   with the pinned libprocessing, even
                                          when ~/lp-fork is there (releases)

--bg detaches from the terminal (its own session, so closing the terminal,
Ctrl-C or Processing can't stop it) and logs to
~/.cache/rustmode-cache/build.log. Watch it with
    tail -f ~/.cache/rustmode-cache/build.log
You get a desktop notification when it's done, if notify-send is there.

With a cache in place RustMode compiles a sketch with one rustc call against
the cached libraries (cache/<platform>/<renderer>/deps). It needs the exact
Rust version the cache was built with (written to cache.json); RustMode
fetches it with rustup the first time if it isn't installed. When anything
doesn't match (other engine sources, no rustup, a different platform) it
goes back to the normal cargo build.

The CI workflow build-cache.yml runs this on Linux, Windows and macOS.

Uses your own libprocessing checkout (LIBPROCESSING_DIR, or
sketchbook/libprocessing next to the modes folder) when there is one.
"""
import hashlib, json, os, platform, shutil, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def platform_dir():
    s, m = platform.system().lower(), platform.machine().lower()
    arm = "arm" in m or "aarch64" in m
    if s == "windows":
        return "windows-x86-64"
    if s == "darwin":
        return "macos-arm64" if arm else "macos-x86-64"
    return "linux-arm64" if arm else "linux-x86-64"


def engine_hash(root: Path) -> str:
    """Same as RustBuild.engineHash(): the engine's sources and lockfile,
    line endings ignored so a Windows checkout gives the same answer."""
    h = hashlib.sha256()
    files = ["Cargo.toml", "Cargo.lock"] + (["build.rs"] if (root / "build.rs").is_file() else []) \
        + sorted("src/" + p.name for p in (root / "src").glob("*.rs"))
    for name in files:
        data = (root / name).read_bytes().replace(b"\r\n", b"\n")
        h.update(name.encode() + b"\0" + data + b"\0")
    return h.hexdigest()


def toolchain():
    t = (ROOT / "rust-toolchain.toml").read_text()
    for line in t.splitlines():
        if line.strip().startswith("channel"):
            return line.split("=", 1)[1].strip().strip('"')
    return "stable"


def local_fork():
    env = os.environ.get("LIBPROCESSING_DIR")
    for d in ([env] if env else []) + [str(Path.home() / "lp-fork"), str(ROOT.parent.parent / "libprocessing")]:
        if d and (Path(d) / "crates/processing_render/Cargo.toml").is_file():
            return Path(d)
    return None


def build(renderer: str, upstream: bool = False):
    plat = platform_dir()
    out = ROOT / "cache" / plat / renderer
    # kept between runs, so after an engine change only the engine recompiles
    work = Path(os.environ.get("RUSTMODE_CACHE_WORK", Path.home() / ".cache" / "rustmode-cache")) / plat / (renderer + ("-upstream" if upstream else ""))
    (work / "src").mkdir(parents=True, exist_ok=True)
    feature = '\n[features]\ndefault = ["processing/libprocessing"]\n' if renderer == "libprocessing" else ""
    patch = ""
    # --upstream: the libprocessing commit pinned in Cargo.toml, what users
    # get, even when there's a checkout of your own (releases use this)
    fork = local_fork() if renderer == "libprocessing" and not upstream else None
    if fork:
        patch = '\n[patch."https://github.com/processing/libprocessing"]\n' + "".join(
            f'{c} = {{ path = "{(fork / "crates" / c).as_posix()}" }}\n'
            for c in ["processing_core", "processing_render", "processing_input", "processing_glfw"])
    # the same profile RustBuild compiles sketches with
    (work / "Cargo.toml").write_text(f"""[package]
name = "cachebuild"
version = "0.1.0"
edition = "2021"

[dependencies]
processing = {{ path = "{ROOT.as_posix()}" }}
{feature}
[patch."https://github.com/bevyengine/bevy"]
bevy = {{ git = "https://github.com/processing/bevy", branch = "main" }}
{patch}
[profile.dev]
opt-level = 1
debug = 0
incremental = false
""")
    (work / "src/main.rs").write_text("fn main() {}\n")
    shutil.copy(ROOT / "Cargo.lock", work / "Cargo.lock")

    tc = os.environ.get("RUSTUP_TOOLCHAIN") or toolchain()
    env = dict(os.environ, RUSTUP_TOOLCHAIN=tc, CARGO_TARGET_DIR=str(work / "target"))
    # the engine's build.rs checks the same checkout for what it supports
    env["LIBPROCESSING_DIR"] = str(fork) if fork else ""
    print(f"Platform: {plat}  renderer: {renderer}  Rust: {tc}" + (f"  libprocessing: {fork}" if fork else ""))
    p = subprocess.run(["cargo", "build", "--message-format=json-render-diagnostics"],
                       cwd=work, env=env, stdout=subprocess.PIPE, text=True)
    if p.returncode != 0:
        sys.exit(f"cargo build failed for {renderer}")

    files, native, search, engine = set(), set(), [], None
    for line in p.stdout.splitlines():
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("reason") == "compiler-artifact":
            if m["target"]["name"] == "cachebuild":
                continue
            for f in m["filenames"]:
                if f.endswith((".rlib", ".so", ".dylib", ".dll")) and "/build/" not in f.replace("\\", "/"):
                    files.add(f)
                    if m["target"]["name"] == "processing" and f.endswith(".rlib"):
                        engine = Path(f).name
        elif m.get("reason") == "build-script-executed":
            for lp in m["linked_paths"]:
                kind, _, path = lp.rpartition("=")
                if str(work) in path:
                    # a C library a crate built for itself: copy the static libraries
                    for a in Path(path).glob("*"):
                        if a.suffix in (".a", ".lib"):
                            native.add(str(a))
                elif path not in search:
                    search.append(path)
    if not engine:
        sys.exit("the engine library wasn't built")

    rustc = subprocess.run(["rustc", "-V"], env=env, capture_output=True, text=True).stdout.strip()
    version = rustc.split()[1] if rustc else tc

    if out.exists():
        shutil.rmtree(out)
    (out / "deps").mkdir(parents=True)
    (out / "native").mkdir()
    for f in files:
        shutil.copy2(f, out / "deps" / Path(f).name)
    for f in native:
        shutil.copy2(f, out / "native" / Path(f).name)
    meta = {
        "rustc": version,
        "rustc_full": rustc,
        "engine": engine,
        "engine_hash": engine_hash(ROOT),
        "renderer": renderer,
        "platform": plat,
        "link_search": search,
        "libprocessing": (str(fork) if fork else "pinned") if renderer == "libprocessing" else None,
        "edition": "2021",
        "opt_level": "1",
    }
    (out / "cache.json").write_text(json.dumps(meta, indent=2) + "\n")
    size = sum(f.stat().st_size for f in out.rglob("*") if f.is_file()) // (1024 * 1024)
    print(f"Done: {out}  ({len(files)} libraries, {size} MB, {rustc})")


def notify(msg):
    if shutil.which("notify-send"):
        subprocess.run(["notify-send", "RustMode", msg])


def background(args):
    base = Path(os.environ.get("RUSTMODE_CACHE_WORK", Path.home() / ".cache" / "rustmode-cache"))
    base.mkdir(parents=True, exist_ok=True)
    log = base / "build.log"
    pidfile = base / "build.pid"
    if pidfile.exists():
        try:
            old = int(pidfile.read_text())
            cmd = Path(f"/proc/{old}/cmdline").read_bytes() if Path(f"/proc/{old}").exists() else b""
            if sys.platform.startswith("linux") and b"build-cache" not in cmd:
                raise OSError
            os.kill(old, 0)
            print(f"A cache build is already running (pid {old}). Log: {log}")
            return
        except (ValueError, OSError):
            pass
    with open(log, "w") as f:
        p = subprocess.Popen([sys.executable, "-u", __file__, *args, "--fg-from-bg"],
                             stdin=subprocess.DEVNULL, stdout=f, stderr=subprocess.STDOUT,
                             start_new_session=True, cwd=ROOT)
    pidfile.write_text(str(p.pid))
    print(f"Building in the background (pid {p.pid}).")
    print(f"Follow it:  tail -f {log}")


if __name__ == "__main__":
    flags = [a for a in sys.argv[1:] if a.startswith("--")]
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if "--bg" in flags:
        background(args + [f for f in flags if f == "--upstream"])
        sys.exit(0)
    what = args[0] if args else "cpu"
    todo = ["cpu", "libprocessing"] if what == "all" else [what]
    for r in todo:
        if r not in ("cpu", "libprocessing"):
            sys.exit(f"unknown renderer {r}: use cpu, libprocessing or all")
    try:
        for r in todo:
            build(r, upstream="--upstream" in flags)
        if "--fg-from-bg" in flags:
            notify(f"Engine cache ready: {', '.join(todo)}")
    except SystemExit as e:
        if "--fg-from-bg" in flags:
            notify(f"Cache build failed: {e}")
        raise
    finally:
        if "--fg-from-bg" in flags:
            pid = Path(os.environ.get("RUSTMODE_CACHE_WORK", Path.home() / ".cache" / "rustmode-cache")) / "build.pid"
            pid.unlink(missing_ok=True)
