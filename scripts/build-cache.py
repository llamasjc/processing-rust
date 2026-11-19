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


def fork_stamp(fork: Path) -> int:
    """Newest modification time (whole seconds) of the libprocessing crates
    RustMode uses from a checkout: the same walk as RustBuild.forkStamp()."""
    newest = 0
    for c in ["processing_core", "processing_render", "processing_input", "processing_glfw"]:
        d = fork / "crates" / c
        for f in [d / "Cargo.toml"] + sorted((d / "src").rglob("*")):
            if f.is_file():
                newest = max(newest, int(f.stat().st_mtime))
    return newest


def bundle_glfw(native: Path):
    """GLFW 3.4 goes in the cache next to the engine, so a sketch links and runs
    against it on computers without GLFW 3.4 installed (Ubuntu 22.04 has 3.3).
    RustMode puts this folder on the library path when it runs a sketch and
    copies it into exported applications."""
    if sys.platform.startswith("linux"):
        libdir = subprocess.run(["pkg-config", "--variable=libdir", "glfw3"], capture_output=True, text=True).stdout.strip()
        cands = [Path(libdir)] if libdir else []
        cands += [Path("/usr/local/lib"), Path("/usr/lib"), Path("/usr/lib/x86_64-linux-gnu"), Path("/usr/lib/aarch64-linux-gnu")]
        for d in cands:
            so = d / "libglfw.so.3"
            if so.exists():
                real = so.resolve()
                shutil.copy2(real, native / "libglfw.so.3")
                # the name the linker looks for
                shutil.copy2(real, native / "libglfw.so")
                print(f"GLFW: {real} bundled")
                return
        print("GLFW: not found to bundle; sketches will need GLFW 3.4 installed")
    elif sys.platform == "darwin":
        if any(native.glob("libglfw*.dylib")):
            return
        brew = subprocess.run(["brew", "--prefix", "glfw"], capture_output=True, text=True).stdout.strip()
        for d in [Path(brew) / "lib"] if brew else []:
            for f in d.glob("libglfw.3*.dylib"):
                shutil.copy2(f.resolve(), native / "libglfw.3.dylib")
                print(f"GLFW: {f} bundled")
                return


def prune_unlinked(out: Path, engine: str, env: dict):
    """Drops the libraries a sketch never links: build-script dependencies
    (lalrpop and the like) and the ones only proc macros use. That's about a
    seventh of the libprocessing cache. rustc prints what it would link for a
    program that only uses the engine, and that list is the same for every
    sketch. Proc macros themselves (.so, .dylib, .dll) stay: rustc loads them
    when it reads the engine."""
    import re, tempfile
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "m.rs"
        src.write_text("extern crate processing;\nfn main() {}\n")
        p = subprocess.run(["rustc", "--edition", "2021", "--crate-type", "bin",
                            "-L", f"dependency={out / 'deps'}", "-L", f"native={out / 'native'}",
                            "--extern", f"processing={out / 'deps' / engine}",
                            "--print", "link-args", "-o", str(Path(tmp) / "m"), str(src)],
                           env=env, capture_output=True, text=True)
    text = p.stdout
    linked = set()
    if not text:
        return ""
    for body in re.findall(r"\{([^}]*)\}\.rlib", text):   # deps/{liba-1,libb-2}.rlib
        linked.update(n + ".rlib" for n in body.split(","))
    linked.update(re.findall(r"([\w\-]+\.rlib)", re.sub(r"\{[^}]*\}", "", text)))
    if engine not in linked:
        print("  (couldn't read what a sketch links; keeping every library)")
        return text
    dropped = 0
    for f in (out / "deps").glob("*.rlib"):
        if f.name not in linked:
            dropped += f.stat().st_size
            f.unlink()
    print(f"  dropped {dropped // (1024 * 1024)} MB a sketch never links")
    return text


