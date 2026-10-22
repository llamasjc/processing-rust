//! The rest of Processing's constants. Values are Processing's own (from
//! PConstants), except where RustMode already numbers that family its own way
//! (key codes), so they work with the functions that take them.

// image formats (RGB is in lib.rs)
pub const ARGB: u32 = 2;
pub const ALPHA: u32 = 4;

// shape kinds, for PShape::get_kind and createShape
pub const POINT: u8 = 2;
pub const LINE: u8 = 4;
pub const TRIANGLE: u8 = 8;
pub const QUAD: u8 = 16;
pub const RECT: u8 = 30;
pub const ELLIPSE: u8 = 31;
pub const ARC: u8 = 32;
pub const SPHERE: u8 = 40;
pub const BOX: u8 = 41;
/// Processing's numbers for these; begin_shape_kind draws kinds it does not
/// know as a plain shape.
pub const LINE_STRIP: u8 = 50;
pub const LINE_LOOP: u8 = 51;

// PShape families
pub const GROUP: u8 = 0;
pub const PRIMITIVE: u8 = 101;
pub const PATH: u8 = 102;
pub const GEOMETRY: u8 = 103;

// vertex codes
pub const VERTEX: u32 = 0;
pub const BEZIER_VERTEX: u32 = 1;
pub const QUADRATIC_VERTEX: u32 = 2;
pub const CURVE_VERTEX: u32 = 3;
pub const BREAK: u32 = 4;

// cursor kinds (java.awt.Cursor numbers, as Processing uses)
pub const ARROW: u32 = 0;
pub const CROSS: u32 = 1;
pub const TEXT: u32 = 2;
pub const WAIT: u32 = 3;
pub const HAND: u32 = 12;
pub const MOVE: u32 = 13;

// smoothing for resize / textures
pub const NEAREST_NEIGHBOR: u32 = 0;
pub const BILINEAR: u32 = 1;
pub const BICUBIC: u32 = 2;

// more blend modes, for blend() and PImage blend()
pub const OVERLAY: u32 = 1 << 9;
pub const HARD_LIGHT: u32 = 1 << 10;
pub const SOFT_LIGHT: u32 = 1 << 11;
pub const DODGE: u32 = 1 << 12;
pub const BURN: u32 = 1 << 13;

// numbers
pub const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0;
pub const RAD_TO_DEG: f32 = 180.0 / std::f32::consts::PI;
pub const THIRD_PI: f32 = std::f32::consts::FRAC_PI_3;
pub const EPSILON: f32 = 0.0001;
pub const MAX_FLOAT: f32 = f32::MAX;
pub const MIN_FLOAT: f32 = -f32::MAX;
pub const MAX_INT: i32 = i32::MAX;
pub const MIN_INT: i32 = i32::MIN;

// file formats
pub const TIFF: u32 = 0;
pub const TARGA: u32 = 1;
pub const JPEG: u32 = 2;
pub const GIF: u32 = 3;

// orientation
pub const PORTRAIT: u32 = 1;
pub const LANDSCAPE: u32 = 2;

// platforms, see platform()
pub const OTHER: u32 = 0;
pub const WINDOWS: u32 = 1;
pub const MACOS: u32 = 2;
pub const MACOSX: u32 = 2;
pub const LINUX: u32 = 3;

/// The system the sketch is running on: WINDOWS, MACOS, LINUX or OTHER.
pub fn platform() -> u32 {
    if cfg!(target_os = "windows") { WINDOWS }
    else if cfg!(target_os = "macos") { MACOS }
    else if cfg!(target_os = "linux") { LINUX }
    else { OTHER }
}

// matrices, projections and lights
pub const PROJECTION: u32 = 0;
pub const MODELVIEW: u32 = 1;
pub const ORTHOGRAPHIC: u32 = 2;
pub const PERSPECTIVE: u32 = 3;
pub const AMBIENT: u32 = 0;
pub const DIRECTIONAL: u32 = 1;
pub const SPOT: u32 = 3;

/// ellipse_mode(DIAMETER) is the same as CENTER.
pub const DIAMETER: u32 = crate::CENTER;

// more hint() values; accepted and ignored
pub const ENABLE_NATIVE_FONTS: i32 = 1;
pub const DISABLE_NATIVE_FONTS: i32 = -1;
pub const ENABLE_BUFFER_READING: i32 = 10;
pub const DISABLE_BUFFER_READING: i32 = -10;
pub const DISABLE_KEY_REPEAT: i32 = 11;
pub const ENABLE_KEY_REPEAT: i32 = -11;

// X, Y and Z are left out: as constants in scope they would turn a sketch's
// own `let X = ...` into a pattern match and break it.

/// The characters split_tokens() treats as space by default.
pub const WHITESPACE: &str = " \t\n\r\x0C\u{00A0}";

// message levels
pub const CHATTER: u32 = 0;
pub const COMPLAINT: u32 = 1;
pub const PROBLEM: u32 = 2;

/// Key codes in RustMode's own numbering, like ENTER and ESCAPE.
pub const ESC: u32 = crate::ESCAPE;
pub const RETURN: u32 = crate::ENTER;
