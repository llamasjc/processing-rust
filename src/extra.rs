//! The rest of Processing's API that isn't drawing: XML, writers and readers,
//! file dialogs, hex and binary, regular expressions, threads, printArray.

use std::io::{BufRead, Write};

// ── XML ──────────────────────────────────────────────────────────────────

/// An XML element: a name, attributes, children and text.
///
///   let xml = load_xml("data.xml").unwrap();
///   for c in xml.get_children("item") { println!("{}", c.get_string("name")); }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Xml {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Xml>,
    content: String,
}

impl Xml {
    /// A new, empty element.
    pub fn new(name: &str) -> Xml { Xml { name: name.to_string(), ..Default::default() } }
    pub fn get_name(&self) -> &str { &self.name }
    pub fn set_name(&mut self, n: &str) -> &mut Self { self.name = n.to_string(); self }

    // children
    pub fn has_children(&self) -> bool { !self.children.is_empty() }
    pub fn get_child_count(&self) -> usize { self.children.len() }
    pub fn list_children(&self) -> Vec<String> { self.children.iter().map(|c| c.name.clone()).collect() }
    /// The first child with this name, or a path like "a/b/c".
    pub fn get_child(&self, path: &str) -> Option<&Xml> {
        let mut cur = self;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            cur = cur.children.iter().find(|c| c.name == part)?;
        }
        Some(cur)
    }
    pub fn get_child_mut(&mut self, path: &str) -> Option<&mut Xml> {
        let mut cur = self;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            cur = cur.children.iter_mut().find(|c| c.name == part)?;
        }
        Some(cur)
    }
    /// get_child_at(i): the i-th child (text between elements doesn't count).
    pub fn get_child_at(&self, i: usize) -> Option<&Xml> { self.children.get(i) }
    /// Every child with this name (or at the end of a path like "a/b"), or all of them for "".
    pub fn get_children(&self, path: &str) -> Vec<&Xml> {
        if path.is_empty() { return self.children.iter().collect(); }
        let (parent, last) = match path.rfind('/') { Some(i) => (&path[..i], &path[i + 1..]), None => ("", path) };
        match self.get_child(parent) {
            Some(p) => p.children.iter().filter(|c| c.name == last).collect(),
            None => Vec::new(),
        }
    }
    /// add_child("name") makes a new child and gives it back to fill in.
    pub fn add_child(&mut self, name: &str) -> &mut Xml {
        self.children.push(Xml::new(name));
        self.children.last_mut().unwrap()
    }
    pub fn add_child_xml(&mut self, child: Xml) -> &mut Xml {
        self.children.push(child);
        self.children.last_mut().unwrap()
    }
    /// Removes the first child with this name.
    pub fn remove_child(&mut self, name: &str) -> &mut Self {
        if let Some(i) = self.children.iter().position(|c| c.name == name) { self.children.remove(i); }
        self
    }

    // attributes
    pub fn get_attribute_count(&self) -> usize { self.attrs.len() }
    pub fn list_attributes(&self) -> Vec<String> { self.attrs.iter().map(|a| a.0.clone()).collect() }
    pub fn has_attribute(&self, name: &str) -> bool { self.attrs.iter().any(|a| a.0 == name) }
    fn attr(&self, name: &str) -> Option<&str> { self.attrs.iter().find(|a| a.0 == name).map(|a| a.1.as_str()) }
    pub fn get_string(&self, name: &str) -> String { self.attr(name).unwrap_or("").to_string() }
    pub fn get_string_or(&self, name: &str, default: &str) -> String { self.attr(name).unwrap_or(default).to_string() }
    pub fn get_int(&self, name: &str) -> i32 { self.get_int_or(name, 0) }
    pub fn get_int_or(&self, name: &str, default: i32) -> i32 { self.attr(name).and_then(|v| v.trim().parse().ok()).unwrap_or(default) }
    pub fn get_float(&self, name: &str) -> f32 { self.get_float_or(name, 0.0) }
    pub fn get_float_or(&self, name: &str, default: f32) -> f32 { self.attr(name).and_then(|v| v.trim().parse().ok()).unwrap_or(default) }
    pub fn set_string(&mut self, name: &str, v: &str) -> &mut Self {
        match self.attrs.iter_mut().find(|a| a.0 == name) {
            Some(a) => a.1 = v.to_string(),
            None => self.attrs.push((name.to_string(), v.to_string())),
        }
        self
    }
    pub fn set_int(&mut self, name: &str, v: i32) -> &mut Self { self.set_string(name, &v.to_string()) }
    pub fn set_float(&mut self, name: &str, v: f32) -> &mut Self { self.set_string(name, &v.to_string()) }
    pub fn remove_attribute(&mut self, name: &str) -> &mut Self { self.attrs.retain(|a| a.0 != name); self }

    // text
    pub fn get_content(&self) -> String { self.content.clone() }
    pub fn get_int_content(&self) -> i32 { self.content.trim().parse().unwrap_or(0) }
    pub fn get_float_content(&self) -> f32 { self.content.trim().parse().unwrap_or(0.0) }
    pub fn set_content(&mut self, t: &str) -> &mut Self { self.content = t.to_string(); self }
    pub fn set_int_content(&mut self, v: i32) -> &mut Self { self.set_content(&v.to_string()) }
    pub fn set_float_content(&mut self, v: f32) -> &mut Self { self.set_content(&v.to_string()) }

    /// The element as text; indent is the spaces per level (-1: all on one line).
    pub fn format(&self, indent: i32) -> String {
        let mut out = String::new();
        self.write_to(&mut out, indent, 0);
        out
    }
    fn write_to(&self, out: &mut String, indent: i32, depth: usize) {
        let pad = if indent >= 0 { " ".repeat(indent as usize * depth) } else { String::new() };
        let nl = if indent >= 0 { "\n" } else { "" };
        out.push_str(&pad);
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs { out.push_str(&format!(" {}=\"{}\"", k, escape(v))); }
        if self.children.is_empty() && self.content.is_empty() {
            out.push_str("/>");
            out.push_str(nl);
            return;
        }
        out.push('>');
        if self.children.is_empty() {
            out.push_str(&escape(&self.content));
        } else {
            out.push_str(nl);
            if !self.content.trim().is_empty() {
                out.push_str(&pad);
                out.push_str(&escape(self.content.trim()));
                out.push_str(nl);
            }
            for c in &self.children { c.write_to(out, indent, depth + 1); }
            out.push_str(&pad);
        }
        out.push_str(&format!("</{}>", self.name));
        out.push_str(nl);
    }

    // camelCase, as in Processing
    #[inline] pub fn getName(&self) -> &str { self.get_name() }
    #[inline] pub fn getChild(&self, path: &str) -> Option<&Xml> { self.get_child(path) }
    #[inline] pub fn getChildren(&self, path: &str) -> Vec<&Xml> { self.get_children(path) }
    #[inline] pub fn getChildCount(&self) -> usize { self.get_child_count() }
    #[inline] pub fn getString(&self, n: &str) -> String { self.get_string(n) }
    #[inline] pub fn getInt(&self, n: &str) -> i32 { self.get_int(n) }
    #[inline] pub fn getFloat(&self, n: &str) -> f32 { self.get_float(n) }
    #[inline] pub fn getContent(&self) -> String { self.get_content() }
    #[inline] pub fn hasAttribute(&self, n: &str) -> bool { self.has_attribute(n) }
}