def link_kit(kit: Path, link_args: str):
    """The system files a sketch links against on this Linux: glibc's start
    files and libraries, libgcc_s, and whatever the engine asks for (-lX11,
    -lasound...). With them, another computer's cross linker can link a
    sketch for this platform (Export Application on Linux), and the program
    asks for this glibc, not a newer one. Copies, no symlinks, all in
    usr/lib; linker scripts like libc.so are rewritten to plain names."""
    import re
    lib = kit / "usr" / "lib"
    if kit.exists():
        shutil.rmtree(kit)
    lib.mkdir(parents=True)

    def find(name):
        r = subprocess.run(["cc", f"-print-file-name={name}"], capture_output=True, text=True).stdout.strip()
        return Path(r) if r and r != name and Path(r).exists() else None

    def copy_lib(path: Path, as_name: str):
        real = path.resolve()
        data = real.read_bytes()
        if not data.startswith(b"\x7fELF"):
            # a linker script: copy what it names, refer to them by name
            text = data.decode(errors="replace")
            body = re.sub(r"/\*.*?\*/", "", text, flags=re.S)   # not the comments
            for ref in set(re.findall(r"((?:/[\w.+-]+)+|lib[\w.+-]+\.(?:so[.\d]*|a))", body)):
                rp = Path(ref) if ref.startswith("/") else real.parent / ref
                if not rp.exists() and not ref.startswith("/"):
                    rp = find(ref) or rp
                if rp.exists():
                    shutil.copy2(rp.resolve(), lib / rp.name)
                text = text.replace(ref, rp.name)
            (lib / as_name).write_text(text)
        else:
            # the linker takes the name a program asks for (libX11.so.6)
            # from inside the file, so one copy under the plain name will do
            shutil.copyfile(real, lib / as_name)

    for f in ["crt1.o", "Scrt1.o", "crti.o", "crtn.o", "libc_nonshared.a"]:
        p = find(f)
        if p:
            shutil.copy2(p, lib / f)
    names = set(re.findall(r'"-l([^"\s]+)"|\s-l([^\s"]+)', link_args))
    wanted = {a or b for a, b in names} | {"c", "m", "pthread", "dl", "rt", "util", "gcc_s"}
    for n in sorted(wanted):
        p = find(f"lib{n}.so")
        if p:
            copy_lib(p, f"lib{n}.so")
        elif find(f"lib{n}.a"):
            # glibc 2.34+: libpthread, libdl... are empty archives now
            shutil.copy2(find(f"lib{n}.a"), lib / f"lib{n}.a")
        else:
            print(f"  link kit: lib{n}.so not found")
    for ld in Path("/lib").glob("ld-linux*.so*"):
        shutil.copyfile(ld.resolve(), lib / ld.name)
    for ld in Path("/lib64").glob("ld-linux*.so*"):
        shutil.copyfile(ld.resolve(), lib / ld.name)
    size = sum(f.stat().st_size for f in lib.iterdir()) // (1024 * 1024)
    print(f"  link kit: {len(list(lib.iterdir()))} files, {size} MB")


def build(renderer: str, upstream: bool = False):
    plat = platform_dir()
    fork = local_fork() if renderer == "libprocessing" and not upstream else None
    # a cache built against your own libprocessing checkout is for your IDE
    # only: it goes in libprocessing-fork, which releases leave out
    out = ROOT / "cache" / plat / (renderer + ("-fork" if fork else ""))
    # kept between runs, so after an engine change only the engine recompiles
    work = Path(os.environ.get("RUSTMODE_CACHE_WORK", Path.home() / ".cache" / "rustmode-cache")) / plat / (renderer + ("-upstream" if upstream else ""))
    (work / "src").mkdir(parents=True, exist_ok=True)
    feature = '\n[features]\ndefault = ["processing/libprocessing"]\n' if renderer == "libprocessing" else ""
    patch = ""
    # (--upstream: the libprocessing commit pinned in Cargo.toml, what users
    # get, even when there's a checkout of your own; releases use this)
    fork_newest = fork_stamp(fork) if fork else None
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
    # One target folder for every cache build on this platform: CPU,
    # libprocessing, your fork and the pinned commit. Cargo keeps each
    # crate's build there and reuses it whenever the crate, its features and
    # the settings are the same, so Bevy and the other big crates compile
    # once and every later build only compiles what changed.
    target = work.parent / "target"
    env = dict(os.environ, RUSTUP_TOOLCHAIN=tc, CARGO_TARGET_DIR=str(target))
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
                if str(target) in path:
                    # a C library a crate built or fetched for itself: the
                    # static libraries, and shared ones (glfw-sys downloads
                    # GLFW 3.4 as a .dylib / .dll on macOS and Windows)
                    for a in Path(path).glob("*"):
                        if a.suffix in (".a", ".lib", ".dylib", ".dll") or ".so" in a.name:
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
    if renderer == "libprocessing":
        bundle_glfw(out / "native")
    link_args = prune_unlinked(out, engine, env)
    if sys.platform.startswith("linux") and link_args:
        link_kit(out / "linkkit", link_args)
    meta = {
        "rustc": version,
        "rustc_full": rustc,
        "engine": engine,
        "engine_hash": engine_hash(ROOT),
        "renderer": renderer,
        "platform": plat,
        "link_search": search,
        "libprocessing": (str(fork) if fork else "pinned") if renderer == "libprocessing" else None,
        # the newest file in the checkout's crates when this was built: the
        # IDE skips the cache once anything there is newer
        "fork_newest": fork_newest,
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
