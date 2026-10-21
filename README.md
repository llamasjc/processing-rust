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

### Fast builds: the precompiled engine

Like CppMode's precompiled `Processing.o`, RustMode can ship the engine
already compiled, per platform and renderer, in `cache/<platform>/<renderer>/`
(`cpu` or `libprocessing`). With a cache there, a sketch is one `rustc` call
against it: about a second for CPU sketches, and no Bevy build at all for
libprocessing ones.

```sh
scripts/build-cache.py              # this machine, CPU renderer
scripts/build-cache.py all          # CPU and libprocessing (the second takes a while, once)
scripts/create-release.py 0.3.0     # every platform via the build-cache workflow, then a zip
```

The cache is tied to the exact Rust version in `rust-toolchain.toml` and to
the engine sources it was built from. RustMode fetches that Rust version with
rustup the first time; if the engine changed since, or something else
doesn't fit, it quietly goes back to the normal cargo build. Exports always
use cargo.

### Two renderers

The menu left of the mode selector in the toolbar picks where sketches draw:

- **CPU** (default): the rasterizer above, every pixel drawn on the CPU. Small build, works anywhere.
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

- Lights apply to the whole frame: `no_lights()` makes the shapes after it
  flat again, but the lights stay for the shapes drawn before.
- `specular()` only turns the shine off (black) or on; the shine takes the
  light's color. Textures are slightly smoothed.

The sketch draws on a canvas of exactly its own size, which is then shown in
the top left of the window. A window manager that won't make a window that
small (i3 stops at about 75 pixels) or a window resized by hand shows the
sketch at its own size, with black around it. `get()` and `saveFrame()` always
see just the sketch.

Textures need libprocessing at the pinned commit or newer. Built against an
older checkout of your own, textured shapes draw untextured and the build says
so; older checkouts also tone-map colors, which makes everything a little dull.
To build against the pinned commit without touching your checkout, set
`LIBPROCESSING_DIR` to a folder that doesn't exist.

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

### Shaders

`load_shader("blur.glsl")` (or `shader_from_source(text)`) reads a
Processing filter shader: GLSL with `uniform sampler2D texture`, `varying vec4
vertTexCoord`, `gl_FragColor`, `gl_FragCoord`, `texOffset` and `resolution`.
`shader.set("name", value)` sets a uniform (`set("v", x, y)` with up to four
numbers), and `filter(shader)` runs it over the canvas. On the default
renderer the shader runs on the CPU (an interpreter in `src/glsl.rs`, on every
core); on libprocessing it is translated to WGSL and runs on the GPU. Both give
the same picture.

`shader(s)` colors the shapes drawn after it with the shader (its fragment
part; `vertColor` is the shape's fill), until `reset_shader()`. On the default
renderer and on libprocessing that is every filled shape, 2D and 3D. (On
libprocessing a shape drawn with a shader is rendered as soon as it's drawn,
so each keeps its own fill.)

`load_shader_vf("frag.glsl", "vert.glsl")` (or `shader_from_source_vf(frag,
vert)`) adds a vertex shader. It gets Processing's attributes (`position`,
`color`, `normal`, `texCoord`) and matrices (`transform`, `modelview`,
`projection`, `normalMatrix`, `texMatrix`), writes `gl_Position`, and its
varyings reach the fragment shader, blended across each triangle. While one is
active every shape goes through the 3D path, as in P3D. On the default renderer
it also gets `lightCount`, `lightPosition[8]` and the other light uniforms. On
libprocessing it needs the lp-fork (its `processing::vertex` module); there
`color` is the shape's fill and varyings are floats or vecs. Vertex shaders
can read images (`sampler2D` uniforms, set with `shader.set("height", img)`)
on both. See `examples/VertexWave`.

On the default renderer shaders are turned into Rust closures once when they
load (constants worked out ahead), so a pixel costs a chain of direct calls.

### Clipping, push / pop, hints, links