impl std::fmt::Display for Xml {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.format(2).trim_end()) }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
fn unescape(s: &str) -> String {
    if !s.contains('&') { return s.to_string(); }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch { Some(c) => { out.push(c); rest = &rest[end + 1..]; } None => { out.push('&'); rest = &rest[1..]; } }
    }
    out.push_str(rest);
    out
}

/// parse_xml(text): the document's root element, or why it couldn't be read.
pub fn parse_xml(text: &str) -> Result<Xml, String> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Xml> = Vec::new();
    let mut root: Option<Xml> = None;
    let err = |i: usize, what: &str| Err(format!("{what} at line {}", text[..i.min(text.len())].lines().count().max(1)));
    while i < b.len() {
        if b[i] == b'<' {
            if text[i..].starts_with("<!--") {
                match text[i..].find("-->") { Some(e) => { i += e + 3; continue; } None => return err(i, "a comment that never ends") }
            }
            if text[i..].starts_with("<![CDATA[") {
                let Some(e) = text[i..].find("]]>") else { return err(i, "CDATA that never ends") };
                if let Some(top) = stack.last_mut() { top.content.push_str(&text[i + 9..i + e]); }
                i += e + 3;
                continue;
            }
            if text[i..].starts_with("<?") || text[i..].starts_with("<!") {
                match text[i..].find('>') { Some(e) => { i += e + 1; continue; } None => return err(i, "a tag that never ends") }
            }
            let Some(e) = text[i..].find('>') else { return err(i, "a tag that never ends") };
            let tag = &text[i + 1..i + e];
            i += e + 1;
            if let Some(name) = tag.strip_prefix('/') {
                let Some(done) = stack.pop() else { return err(i, "a closing tag with nothing open") };
                if done.name != name.trim() { return err(i, &format!("</{}> closes <{}>", name.trim(), done.name)); }
                match stack.last_mut() { Some(p) => p.children.push(done), None => root = Some(done) }
                continue;
            }
            let self_close = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
            let mut el = Xml::new(&tag[..name_end]);
            let mut a = tag[name_end..].trim();
            while !a.is_empty() {
                let Some(eq) = a.find('=') else { break };
                let key = a[..eq].trim().to_string();
                let v = a[eq + 1..].trim_start();
                let q = v.chars().next().unwrap_or('"');
                if q != '"' && q != '\'' { return err(i, "an attribute without quotes"); }
                let Some(close) = v[1..].find(q) else { return err(i, "an attribute that never ends") };
                el.attrs.push((key, unescape(&v[1..1 + close])));
                a = v[close + 2..].trim_start();
            }
            if self_close {
                match stack.last_mut() { Some(p) => p.children.push(el), None => root = Some(el) }
            } else {
                stack.push(el);
            }
        } else {
            let e = text[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            let t = &text[i..e];
            if let Some(top) = stack.last_mut() {
                if !t.trim().is_empty() || !top.content.is_empty() { top.content.push_str(&unescape(t)); }
            }
            i = e;
        }
    }
    if let Some(open) = stack.last() { return Err(format!("<{}> is never closed", open.name)); }
    // whitespace around child elements isn't content
    fn tidy(x: &mut Xml) {
        if !x.children.is_empty() { x.content = x.content.trim().to_string(); }
        for c in &mut x.children { tidy(c); }
    }
    let mut root = root.ok_or("no element in the text")?;
    tidy(&mut root);
    Ok(root)
}
/// load_xml(path): the file's root element (looked for as given, then in data/).
pub fn load_xml(path: &str) -> Option<Xml> {
    let text = crate::load_text(path);
    if text.is_empty() { eprintln!("loadXML(\"{path}\"): couldn't read the file"); return None; }
    match parse_xml(&text) {
        Ok(x) => Some(x),
        Err(e) => { eprintln!("loadXML(\"{path}\"): {e}"); None }
    }
}
pub fn save_xml(xml: &Xml, path: &str) -> bool {
    crate::save_text(path, &format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml.format(2)))
}
#[inline] pub fn parseXML(text: &str) -> Option<Xml> { parse_xml(text).ok() }
#[inline] pub fn loadXML(path: &str) -> Option<Xml> { load_xml(path) }
#[inline] pub fn saveXML(xml: &Xml, path: &str) -> bool { save_xml(xml, path) }

