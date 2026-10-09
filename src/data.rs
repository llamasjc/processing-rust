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
    pub fn get_int(&self, key: &str) -> i32 { self.get(key).as_i32() }
    pub fn get_float(&self, key: &str) -> f32 { self.get(key).as_f32() }
    pub fn get_string(&self, key: &str) -> String { self.get(key).as_string() }
    pub fn get_boolean(&self, key: &str) -> bool { self.get(key).as_bool() }

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
    pub fn pretty(&self) -> String { let mut s = String::new(); self.dump(&mut s, Some(0)); s }
    fn dump(&self, out: &mut String, indent: Option<usize>) {
        let nl = |out: &mut String, n: usize| { out.push('\n'); for _ in 0..n { out.push_str("  "); } };
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
                    x.dump(out, indent.map(|d| d + 1));
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
                    x.dump(out, indent.map(|d| d + 1));
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
        let mut s = String::new(); self.dump(&mut s, None); f.write_str(&s)
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
}

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

    pub fn get_string(&self, row: usize, c: impl Col) -> String {
        c.col(self).and_then(|c| self.rows.get(row)?.get(c).cloned()).unwrap_or_default()
    }
    pub fn get_float(&self, row: usize, c: impl Col) -> f32 { self.get_string(row, c).trim().parse().unwrap_or(0.0) }
    pub fn get_int(&self, row: usize, c: impl Col) -> i32 {
        let s = self.get_string(row, c);
        let s = s.trim();
        s.parse().unwrap_or_else(|_| s.parse::<f64>().map(|f| f as i32).unwrap_or(0))
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
    pub fn sort(&mut self, c: impl Col) {
        let Some(c) = c.col(self) else { return };
        let cell = |r: &Vec<String>| r.get(c).cloned().unwrap_or_default();
        let numeric = self.rows.iter().all(|r| cell(r).trim().parse::<f64>().is_ok());
        if numeric {
            self.rows.sort_by(|a, b| {
                let x: f64 = cell(a).trim().parse().unwrap_or(0.0);
                let y: f64 = cell(b).trim().parse().unwrap_or(0.0);
                x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
            });
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
    fn splitting() {
        assert_eq!(split_tokens("  a, b,,c ", ", "), vec!["a", "b", "c"]);
        assert_eq!(split("a,,b", ","), vec!["a", "", "b"]);
        assert_eq!(join(&["a", "b"], "-"), "a-b");
    }
}
