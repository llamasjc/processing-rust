//! Files, JSON and tables: loadStrings, saveStrings, loadJSONObject,
//! loadTable and friends. No extra crates; everything here is plain std.
//! Relative paths are tried as given, then inside data/, like images.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn find(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_file() || p.is_absolute() { return p.to_path_buf(); }
    let d = Path::new("data").join(path);
    if d.is_file() { d } else { p.to_path_buf() }
}

fn warn(what: &str, path: &str, e: impl std::fmt::Display) {
    eprintln!("{what}: could not use \"{path}\" ({e})");
}

/// loadStrings("words.txt"): every line of the file. Empty if it can't be read.
pub fn load_strings(path: &str) -> Vec<String> {
    match std::fs::read_to_string(find(path)) {
        Ok(s) => s.lines().map(|l| l.to_string()).collect(),
        Err(e) => { warn("loadStrings", path, e); Vec::new() }
    }
}
/// The whole file as one string.
pub fn load_text(path: &str) -> String {
    std::fs::read_to_string(find(path)).unwrap_or_else(|e| { warn("loadText", path, e); String::new() })
}
pub fn load_bytes(path: &str) -> Vec<u8> {
    std::fs::read(find(path)).unwrap_or_else(|e| { warn("loadBytes", path, e); Vec::new() })
}

fn write(path: &str, bytes: &[u8]) -> bool {
    let p = Path::new(path);
    if let Some(dir) = p.parent() { if !dir.as_os_str().is_empty() { let _ = std::fs::create_dir_all(dir); } }
    match std::fs::write(p, bytes) { Ok(_) => true, Err(e) => { warn("save", path, e); false } }
}
/// saveStrings("out.txt", &lines): one line per entry. Folders are made as needed.
pub fn save_strings<S: AsRef<str>>(path: &str, lines: &[S]) -> bool {
    let mut s = String::new();
    for l in lines { s.push_str(l.as_ref()); s.push('\n'); }
    write(path, s.as_bytes())
}
pub fn save_text(path: &str, text: &str) -> bool { write(path, text.as_bytes()) }
pub fn save_bytes(path: &str, bytes: &[u8]) -> bool { write(path, bytes) }

/// splitTokens: split on any of the given characters, dropping empty pieces.
pub fn split_tokens(s: &str, delims: &str) -> Vec<String> {
    s.split(|c| delims.contains(c)).filter(|t| !t.is_empty()).map(String::from).collect()
}
/// split(s, ","): split on a string, keeping empty pieces.
pub fn split(s: &str, delim: &str) -> Vec<String> { s.split(delim).map(String::from).collect() }
pub fn join<S: AsRef<str>>(parts: &[S], sep: &str) -> String {
    parts.iter().map(|p| p.as_ref()).collect::<Vec<_>>().join(sep)
}
pub fn trim(s: &str) -> String { s.trim().to_string() }

// ---------------------------------------------------------------- JSON

/// A JSON value. Objects keep their keys sorted, so saved files are stable.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Json {
    #[default] Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

static NULL: Json = Json::Null;

/// A place in a Json value: a key ("name") for objects, an index (0) for arrays.
pub trait JsonKey {
    fn pick<'a>(&self, j: &'a Json) -> &'a Json;
    fn put(&self, j: &mut Json, v: Json);
}
impl JsonKey for &str {
    fn pick<'a>(&self, j: &'a Json) -> &'a Json { j.get(self) }
    fn put(&self, j: &mut Json, v: Json) { j.set(self, v); }
}
impl JsonKey for String {
    fn pick<'a>(&self, j: &'a Json) -> &'a Json { j.get(self) }
    fn put(&self, j: &mut Json, v: Json) { j.set(self, v); }
}
impl JsonKey for &String {
    fn pick<'a>(&self, j: &'a Json) -> &'a Json { j.get(self) }
    fn put(&self, j: &mut Json, v: Json) { j.set(self, v); }
}
impl JsonKey for usize {
    fn pick<'a>(&self, j: &'a Json) -> &'a Json { j.at(*self) }
    fn put(&self, j: &mut Json, v: Json) {
        if !matches!(j, Json::Arr(_)) { *j = Json::array(); }
        if let Json::Arr(a) = j { if a.len() <= *self { a.resize(*self + 1, Json::Null); } a[*self] = v; }
    }
}
impl JsonKey for i32 {
    fn pick<'a>(&self, j: &'a Json) -> &'a Json { if *self < 0 { &NULL } else { j.at(*self as usize) } }
    fn put(&self, j: &mut Json, v: Json) { if *self >= 0 { (*self as usize).put(j, v) } }
}

impl Json {
    pub fn object() -> Json { Json::Obj(BTreeMap::new()) }
    pub fn array() -> Json { Json::Arr(Vec::new()) }