// ── writers and readers ──────────────────────────────────────────────────

/// create_writer("out.txt"): a file to print to; flush() and close() at the end.
pub struct PrintWriter { out: Option<std::io::BufWriter<std::fs::File>> }
impl PrintWriter {
    pub fn print(&mut self, s: impl std::fmt::Display) { if let Some(o) = &mut self.out { let _ = write!(o, "{s}"); } }
    pub fn println(&mut self, s: impl std::fmt::Display) { if let Some(o) = &mut self.out { let _ = writeln!(o, "{s}"); } }
    pub fn flush(&mut self) { if let Some(o) = &mut self.out { let _ = o.flush(); } }
    pub fn close(&mut self) { self.flush(); self.out = None; }
}
pub fn create_writer(path: &str) -> PrintWriter {
    if let Some(dir) = std::path::Path::new(path).parent() { if !dir.as_os_str().is_empty() { let _ = std::fs::create_dir_all(dir); } }
    match std::fs::File::create(path) {
        Ok(f) => PrintWriter { out: Some(std::io::BufWriter::new(f)) },
        Err(e) => { eprintln!("createWriter(\"{path}\"): {e}"); PrintWriter { out: None } }
    }
}
/// create_reader("in.txt"): read_line() gives each line, then None.
pub struct Reader { lines: Option<std::io::Lines<std::io::BufReader<std::fs::File>>> }
impl Reader {
    pub fn read_line(&mut self) -> Option<String> { self.lines.as_mut()?.next()?.ok() }
    #[inline] pub fn readLine(&mut self) -> Option<String> { self.read_line() }
    pub fn close(&mut self) { self.lines = None; }
}
pub fn create_reader(path: &str) -> Reader {
    let file = if std::path::Path::new(path).is_file() { path.to_string() } else { format!("data/{path}") };
    match std::fs::File::open(&file) {
        Ok(f) => Reader { lines: Some(std::io::BufReader::new(f).lines()) },
        Err(e) => { eprintln!("createReader(\"{path}\"): {e}"); Reader { lines: None } }
    }
}
/// create_output / create_input: the file itself, to write or read bytes.
pub fn create_output(path: &str) -> Option<std::fs::File> {
    if let Some(dir) = std::path::Path::new(path).parent() { if !dir.as_os_str().is_empty() { let _ = std::fs::create_dir_all(dir); } }
    std::fs::File::create(path).map_err(|e| eprintln!("createOutput(\"{path}\"): {e}")).ok()
}
pub fn create_input(path: &str) -> Option<std::fs::File> {
    let file = if std::path::Path::new(path).is_file() { path.to_string() } else { format!("data/{path}") };
    std::fs::File::open(&file).map_err(|e| eprintln!("createInput(\"{path}\"): {e}")).ok()
}
#[inline] pub fn createWriter(path: &str) -> PrintWriter { create_writer(path) }
#[inline] pub fn createReader(path: &str) -> Reader { create_reader(path) }

