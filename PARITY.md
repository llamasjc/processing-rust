# Processing API parity

How RustMode and LuaMode compare with Processing 4's Java API. Checked by
pulling every public method out of Processing's source (PApplet, PGraphics,
PImage, PShape, PVector, PFont, PMatrix2D/3D, Table, TableRow, XML,
JSONObject, JSONArray, the list and dict classes, PConstants) and looking
for it in each mode: RustMode by function and method name (snake_case),
LuaMode by asking a live sketch for each global and each object's methods.

## Covered

Everything a sketch calls, on both modes and both renderers, with these
names in RustMode:

- Images and fonts are ids (u64); their methods come from `PImageExt` and
  `PFontExt` (`img.resize(w, h)`, `img.mask(m)`, `font.ascent()`...).
- `TableRow` edits the table it came from: `table.get_row_mut(i)` /
  `table.add_row_handle()`; `table.get_row(i)` is read only.
- Java overloads that Rust can't have get their own names, like
  `get_subset_n`, `create_vector3`, `set_matrix_2d`.
- `surface` is a function, `surface().set_title("x")`, so sketches can still
  have their own variables called `surface`.

In LuaMode images and fonts stay numbers but answer Processing's methods
(`img:resize(w, h)`, `img.width`, `font:getSize()`). JSON values are plain
tables that also answer Processing's methods (`o:getInt("id")`,
`a:getString(0)`, `a:size()`): the methods count array items from 0, `a[i]`
from 1. `LongList`, `DoubleList`, `LongDict` and `DoubleDict` are the Int and
Float ones, since Lua numbers are already 64-bit.

Window events: `focusGained`, `focusLost`, `mouseEntered`, `mouseExited`,
`windowMoved`, `windowResized` are called on both renderers (libprocessing
needs the lp-fork).

## Left out on purpose

- Java internals: iterators (`iterator`, `hasNext`, `keyIterator`...),
  `hashCode`, `compare`, `write(PrintWriter)`, `getNative`, `init`,
  `checkAlpha`, the `isModified` family, `typedParse`, `parseInto`,
  `setTableType`, surface and PGL methods, `registerMethod`, `handleDraw`,
  `postEvent` and the rest of PApplet's plumbing.
- `X`, `Y`, `Z`: as constants they'd break sketches that use those names
  for variables.
- `mask()` on the canvas and `choice()`: deprecated in Processing.
- `PShape` tessellation (`getTessellation` and friends) and `setPath`.
- `PFont.getGlyph` and `save`: Processing's own glyph bitmaps and .vlw
  files. `getShape(c)` works (the outline, holes as contours).
- `JSONArray` value iterators (use the `get_*_array` ones).
- `surface.setVisible`, `setAlwaysOnTop` and `setUndecorated`: the window is
  always shown, decorated and not pinned.

## Partial

- `hint()`: `DISABLE_DEPTH_TEST` / `ENABLE_DEPTH_TEST` work on both renderers
  (libprocessing needs the lp-fork). The other hints tune Processing's
  OpenGL renderers (stroke sorting, mipmaps, key repeat...) and do nothing
  here, as in Processing's default renderer.
- `begin_record`: shapes (also ones made with `vertex(x, y, z)`, drawn where
  the camera shows them), text with its font (embedded), images (embedded)
  and boxes / spheres (as lit triangles) are written. Shader effects aren't,
  as in Processing: a PDF or SVG can't run a shader.

Cursors: `cursor(img)` / `cursor(img, x, y)` show a picture (RustMode:
`cursor_image(img)` / `cursor_image_at(img, x, y)`; libprocessing needs the
lp-fork). `WAIT` is a drawn hourglass where the window system has no busy
cursor, and `MOVE` drawn arrows on libprocessing. Images are numbered from
65536, so in LuaMode an image is never mistaken for a cursor kind.