    /// obj.get("key"): the value, or Null if it's missing or this isn't an object.
    pub fn get(&self, key: &str) -> &Json {
        match self { Json::Obj(m) => m.get(key).unwrap_or(&NULL), _ => &NULL }
    }
    /// arr.at(i): the value, or Null if out of range.
    pub fn at(&self, i: usize) -> &Json {
        match self { Json::Arr(v) => v.get(i).unwrap_or(&NULL), _ => &NULL }
    }
    pub fn has(&self, key: &str) -> bool { matches!(self, Json::Obj(m) if m.contains_key(key)) }
    pub fn is_null(&self) -> bool { matches!(self, Json::Null) }
    pub fn len(&self) -> usize {
        match self { Json::Arr(v) => v.len(), Json::Obj(m) => m.len(), _ => 0 }
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn keys(&self) -> Vec<String> {
        match self { Json::Obj(m) => m.keys().cloned().collect(), _ => Vec::new() }
    }
    pub fn items(&self) -> &[Json] { match self { Json::Arr(v) => v, _ => &[] } }

    pub fn as_f64(&self) -> f64 {
        match self { Json::Num(n) => *n, Json::Bool(b) => *b as i32 as f64,
                     Json::Str(s) => s.trim().parse().unwrap_or(0.0), _ => 0.0 }
    }
    pub fn as_f32(&self) -> f32 { self.as_f64() as f32 }
    pub fn as_i32(&self) -> i32 { self.as_f64() as i32 }
    pub fn as_bool(&self) -> bool {
        match self { Json::Bool(b) => *b, Json::Num(n) => *n != 0.0, Json::Str(s) => s == "true", _ => false }
    }
    pub fn as_str(&self) -> &str { match self { Json::Str(s) => s, _ => "" } }
    pub fn as_string(&self) -> String {
        match self { Json::Str(s) => s.clone(), Json::Null => String::new(), other => other.to_string() }
    }

    // Processing-style getters
    pub fn get_int(&self, k: impl JsonKey) -> i32 { k.pick(self).as_i32() }
    pub fn get_float(&self, k: impl JsonKey) -> f32 { k.pick(self).as_f32() }
    pub fn get_string(&self, k: impl JsonKey) -> String { k.pick(self).as_string() }
    pub fn get_boolean(&self, k: impl JsonKey) -> bool { k.pick(self).as_bool() }
    pub fn get_double(&self, k: impl JsonKey) -> f64 { k.pick(self).as_f64() }
    pub fn get_long(&self, k: impl JsonKey) -> i64 {
        match k.pick(self) { Json::Num(n) => *n as i64, Json::Str(s) => s.trim().parse().unwrap_or_else(|_| s.trim().parse::<f64>().unwrap_or(0.0) as i64), o => o.as_f64() as i64 }
    }
    /// The value, or the default if the key (or index) isn't there.
    pub fn get_int_or(&self, k: impl JsonKey, d: i32) -> i32 { let v = k.pick(self); if v.is_null() { d } else { v.as_i32() } }
    pub fn get_float_or(&self, k: impl JsonKey, d: f32) -> f32 { let v = k.pick(self); if v.is_null() { d } else { v.as_f32() } }
    pub fn get_double_or(&self, k: impl JsonKey, d: f64) -> f64 { let v = k.pick(self); if v.is_null() { d } else { v.as_f64() } }
    pub fn get_long_or(&self, k: impl JsonKey, d: i64) -> i64 { if k.pick(self).is_null() { d } else { self.get_long(k) } }
    pub fn get_string_or(&self, k: impl JsonKey, d: &str) -> String { let v = k.pick(self); if v.is_null() { d.to_string() } else { v.as_string() } }
    pub fn get_boolean_or(&self, k: impl JsonKey, d: bool) -> bool { let v = k.pick(self); if v.is_null() { d } else { v.as_bool() } }
    /// The object stored under a key (or at an index). Null if it is missing.
    pub fn get_json_object(&self, k: impl JsonKey) -> &Json { k.pick(self) }
    /// The array stored under a key (or at an index). Null if it is missing.
    pub fn get_json_array(&self, k: impl JsonKey) -> &Json { k.pick(self) }
    pub fn has_key(&self, key: &str) -> bool { self.has(key) }
    /// True when the key (or index) is missing or holds null.
    pub fn is_null_at(&self, k: impl JsonKey) -> bool { k.pick(self).is_null() }
    /// Number of keys or items.
    pub fn size(&self) -> usize { self.len() }

    /// Sets by key on objects, by index on arrays (arrays grow with nulls).
    pub fn put(&mut self, k: impl JsonKey, v: impl Into<Json>) -> &mut Self { k.put(self, v.into()); self }
    pub fn set_string(&mut self, k: impl JsonKey, v: &str) -> &mut Self { self.put(k, v) }
    pub fn set_int(&mut self, k: impl JsonKey, v: i32) -> &mut Self { self.put(k, v) }
    pub fn set_float(&mut self, k: impl JsonKey, v: f32) -> &mut Self { self.put(k, v) }
    pub fn set_double(&mut self, k: impl JsonKey, v: f64) -> &mut Self { self.put(k, v) }
    pub fn set_long(&mut self, k: impl JsonKey, v: i64) -> &mut Self { self.put(k, v) }
    pub fn set_boolean(&mut self, k: impl JsonKey, v: bool) -> &mut Self { self.put(k, v) }
    pub fn set_json_object(&mut self, k: impl JsonKey, v: Json) -> &mut Self { self.put(k, v) }
    pub fn set_json_array(&mut self, k: impl JsonKey, v: Json) -> &mut Self { self.put(k, v) }
    /// Adds to the end of an array (a Null becomes an array first).
    pub fn append(&mut self, v: impl Into<Json>) -> &mut Self { self.push(v) }
    /// Removes an array item and returns it.
    pub fn remove_at(&mut self, i: usize) -> Json {
        match self { Json::Arr(a) if i < a.len() => a.remove(i), _ => Json::Null }
    }

    /// An array's items as numbers, text or booleans.
    pub fn get_int_array(&self) -> Vec<i32> { self.items().iter().map(|j| j.as_i32()).collect() }
    pub fn get_long_array(&self) -> Vec<i64> { (0..self.len()).map(|i| self.get_long(i)).collect() }
    pub fn get_float_array(&self) -> Vec<f32> { self.items().iter().map(|j| j.as_f32()).collect() }
    #[inline] pub fn toDoubleArray(&self) -> Vec<f64> { self.get_double_array() }
    #[inline] pub fn toLongArray(&self) -> Vec<i64> { self.get_long_array() }
    pub fn get_double_array(&self) -> Vec<f64> { self.items().iter().map(|j| j.as_f64()).collect() }
    pub fn get_string_array(&self) -> Vec<String> { self.items().iter().map(|j| j.as_string()).collect() }
    pub fn get_boolean_array(&self) -> Vec<bool> { self.items().iter().map(|j| j.as_bool()).collect() }
    pub fn to_int_list(&self) -> crate::IntList { self.get_int_array().into() }
    pub fn to_float_list(&self) -> crate::FloatList { self.get_float_array().into() }
    pub fn to_string_list(&self) -> crate::StringList { self.get_string_array().into() }
    pub fn to_long_list(&self) -> crate::LongList { self.get_long_array().into() }
    pub fn to_double_list(&self) -> crate::DoubleList { self.get_double_array().into() }
    #[inline] pub fn to_boolean_array(&self) -> Vec<bool> { self.get_boolean_array() }
    #[inline] pub fn toLongList(&self) -> crate::LongList { self.to_long_list() }
    #[inline] pub fn toDoubleList(&self) -> crate::DoubleList { self.to_double_list() }
    #[inline] pub fn toBooleanArray(&self) -> Vec<bool> { self.get_boolean_array() }
    pub fn get_long_list(&self, k: impl JsonKey) -> crate::LongList { k.pick(self).to_long_list() }
    pub fn get_double_list(&self, k: impl JsonKey) -> crate::DoubleList { k.pick(self).to_double_list() }
    #[inline] pub fn getLongList(&self, k: impl JsonKey) -> crate::LongList { self.get_long_list(k) }
    #[inline] pub fn getDoubleList(&self, k: impl JsonKey) -> crate::DoubleList { self.get_double_list(k) }
    /// The array under a key as a list.
    pub fn get_int_list(&self, k: impl JsonKey) -> crate::IntList { k.pick(self).to_int_list() }
    pub fn get_float_list(&self, k: impl JsonKey) -> crate::FloatList { k.pick(self).to_float_list() }
    pub fn get_string_list(&self, k: impl JsonKey) -> crate::StringList { k.pick(self).to_string_list() }
    /// An array's items joined as JSON text ("a" keeps its quotes).
    pub fn join(&self, sep: &str) -> String { self.items().iter().map(|j| j.to_string()).collect::<Vec<_>>().join(sep) }
    /// Writes the value to a file, two spaces per level.
    pub fn save(&self, path: &str) -> bool { save_json(path, self) }

    #[inline] pub fn getInt(&self, k: impl JsonKey) -> i32 { self.get_int(k) }
    #[inline] pub fn getFloat(&self, k: impl JsonKey) -> f32 { self.get_float(k) }
    #[inline] pub fn getString(&self, k: impl JsonKey) -> String { self.get_string(k) }
    #[inline] pub fn getBoolean(&self, k: impl JsonKey) -> bool { self.get_boolean(k) }
    #[inline] pub fn getDouble(&self, k: impl JsonKey) -> f64 { self.get_double(k) }
    #[inline] pub fn getLong(&self, k: impl JsonKey) -> i64 { self.get_long(k) }
    #[inline] pub fn getJSONObject(&self, k: impl JsonKey) -> &Json { self.get_json_object(k) }
    #[inline] pub fn getJSONArray(&self, k: impl JsonKey) -> &Json { self.get_json_array(k) }
    #[inline] pub fn hasKey(&self, key: &str) -> bool { self.has_key(key) }
    #[inline] pub fn isNull(&self, k: impl JsonKey) -> bool { self.is_null_at(k) }
    #[inline] pub fn setString(&mut self, k: impl JsonKey, v: &str) -> &mut Self { self.set_string(k, v) }
    #[inline] pub fn setInt(&mut self, k: impl JsonKey, v: i32) -> &mut Self { self.set_int(k, v) }
    #[inline] pub fn setFloat(&mut self, k: impl JsonKey, v: f32) -> &mut Self { self.set_float(k, v) }
    #[inline] pub fn setDouble(&mut self, k: impl JsonKey, v: f64) -> &mut Self { self.set_double(k, v) }
    #[inline] pub fn setLong(&mut self, k: impl JsonKey, v: i64) -> &mut Self { self.set_long(k, v) }
    #[inline] pub fn setBoolean(&mut self, k: impl JsonKey, v: bool) -> &mut Self { self.set_boolean(k, v) }
    #[inline] pub fn setJSONObject(&mut self, k: impl JsonKey, v: Json) -> &mut Self { self.set_json_object(k, v) }
    #[inline] pub fn setJSONArray(&mut self, k: impl JsonKey, v: Json) -> &mut Self { self.set_json_array(k, v) }
    #[inline] pub fn getIntArray(&self) -> Vec<i32> { self.get_int_array() }
    #[inline] pub fn getLongArray(&self) -> Vec<i64> { self.get_long_array() }
    #[inline] pub fn getFloatArray(&self) -> Vec<f32> { self.get_float_array() }
    #[inline] pub fn getDoubleArray(&self) -> Vec<f64> { self.get_double_array() }
    #[inline] pub fn getStringArray(&self) -> Vec<String> { self.get_string_array() }
    #[inline] pub fn getBooleanArray(&self) -> Vec<bool> { self.get_boolean_array() }
    #[inline] pub fn toIntArray(&self) -> Vec<i32> { self.get_int_array() }
    #[inline] pub fn toFloatArray(&self) -> Vec<f32> { self.get_float_array() }
    #[inline] pub fn toStringArray(&self) -> Vec<String> { self.get_string_array() }
    #[inline] pub fn toIntList(&self) -> crate::IntList { self.to_int_list() }
    #[inline] pub fn toFloatList(&self) -> crate::FloatList { self.to_float_list() }
    #[inline] pub fn toStringList(&self) -> crate::StringList { self.to_string_list() }
    #[inline] pub fn getIntList(&self, k: impl JsonKey) -> crate::IntList { self.get_int_list(k) }
    #[inline] pub fn getFloatList(&self, k: impl JsonKey) -> crate::FloatList { self.get_float_list(k) }
    #[inline] pub fn getStringList(&self, k: impl JsonKey) -> crate::StringList { self.get_string_list(k) }

    /// obj.set("key", 3) / obj.set("name", "Pep") / obj.set("list", Json::array())
    pub fn set(&mut self, key: &str, v: impl Into<Json>) -> &mut Self {
        if !matches!(self, Json::Obj(_)) { *self = Json::object(); }
        if let Json::Obj(m) = self { m.insert(key.to_string(), v.into()); }
        self
    }
    pub fn remove(&mut self, key: &str) -> Json {
        match self { Json::Obj(m) => m.remove(key).unwrap_or_default(), _ => Json::Null }
    }
    /// arr.push(v): turns a Null into an array first.
    pub fn push(&mut self, v: impl Into<Json>) -> &mut Self {
        if !matches!(self, Json::Arr(_)) { *self = Json::array(); }
        if let Json::Arr(a) = self { a.push(v.into()); }
        self
    }
    pub fn get_mut(&mut self, key: &str) -> &mut Json {
        if !matches!(self, Json::Obj(_)) { *self = Json::object(); }
        match self { Json::Obj(m) => m.entry(key.to_string()).or_default(), _ => unreachable!() }
    }

    /// Indented text, two spaces per level (what saveJSONObject writes).
    pub fn pretty(&self) -> String { let mut s = String::new(); self.dump(&mut s, Some(0), 2); s }
    /// format(2): text with that many spaces per level; format(-1) is one line.
    pub fn format(&self, indent: i32) -> String {
        let mut s = String::new();
        if indent < 0 { self.dump(&mut s, None, 0) } else { self.dump(&mut s, Some(0), indent as usize) }
        s
    }
    fn dump(&self, out: &mut String, indent: Option<usize>, w: usize) {
        let nl = |out: &mut String, n: usize| { out.push('\n'); for _ in 0..n * w { out.push(' '); } };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if !n.is_finite() { out.push_str("null") }
                else if n.fract() == 0.0 && n.abs() < 1e15 { let _ = write!(out, "{}", *n as i64); }
                else { let _ = write!(out, "{n}"); }
            }
            Json::Str(s) => quote(out, s),
            Json::Arr(v) => {
                if v.is_empty() { out.push_str("[]"); return; }
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    if let Some(d) = indent { nl(out, d + 1); }
                    x.dump(out, indent.map(|d| d + 1), w);
                }
                if let Some(d) = indent { nl(out, d); }
                out.push(']');
            }
            Json::Obj(m) => {
                if m.is_empty() { out.push_str("{}"); return; }
                out.push('{');
                for (i, (k, x)) in m.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    if let Some(d) = indent { nl(out, d + 1); }
                    quote(out, k);
                    out.push_str(if indent.is_some() { ": " } else { ":" });
                    x.dump(out, indent.map(|d| d + 1), w);
                }
                if let Some(d) = indent { nl(out, d); }
                out.push('}');
            }
        }
    }
}