// ── file dialogs ─────────────────────────────────────────────────────────
// Processing calls back later; here the dialog waits and gives back the path
// (None when cancelled). Uses zenity or kdialog on Linux, the system dialogs
// on macOS and Windows.

fn run_dialog(prompt: &str, kind: &str) -> Option<String> {
    let out = if cfg!(target_os = "macos") {
        let what = match kind { "folder" => "choose folder", "save" => "choose file name", _ => "choose file" };
        std::process::Command::new("osascript").args(["-e", &format!("POSIX path of ({what} with prompt \"{}\")", prompt.replace('"', "'"))]).output().ok()
    } else if cfg!(target_os = "windows") {
        let script = match kind {
            "folder" => "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }".to_string(),
            "save" => "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.SaveFileDialog; if ($d.ShowDialog() -eq 'OK') { $d.FileName }".to_string(),
            _ => "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; if ($d.ShowDialog() -eq 'OK') { $d.FileName }".to_string(),
        };
        std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().ok()
    } else {
        let zenity = |extra: &[&str]| {
            let mut c = std::process::Command::new("zenity");
            c.args(["--file-selection", &format!("--title={prompt}")]).args(extra);
            c.output().ok()
        };
        let kdialog = |flag: &str| std::process::Command::new("kdialog").args(["--title", prompt, flag, "."]).output().ok();
        match kind {
            "folder" => zenity(&["--directory"]).or_else(|| kdialog("--getexistingdirectory")),
            "save" => zenity(&["--save"]).or_else(|| kdialog("--getsavefilename")),
            _ => zenity(&[]).or_else(|| kdialog("--getopenfilename")),
        }
    };
    let Some(out) = out else {
        eprintln!("select: no file dialog here (on Linux, install zenity or kdialog)");
        return None;
    };
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}
/// select_input(prompt): asks for a file to open.
pub fn select_input(prompt: &str) -> Option<String> { run_dialog(prompt, "open") }
/// select_output(prompt): asks for a file name to save to.
pub fn select_output(prompt: &str) -> Option<String> { run_dialog(prompt, "save") }
/// select_folder(prompt): asks for a folder.
pub fn select_folder(prompt: &str) -> Option<String> { run_dialog(prompt, "folder") }
#[inline] pub fn selectInput(prompt: &str) -> Option<String> { select_input(prompt) }
#[inline] pub fn selectOutput(prompt: &str) -> Option<String> { select_output(prompt) }
#[inline] pub fn selectFolder(prompt: &str) -> Option<String> { select_folder(prompt) }

// ── numbers as text ──────────────────────────────────────────────────────

/// Something hex() and binary() can show: whole numbers, chars, colors.
pub trait Digits { fn bits(&self) -> (u64, u32); }
impl Digits for i32 { fn bits(&self) -> (u64, u32) { (*self as u32 as u64, 32) } }
impl Digits for u32 { fn bits(&self) -> (u64, u32) { (*self as u64, 32) } }
impl Digits for i64 { fn bits(&self) -> (u64, u32) { (*self as u64, 64) } }
impl Digits for u8 { fn bits(&self) -> (u64, u32) { (*self as u64, 8) } }
impl Digits for char { fn bits(&self) -> (u64, u32) { (*self as u64, 16) } }
impl Digits for crate::Color {
    fn bits(&self) -> (u64, u32) {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u64;
        ((b(self.a) << 24) | (b(self.r) << 16) | (b(self.g) << 8) | b(self.b), 32)
    }
}
/// hex(n): the number in hexadecimal, as many digits as its type has (8 for an int).
pub fn hex(v: impl Digits) -> String { let (n, bits) = v.bits(); format!("{:0w$X}", n, w = (bits / 4) as usize) }
/// hex_digits(n, digits): the last `digits` hex digits.
pub fn hex_digits(v: impl Digits, digits: usize) -> String { let s = hex(v); s[s.len().saturating_sub(digits.max(1))..].to_string() }
/// binary(n): the number in base 2, as many digits as its type has.
pub fn binary(v: impl Digits) -> String { let (n, bits) = v.bits(); format!("{:0w$b}", n, w = bits as usize) }
pub fn binary_digits(v: impl Digits, digits: usize) -> String { let s = binary(v); s[s.len().saturating_sub(digits.max(1))..].to_string() }
/// unhex("FF8000"): the number a hex string stands for (0 if it isn't one).
pub fn unhex(s: &str) -> i32 { u32::from_str_radix(s.trim().trim_start_matches('#'), 16).map(|v| v as i32).unwrap_or(0) }
/// unbinary("1010"): the number a binary string stands for.
pub fn unbinary(s: &str) -> i32 { u32::from_str_radix(s.trim(), 2).map(|v| v as i32).unwrap_or(0) }

