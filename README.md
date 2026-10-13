# RustMode

A [Processing 4](https://processing.org) mode that lets you write sketches in Rust, compiled via Cargo.  
Rendering uses a self-contained software rasterizer backed by [minifb](https://github.com/emoon/rust_minifb) — no GPU or external native libraries required beyond the Rust toolchain.

---

## Quick-start

### 1 — Clone into your sketchbook

```
~/.../sketchbook/modes/RustMode/
```

### 2 - Build the tree-sitter-rust native grammar

```bash
cd ~/sketchbook/modes/RustMode
bash native/build.sh        # builds lib/<platform>/libjava-tree-sitter.so with the Rust grammar
```

### 3 — Build the jar

```bash
# From your processing4 repository root (or standalone from RustMode/):
./gradlew :RustMode:jar

# Standalone (from RustMode/):
gradle jar          # needs Gradle ≥ 7 on PATH
```

Output: `RustMode/mode/RustMode.jar`

### 4 — Link Java source into the processing4 tree (optional, for IDE hacking)

```bash
cd RustMode
PROCESSING4_DIR=~/Projects/processing4 ./scripts/link-sources.sh
```

### 5 — Install

Place (or symlink) the whole `RustMode/` folder into:

```
~/.processing/modes/RustMode/
```

or the Processing 4 IDE's Sketchbook → Modes folder.  Restart the IDE.

---

## Architecture

```
RustMode/
├── Cargo.toml              ← the "processing" crate (minifb-backed software renderer)
├── src/
│   ├── lib.rs              ← Processing free-function API (fill, ellipse, mouseX, …)
│   └── java/               ← Java mode plugin source
│       ├── RustMode.java        Mode entry-point; loads native tree-sitter grammar
│       ├── RustEditor.java      Editor window; wires Run/Stop to the build pipeline
│       ├── RustBuild.java       Cargo build driver; manages the persistent cache project
│       ├── RustAnalyzer.java    Tree-sitter walk → RustAnalysis
│       ├── RustAnalysis.java    Immutable parse result value object
│       ├── RustEmitter.java     RustAnalysis → src/main.rs (scope-aware AST rewrite)
│       ├── RustLinter.java      Pre-build diagnostics (missing draw, bad names, …)
│       ├── RustInputHandler.java  Editor keystroke conveniences (auto-braces, indent)
│       └── SketchBinding.java   Record: top-level let binding
├── lib/
│   ├── linux-x86-64/       ← libtree-sitter-rust.so  (you build this)
│   ├── macos-arm64/        ← libtree-sitter-rust.dylib
│   ├── macos-x86-64/       ← libtree-sitter-rust.dylib
│   └── windows-x86-64/     ← tree-sitter-rust.dll
├── mode/                   ← RustMode.jar lands here after build
├── keywords.txt            ← IDE syntax colouring
├── mode.properties         ← Mode registration (class name, version, …)
├── examples/
│   ├── Bouncing_Ball/
│   ├── Interactive_Colors/
│   └── Clock/
└── scripts/
    ├── link-sources.sh     ← Symlink java/ into processing4 source tree
    └── rebuild-engine.sh   ← Propagate lib.rs changes to all existing sketch caches
```

### Sketch transform pipeline

```
sketch tab(s)  ──concat──▶  RustAnalyzer  ──▶  RustAnalysis
                                                     │
                                               RustEmitter
                                                     │
                                               src/main.rs
                                                     │
                                            cargo build [--release]
                                                     │
                                               target/.../sketch
                                                     │
                                              launched by IDE
```

### Sketch-variable hoisting

Top-level `let` / `let mut` bindings become fields of a generated `SketchState`
struct that is threaded through every lifecycle function as `&mut SketchState`:

```rust
// Sketch source:
let mut x: f32 = 300.0;

fn draw() {
    x += 1.0;      // ← rewritten to s.x by the emitter
}

// Generated main.rs:
struct SketchState { x: f32 }
fn draw(s: &mut SketchState) { s.x += 1.0; }
fn main() { processing::App::new(SketchState::new(), setup, draw).run(); }
```

The rewrite is scope-aware (uses Tree-sitter, not regex): inner `let` bindings
that shadow a sketch variable and closure parameters are tracked per-block and
never rewritten.

### Rendering pipeline

`src/lib.rs` is a fully self-contained software rasterizer.  The `processing`
crate maintains a packed `u32` pixel buffer (`0x00RRGGBB`) and blits it to a
native window via `minifb` every frame.  No GPU, WebGPU, or external native
library is needed; `cargo build` is the only prerequisite beyond Rust stable.

Key modules inside `lib.rs`:

| Module | Responsibility |
|---|---|
| `core` | Color, constants (`PI`, `TWO_PI`, …), math helpers |
| `render` | Software rasterizer — Bresenham lines, scan-line triangles, ellipses, affine transform stack, style stack |
| `glfw` (native) | minifb event loop, mouse/keyboard event dispatch, frame timing |
| `wasm` | Stub (future browser export path, gated by `--features wasm`) |

### Two renderers

The menu left of the mode selector in the toolbar picks where sketches draw:

- **Software** (default): the rasterizer above. Small build, works anywhere.
- **libprocessing**: the Processing Foundation's [libprocessing](https://github.com/processing/libprocessing)
  (Bevy + wgpu, on the GPU). The engine is built with the `libprocessing` cargo
  feature and the sketch runs with `PROCESSING_RENDERER=libprocessing`; every
  drawing call is handed to `src/lp.rs`, which records it as a libprocessing
  draw command. The first build compiles Bevy and takes several minutes.

What libprocessing doesn't have yet is filled in by `lp.rs`: `get()`,
`loadPixels()`, `filter()` and `saveFrame()` read the canvas back and work on
it here; images made or edited in code are uploaded when drawn; layers
(`create_graphics`) are offscreen libprocessing graphics; SVG files become
libprocessing shapes; 3D lines, points and triangles are small meshes; lights
are remade only when a frame asks for different ones.

Needs Rust stable (libprocessing uses the 2024 edition) and GLFW 3.4
(`pacman -S glfw` on Arch). There is no sound for now.

Known differences on libprocessing, for now:

- Colors blend in linear light, so half-transparent colors and `ADD` come out
  a little different from the software renderer.
- `shear_x` / `shear_y` turn shapes instead of slanting them (libprocessing
  keeps transforms as position, rotation and scale).
- Edges are always smooth; `no_smooth()` does nothing.
- Lights apply to the whole frame: `no_lights()` makes the shapes after it
  flat again, but the lights stay for the shapes drawn before.

Sketches build against your own libprocessing checkout when there is one:
`LIBPROCESSING_DIR`, the `rust.libprocessing` preference, or
`~/sketchbook/libprocessing`. Otherwise against the commit pinned in
`Cargo.toml`. The build status line says which one it used.

---

## Sketch syntax

Sketches are valid Rust files.  The Processing conventions:

| Feature | Rust syntax |
|---|---|
| Sketch state | top-level `let [mut] name [: Type] = init;` |
| Setup | `fn setup() { … }` |
| Draw loop | `fn draw() { … }` |
| Mouse events | `fn mouse_pressed() { … }` |
| Key events | `fn key_pressed() { … }` |
| Both naming styles | `mouseX()` ≡ `mouse_x()`, `noFill()` ≡ `no_fill()`, … |
| File extension | `.pde` (default) or `.prs` |

---

## Contributing

The whole mode is ~9 Java files + one Rust crate.  Good first areas:

- Add missing Processing API calls to `src/lib.rs` (e.g. `bezier`, `image`, `loadImage`, text metrics).
- Wire `RustLinter.LintProblem` results to the IDE gutter (currently logged + status bar only).
- WASM export path (feature flag `wasm` exists, needs a canvas-based pixel blit in the stub module).
- Windows testing (minifb supports Windows; the CI only runs on ubuntu-latest).
- GPU-accelerated backend: swap `mod glfw` for a `wgpu`/`winit` backend — the pixel-buffer
  contract (`PIXEL_BUF`, `CANVAS_W/H`) and the public API remain unchanged.

After modifying `src/lib.rs`, run `./scripts/rebuild-engine.sh` to propagate the
updated crate to all existing sketch build caches.