fn quote(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => { let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Compact one-line text.
impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new(); self.dump(&mut s, None, 0); f.write_str(&s)
    }
}

impl std::ops::Index<&str> for Json { type Output = Json; fn index(&self, k: &str) -> &Json { self.get(k) } }
impl std::ops::Index<usize> for Json { type Output = Json; fn index(&self, i: usize) -> &Json { self.at(i) } }

macro_rules! from_num { ($($t:ty),*) => { $(impl From<$t> for Json { fn from(v: $t) -> Json { Json::Num(v as f64) } })* } }
from_num!(i32, i64, u32, u64, usize, f32, f64);
impl From<bool> for Json { fn from(v: bool) -> Json { Json::Bool(v) } }
impl From<&str> for Json { fn from(v: &str) -> Json { Json::Str(v.to_string()) } }
impl From<String> for Json { fn from(v: String) -> Json { Json::Str(v) } }
impl From<&String> for Json { fn from(v: &String) -> Json { Json::Str(v.clone()) } }
impl<T: Into<Json>> From<Vec<T>> for Json { fn from(v: Vec<T>) -> Json { Json::Arr(v.into_iter().map(Into::into).collect()) } }
impl<T: Into<Json>> From<Option<T>> for Json { fn from(v: Option<T>) -> Json { v.map(Into::into).unwrap_or_default() } }

/// parseJSON(text): Err says where it went wrong (line and column).
pub fn parse_json(text: &str) -> Result<Json, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i < p.s.len() { return Err(p.err("extra text after the value")); }
    Ok(v)
}