// ── regular expressions ──────────────────────────────────────────────────

/// match_(text, regex): the whole match and each group, or None. (match is a
/// Rust keyword; match_re works too.)
pub fn match_re(text: &str, pattern: &str) -> Option<Vec<String>> {
    let re = regex::Regex::new(pattern).map_err(|e| eprintln!("match(): {e}")).ok()?;
    let c = re.captures(text)?;
    Some(c.iter().map(|m| m.map(|m| m.as_str().to_string()).unwrap_or_default()).collect())
}
#[inline] pub fn match_(text: &str, pattern: &str) -> Option<Vec<String>> { match_re(text, pattern) }
/// match_all(text, regex): every match, each with its groups.
pub fn match_all(text: &str, pattern: &str) -> Vec<Vec<String>> {
    let Ok(re) = regex::Regex::new(pattern).map_err(|e| eprintln!("matchAll(): {e}")) else { return Vec::new() };
    re.captures_iter(text).map(|c| c.iter().map(|m| m.map(|m| m.as_str().to_string()).unwrap_or_default()).collect()).collect()
}
#[inline] pub fn matchAll(text: &str, pattern: &str) -> Vec<Vec<String>> { match_all(text, pattern) }

// ── odds and ends ────────────────────────────────────────────────────────

/// print_array(list): each item on its own line, with its index.
pub fn print_array<T: std::fmt::Debug>(items: &[T]) {
    for (i, v) in items.iter().enumerate() { println!("[{i}] {v:?}"); }
}
#[inline] pub fn printArray<T: std::fmt::Debug>(items: &[T]) { print_array(items) }
/// thread(f): runs f on its own thread. Drawing calls belong in draw().
pub fn thread(f: impl FnOnce() + Send + 'static) { std::thread::spawn(f); }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xml_round_trip() {
        let x = parse_xml("<?xml version=\"1.0\"?><!-- hi --><root a=\"1\" b='x &amp; y'><item id=\"3\">hello &lt;3</item><item id=\"4\"/><group><deep v=\"2.5\"/></group></root>").unwrap();
        assert_eq!(x.get_name(), "root");
        assert_eq!(x.get_int("a"), 1);
        assert_eq!(x.get_string("b"), "x & y");
        assert_eq!(x.get_children("item").len(), 2);
        assert_eq!(x.get_child("item").unwrap().get_content(), "hello <3");
        assert_eq!(x.get_child("group/deep").unwrap().get_float("v"), 2.5);
        assert_eq!(x.get_children("group/deep").len(), 1);
        let again = parse_xml(&x.format(2)).unwrap();
        assert_eq!(again, x);
        assert!(parse_xml("<a><b></a>").is_err());
        let mut n = Xml::new("list");
        n.add_child("x").set_int("v", 7);
        assert_eq!(n.get_child("x").unwrap().get_int("v"), 7);
    }
    #[test]
    fn numbers() {
        assert_eq!(hex(255), "000000FF");
        assert_eq!(hex_digits(255, 2), "FF");
        assert_eq!(hex('A'), "0041");
        assert_eq!(binary_digits(5, 4), "0101");
        assert_eq!(unhex("FF"), 255);
        assert_eq!(unhex("#FF0000"), 0xFF0000);
        assert_eq!(unbinary("1010"), 10);
        assert_eq!(hex(-1), "FFFFFFFF");
    }
    #[test]
    fn regex_matches() {
        assert_eq!(match_re("width: 300px", r"(\d+)px").unwrap(), vec!["300px", "300"]);
        assert!(match_re("nothing", r"\d").is_none());
        assert_eq!(match_all("a1 b2 c3", r"([a-z])(\d)").len(), 3);
    }
}
