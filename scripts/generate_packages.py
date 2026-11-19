#!/usr/bin/env python3
"""The engine for people not using the Processing IDE, like C++ Mode's
plug-and-play and CMake packages:

  processing-rust-starter.zip  a Cargo project with the engine and an example:
                               unzip, `cargo run`
  processing-rust-crate.zip    the engine as a crate, to add to your own
                               Cargo.toml (path or git dependency)

Both come from this repo's real engine (Cargo.toml, build.rs, src/*.rs), the
same one Rust Mode builds sketches against.

    scripts/generate_packages.py [--out FOLDER]     (default: dist/)
"""
import argparse, re, shutil, zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
REPO = "https://github.com/llamasjc/processing-rust"

EXAMPLE = '''// A Processing sketch in Rust. setup() runs once, draw() every frame.
use processing::*;

struct Sketch {
    x: f32,
}

fn setup(_s: &mut Sketch) {
    size(640, 360);
}

fn draw(s: &mut Sketch) {
    background(30);
    s.x = (s.x + 2.0) % width();
    fill_rgb(255, 120, 0);
    ellipse(s.x, height() / 2.0, 60.0, 60.0);
    fill(255);
    text("Hello from processing-rust", 20.0, 30.0);
}

fn main() {
    App::new(Sketch { x: 0.0 }, setup, draw).title("Hello").run();
}
'''

CRATE_README = f'''# processing-rust {VERSION}

Processing's API as a Rust crate: the same engine Rust Mode builds sketches
against, for your own Cargo projects.

## Use it

Unzip this folder next to your project and add it to your `Cargo.toml`:

```toml
[dependencies]
processing = {{ path = "../processing-rust" }}
```

Or straight from GitHub, without downloading anything:

```toml
[dependencies]
processing = {{ git = "{REPO}", tag = "v{VERSION}" }}
```

And, also in your `Cargo.toml` (Cargo only takes this from the top level):

```toml
[patch."https://github.com/bevyengine/bevy"]
# libprocessing uses Processing's fork of Bevy; Cargo only takes patches from
# the top-level Cargo.toml, so this line has to be in yours too
bevy = {{ git = "https://github.com/processing/bevy", branch = "main" }}
```

Then write a sketch:

```rust
{EXAMPLE}```

## Renderers

The CPU renderer is the default and works everywhere. For libprocessing
(Bevy and wgpu, on the graphics card), turn on the feature and set the
renderer when running:

```toml
processing = {{ path = "../processing-rust", features = ["libprocessing"] }}
```

```
PROCESSING_RENDERER=libprocessing cargo run --release
```

The first libprocessing build compiles Bevy and takes several minutes. On
Linux it needs GLFW 3.4 and the X11, ALSA and udev development files.

The Rust version is pinned in `rust-toolchain.toml`.
'''

STARTER_README = f'''# processing-rust starter {VERSION}

A Processing sketch in Rust, ready to run. You need Rust
(https://rustup.rs); nothing else.

```
cargo run --release
```

The sketch is `src/main.rs`. Files it loads go in `data/`.
`processing-rust/` is the engine (see its README for the libprocessing
renderer and for using it in other projects).

On Linux you also need a C compiler (`cc`). That's all: the CPU renderer
draws with nothing else installed.
'''

STARTER_TOML = f'''[package]
name = "sketch"
version = "0.1.0"
edition = "2021"

[dependencies]
processing = {{ path = "processing-rust" }}

[features]
# cargo run --release --features libprocessing, with PROCESSING_RENDERER=libprocessing
libprocessing = ["processing/libprocessing"]

[patch."https://github.com/bevyengine/bevy"]
# libprocessing uses Processing's fork of Bevy; Cargo only takes patches from
# the top-level Cargo.toml, so this line has to be in yours too
bevy = {{ git = "https://github.com/processing/bevy", branch = "main" }}
'''


def copy_crate(dest: Path):
    dest.mkdir(parents=True)
    for f in ["Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml"]:
        if (ROOT / f).exists():
            shutil.copy2(ROOT / f, dest / f)
    for lic in ROOT.glob("LICENSE*"):
        shutil.copy2(lic, dest / lic.name)
    (dest / "src").mkdir()
    for rs in sorted((ROOT / "src").glob("*.rs")):
        shutil.copy2(rs, dest / "src" / rs.name)


def zip_dir(folder: Path, out: Path):
    if out.exists():
        out.unlink()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for f in sorted(folder.rglob("*")):
            if f.is_file():
                z.write(f, f.relative_to(folder.parent))
    print(f"Wrote {out} ({out.stat().st_size // 1024} KB)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=ROOT / "dist")
    out = ap.parse_args().out
    out.mkdir(parents=True, exist_ok=True)
    work = out / ".packages"
    shutil.rmtree(work, ignore_errors=True)

    crate = work / "processing-rust"
    copy_crate(crate)
    (crate / "README.md").write_text(CRATE_README)
    zip_dir(crate, out / "processing-rust-crate.zip")

    starter = work / "processing-rust-starter"
    starter.mkdir()
    copy_crate(starter / "processing-rust")
    (starter / "processing-rust" / "README.md").write_text(CRATE_README)
    (starter / "Cargo.toml").write_text(STARTER_TOML)
    shutil.copy2(ROOT / "rust-toolchain.toml", starter / "rust-toolchain.toml")
    # the engine's tested versions of everything, for the project as a whole
    shutil.copy2(ROOT / "Cargo.lock", starter / "Cargo.lock")
    (starter / "src").mkdir()
    (starter / "src" / "main.rs").write_text(EXAMPLE)
    (starter / "data").mkdir()
    (starter / "data" / ".keep").write_text("")
    (starter / "README.md").write_text(STARTER_README)
    zip_dir(starter, out / "processing-rust-starter.zip")
    shutil.rmtree(work)


if __name__ == "__main__":
    main()