struct Parser<'a> { s: &'a [u8], i: usize, depth: usize }

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        let before = &self.s[..self.i.min(self.s.len())];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        let col = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
        format!("{what} at line {line}, column {col}")
    }
    fn ws(&mut self) {
        while self.i < self.s.len() {
            match self.s[self.i] {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                // Comments are not JSON, but hand-written config files have them.
                b'/' if self.s.get(self.i + 1) == Some(&b'/') => {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' { self.i += 1; }
                }
                _ => break,
            }
        }
    }
    fn eat(&mut self, b: u8) -> bool { if self.s.get(self.i) == Some(&b) { self.i += 1; true } else { false } }
    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) { self.i += word.len(); Ok(v) }
        else { Err(self.err("unexpected word")) }
    }
    fn value(&mut self) -> Result<Json, String> {
        if self.depth > 500 { return Err(self.err("nested too deep")); }
        match self.s.get(self.i) {
            None => Err(self.err("ran out of text")),
            Some(b'{') => {
                self.i += 1; self.depth += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        if self.s.get(self.i) != Some(&b'"') { return Err(self.err("expected a \"key\"")); }
                        let k = self.string()?;
                        self.ws();
                        if !self.eat(b':') { return Err(self.err("expected ':'")); }
                        self.ws();
                        let v = self.value()?;
                        m.insert(k, v);
                        self.ws();
                        if self.eat(b',') { self.ws(); if self.eat(b'}') { break; } continue; }
                        if self.eat(b'}') { break; }
                        return Err(self.err("expected ',' or '}'"));
                    }
                }
                self.depth -= 1;
                Ok(Json::Obj(m))
            }
            Some(b'[') => {
                self.i += 1; self.depth += 1;
                let mut v = Vec::new();
                self.ws();
                if !self.eat(b']') {
                    loop {
                        self.ws();
                        v.push(self.value()?);
                        self.ws();
                        if self.eat(b',') { self.ws(); if self.eat(b']') { break; } continue; }
                        if self.eat(b']') { break; }
                        return Err(self.err("expected ',' or ']'"));
                    }
                }
                self.depth -= 1;
                Ok(Json::Arr(v))
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len() && matches!(self.s[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') { self.i += 1; }
                let t = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
                t.parse::<f64>().map(Json::Num).map_err(|_| { self.i = start; self.err("bad number") })
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u escape"))?;
        let n = u32::from_str_radix(std::str::from_utf8(h).unwrap_or("x"), 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(n)
    }
    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else { return Err(self.err("string never closed")) };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return Err(self.err("string never closed")) };
                    self.i += 1;
                    let c: char = match e {
                        b'"' => '"', b'\\' => '\\', b'/' => '/', b'n' => '\n', b't' => '\t',
                        b'r' => '\r', b'b' => '\u{8}', b'f' => '\u{c}',
                        b'u' => {
                            let mut n = self.hex4()?;
                            if (0xD800..0xDC00).contains(&n) && self.s[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                n = 0x10000 + ((n - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            char::from_u32(n).unwrap_or('\u{FFFD}')
                        }
                        _ => return Err(self.err("bad escape")),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(b),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

/// loadJSONObject("config.json"). Null (and a message) if it can't be read or parsed.
pub fn load_json(path: &str) -> Json {
    let p = find(path);
    match std::fs::read_to_string(&p) {
        Err(e) => { warn("loadJSON", path, e); Json::Null }
        Ok(s) => parse_json(&s).unwrap_or_else(|e| { eprintln!("loadJSON: \"{path}\": {e}"); Json::Null }),
    }
}
pub fn save_json(path: &str, v: &Json) -> bool { let mut s = v.pretty(); s.push('\n'); write(path, s.as_bytes()) }

// ---------------------------------------------------------------- Table

/// A table of text cells, like Processing's Table. Columns are named when
/// the file is loaded with a header; numbers are parsed when you ask.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Column types by index ("String", "int", "float", "double", "long").
    /// Only stored and given back; every cell is text either way.
    types: Vec<String>,
    missing: Missing,
}

/// What getters give back for an empty or unreadable cell.
#[derive(Clone, Debug, Default, PartialEq)]
struct Missing { string: String, int: i32, float: f32, double: f64, long: i64 }

/// A column by name or by number: both t.get_int(0, "age") and t.get_int(0, 2) work.
pub trait Col { fn col(&self, t: &Table) -> Option<usize>; }
impl Col for usize { fn col(&self, _: &Table) -> Option<usize> { Some(*self) } }
impl Col for i32 { fn col(&self, _: &Table) -> Option<usize> { usize::try_from(*self).ok() } }
impl Col for &str { fn col(&self, t: &Table) -> Option<usize> { t.columns.iter().position(|c| c == self) } }
impl Col for String { fn col(&self, t: &Table) -> Option<usize> { self.as_str().col(t) } }
impl Col for &String { fn col(&self, t: &Table) -> Option<usize> { self.as_str().col(t) } }

impl Table {
    pub fn new() -> Table { Table::default() }
    pub fn add_column(&mut self, name: &str) -> &mut Self {
        self.columns.push(name.to_string());
        let n = self.columns.len();
        for r in &mut self.rows { r.resize(n, String::new()); }
        self
    }
    pub fn row_count(&self) -> usize { self.rows.len() }
    pub fn column_count(&self) -> usize { self.columns.len().max(self.rows.iter().map(|r| r.len()).max().unwrap_or(0)) }
    pub fn column_index(&self, name: &str) -> Option<usize> { self.columns.iter().position(|c| c == name) }
    /// Adds an empty row and returns its index.
    pub fn add_row(&mut self) -> usize { self.rows.push(vec![String::new(); self.columns.len()]); self.rows.len() - 1 }
    /// Adds a row from values: t.add_row_with(&["Pep", "19"]).
    pub fn add_row_with<S: ToString>(&mut self, cells: &[S]) -> usize {
        self.rows.push(cells.iter().map(|c| c.to_string()).collect()); self.rows.len() - 1
    }
    pub fn remove_row(&mut self, row: usize) { if row < self.rows.len() { self.rows.remove(row); } }
    pub fn clear_rows(&mut self) { self.rows.clear(); }

    fn cell(&self, row: usize, c: impl Col) -> Option<&str> {
        c.col(self).and_then(|c| self.rows.get(row)?.get(c)).map(|s| s.as_str())
    }
    pub fn get_string(&self, row: usize, c: impl Col) -> String {
        match self.cell(row, c) { Some(s) if !s.is_empty() => s.to_string(), _ => self.missing.string.clone() }
    }
    pub fn get_float(&self, row: usize, c: impl Col) -> f32 {
        self.cell(row, c).and_then(|s| s.trim().parse().ok()).unwrap_or(self.missing.float)
    }
    pub fn get_int(&self, row: usize, c: impl Col) -> i32 {
        let s = self.cell(row, c).unwrap_or("").trim();
        s.parse().ok().or_else(|| s.parse::<f64>().ok().map(|f| f as i32)).unwrap_or(self.missing.int)
    }
    pub fn get_double(&self, row: usize, c: impl Col) -> f64 {
        self.cell(row, c).and_then(|s| s.trim().parse().ok()).unwrap_or(self.missing.double)
    }
    pub fn get_long(&self, row: usize, c: impl Col) -> i64 {
        let s = self.cell(row, c).unwrap_or("").trim();
        s.parse().ok().or_else(|| s.parse::<f64>().ok().map(|f| f as i64)).unwrap_or(self.missing.long)
    }
    /// Sets a cell, growing the table if the row or column isn't there yet.
    pub fn set(&mut self, row: usize, c: impl Col, v: impl ToString) {
        let Some(c) = c.col(self) else { eprintln!("Table: no column with that name"); return };
        if self.rows.len() <= row { self.rows.resize(row + 1, Vec::new()); }
        let r = &mut self.rows[row];
        if r.len() <= c { r.resize(c + 1, String::new()); }
        r[c] = v.to_string();
    }
    pub fn set_string(&mut self, row: usize, c: impl Col, v: &str) { self.set(row, c, v) }
    pub fn set_int(&mut self, row: usize, c: impl Col, v: i32) { self.set(row, c, v) }
    pub fn set_float(&mut self, row: usize, c: impl Col, v: f32) { self.set(row, c, v) }
    pub fn set_double(&mut self, row: usize, c: impl Col, v: f64) { self.set(row, c, v) }
    pub fn set_long(&mut self, row: usize, c: impl Col, v: i64) { self.set(row, c, v) }

    /// Every value in one column.
    pub fn column(&self, c: impl Col) -> Vec<String> {
        let Some(c) = c.col(self) else { return Vec::new() };
        self.rows.iter().map(|r| r.get(c).cloned().unwrap_or_default()).collect()
    }
    /// The first row whose cell in that column equals value.
    pub fn find_row(&self, value: &str, c: impl Col) -> Option<usize> {
        let c = c.col(self)?;
        self.rows.iter().position(|r| r.get(c).map(|s| s == value).unwrap_or(false))
    }
    pub fn find_rows(&self, value: &str, c: impl Col) -> Vec<usize> {
        let Some(c) = c.col(self) else { return Vec::new() };
        (0..self.rows.len()).filter(|&i| self.rows[i].get(c).map(|s| s == value).unwrap_or(false)).collect()
    }
    /// Sorts rows by a column: numerically if every cell is a number, else as text.
    pub fn sort(&mut self, c: impl Col) { self.sort_impl(c, false) }
    /// Like sort, largest (or Z) first.
    pub fn sort_reverse(&mut self, c: impl Col) { self.sort_impl(c, true) }
    fn sort_impl(&mut self, c: impl Col, rev: bool) {
        let Some(c) = c.col(self) else { return };
        let cell = |r: &Vec<String>| r.get(c).cloned().unwrap_or_default();
        let numeric = self.rows.iter().all(|r| cell(r).trim().parse::<f64>().is_ok());
        if numeric {
            self.rows.sort_by(|a, b| {
                let x: f64 = cell(a).trim().parse().unwrap_or(0.0);
                let y: f64 = cell(b).trim().parse().unwrap_or(0.0);
                let o = x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
                if rev { o.reverse() } else { o }
            });
        } else if rev {
            self.rows.sort_by_key(|r| std::cmp::Reverse(cell(r)));
        } else {
            self.rows.sort_by_key(cell);
        }
    }
    pub fn reverse(&mut self) { self.rows.reverse(); }

    fn to_text(&self, sep: char) -> String {
        let cell = |s: &str| -> String {
            if sep == ',' && (s.contains(',') || s.contains('"') || s.contains('\n') || s.starts_with(' ') || s.ends_with(' ')) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else if sep == '\t' { s.replace(['\t', '\n'], " ") } else { s.to_string() }
        };
        let mut out = String::new();
        let line = |out: &mut String, r: &[String]| {
            let v: Vec<String> = r.iter().map(|c| cell(c)).collect();
            out.push_str(&v.join(&sep.to_string())); out.push('\n');
        };
        if !self.columns.is_empty() { line(&mut out, &self.columns); }
        for r in &self.rows { line(&mut out, r); }
        out
    }
}

// More of Processing's Table API.
impl Table {
    pub fn get_row_count(&self) -> usize { self.row_count() }
    pub fn get_column_count(&self) -> usize { self.column_count() }
    /// Index of the column with this title, or -1.
    pub fn get_column_index(&self, name: &str) -> i32 { self.column_index(name).map(|i| i as i32).unwrap_or(-1) }
    pub fn get_column_title(&self, col: usize) -> String { self.columns.get(col).cloned().unwrap_or_default() }
    pub fn get_column_titles(&self) -> Vec<String> { self.columns.clone() }
    /// Names a column, adding untitled ones before it if needed.
    pub fn set_column_title(&mut self, col: usize, title: &str) {
        if self.columns.len() <= col { self.columns.resize(col + 1, String::new()); }
        self.columns[col] = title.to_string();
    }
    pub fn set_column_titles<S: AsRef<str>>(&mut self, titles: &[S]) {
        self.columns = titles.iter().map(|t| t.as_ref().to_string()).collect();
    }
    pub fn has_column_titles(&self) -> bool { !self.columns.is_empty() }
    /// "String", "int", "float", "double" or "long". Defaults to "String".
    pub fn get_column_type(&self, c: impl Col) -> String {
        c.col(self).and_then(|i| self.types.get(i).cloned()).filter(|t| !t.is_empty()).unwrap_or_else(|| "String".into())
    }
    pub fn get_column_types(&self) -> Vec<String> { (0..self.column_count()).map(|i| self.get_column_type(i)).collect() }
    /// Records a column's type. Cells stay text; this is only given back by get_column_type.
    pub fn set_column_type(&mut self, c: impl Col, t: &str) {
        let Some(i) = c.col(self) else { eprintln!("Table: no column with that name"); return };
        if self.types.len() <= i { self.types.resize(i + 1, String::new()); }
        self.types[i] = t.to_string();
    }
    pub fn set_column_types<S: AsRef<str>>(&mut self, types: &[S]) {
        self.types = types.iter().map(|t| t.as_ref().to_string()).collect();
    }

    pub fn set_missing_string(&mut self, v: &str) { self.missing.string = v.to_string(); }
    pub fn set_missing_int(&mut self, v: i32) { self.missing.int = v; }
    pub fn set_missing_float(&mut self, v: f32) { self.missing.float = v; }
    pub fn set_missing_double(&mut self, v: f64) { self.missing.double = v; }
    pub fn set_missing_long(&mut self, v: i64) { self.missing.long = v; }

    pub fn get_string_column(&self, c: impl Col) -> Vec<String> {
        let Some(c) = c.col(self) else { return Vec::new() };
        (0..self.rows.len()).map(|r| self.get_string(r, c)).collect()
    }
    pub fn get_int_column(&self, c: impl Col) -> Vec<i32> {
        let Some(c) = c.col(self) else { return Vec::new() };
        (0..self.rows.len()).map(|r| self.get_int(r, c)).collect()
    }
    pub fn get_float_column(&self, c: impl Col) -> Vec<f32> {
        let Some(c) = c.col(self) else { return Vec::new() };
        (0..self.rows.len()).map(|r| self.get_float(r, c)).collect()
    }
    pub fn get_double_column(&self, c: impl Col) -> Vec<f64> {
        let Some(c) = c.col(self) else { return Vec::new() };
        (0..self.rows.len()).map(|r| self.get_double(r, c)).collect()
    }
    pub fn get_long_column(&self, c: impl Col) -> Vec<i64> {
        let Some(c) = c.col(self) else { return Vec::new() };
        (0..self.rows.len()).map(|r| self.get_long(r, c)).collect()
    }
    /// Every cell in one row.
    pub fn get_string_row(&self, row: usize) -> Vec<String> { (0..self.column_count()).map(|c| self.get_string(row, c)).collect() }
    pub fn get_int_row(&self, row: usize) -> Vec<i32> { (0..self.column_count()).map(|c| self.get_int(row, c)).collect() }
    pub fn get_float_row(&self, row: usize) -> Vec<f32> { (0..self.column_count()).map(|c| self.get_float(row, c)).collect() }
    pub fn get_double_row(&self, row: usize) -> Vec<f64> { (0..self.column_count()).map(|c| self.get_double(row, c)).collect() }
    pub fn get_long_row(&self, row: usize) -> Vec<i64> { (0..self.column_count()).map(|c| self.get_long(row, c)).collect() }

    /// A read-only view of one row.
    pub fn get_row(&self, row: usize) -> TableRowRef<'_> { TableRowRef { table: self, row } }
    /// A view of one row that can change its cells.
    pub fn get_row_mut(&mut self, row: usize) -> TableRow<'_> { TableRow { table: self, row } }
    /// Read-only views of every row, for `for row in t.rows() { ... }`.
    pub fn rows(&self) -> impl Iterator<Item = TableRowRef<'_>> { (0..self.rows.len()).map(move |row| TableRowRef { table: self, row }) }
    /// Adds an empty row and gives back a handle to fill it in.
    pub fn add_row_handle(&mut self) -> TableRow<'_> { let row = self.add_row(); TableRow { table: self, row } }
    /// Adds every row of another table (cells by position).
    pub fn add_rows(&mut self, other: &Table) { self.rows.extend(other.rows.iter().cloned()); }
    /// Puts a row of values at index, moving the rest down.
    pub fn insert_row<S: ToString>(&mut self, index: usize, cells: &[S]) {
        let r = cells.iter().map(|c| c.to_string()).collect();
        if index >= self.rows.len() { self.rows.resize(index, Vec::new()); self.rows.push(r); } else { self.rows.insert(index, r); }
    }
    /// Replaces a row's values.
    pub fn set_row<S: ToString>(&mut self, index: usize, cells: &[S]) {
        if self.rows.len() <= index { self.rows.resize(index + 1, Vec::new()); }
        self.rows[index] = cells.iter().map(|c| c.to_string()).collect();
    }
    /// Adds empty rows or drops rows from the end.
    pub fn set_row_count(&mut self, n: usize) { let w = self.columns.len(); self.rows.resize(n, vec![String::new(); w]); }
    /// Adds empty columns or drops columns from the right.
    pub fn set_column_count(&mut self, n: usize) {
        if !self.columns.is_empty() || n < self.columns.len() { self.columns.resize(n, String::new()); }
        self.types.truncate(n);
        for r in &mut self.rows { r.resize(n, String::new()); }
    }
    /// Adds a column at index, moving the others right.
    pub fn insert_column(&mut self, index: usize, title: &str) {
        let n = self.column_count().max(index);
        if !self.columns.is_empty() || !title.is_empty() {
            self.columns.resize(n, String::new());
            self.columns.insert(index, title.to_string());
        }
        if self.types.len() > index { self.types.insert(index, String::new()); }
        for r in &mut self.rows { if r.len() < index { r.resize(index, String::new()); } r.insert(index, String::new()); }
    }
    pub fn remove_column(&mut self, c: impl Col) {
        let Some(i) = c.col(self) else { return };
        if i < self.columns.len() { self.columns.remove(i); }
        if i < self.types.len() { self.types.remove(i); }
        for r in &mut self.rows { if i < r.len() { r.remove(i); } }
    }
    /// Uses the first row as the column titles and removes it. Gives back the titles.
    pub fn remove_title_row(&mut self) -> Vec<String> {
        if self.rows.is_empty() { return Vec::new(); }
        let t = self.rows.remove(0);
        self.columns = t.clone();
        t
    }
    /// Index of the last row, or -1 when empty.
    pub fn last_row_index(&self) -> i32 { self.rows.len() as i32 - 1 }
    /// Same as find_row, but -1 when nothing matches.
    pub fn find_row_index(&self, value: &str, c: impl Col) -> i32 { self.find_row(value, c).map(|i| i as i32).unwrap_or(-1) }
    pub fn find_row_indices(&self, value: &str, c: impl Col) -> Vec<usize> { self.find_rows(value, c) }

    fn regex_rows(&self, pattern: &str, c: impl Col) -> Vec<usize> {
        let Some(c) = c.col(self) else { return Vec::new() };
        let Ok(re) = regex::Regex::new(pattern).map_err(|e| eprintln!("Table.matchRow: {e}")) else { return Vec::new() };
        (0..self.rows.len()).filter(|&i| self.rows[i].get(c).map(|s| re.is_match(s)).unwrap_or(false)).collect()
    }
    /// The first row whose cell in that column matches the regex.
    pub fn match_row(&self, pattern: &str, c: impl Col) -> Option<usize> { self.regex_rows(pattern, c).first().copied() }
    pub fn match_row_index(&self, pattern: &str, c: impl Col) -> i32 { self.match_row(pattern, c).map(|i| i as i32).unwrap_or(-1) }
    /// Every row whose cell in that column matches the regex.
    pub fn match_rows(&self, pattern: &str, c: impl Col) -> Vec<usize> { self.regex_rows(pattern, c) }
    pub fn match_row_indices(&self, pattern: &str, c: impl Col) -> Vec<usize> { self.regex_rows(pattern, c) }

    /// Each distinct value in a column, in first-seen order.
    pub fn get_unique(&self, c: impl Col) -> Vec<String> { self.get_tally(c).key_array() }
    /// How many times each value appears in a column.
    pub fn get_tally(&self, c: impl Col) -> crate::IntDict {
        let mut d = crate::IntDict::new();
        for v in self.get_string_column(c) { d.increment(&v); }
        d
    }
    /// Each value in a column mapped to its (last) row.
    pub fn get_order(&self, c: impl Col) -> crate::IntDict {
        let mut d = crate::IntDict::new();
        for (i, v) in self.get_string_column(c).iter().enumerate() { d.set(v, i as i32); }
        d
    }
    /// A dict from one column's values to another's.
    pub fn get_int_dict(&self, key: impl Col, val: impl Col) -> crate::IntDict {
        let mut d = crate::IntDict::new();
        let (Some(k), Some(v)) = (key.col(self), val.col(self)) else { return d };
        for r in 0..self.rows.len() { d.set(&self.get_string(r, k), self.get_int(r, v)); }
        d
    }
    pub fn get_float_dict(&self, key: impl Col, val: impl Col) -> crate::FloatDict {
        let mut d = crate::FloatDict::new();
        let (Some(k), Some(v)) = (key.col(self), val.col(self)) else { return d };
        for r in 0..self.rows.len() { d.set(&self.get_string(r, k), self.get_float(r, v)); }
        d
    }
    pub fn get_string_dict(&self, key: impl Col, val: impl Col) -> crate::StringDict {
        let mut d = crate::StringDict::new();
        let (Some(k), Some(v)) = (key.col(self), val.col(self)) else { return d };
        for r in 0..self.rows.len() { d.set(&self.get_string(r, k), self.get_string(r, v)); }
        d
    }
    pub fn get_int_list(&self, c: impl Col) -> crate::IntList { self.get_int_column(c).into() }
    pub fn get_float_list(&self, c: impl Col) -> crate::FloatList { self.get_float_column(c).into() }
    pub fn get_string_list(&self, c: impl Col) -> crate::StringList { self.get_string_column(c).into() }
    /// Each value in a column mapped to its row index.
    pub fn get_row_map(&self, c: impl Col) -> std::collections::HashMap<String, usize> {
        self.get_string_column(c).into_iter().enumerate().map(|(i, v)| (v, i)).collect()
    }

    fn cells_mut(&mut self, c: Option<usize>) -> impl Iterator<Item = &mut String> {
        self.rows.iter_mut().flat_map(move |r| r.iter_mut().enumerate().filter(move |(i, _)| c.map_or(true, |c| c == *i)).map(|(_, s)| s))
    }
    /// Takes these characters out of every cell.
    pub fn remove_tokens(&mut self, tokens: &str) { for s in self.cells_mut(None) { s.retain(|ch| !tokens.contains(ch)); } }
    /// Takes these characters out of one column.
    pub fn remove_tokens_in(&mut self, tokens: &str, c: impl Col) {
        let Some(c) = c.col(self) else { return };
        for s in self.cells_mut(Some(c)) { s.retain(|ch| !tokens.contains(ch)); }
    }
    /// Swaps every cell that is exactly orig for replacement.
    pub fn replace(&mut self, orig: &str, replacement: &str) {
        for s in self.cells_mut(None) { if s == orig { *s = replacement.to_string(); } }
    }
    pub fn replace_in(&mut self, orig: &str, replacement: &str, c: impl Col) {
        let Some(c) = c.col(self) else { return };
        for s in self.cells_mut(Some(c)) { if s == orig { *s = replacement.to_string(); } }
    }
    /// Regex replace inside every cell.
    pub fn replace_all(&mut self, pattern: &str, replacement: &str) { self.replace_all_impl(pattern, replacement, None) }
    pub fn replace_all_in(&mut self, pattern: &str, replacement: &str, c: impl Col) {
        if let Some(c) = c.col(self) { self.replace_all_impl(pattern, replacement, Some(c)) }
    }
    fn replace_all_impl(&mut self, pattern: &str, replacement: &str, c: Option<usize>) {
        let Ok(re) = regex::Regex::new(pattern).map_err(|e| eprintln!("Table.replaceAll: {e}")) else { return };
        for s in self.cells_mut(c) { *s = re.replace_all(s, replacement).into_owned(); }
    }
    /// Trims spaces around every cell and drops empty rows and columns at the end.
    pub fn trim(&mut self) {
        for s in self.cells_mut(None) { *s = s.trim().to_string(); }
        while self.rows.last().map(|r| r.iter().all(|s| s.is_empty())).unwrap_or(false) { self.rows.pop(); }
        let mut n = self.column_count();
        while n > 0 && self.columns.get(n - 1).map_or(true, |t| t.is_empty())
            && self.rows.iter().all(|r| r.get(n - 1).map_or(true, |s| s.is_empty())) { n -= 1; }
        if n < self.column_count() { self.set_column_count(n); }
    }
    /// Trims spaces around every cell in one column.
    pub fn trim_column(&mut self, c: impl Col) {
        let Some(c) = c.col(self) else { return };
        for s in self.cells_mut(Some(c)) { *s = s.trim().to_string(); }
    }
    pub fn copy(&self) -> Table { self.clone() }
    pub fn swap(&mut self, a: usize, b: usize) { self.rows.swap(a, b); }
    pub fn print(&self) { print!("{}", self.to_text(',')); }

    #[inline] pub fn getRowCount(&self) -> usize { self.get_row_count() }
    #[inline] pub fn getColumnCount(&self) -> usize { self.get_column_count() }
    #[inline] pub fn getColumnIndex(&self, name: &str) -> i32 { self.get_column_index(name) }
    #[inline] pub fn getColumnTitle(&self, col: usize) -> String { self.get_column_title(col) }
    #[inline] pub fn getColumnTitles(&self) -> Vec<String> { self.get_column_titles() }
    #[inline] pub fn setColumnTitle(&mut self, col: usize, t: &str) { self.set_column_title(col, t) }
    #[inline] pub fn setColumnTitles<S: AsRef<str>>(&mut self, t: &[S]) { self.set_column_titles(t) }
    #[inline] pub fn hasColumnTitles(&self) -> bool { self.has_column_titles() }
    #[inline] pub fn getColumnType(&self, c: impl Col) -> String { self.get_column_type(c) }
    #[inline] pub fn getColumnTypes(&self) -> Vec<String> { self.get_column_types() }
    #[inline] pub fn setColumnType(&mut self, c: impl Col, t: &str) { self.set_column_type(c, t) }
    #[inline] pub fn setColumnTypes<S: AsRef<str>>(&mut self, t: &[S]) { self.set_column_types(t) }
    #[inline] pub fn getString(&self, row: usize, c: impl Col) -> String { self.get_string(row, c) }
    #[inline] pub fn getInt(&self, row: usize, c: impl Col) -> i32 { self.get_int(row, c) }
    #[inline] pub fn getFloat(&self, row: usize, c: impl Col) -> f32 { self.get_float(row, c) }
    #[inline] pub fn getDouble(&self, row: usize, c: impl Col) -> f64 { self.get_double(row, c) }
    #[inline] pub fn getLong(&self, row: usize, c: impl Col) -> i64 { self.get_long(row, c) }
    #[inline] pub fn setString(&mut self, row: usize, c: impl Col, v: &str) { self.set_string(row, c, v) }
    #[inline] pub fn setInt(&mut self, row: usize, c: impl Col, v: i32) { self.set_int(row, c, v) }
    #[inline] pub fn setFloat(&mut self, row: usize, c: impl Col, v: f32) { self.set_float(row, c, v) }
    #[inline] pub fn setDouble(&mut self, row: usize, c: impl Col, v: f64) { self.set_double(row, c, v) }
    #[inline] pub fn setLong(&mut self, row: usize, c: impl Col, v: i64) { self.set_long(row, c, v) }
    #[inline] pub fn setMissingString(&mut self, v: &str) { self.set_missing_string(v) }
    #[inline] pub fn setMissingInt(&mut self, v: i32) { self.set_missing_int(v) }
    #[inline] pub fn setMissingFloat(&mut self, v: f32) { self.set_missing_float(v) }
    #[inline] pub fn setMissingDouble(&mut self, v: f64) { self.set_missing_double(v) }
    #[inline] pub fn setMissingLong(&mut self, v: i64) { self.set_missing_long(v) }
    #[inline] pub fn getStringColumn(&self, c: impl Col) -> Vec<String> { self.get_string_column(c) }
    #[inline] pub fn getIntColumn(&self, c: impl Col) -> Vec<i32> { self.get_int_column(c) }
    #[inline] pub fn getFloatColumn(&self, c: impl Col) -> Vec<f32> { self.get_float_column(c) }
    #[inline] pub fn getDoubleColumn(&self, c: impl Col) -> Vec<f64> { self.get_double_column(c) }
    #[inline] pub fn getLongColumn(&self, c: impl Col) -> Vec<i64> { self.get_long_column(c) }
    #[inline] pub fn getStringRow(&self, row: usize) -> Vec<String> { self.get_string_row(row) }
    #[inline] pub fn getIntRow(&self, row: usize) -> Vec<i32> { self.get_int_row(row) }
    #[inline] pub fn getFloatRow(&self, row: usize) -> Vec<f32> { self.get_float_row(row) }
    #[inline] pub fn getDoubleRow(&self, row: usize) -> Vec<f64> { self.get_double_row(row) }
    #[inline] pub fn getLongRow(&self, row: usize) -> Vec<i64> { self.get_long_row(row) }
    /// Processing's getRow: a handle that can read and change the row.
    #[inline] pub fn getRow(&mut self, row: usize) -> TableRow<'_> { self.get_row_mut(row) }
    /// Processing's addRow: adds an empty row and gives back its handle.
    #[inline] pub fn addRow(&mut self) -> TableRow<'_> { self.add_row_handle() }
    #[inline] pub fn addColumn(&mut self, name: &str) -> &mut Self { self.add_column(name) }
    #[inline] pub fn addRows(&mut self, other: &Table) { self.add_rows(other) }
    #[inline] pub fn insertRow<S: ToString>(&mut self, i: usize, cells: &[S]) { self.insert_row(i, cells) }
    #[inline] pub fn insertColumn(&mut self, i: usize, title: &str) { self.insert_column(i, title) }
    #[inline] pub fn removeColumn(&mut self, c: impl Col) { self.remove_column(c) }
    #[inline] pub fn removeRow(&mut self, row: usize) { self.remove_row(row) }
    #[inline] pub fn clearRows(&mut self) { self.clear_rows() }
    #[inline] pub fn removeTitleRow(&mut self) -> Vec<String> { self.remove_title_row() }
    #[inline] pub fn setRow<S: ToString>(&mut self, i: usize, cells: &[S]) { self.set_row(i, cells) }
    #[inline] pub fn setRowCount(&mut self, n: usize) { self.set_row_count(n) }
    #[inline] pub fn setColumnCount(&mut self, n: usize) { self.set_column_count(n) }
    #[inline] pub fn lastRowIndex(&self) -> i32 { self.last_row_index() }
    #[inline] pub fn findRow(&self, v: &str, c: impl Col) -> Option<usize> { self.find_row(v, c) }
    #[inline] pub fn findRows(&self, v: &str, c: impl Col) -> Vec<usize> { self.find_rows(v, c) }
    #[inline] pub fn findRowIndex(&self, v: &str, c: impl Col) -> i32 { self.find_row_index(v, c) }
    #[inline] pub fn findRowIndices(&self, v: &str, c: impl Col) -> Vec<usize> { self.find_row_indices(v, c) }
    #[inline] pub fn matchRow(&self, p: &str, c: impl Col) -> Option<usize> { self.match_row(p, c) }
    #[inline] pub fn matchRows(&self, p: &str, c: impl Col) -> Vec<usize> { self.match_rows(p, c) }
    #[inline] pub fn matchRowIndex(&self, p: &str, c: impl Col) -> i32 { self.match_row_index(p, c) }
    #[inline] pub fn matchRowIndices(&self, p: &str, c: impl Col) -> Vec<usize> { self.match_row_indices(p, c) }
    #[inline] pub fn getUnique(&self, c: impl Col) -> Vec<String> { self.get_unique(c) }
    #[inline] pub fn getTally(&self, c: impl Col) -> crate::IntDict { self.get_tally(c) }
    #[inline] pub fn getOrder(&self, c: impl Col) -> crate::IntDict { self.get_order(c) }
    #[inline] pub fn getIntDict(&self, k: impl Col, v: impl Col) -> crate::IntDict { self.get_int_dict(k, v) }
    #[inline] pub fn getFloatDict(&self, k: impl Col, v: impl Col) -> crate::FloatDict { self.get_float_dict(k, v) }
    #[inline] pub fn getStringDict(&self, k: impl Col, v: impl Col) -> crate::StringDict { self.get_string_dict(k, v) }
    #[inline] pub fn getIntList(&self, c: impl Col) -> crate::IntList { self.get_int_list(c) }
    #[inline] pub fn getFloatList(&self, c: impl Col) -> crate::FloatList { self.get_float_list(c) }
    #[inline] pub fn getStringList(&self, c: impl Col) -> crate::StringList { self.get_string_list(c) }
    #[inline] pub fn getRowMap(&self, c: impl Col) -> std::collections::HashMap<String, usize> { self.get_row_map(c) }
    #[inline] pub fn removeTokens(&mut self, t: &str) { self.remove_tokens(t) }
    #[inline] pub fn replaceAll(&mut self, p: &str, r: &str) { self.replace_all(p, r) }
    #[inline] pub fn sortReverse(&mut self, c: impl Col) { self.sort_reverse(c) }
}

/// Getters shared by TableRow and TableRowRef.
macro_rules! row_getters {
    () => {
        pub fn index(&self) -> usize { self.row }
        pub fn get_string(&self, c: impl Col) -> String { self.table.get_string(self.row, c) }
        pub fn get_int(&self, c: impl Col) -> i32 { self.table.get_int(self.row, c) }
        pub fn get_float(&self, c: impl Col) -> f32 { self.table.get_float(self.row, c) }
        pub fn get_double(&self, c: impl Col) -> f64 { self.table.get_double(self.row, c) }
        pub fn get_long(&self, c: impl Col) -> i64 { self.table.get_long(self.row, c) }
        pub fn get_column_count(&self) -> usize { self.table.column_count() }
        pub fn get_column_title(&self, col: usize) -> String { self.table.get_column_title(col) }
        pub fn get_column_titles(&self) -> Vec<String> { self.table.get_column_titles() }
        pub fn get_column_type(&self, c: impl Col) -> String { self.table.get_column_type(c) }
        pub fn get_column_types(&self) -> Vec<String> { self.table.get_column_types() }
        pub fn print(&self) { println!("{}", self.table.get_string_row(self.row).join(", ")); }
        #[inline] pub fn getString(&self, c: impl Col) -> String { self.get_string(c) }
        #[inline] pub fn getInt(&self, c: impl Col) -> i32 { self.get_int(c) }
        #[inline] pub fn getFloat(&self, c: impl Col) -> f32 { self.get_float(c) }
        #[inline] pub fn getDouble(&self, c: impl Col) -> f64 { self.get_double(c) }
        #[inline] pub fn getLong(&self, c: impl Col) -> i64 { self.get_long(c) }
        #[inline] pub fn getColumnCount(&self) -> usize { self.get_column_count() }
        #[inline] pub fn getColumnTitle(&self, col: usize) -> String { self.get_column_title(col) }
        #[inline] pub fn getColumnTitles(&self) -> Vec<String> { self.get_column_titles() }
        #[inline] pub fn getColumnType(&self, c: impl Col) -> String { self.get_column_type(c) }
        #[inline] pub fn getColumnTypes(&self) -> Vec<String> { self.get_column_types() }
    };
}

/// One row of a Table that can be changed, from get_row_mut, getRow or add_row_handle.
pub struct TableRow<'a> { table: &'a mut Table, row: usize }
impl TableRow<'_> {
    row_getters!();
    pub fn set_string(&mut self, c: impl Col, v: &str) { self.table.set(self.row, c, v) }
    pub fn set_int(&mut self, c: impl Col, v: i32) { self.table.set(self.row, c, v) }
    pub fn set_float(&mut self, c: impl Col, v: f32) { self.table.set(self.row, c, v) }
    pub fn set_double(&mut self, c: impl Col, v: f64) { self.table.set(self.row, c, v) }
    pub fn set_long(&mut self, c: impl Col, v: i64) { self.table.set(self.row, c, v) }
    /// Any value that prints as text.
    pub fn set(&mut self, c: impl Col, v: impl ToString) { self.table.set(self.row, c, v) }
    #[inline] pub fn setString(&mut self, c: impl Col, v: &str) { self.set_string(c, v) }
    #[inline] pub fn setInt(&mut self, c: impl Col, v: i32) { self.set_int(c, v) }
    #[inline] pub fn setFloat(&mut self, c: impl Col, v: f32) { self.set_float(c, v) }
    #[inline] pub fn setDouble(&mut self, c: impl Col, v: f64) { self.set_double(c, v) }
    #[inline] pub fn setLong(&mut self, c: impl Col, v: i64) { self.set_long(c, v) }
}

/// A read-only row of a Table, from get_row or rows().
#[derive(Clone, Copy)]
pub struct TableRowRef<'a> { table: &'a Table, row: usize }
impl TableRowRef<'_> { row_getters!(); }

/// Splits CSV text: handles "quoted, cells", doubled "" quotes, and newlines inside quotes.
fn parse_csv(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') { chars.next(); cell.push('"'); } else { quoted = false; }
            } else { cell.push(c); }
            continue;
        }
        match c {
            '"' if cell.is_empty() && sep != '\t' => quoted = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            c if c == sep => row.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    if any { row.push(cell); rows.push(row); }
    // Blank lines carry nothing; drop them.
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    rows
}

fn sep_for(path: &str, options: &str) -> char {
    if options.contains("tsv") || path.ends_with(".tsv") { '\t' } else { ',' }
}

/// loadTable("data.csv", "header"): the first row names the columns.
/// Options: "header", "csv", "tsv" (tab separated). .tsv files are tab separated anyway.
pub fn load_table(path: &str, options: &str) -> Table {
    let text = match std::fs::read_to_string(find(path)) {
        Ok(s) => s, Err(e) => { warn("loadTable", path, e); return Table::default(); }
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut rows = parse_csv(text, sep_for(path, options));
    let mut t = Table::default();
    if options.contains("header") && !rows.is_empty() {
        t.columns = rows.remove(0).into_iter().map(|s| s.trim().to_string()).collect();
    }
    t.rows = rows;
    t
}
/// saveTable(&t, "out.csv"). A path ending in .tsv is written with tabs.
pub fn save_table(t: &Table, path: &str) -> bool { write(path, t.to_text(sep_for(path, "")).as_bytes()) }

#[cfg(test)]
mod tests {
    #[test]
    fn long_double_lists() {
        let j = super::parse_json(r#"{"a": [1, 2, 3000000000], "b": [0.5, 1.5]}"#).unwrap();
        assert_eq!(j.get_long_list("a").sum(), 3000000003);
        assert_eq!(j.get_double_list("b").sum(), 2.0);
        assert_eq!(j.get_json_array("b").to_boolean_array().len(), 2);
        let mut d = crate::LongDict::new();
        d.increment("x"); d.increment("x");
        assert_eq!(d.get("x"), 2);
        let m = crate::PMatrix3D::new();
        let p = m.cmult(&crate::PVector::new3(1.0, 2.0, 3.0));
        assert_eq!((p.x, p.y, p.z), (1.0, 2.0, 3.0));
    }
    use super::*;

    #[test]
    fn json_round_trip() {
        let src = r#"{ "name": "Pep", "n": 3, "pi": 3.5, "ok": true, "list": [1, 2, {"x": null}],
                       "esc": "a\"b\\c\nd é 😀", // comment
                       "trail": [1,2,], }"#;
        let v = parse_json(src).unwrap();
        assert_eq!(v["name"].as_str(), "Pep");
        assert_eq!(v.get_int("n"), 3);
        assert_eq!(v["pi"].as_f32(), 3.5);
        assert!(v["ok"].as_bool());
        assert_eq!(v["list"].len(), 3);
        assert!(v["list"][2]["x"].is_null());
        assert!(v["missing"]["deeper"][4].is_null());
        assert_eq!(v["esc"].as_str(), "a\"b\\c\nd \u{e9} \u{1F600}");
        let again = parse_json(&v.pretty()).unwrap();
        assert_eq!(again, v);
        let again = parse_json(&v.to_string()).unwrap();
        assert_eq!(again, v);
    }

    #[test]
    fn json_errors_say_where() {
        let e = parse_json("{\n  \"a\": 1\n  \"b\": 2 }").unwrap_err();
        assert!(e.contains("line 3"), "{e}");
        assert!(parse_json("[1, 2").is_err());
        assert!(parse_json("\"open").is_err());
        assert!(parse_json("12 13").is_err());
        assert!(parse_json(&"[".repeat(10000)).is_err());
    }

    #[test]
    fn json_build() {
        let mut o = Json::object();
        o.set("a", 1).set("b", "two").set("c", vec![1.5, 2.5]);
        o.get_mut("d").push(true).push(Json::Null);
        assert_eq!(o.to_string(), r#"{"a":1,"b":"two","c":[1.5,2.5],"d":[true,null]}"#);
        assert_eq!(o.keys(), vec!["a", "b", "c", "d"]);
        o.remove("a");
        assert!(!o.has("a"));
    }

    #[test]
    fn csv_and_table() {
        let rows = parse_csv("a,b,c\r\n1,\"x, y\",\"say \"\"hi\"\"\"\n\n2,\"multi\nline\",\n", ',');
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], vec!["1", "x, y", "say \"hi\""]);
        assert_eq!(rows[2], vec!["2", "multi\nline", ""]);

        let dir = std::env::temp_dir().join(format!("rm-table-{}", std::process::id()));
        let path = dir.join("t.csv");
        let path = path.to_str().unwrap();
        let mut t = Table::new();
        t.add_column("name").add_column("age");
        t.add_row_with(&["Zed", "30"]);
        let r = t.add_row(); t.set(r, "name", "Ana, B"); t.set_int(r, "age", 9);
        t.add_row_with(&["Max", "100"]);
        assert!(save_table(&t, path));
        let mut u = load_table(path, "header");
        assert_eq!(u, t);
        assert_eq!(u.get_int(1, "age"), 9);
        assert_eq!(u.get_string(1, 0usize), "Ana, B");
        assert_eq!(u.get_float(0, "nope"), 0.0);
        assert_eq!(u.find_row("Max", "name"), Some(2));
        u.sort("age");
        assert_eq!(u.column("age"), vec!["9", "30", "100"]);
        u.sort("name");
        assert_eq!(u.get_string(0, "name"), "Ana, B");
        u.set(5, "age", 1);
        assert_eq!(u.row_count(), 6);

        let lines = dir.join("sub/l.txt");
        assert!(save_strings(lines.to_str().unwrap(), &["one", "two"]));
        assert_eq!(load_strings(lines.to_str().unwrap()), vec!["one", "two"]);
        assert!(load_strings("/definitely/not/here.txt").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn table_more() {
        let mut t = Table::new();
        t.add_column("name").add_column("n");
        t.add_row_with(&["ann", "3"]);
        t.add_row_with(&["bob", ""]);
        t.add_row_with(&["ann", "5"]);
        assert_eq!(t.get_column_index("n"), 1);
        assert_eq!(t.get_column_index("zz"), -1);
        t.set_missing_int(-1);
        assert_eq!(t.get_int_column("n"), vec![3, -1, 5]);
        assert_eq!(t.get_long(2, 1usize), 5);
        assert_eq!(t.get_tally("name").get("ann"), 2);
        assert_eq!(t.get_unique("name"), vec!["ann", "bob"]);
        assert_eq!(t.get_int_dict("name", "n").get("ann"), 5);
        assert_eq!(t.match_row_indices("^a", "name"), vec![0, 2]);
        assert_eq!(t.match_row_index("^b", "name"), 1);
        assert_eq!(t.find_row_index("cy", "name"), -1);
        t.set_column_type("n", "int");
        assert_eq!(t.get_column_types(), vec!["String", "int"]);
        {
            let mut r = t.add_row_handle();
            r.set_string("name", "cy");
            r.set_double("n", 1.5);
            assert_eq!(r.get_float("n"), 1.5);
        }
        assert_eq!(t.last_row_index(), 3);
        let names: Vec<String> = t.rows().map(|r| r.get_string("name")).collect();
        assert_eq!(names, vec!["ann", "bob", "ann", "cy"]);
        t.getRow(1).setInt("n", 7);
        assert_eq!(t.get_row(1).get_int(1usize), 7);
        t.sort_reverse("n");
        assert_eq!(t.get_string_column(0usize), vec!["bob", "ann", "ann", "cy"]);
        t.insert_column(1, "mid");
        assert_eq!(t.get_column_titles(), vec!["name", "mid", "n"]);
        assert_eq!(t.get_column_type("n"), "int");
        t.remove_column("mid");
        t.replace_all("[aeiou]", "_");
        t.replace("cy", "see");
        assert_eq!(t.get_string_row(3), vec!["see", "1.5"]);
        t.remove_tokens("_");
        assert_eq!(t.get_string(1, "name"), "nn");
        let mut u = Table::new();
        u.add_row_with(&[" a ", "b", ""]);
        u.add_row_with(&["1", "2", ""]);
        u.add_row_with(&["", "", ""]);
        u.trim();
        assert_eq!(u.row_count(), 2);
        assert_eq!(u.column_count(), 2);
        assert_eq!(u.remove_title_row(), vec!["a", "b"]);
        assert_eq!(u.get_int(0, "b"), 2);
        u.set_row_count(3);
        assert_eq!(u.get_row_count(), 3);
        u.add_rows(&t);
        assert_eq!(u.get_row_count(), 7);
    }

    #[test]
    fn json_more() {
        let mut o = Json::object();
        o.set_int("a", 1).set_long("big", 9_000_000_000).set_double("d", 0.25).set_boolean("ok", true);
        o.set_json_array("list", Json::from(vec![3, 1, 2]));
        assert_eq!(o.get_long("big"), 9_000_000_000);
        assert_eq!(o.get_double("d"), 0.25);
        assert!(o.has_key("ok") && o.size() == 5);
        let l = o.get_json_array("list");
        assert_eq!(l.get_int(1), 1);
        assert_eq!(l.get_int_array(), vec![3, 1, 2]);
        assert_eq!(o.get_int_list("list").sum(), 6);
        assert_eq!(l.join("-"), "3-1-2");
        assert_eq!(o.get_int_or("nope", 9), 9);
        let mut a = Json::array();
        a.append("x").put(3usize, 4);
        assert_eq!(a.to_string(), r#"["x",null,null,4]"#);
        assert_eq!(a.get_string_array(), vec!["x", "", "", "4"]);
        assert!(a.isNull(1));
        assert_eq!(Json::from(vec![1]).format(4), "[\n    1\n]");
        assert_eq!(Json::from(vec![1, 2]).format(-1), "[1,2]");
        let dir = std::env::temp_dir().join(format!("rm-json-{}", std::process::id()));
        let path = dir.join("o.json");
        assert!(o.save(path.to_str().unwrap()));
        assert_eq!(load_json(path.to_str().unwrap()), o);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splitting() {
        assert_eq!(split_tokens("  a, b,,c ", ", "), vec!["a", "b", "c"]);
        assert_eq!(split("a,,b", ","), vec!["a", "", "b"]);
        assert_eq!(join(&["a", "b"], "-"), "a-b");
    }
}