`clip(x, y, w, h)` (placed like an image, following translate and scale)
keeps drawing inside a rectangle until `no_clip()` or the end of `draw()`, on
both renderers. `push()` / `pop()` save and restore the transform and style
together. `hint(DISABLE_DEPTH_TEST)` lets 3D shapes draw over each other in
order on the default renderer; the other hints are accepted and do nothing.
`link(url)` opens the browser, `launch(&["file.pdf"])` opens a file or runs
a program, `save_stream(target, source)` copies a file or downloads an
address (with curl). `load_json_object`, `load_json_array`,
`parse_json_object`, `parse_json_array` and the save ones are Processing's
names for the JSON functions.

Not there yet: `begin_record` / `begin_raw` (PDF and SVG output), custom
vertex attributes (`attrib`), and `window_ratio`.

Supported: float, int, bool, vec2-4, mat2-4, functions (with `out` and `inout`
parameters), if / for / while / do, `discard`, `const` and `#define`, the
usual built-in functions, and images as `sampler2D` uniforms
(`shader.set("other", img)`), and arrays: `uniform float kernel[9]` (set with
`shader.set("kernel", [..9 numbers..])`), `const vec2 offsets[9] =
vec2[9](...)`, local arrays, indexed with any expression. An array can't be
passed around whole yet (only its items). Not yet: vertex shaders. A mistake
in a shader is reported with its line when it's loaded;
`PROCESSING_SHADER_DEBUG=1` prints the WGSL libprocessing gets.

### Lights, materials and the rest of the API

- `spot_light`, `light_falloff`, `light_specular`, and the material calls
  `ambient`, `specular`, `emissive`, `shininess`, with Processing's lighting
  model. With lights on, flat shapes are lit too, as in P3D.
- `bezier_detail`, `curve_detail`, `curve_tightness` (and `curve_point` /
  `curve_tangent` follow the tightness), `shape_mode`, `copy`, `blend`,
  `save`, `print_matrix`, `window_move`, `window_resize`, `mouse_clicked()`.
- `load_xml` / `parse_xml` / `save_xml` and the `Xml` element, `create_writer`,
  `create_reader`, `select_input` / `select_output` / `select_folder` (these
  wait for the choice and give back the path; zenity or kdialog on Linux),
  `hex`, `binary`, `unhex`, `unbinary`, `match_re` / `match_all` (regular
  expressions), `print_array`, `thread`.
- `pixel_density` stays at 1 for now; `text_mode` is accepted.

### Textures, cameras, models and video

- `texture(img)` between `begin_shape()` and `end_shape()` paints the shape
  with an image; give each corner a spot on it with `vertex(x, y, u, v)` or
  `vertex(x, y, z, u, v)`. `texture_mode(NORMAL)` takes u and v from 0 to 1
  (the default, `IMAGE`, takes pixels); `texture_wrap(REPEAT)` tiles it.
- `vertex(x, y, z)` makes shapes in space. With a camera, a projection or a
  turn in depth in play, `rect`, `ellipse`, `triangle`, `quad`, `line`,
  `point` and `image` go through the 3D renderer too, as in P3D.
- `begin_camera()` / `end_camera()`, `frustum(...)`, `ortho(l, r, b, t[, n, f])`,
  `print_camera()`, `print_projection()`, `rotate(angle, x, y, z)`,
  `model_x/y/z`, `screen_z` and `apply_matrix` with 16 numbers.
- `load_shape("model.obj")` reads OBJ models with their materials (`Kd`
  colors and `map_Kd` pictures). `create_shape_box` and `create_shape_sphere`
  make 3D shapes in code; PShape takes 3D vertices, textures and contours too.
- `Movie::new("clip.mp4")` and `Capture::new(640, 480)` play video files and
  cameras through ffmpeg (it has to be installed): `loop_()`, `play()`,
  `pause()`, `jump(t)`, `speed(r)`, `available()`, `read()`, then
  `image(movie, x, y)` or `texture(movie)`. No sound.

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
