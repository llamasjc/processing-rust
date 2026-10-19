//! Processing's filter shaders: the GLSL fragment shaders that `filter(shader)`
//! runs over the whole canvas. One parser, two back ends: an interpreter for
//! the software renderer, and a translation to libprocessing's WGSL filters.
//!
//! The GLSL that Processing sketches write (1.x style): uniforms, `varying
//! vec4 vertTexCoord`, `gl_FragColor`, `gl_FragCoord`, `texture2D(texture,
//! uv)`, float / int / bool, vec2-4, mat2-4, functions, if / for / while,
//! the usual built-in functions. `#define NAME value` and `#ifdef GL_ES` work;
//! other preprocessor lines are skipped.

use std::collections::HashMap;

// ── types and values ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty { Void, Float, Int, Bool, Vec(u8), Mat(u8), Sampler, Arr(u8, u16) }

impl Ty {
    fn parse(s: &str) -> Option<Ty> {
        Some(match s {
            "void" => Ty::Void, "float" => Ty::Float, "int" => Ty::Int, "bool" => Ty::Bool,
            "vec2" => Ty::Vec(2), "vec3" => Ty::Vec(3), "vec4" => Ty::Vec(4),
            // integer and boolean vectors are kept as float vectors
            "ivec2" | "bvec2" => Ty::Vec(2), "ivec3" | "bvec3" => Ty::Vec(3), "ivec4" | "bvec4" => Ty::Vec(4),
            "mat2" => Ty::Mat(2), "mat3" => Ty::Mat(3), "mat4" => Ty::Mat(4),
            "sampler2D" => Ty::Sampler,
            _ => return None,
        })
    }
    fn size(self) -> usize {
        match self { Ty::Float | Ty::Int | Ty::Bool => 1, Ty::Vec(n) => n as usize, Ty::Mat(n) => (n * n) as usize, _ => 0 }
    }
    /// An array of `len` of this type.
    fn array(self, len: u16) -> Ty {
        let code = match self { Ty::Float => 0, Ty::Int => 1, Ty::Bool => 2, Ty::Vec(n) => n + 1, Ty::Mat(n) => n + 4, _ => 0 };
        Ty::Arr(code, len)
    }
    /// What an array holds.
    pub fn elem(self) -> Ty {
        match self {
            Ty::Arr(c, _) => match c { 0 => Ty::Float, 1 => Ty::Int, 2 => Ty::Bool, 3..=5 => Ty::Vec(c - 1), _ => Ty::Mat(c - 4) },
            t => t,
        }
    }
    pub fn len(self) -> usize { if let Ty::Arr(_, n) = self { n as usize } else { 1 } }
    pub fn size_pub(self) -> usize { self.size() }
    fn wgsl(self) -> String {
        match self {
            Ty::Float => "f32".into(), Ty::Int => "i32".into(), Ty::Bool => "bool".into(),
            Ty::Vec(n) => format!("vec{n}<f32>"), Ty::Mat(n) => format!("mat{n}x{n}<f32>"),
            Ty::Arr(_, n) => format!("array<{}, {n}>", self.elem().wgsl()),
            Ty::Void | Ty::Sampler => String::new(),
        }
    }
}

/// A value while the shader runs on the CPU: numbers in up to 16 floats.
#[derive(Clone, Copy, Debug)]
pub struct Val { ty: Ty, v: [f32; 16] }
impl Val {
    fn f(x: f32) -> Val { let mut v = [0.0; 16]; v[0] = x; Val { ty: Ty::Float, v } }
    fn i(x: i32) -> Val { let mut v = [0.0; 16]; v[0] = x as f32; Val { ty: Ty::Int, v } }
    fn b(x: bool) -> Val { let mut v = [0.0; 16]; v[0] = if x { 1.0 } else { 0.0 }; Val { ty: Ty::Bool, v } }
    fn vec(c: &[f32]) -> Val { let mut v = [0.0; 16]; v[..c.len()].copy_from_slice(c); Val { ty: if c.len() == 1 { Ty::Float } else { Ty::Vec(c.len() as u8) }, v } }
    fn zero(ty: Ty) -> Val { Val { ty, v: [0.0; 16] } }
    fn x(&self) -> f32 { self.v[0] }
    fn truthy(&self) -> bool { self.v[0] != 0.0 }
    fn n(&self) -> usize { self.ty.size() }
}

// ── lexer ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok { Id(String), Num(f64, bool), Op(&'static str), End }

const OPS: [&str; 40] = ["<<=", ">>=", "++", "--", "+=", "-=", "*=", "/=", "%=", "==", "!=", "<=", ">=", "&&", "||", "^^",
    "<<", ">>", "+", "-", "*", "/", "%", "=", "<", ">", "!", "?", ":", ";", ",", ".", "(", ")", "[", "]", "{", "}", "&", "|"];

fn preprocess(src: &str) -> Result<String, String> {
    // #define NAME value (no arguments), #ifdef / #ifndef / #if 0 / #else / #endif;
    // nothing is defined except what the shader defines (GL_ES is not)
    let mut defs: HashMap<String, String> = HashMap::new();
    let mut out = String::new();
    let mut stack: Vec<bool> = Vec::new();   // whether each open block is on
    for line in src.lines() {
        let t = line.trim_start();
        let on = stack.iter().all(|b| *b);
        if let Some(d) = t.strip_prefix('#') {
            let d = d.trim_start();
            let (word, rest) = d.split_once(char::is_whitespace).unwrap_or((d, ""));
            let rest = rest.trim();
            match word {
                "ifdef" => stack.push(defs.contains_key(rest)),
                "ifndef" => stack.push(!defs.contains_key(rest)),
                "if" => stack.push(!(rest == "0" || rest == "false")),
                "else" => { if let Some(top) = stack.last_mut() { *top = !*top; } }
                "elif" => { if let Some(top) = stack.last_mut() { *top = false; } }
                "endif" => { stack.pop(); }
                "define" if on => {
                    let (name, val) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    if name.contains('(') { return Err(format!("#define {name}: macros with arguments aren't supported")); }
                    defs.insert(name.to_string(), val.trim().to_string());
                }
                _ => {}
            }
            out.push('\n');
            continue;
        }
        if on { out.push_str(line); }
        out.push('\n');
    }
    if defs.is_empty() { return Ok(out); }
    // swap defined names for their values, word by word
    let mut res = String::with_capacity(out.len());
    let mut word = String::new();
    for c in out.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' { word.push(c); continue; }
        if !word.is_empty() {
            match defs.get(&word) { Some(v) if !v.is_empty() => res.push_str(v), _ => res.push_str(&word) }
            word.clear();
        }
        res.push(c);
    }
    res.pop();
    Ok(res)
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut line = 1;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c == '\n' { line += 1; i += 1; continue; }
        if c.is_whitespace() { i += 1; continue; }
        if src[i..].starts_with("//") { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        if src[i..].starts_with("/*") {
            let e = src[i + 2..].find("*/").map(|e| i + 2 + e + 2).unwrap_or(b.len());
            line += src[i..e].matches('\n').count();
            i = e;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
            out.push((Tok::Id(src[s..i].to_string()), line));
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < b.len() && (b[i + 1] as char).is_ascii_digit()) {
            let s = i;
            let mut float = false;
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') { if b[i] == b'.' { float = true; } i += 1; }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                float = true;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') { i += 1; }
                while i < b.len() && (b[i] as char).is_ascii_digit() { i += 1; }
            }
            let text = &src[s..i];
            if i < b.len() && (b[i] == b'f' || b[i] == b'F') { float = true; i += 1; }
            if i < b.len() && (b[i] == b'u' || b[i] == b'U') { i += 1; }
            let v: f64 = text.parse().map_err(|_| format!("line {line}: bad number {text}"))?;
            out.push((Tok::Num(v, !float), line));
            continue;
        }
        match OPS.iter().find(|o| src[i..].starts_with(**o)) {
            Some(o) => { out.push((Tok::Op(o), line)); i += o.len(); }
            None => return Err(format!("line {line}: unexpected '{c}'")),
        }
    }
    out.push((Tok::End, line));
    Ok(out)
}

// ── syntax ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64, bool),
    Bool(bool),
    Var(String),
    Call(String, Vec<Expr>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Assign(&'static str, Box<Expr>, Box<Expr>),
    IncDec(&'static str, bool, Box<Expr>), // op, prefix, target
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Decl(Ty, Vec<(String, Option<Expr>)>),
    Expr(Expr),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Return(Option<Expr>),
    Break,
    Continue,
    Discard,
    Block(Vec<Stmt>),
    /// declarations from one line (`float a = 1.0, b[2];`), no new scope
    Decls(Vec<Stmt>),
}

impl Stmt {
    /// Several declarations from one line, kept in the enclosing scope.
    fn flat(self) -> Stmt { if let Stmt::Block(v) = self { Stmt::Decls(v) } else { self } }
}

#[derive(Clone, Debug)]
pub struct Func { pub name: String, pub ret: Ty, pub params: Vec<(Ty, String, bool)>, pub body: Vec<Stmt> }

/// A parsed shader.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub uniforms: Vec<(Ty, String)>,
    pub globals: Vec<(Ty, String, Option<Expr>, bool)>, // const?
    pub funcs: Vec<Func>,
}

struct P { t: Vec<(Tok, usize)>, i: usize }

impl P {
    fn peek(&self) -> &Tok { &self.t[self.i].0 }
    fn peek_at(&self, k: usize) -> &Tok { &self.t[(self.i + k).min(self.t.len() - 1)].0 }
    fn line(&self) -> usize { self.t[self.i].1 }
    fn next(&mut self) -> Tok { let t = self.t[self.i].0.clone(); if self.i < self.t.len() - 1 { self.i += 1; } t }
    fn err<T>(&self, what: &str) -> Result<T, String> { Err(format!("line {}: {}", self.line(), what)) }
    fn is(&self, op: &str) -> bool { matches!(self.peek(), Tok::Op(o) if *o == op) }
    fn eat(&mut self, op: &str) -> bool { if self.is(op) { self.i += 1; true } else { false } }
    fn expect(&mut self, op: &str) -> Result<(), String> { if self.eat(op) { Ok(()) } else { self.err(&format!("expected '{op}'")) } }
    fn ident(&mut self) -> Result<String, String> {
        match self.next() { Tok::Id(s) => Ok(s), _ => { self.i -= 1; self.err("expected a name") } }
    }
    fn is_word(&self, w: &str) -> bool { matches!(self.peek(), Tok::Id(s) if s == w) }
    /// Skips qualifiers like const, highp, in, uniform... and says which were seen.
    fn qualifiers(&mut self) -> (bool, bool, bool, bool) {
        let (mut uniform, mut constant, mut varying, mut out) = (false, false, false, false);
        while let Tok::Id(s) = self.peek() {
            match s.as_str() {
                "uniform" => uniform = true,
                "const" => constant = true,
                "varying" | "attribute" => varying = true,
                "out" | "inout" => out = true,
                "in" | "highp" | "mediump" | "lowp" | "flat" | "smooth" | "precise" | "invariant" => {}
                _ => break,
            }
            self.i += 1;
        }
        (uniform, constant, varying, out)
    }
    fn ty(&mut self) -> Option<Ty> {
        if let Tok::Id(s) = self.peek() { if let Some(t) = Ty::parse(s) { self.i += 1; return Some(t); } }
        None
    }
    /// `[N]` after a name or a type: the array's length.
    fn array_len(&mut self) -> Result<Option<u16>, String> {
        if !self.eat("[") { return Ok(None); }
        let n = match self.next() {
            Tok::Num(v, true) if v >= 1.0 && v <= 4096.0 => v as u16,
            Tok::Op("]") => return self.err("an array needs its length here, like [9]"),
            _ => { self.i -= 1; return self.err("an array's length has to be a whole number, like [9]"); }
        };
        self.expect("]")?;
        Ok(Some(n))
    }

    fn program(&mut self) -> Result<Program, String> {
        let mut p = Program::default();
        while *self.peek() != Tok::End {
            if self.eat(";") { continue; }
            if self.is_word("precision") { while !self.eat(";") { if *self.peek() == Tok::End { break; } self.i += 1; } continue; }
            let (uniform, constant, varying, _) = self.qualifiers();
            let Some(ty) = self.ty() else { return self.err("expected a type") };
            let name = self.ident()?;
            if self.is("(") {
                p.funcs.push(self.func(ty, name)?);
                continue;
            }
            let mut names = vec![(name, None, ty)];
            loop {
                if let Some(n) = self.array_len()? { names.last_mut().unwrap().2 = ty.array(n); }
                if self.eat("=") { let e = self.expr()?; names.last_mut().unwrap().1 = Some(e); }
                if self.eat(",") { names.push((self.ident()?, None, ty)); continue; }
                self.expect(";")?;
                break;
            }
            for (n, init, t) in names {
                if varying { continue; } // vertTexCoord and vertColor are built in
                if uniform { p.uniforms.push((t, n)); } else { p.globals.push((t, n, init, constant)); }
            }
        }
        if !p.funcs.iter().any(|f| f.name == "main") { return Err("the shader has no main()".into()); }
        Ok(p)
    }
    fn func(&mut self, ret: Ty, name: String) -> Result<Func, String> {
        self.expect("(")?;
        let mut params = Vec::new();
        if self.is_word("void") && matches!(self.peek_at(1), Tok::Op(")")) { self.i += 1; }
        while !self.eat(")") {
            let (_, _, _, out) = self.qualifiers();
            let Some(t) = self.ty() else { return self.err("expected a parameter type") };
            let n = self.ident()?;
            params.push((t, n, out));
            if !self.eat(",") { self.expect(")")?; break; }
        }
        if self.eat(";") { return Ok(Func { name, ret, params, body: Vec::new() }); } // a prototype
        self.expect("{")?;
        let body = self.block_rest()?;
        Ok(Func { name, ret, params, body })
    }
    fn block_rest(&mut self) -> Result<Vec<Stmt>, String> {
        let mut v = Vec::new();
        while !self.eat("}") {
            if *self.peek() == Tok::End { return self.err("missing '}'"); }
            v.push(self.stmt()?);
        }
        Ok(v)
    }
    fn stmt(&mut self) -> Result<Stmt, String> {
        if self.eat("{") { return Ok(Stmt::Block(self.block_rest()?)); }
        if self.eat(";") { return Ok(Stmt::Block(Vec::new())); }
        if let Tok::Id(w) = self.peek().clone() {
            match w.as_str() {
                "if" => {
                    self.i += 1; self.expect("(")?; let c = self.expr()?; self.expect(")")?;
                    let a = self.stmt()?;
                    let b = if self.is_word("else") { self.i += 1; Some(Box::new(self.stmt()?)) } else { None };
                    return Ok(Stmt::If(c, Box::new(a), b));
                }
                "for" => {
                    self.i += 1; self.expect("(")?;
                    let init = if self.eat(";") { None } else { Some(Box::new(self.simple_stmt()?)) };
                    let cond = if self.is(";") { None } else { Some(self.expr()?) };
                    self.expect(";")?;
                    let step = if self.is(")") { None } else { Some(self.expr()?) };
                    self.expect(")")?;
                    return Ok(Stmt::For(init, cond, step, Box::new(self.stmt()?)));
                }
                "while" => {
                    self.i += 1; self.expect("(")?; let c = self.expr()?; self.expect(")")?;
                    return Ok(Stmt::While(c, Box::new(self.stmt()?)));
                }
                "do" => {
                    self.i += 1; let body = self.stmt()?;
                    if !self.is_word("while") { return self.err("expected 'while'"); }
                    self.i += 1; self.expect("(")?; let c = self.expr()?; self.expect(")")?; self.expect(";")?;
                    return Ok(Stmt::DoWhile(Box::new(body), c));
                }
                "return" => {
                    self.i += 1;
                    let e = if self.is(";") { None } else { Some(self.expr()?) };
                    self.expect(";")?;
                    return Ok(Stmt::Return(e));
                }
                "break" => { self.i += 1; self.expect(";")?; return Ok(Stmt::Break); }
                "continue" => { self.i += 1; self.expect(";")?; return Ok(Stmt::Continue); }
                "discard" => { self.i += 1; self.expect(";")?; return Ok(Stmt::Discard); }
                _ => {}
            }
        }
        self.simple_stmt()
    }
    /// A declaration or an expression, with its ';'.
    fn simple_stmt(&mut self) -> Result<Stmt, String> {
        let save = self.i;
        self.qualifiers();
        if let Some(mut ty) = self.ty() {
            // float[3] a = ...
            if let Some(n) = self.array_len()? { ty = ty.array(n); }
            if let Tok::Id(_) = self.peek() {
                let mut decls = Vec::new();
                let mut name = self.ident()?;
                loop {
                    let mut t = ty;
                    if let Some(n) = self.array_len()? { t = ty.array(n); }
                    let init = if self.eat("=") { Some(self.expr()?) } else { None };
                    decls.push(Stmt::Decl(t, vec![(name, init)]));
                    if self.eat(",") { name = self.ident()?; continue; }
                    self.expect(";")?;
                    return Ok(if decls.len() == 1 { decls.pop().unwrap() } else { Stmt::Block(decls).flat() });
                }
            }
        }
        self.i = save;
        let e = self.expr()?;
        self.expect(";")?;
        Ok(Stmt::Expr(e))
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut e = self.assign()?;
        while self.is(",") && false { self.i += 1; e = self.assign()?; }
        Ok(e)
    }
    fn assign(&mut self) -> Result<Expr, String> {
        let lhs = self.ternary()?;
        for op in ["=", "+=", "-=", "*=", "/="] {
            if self.eat(op) {
                let rhs = self.assign()?;
                let op: &'static str = match op { "=" => "=", "+=" => "+", "-=" => "-", "*=" => "*", _ => "/" };
                return Ok(Expr::Assign(op, Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }
    fn ternary(&mut self) -> Result<Expr, String> {
        let c = self.binary(0)?;
        if self.eat("?") {
            let a = self.assign()?;
            self.expect(":")?;
            let b = self.assign()?;
            return Ok(Expr::Ternary(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }
    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[&str]; 7] = [&["||"], &["^^"], &["&&"], &["==", "!="], &["<", ">", "<=", ">="], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() { return self.unary(); }
        let mut e = self.binary(level + 1)?;
        loop {
            let Tok::Op(o) = *self.peek() else { break };
            let Some(op) = LEVELS[level].iter().find(|x| **x == o) else { break };
            self.i += 1;
            let r = self.binary(level + 1)?;
            e = Expr::Binary(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        for op in ["-", "!", "+"] {
            if self.eat(op) { let e = self.unary()?; return Ok(if op == "+" { e } else { Expr::Unary(if op == "-" { "-" } else { "!" }, Box::new(e)) }); }
        }
        for op in ["++", "--"] {
            if self.eat(op) { let e = self.unary()?; return Ok(Expr::IncDec(if op == "++" { "+" } else { "-" }, true, Box::new(e))); }
        }
        self.postfix()
    }
    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") { let f = self.ident()?; e = Expr::Field(Box::new(e), f); continue; }
            if self.eat("[") { let i = self.expr()?; self.expect("]")?; e = Expr::Index(Box::new(e), Box::new(i)); continue; }
            if self.eat("++") { e = Expr::IncDec("+", false, Box::new(e)); continue; }
            if self.eat("--") { e = Expr::IncDec("-", false, Box::new(e)); continue; }
            break;
        }
        Ok(e)
    }
    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Tok::Num(v, int) => Ok(Expr::Num(v, int)),
            Tok::Id(s) if s == "true" => Ok(Expr::Bool(true)),
            Tok::Id(s) if s == "false" => Ok(Expr::Bool(false)),
            Tok::Id(s) if Ty::parse(&s).is_some() && self.is("[") => {
                self.i += 1;
                let n = if self.eat("]") { None } else {
                    let n = match self.next() { Tok::Num(v, true) if v >= 1.0 => v as u16, _ => { self.i -= 1; return self.err("an array's length has to be a whole number"); } };
                    self.expect("]")?;
                    Some(n)
                };
                self.expect("(")?;
                let mut args = Vec::new();
                if !self.eat(")") { loop { args.push(self.assign()?); if self.eat(")") { break; } self.expect(",")?; } }
                let n = n.unwrap_or(args.len() as u16);
                if n as usize != args.len() { return self.err(&format!("{s}[{n}] needs {n} values, it has {}", args.len())); }
                Ok(Expr::Call(format!("{s}[{n}]"), args))
            }
            Tok::Id(s) => {
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop { args.push(self.assign()?); if self.eat(")") { break; } self.expect(",")?; }
                    }
                    // ivec / bvec constructors are float vectors here
                    let s = match s.as_str() { "ivec2" | "bvec2" => "vec2".into(), "ivec3" | "bvec3" => "vec3".into(), "ivec4" | "bvec4" => "vec4".into(), _ => s };
                    return Ok(Expr::Call(s, args));
                }
                Ok(Expr::Var(s))
            }
            Tok::Op("(") => { let e = self.expr()?; self.expect(")")?; Ok(e) }
            _ => { self.i -= 1; self.err("expected a value") }
        }
    }
}

/// parse(source): the shader, or what's wrong with it (with the line).
pub fn parse(src: &str) -> Result<Program, String> {
    let src = preprocess(src)?;
    let toks = lex(&src)?;
    P { t: toks, i: 0 }.program()
}

// ── types of expressions ─────────────────────────────────────────────────

/// What a name is while checking or translating.
#[derive(Clone)]
struct Scope { vars: Vec<HashMap<String, Ty>> }
impl Scope {
    fn get(&self, n: &str) -> Option<Ty> { self.vars.iter().rev().find_map(|m| m.get(n).copied()) }
    fn set(&mut self, n: &str, t: Ty) { self.vars.last_mut().unwrap().insert(n.to_string(), t); }
}

/// "float[3]" -> an array type.
fn array_ctor(name: &str) -> Option<Ty> {
    let (base, rest) = name.split_once('[')?;
    let n: u16 = rest.strip_suffix(']')?.parse().ok()?;
    Some(Ty::parse(base)?.array(n))
}

fn swizzle_index(c: char) -> Option<usize> {
    match c { 'x' | 'r' | 's' => Some(0), 'y' | 'g' | 't' => Some(1), 'z' | 'b' | 'p' => Some(2), 'w' | 'a' | 'q' => Some(3), _ => None }
}

/// The built-in names a filter shader can use.
fn builtin_var(n: &str) -> Option<Ty> {
    Some(match n {
        "gl_FragColor" | "gl_FragCoord" | "vertTexCoord" | "vertColor" => Ty::Vec(4),
        _ => return None,
    })
}

fn component_wise(name: &str) -> bool {
    matches!(name, "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "radians" | "degrees" | "exp" | "log" | "exp2" | "log2"
        | "sqrt" | "inversesqrt" | "abs" | "sign" | "floor" | "ceil" | "fract" | "mod" | "min" | "max" | "clamp" | "mix"
        | "step" | "smoothstep" | "pow" | "round" | "trunc" | "sinh" | "cosh" | "tanh")
}

fn type_of(e: &Expr, sc: &Scope, prog: &Program) -> Result<Ty, String> {
    Ok(match e {
        Expr::Num(_, int) => if *int { Ty::Int } else { Ty::Float },
        Expr::Bool(_) => Ty::Bool,
        Expr::Var(n) => sc.get(n).or_else(|| builtin_var(n)).ok_or_else(|| format!("unknown name '{n}'"))?,
        Expr::Field(b, f) => {
            let bt = type_of(b, sc, prog)?;
            match bt {
                Ty::Vec(_) => {
                    if f.chars().any(|c| swizzle_index(c).is_none()) || f.len() > 4 { return Err(format!("no field '.{f}'")); }
                    if f.len() == 1 { Ty::Float } else { Ty::Vec(f.len() as u8) }
                }
                Ty::Float | Ty::Int if f.chars().all(|c| matches!(c, 'x' | 'r' | 's')) => if f.len() == 1 { Ty::Float } else { Ty::Vec(f.len() as u8) },
                _ => return Err(format!("no field '.{f}'")),
            }
        }
        Expr::Index(b, _) => match type_of(b, sc, prog)? { Ty::Vec(_) => Ty::Float, Ty::Mat(n) => Ty::Vec(n), t @ Ty::Arr(..) => t.elem(), _ => return Err("only arrays, vectors and matrices take [ ]".into()) },
        Expr::Unary("!", _) => Ty::Bool,
        Expr::Unary(_, x) => { let t = type_of(x, sc, prog)?; if t == Ty::Bool { return Err("'-' on a bool".into()); } t }
        Expr::IncDec(_, _, x) => type_of(x, sc, prog)?,
        Expr::Assign(_, l, _) => type_of(l, sc, prog)?,
        Expr::Ternary(_, a, b) => { let (ta, tb) = (type_of(a, sc, prog)?, type_of(b, sc, prog)?); if ta == Ty::Int { tb } else { ta } }
        Expr::Binary(op, a, b) => {
            if matches!(*op, "==" | "!=" | "<" | ">" | "<=" | ">=" | "&&" | "||" | "^^") { return Ok(Ty::Bool); }
            let (ta, tb) = (type_of(a, sc, prog)?, type_of(b, sc, prog)?);
            match (ta, tb) {
                (Ty::Int, Ty::Int) => Ty::Int,
                (Ty::Mat(n), Ty::Vec(_)) if *op == "*" => Ty::Vec(n),
                (Ty::Vec(n), Ty::Mat(_)) if *op == "*" => Ty::Vec(n),
                (Ty::Mat(n), _) | (_, Ty::Mat(n)) => Ty::Mat(n),
                (Ty::Vec(n), _) | (_, Ty::Vec(n)) => Ty::Vec(n),
                _ => Ty::Float,
            }
        }
        Expr::Call(name, args) => {
            if let Some(t) = Ty::parse(name) { return Ok(t); }
            if let Some(t) = array_ctor(name) { return Ok(t); }
            let at = |i: usize| -> Result<Ty, String> { type_of(args.get(i).ok_or_else(|| format!("{name}() needs more arguments"))?, sc, prog) };
            match name.as_str() {
                "texture2D" | "texture" => Ty::Vec(4),
                "length" | "distance" | "dot" => Ty::Float,
                "cross" => Ty::Vec(3),
                "normalize" | "reflect" | "faceforward" | "refract" => at(0)?,
                "step" | "smoothstep" => { let last = at(args.len().saturating_sub(1))?; if last == Ty::Int { Ty::Float } else { last } }
                n if component_wise(n) => {
                    let mut t = Ty::Float;
                    for i in 0..args.len() { if let Ty::Vec(k) = at(i)? { t = Ty::Vec(k); } }
                    t
                }
                _ => match prog.funcs.iter().find(|f| f.name == *name) {
                    Some(f) => f.ret,
                    None => return Err(format!("unknown function {name}()")),
                },
            }
        }
    })
}

// ── checking ─────────────────────────────────────────────────────────────

/// Checks every expression's type once, so mistakes show when the shader is
/// loaded rather than when it runs.
pub fn check(prog: &Program) -> Result<(), String> {
    let mut sc = Scope { vars: vec![HashMap::new()] };
    for (t, n) in &prog.uniforms { sc.set(n, *t); }
    for (t, n, init, _) in &prog.globals {
        if let Some(e) = init { type_of(e, &sc, prog).map_err(|m| format!("{n}: {m}"))?; }
        sc.set(n, *t);
    }
    for f in &prog.funcs {
        let mut s = sc.clone();
        s.vars.push(HashMap::new());
        for (t, n, _) in &f.params { s.set(n, *t); }
        check_block(&f.body, &mut s, prog).map_err(|m| format!("in {}(): {m}", f.name))?;
    }
    Ok(())
}
fn check_block(b: &[Stmt], sc: &mut Scope, prog: &Program) -> Result<(), String> {
    sc.vars.push(HashMap::new());
    for s in b { check_stmt(s, sc, prog)?; }
    sc.vars.pop();
    Ok(())
}
fn check_stmt(s: &Stmt, sc: &mut Scope, prog: &Program) -> Result<(), String> {
    match s {
        Stmt::Decl(t, names) => for (n, init) in names {
            if let Some(e) = init { type_of(e, sc, prog)?; }
            sc.set(n, *t);
        },
        Stmt::Expr(e) | Stmt::Return(Some(e)) => { type_of(e, sc, prog)?; }
        Stmt::If(c, a, b) => {
            type_of(c, sc, prog)?;
            check_block(std::slice::from_ref(a), sc, prog)?;
            if let Some(b) = b { check_block(std::slice::from_ref(b), sc, prog)?; }
        }
        Stmt::For(i, c, st, body) => {
            sc.vars.push(HashMap::new());
            if let Some(i) = i { check_stmt(i, sc, prog)?; }
            if let Some(c) = c { type_of(c, sc, prog)?; }
            if let Some(st) = st { type_of(st, sc, prog)?; }
            check_block(std::slice::from_ref(body), sc, prog)?;
            sc.vars.pop();
        }
        Stmt::While(c, body) | Stmt::DoWhile(body, c) => { type_of(c, sc, prog)?; check_block(std::slice::from_ref(body), sc, prog)?; }
        Stmt::Block(b) => check_block(b, sc, prog)?,
        Stmt::Decls(v) => for s in v { check_stmt(s, sc, prog)?; },
        _ => {}
    }
    Ok(())
}

// ── the interpreter (software renderer) ──────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum Bi { Sin, Cos, Tan, Asin, Acos, Atan, Atan2, Radians, Degrees, Exp, Log, Exp2, Log2, Sqrt, InvSqrt, Abs, Sign, Floor,
    Ceil, Fract, Mod, Min, Max, Clamp, Mix, Step, Smoothstep, Pow, Round, Trunc, Sinh, Cosh, Tanh,
    Length, Distance, Dot, Cross, Normalize, Reflect, Texture }

#[derive(Clone, Debug)]
enum Acc { Swz([u8; 4], u8), Idx(Box<C>) }

#[derive(Clone, Debug)]
enum C {
    K(Val),
    L(usize),
    G(usize),
    Acc(Box<C>, Acc, Ty),
    Neg(Box<C>),
    Not(Box<C>),
    Bin(&'static str, Box<C>, Box<C>, Ty),
    And(Box<C>, Box<C>),
    Or(Box<C>, Box<C>),
    Tern(Box<C>, Box<C>, Box<C>),
    Make(Ty, Vec<C>),
    Bi(Bi, Vec<C>),
    Call(usize, Vec<C>, Vec<(usize, LV)>), // function, args, out params copied back
    ArrIdx(bool, usize, usize, Box<C>),    // local?, first slot, length, index
    Set(LV, &'static str, Box<C>, Ty),     // op ("=" or + - * /), value, type of the target
    IncDec(LV, bool, bool, Ty),            // +?, prefix?
}
#[derive(Clone, Debug)]
struct LV { local: bool, slot: usize, arr: Option<(usize, Box<C>)>, path: Vec<Acc> }

#[derive(Clone, Debug)]
enum S {
    E(C),
    Init(usize, C),
    InitArr(usize, Vec<C>),
    Zero(usize, Ty),
    If(C, Vec<S>, Vec<S>),
    Loop(Vec<S>, Option<C>, Option<C>, Vec<S>, bool), // init, cond, step, body, cond at the end?
    Ret(Option<C>),
    Brk,
    Cont,
    Discard,
}

#[derive(Clone, Debug)]
struct CFunc { slots: usize, params: usize, ret: Ty, body: Vec<S> }

/// A shader ready to run on the CPU.
#[derive(Clone, Debug)]
pub struct Cpu {
    funcs: Vec<CFunc>,
    main: usize,
    globals: Vec<Val>,          // starting values
    global_init: Vec<(usize, C)>,
    pub uniforms: HashMap<String, (usize, Ty)>,
}

// builtin variables live in the first global slots
const G_FRAGCOLOR: usize = 0;
const G_FRAGCOORD: usize = 1;
const G_TEXCOORD: usize = 2;
const G_VERTCOLOR: usize = 3;

struct Comp<'a> {
    prog: &'a Program,
    globals: HashMap<String, (usize, Ty)>,
    nglobals: usize,
    fn_index: HashMap<String, usize>,
    scopes: Vec<HashMap<String, (usize, Ty)>>,
    nslots: usize,
}

fn bi(name: &str, nargs: usize) -> Option<Bi> {
    Some(match name {
        "sin" => Bi::Sin, "cos" => Bi::Cos, "tan" => Bi::Tan, "asin" => Bi::Asin, "acos" => Bi::Acos,
        "atan" => if nargs == 2 { Bi::Atan2 } else { Bi::Atan },
        "radians" => Bi::Radians, "degrees" => Bi::Degrees, "exp" => Bi::Exp, "log" => Bi::Log, "exp2" => Bi::Exp2,
        "log2" => Bi::Log2, "sqrt" => Bi::Sqrt, "inversesqrt" => Bi::InvSqrt, "abs" => Bi::Abs, "sign" => Bi::Sign,
        "floor" => Bi::Floor, "ceil" => Bi::Ceil, "fract" => Bi::Fract, "mod" => Bi::Mod, "min" => Bi::Min, "max" => Bi::Max,
        "clamp" => Bi::Clamp, "mix" => Bi::Mix, "step" => Bi::Step, "smoothstep" => Bi::Smoothstep, "pow" => Bi::Pow,
        "round" => Bi::Round, "trunc" => Bi::Trunc, "sinh" => Bi::Sinh, "cosh" => Bi::Cosh, "tanh" => Bi::Tanh,
        "length" => Bi::Length, "distance" => Bi::Distance, "dot" => Bi::Dot, "cross" => Bi::Cross,
        "normalize" => Bi::Normalize, "reflect" => Bi::Reflect, "texture2D" | "texture" => Bi::Texture,
        _ => return None,
    })
}

impl Comp<'_> {
    fn scope(&self) -> Scope {
        let mut s = Scope { vars: vec![HashMap::new()] };
        for (n, (_, t)) in &self.globals { s.set(n, *t); }
        for m in &self.scopes { s.vars.push(m.iter().map(|(k, (_, t))| (k.clone(), *t)).collect()); }
        s
    }
    fn ty(&self, e: &Expr) -> Result<Ty, String> { type_of(e, &self.scope(), self.prog) }
    fn local(&mut self, n: &str, t: Ty) -> usize {
        let s = self.nslots;
        self.nslots += t.len();
        self.scopes.last_mut().unwrap().insert(n.to_string(), (s, t));
        s
    }
    fn find(&self, n: &str) -> Option<(bool, usize, Ty)> {
        for m in self.scopes.iter().rev() { if let Some(&(s, t)) = m.get(n) { return Some((true, s, t)); } }
        self.globals.get(n).map(|&(s, t)| (false, s, t))
    }
    fn lvalue(&mut self, e: &Expr) -> Result<LV, String> {
        match e {
            Expr::Var(n) => {
                let (local, slot, t) = self.find(n).ok_or_else(|| format!("unknown name '{n}'"))?;
                if matches!(t, Ty::Arr(..)) { return Err(format!("'{n}' is an array: assign to one item, like {n}[0]")); }
                Ok(LV { local, slot, arr: None, path: Vec::new() })
            }
            Expr::Field(b, f) => { let mut lv = self.lvalue(b)?; lv.path.push(swz(f)); Ok(lv) }
            Expr::Index(b, i) => {
                // an item of an array: a slot picked when it runs
                if let Expr::Var(n) = &**b {
                    if let Some((local, slot, t @ Ty::Arr(..))) = self.find(n) {
                        let i = self.expr(i)?;
                        return Ok(LV { local, slot, arr: Some((t.len(), Box::new(i))), path: Vec::new() });
                    }
                }
                let mut lv = self.lvalue(b)?; let i = self.expr(i)?; lv.path.push(Acc::Idx(Box::new(i))); Ok(lv)
            }
            _ => Err("this can't be assigned to".into()),
        }
    }
    fn expr(&mut self, e: &Expr) -> Result<C, String> {
        Ok(match e {
            Expr::Num(v, int) => C::K(if *int { Val::i(*v as i32) } else { Val::f(*v as f32) }),
            Expr::Bool(b) => C::K(Val::b(*b)),
            Expr::Var(n) => match self.find(n) {
                Some((_, _, Ty::Arr(..))) => return Err(format!("'{n}' is an array: use one item, like {n}[i]")),
                Some((true, s, _)) => C::L(s), Some((false, s, _)) => C::G(s), None => return Err(format!("unknown name '{n}'")),
            },
            Expr::Field(b, f) => { let t = self.ty(e)?; C::Acc(Box::new(self.expr(b)?), swz(f), t) }
            Expr::Index(b, i) => {
                if let Expr::Var(n) = &**b {
                    if let Some((local, slot, t @ Ty::Arr(..))) = self.find(n) {
                        return Ok(C::ArrIdx(local, slot, t.len(), Box::new(self.expr(i)?)));
                    }
                }
                let t = self.ty(e)?; C::Acc(Box::new(self.expr(b)?), Acc::Idx(Box::new(self.expr(i)?)), t)
            }
            Expr::Unary("!", x) => C::Not(Box::new(self.expr(x)?)),
            Expr::Unary(_, x) => C::Neg(Box::new(self.expr(x)?)),
            Expr::Binary("&&", a, b) => C::And(Box::new(self.expr(a)?), Box::new(self.expr(b)?)),
            Expr::Binary("||", a, b) => C::Or(Box::new(self.expr(a)?), Box::new(self.expr(b)?)),
            Expr::Binary(op, a, b) => { let t = self.ty(e)?; C::Bin(op, Box::new(self.expr(a)?), Box::new(self.expr(b)?), t) }
            Expr::Ternary(c, a, b) => C::Tern(Box::new(self.expr(c)?), Box::new(self.expr(a)?), Box::new(self.expr(b)?)),
            Expr::Assign(op, l, r) => { let t = self.ty(l)?; C::Set(self.lvalue(l)?, op, Box::new(self.expr(r)?), t) }
            Expr::IncDec(op, pre, x) => { let t = self.ty(x)?; C::IncDec(self.lvalue(x)?, *op == "+", *pre, t) }
            Expr::Call(name, args) => {
                if array_ctor(name).is_some() { return Err("an array's values only go in its declaration: float w[3] = float[3](...)".into()); }
                let ca: Vec<C> = args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?;
                if let Some(t) = Ty::parse(name) { return Ok(C::Make(t, ca)); }
                if let Some(b) = bi(name, args.len()) {
                    return Ok(C::Bi(b, ca));
                }
                let Some(&fi) = self.fn_index.get(name) else { return Err(format!("unknown function {name}()")) };
                let f = &self.prog.funcs.iter().filter(|f| !f.body.is_empty() || f.name == "main").nth(fi).unwrap();
                let mut outs = Vec::new();
                for (i, (_, _, out)) in f.params.iter().enumerate() {
                    if *out { outs.push((i, self.lvalue(&args[i])?)); }
                }
                C::Call(fi, ca, outs)
            }
        })
    }
    /// The starting items of an array: from float[N](...), or zeros.
    fn array_init(&mut self, t: Ty, init: Option<&Expr>) -> Result<Vec<C>, String> {
        let et = t.elem();
        match init {
            None => Ok((0..t.len()).map(|_| C::K(Val::zero(et))).collect()),
            Some(Expr::Call(name, args)) if array_ctor(name).is_some() => {
                if args.len() != t.len() { return Err(format!("the array has {} items, {name} gives {}", t.len(), args.len())); }
                args.iter().map(|a| Ok(C::Make(et, vec![self.expr(a)?]))).collect()
            }
            Some(_) => Err("an array starts from float[N](...) (or another array type's), not from another array".into()),
        }
    }
    fn block(&mut self, b: &[Stmt]) -> Result<Vec<S>, String> {
        self.scopes.push(HashMap::new());
        let r = b.iter().map(|s| self.stmt(s)).collect::<Result<Vec<_>, _>>();
        self.scopes.pop();
        r
    }
    fn stmt(&mut self, s: &Stmt) -> Result<S, String> {
        Ok(match s {
            Stmt::Decl(t, names) => {
                let mut v = Vec::new();
                for (n, init) in names {
                    if let Ty::Arr(..) = t {
                        let items = self.array_init(*t, init.as_ref())?;
                        let slot = self.local(n, *t);
                        v.push(S::InitArr(slot, items));
                        continue;
                    }
                    let c = match init { Some(e) => Some(self.expr(e)?), None => None };
                    let slot = self.local(n, *t);
                    v.push(match c { Some(c) => S::Init(slot, C::Make(*t, vec![c])), None => S::Zero(slot, *t) });
                }
                if v.len() == 1 { v.pop().unwrap() } else { S::If(C::K(Val::b(true)), v, Vec::new()) }
            }
            Stmt::Decls(list) => {
                let mut v = Vec::new();
                for d in list { v.push(self.stmt(d)?); }
                S::If(C::K(Val::b(true)), v, Vec::new())
            }
            Stmt::Expr(e) => S::E(self.expr(e)?),
            Stmt::If(c, a, b) => S::If(self.expr(c)?, self.block(std::slice::from_ref(a))?, match b { Some(b) => self.block(std::slice::from_ref(b))?, None => Vec::new() }),
            Stmt::For(i, c, st, body) => {
                self.scopes.push(HashMap::new());
                let init = match i { Some(i) => vec![self.stmt(i)?], None => Vec::new() };
                let cond = match c { Some(c) => Some(self.expr(c)?), None => None };
                let step = match st { Some(s) => Some(self.expr(s)?), None => None };
                let body = self.block(std::slice::from_ref(body))?;
                self.scopes.pop();
                S::Loop(init, cond, step, body, false)
            }
            Stmt::While(c, body) => { let c = self.expr(c)?; S::Loop(Vec::new(), Some(c), None, self.block(std::slice::from_ref(body))?, false) }
            Stmt::DoWhile(body, c) => { let b = self.block(std::slice::from_ref(body))?; S::Loop(Vec::new(), Some(self.expr(c)?), None, b, true) }
            Stmt::Return(e) => S::Ret(match e { Some(e) => Some(self.expr(e)?), None => None }),
            Stmt::Break => S::Brk,
            Stmt::Continue => S::Cont,
            Stmt::Discard => S::Discard,
            Stmt::Block(b) => S::If(C::K(Val::b(true)), self.block(b)?, Vec::new()),
        })
    }
}

fn swz(f: &str) -> Acc {
    let mut idx = [0u8; 4];
    for (i, c) in f.chars().take(4).enumerate() { idx[i] = swizzle_index(c).unwrap_or(0) as u8; }
    Acc::Swz(idx, f.len().min(4) as u8)
}

/// compile(program): the shader made ready for the CPU.
pub fn compile(prog: &Program) -> Result<Cpu, String> {
    check(prog)?;
    let mut globals = HashMap::new();
    for (i, n) in ["gl_FragColor", "gl_FragCoord", "vertTexCoord", "vertColor"].iter().enumerate() { globals.insert(n.to_string(), (i, Ty::Vec(4))); }
    let mut n = 4;
    let mut uniforms = HashMap::new();
    let mut start = vec![Val::zero(Ty::Vec(4)); 4];
    for (t, name) in &prog.uniforms {
        globals.insert(name.clone(), (n, *t));
        uniforms.insert(name.clone(), (n, *t));
        for _ in 0..t.len() { start.push(Val::zero(t.elem())); }
        n += t.len();
    }
    let bodies: Vec<&Func> = prog.funcs.iter().filter(|f| !f.body.is_empty() || f.name == "main").collect();
    let fn_index: HashMap<String, usize> = bodies.iter().enumerate().map(|(i, f)| (f.name.clone(), i)).collect();
    let mut c = Comp { prog, globals, nglobals: n, fn_index, scopes: Vec::new(), nslots: 0 };
    let mut global_init = Vec::new();
    for (t, name, init, _) in &prog.globals {
        let slot = c.nglobals;
        c.nglobals += t.len();
        for _ in 0..t.len() { start.push(Val::zero(t.elem())); }
        c.scopes.clear();
        if let Ty::Arr(..) = t {
            let items = c.array_init(*t, init.as_ref())?;
            for (k, it) in items.into_iter().enumerate() { global_init.push((slot + k, it)); }
        } else if let Some(e) = init {
            let ce = c.expr(e)?; global_init.push((slot, C::Make(*t, vec![ce])));
        }
        c.globals.insert(name.clone(), (slot, *t));
    }
    let mut funcs = Vec::new();
    for f in &bodies {
        c.scopes = vec![HashMap::new()];
        c.nslots = 0;
        for (t, n, _) in &f.params { c.local(n, *t); }
        let body = c.block(&f.body)?;
        funcs.push(CFunc { slots: c.nslots, params: f.params.len(), ret: f.ret, body });
    }
    let main = c.fn_index["main"];
    Ok(Cpu { funcs, main, globals: start, global_init, uniforms })
}

enum Flow { Go, Brk, Cont, Ret(Val), Discard }

/// See Cpu::one_pixel.
pub struct OnePixel<'a> {
    cpu: &'a Cpu,
    w: usize,
    h: usize,
    src: Vec<u32>,
    images: Vec<(usize, usize, Vec<u32>)>,
    g: std::cell::RefCell<(Vec<Val>, Vec<Val>)>,
}
impl OnePixel<'_> {
    /// The shader's color at pixel (x, y) for a shape of color `vert`, or
    /// None if it discarded the pixel.
    pub fn color(&self, x: usize, y: usize, vert: [f32; 4]) -> Option<[f32; 4]> {
        let (w, h) = (self.w, self.h);
        let ctx = Ctx { w, h, px: &self.src, images: &self.images };
        let mut st = self.g.borrow_mut();
        let (g, frame) = &mut *st;
        g[G_FRAGCOORD] = Val::vec(&[x as f32 + 0.5, h.saturating_sub(1 + y) as f32 + 0.5, 0.0, 1.0]);
        g[G_TEXCOORD] = Val::vec(&[(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, 0.0, 1.0]);
        g[G_VERTCOLOR] = Val::vec(&vert);
        g[G_FRAGCOLOR] = Val::vec(&vert);
        match self.cpu.exec_block(&self.cpu.funcs[self.cpu.main].body, frame, g, &ctx) {
            Flow::Discard => None,
            _ => { let c = g[G_FRAGCOLOR].v; Some([c[0], c[1], c[2], c[3]]) }
        }
    }
}

/// What a running shader can see: the canvas to sample.
pub struct Ctx<'a> { pub w: usize, pub h: usize, pub px: &'a [u32], pub images: &'a [(usize, usize, Vec<u32>)] }

/// One picture a shader can sample: the canvas, or an image set as a uniform.
struct Pic<'a> { w: usize, h: usize, px: &'a [u32] }

impl Ctx<'_> {
    /// Bilinear sample of picture `which` (0 the canvas, then the images) at
    /// (u, v), 0..1 from the top left, clamped at the edges.
    fn sample(&self, which: usize, u: f32, v: f32) -> [f32; 4] {
        let pic = match which {
            0 => Pic { w: self.w, h: self.h, px: self.px },
            i => match self.images.get(i - 1) { Some((w, h, px)) => Pic { w: *w, h: *h, px }, None => return [0.0; 4] },
        };
        pic.sample(u, v)
    }
}
impl Pic<'_> {
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if self.w == 0 || self.h == 0 || self.px.len() < self.w * self.h { return [0.0; 4]; }
        let (w, h) = (self.w as f32, self.h as f32);
        let (x, y) = (u * w - 0.5, v * h - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let get = |xi: f32, yi: f32| -> [f32; 4] {
            let xi = (xi as i64).clamp(0, self.w as i64 - 1) as usize;
            let yi = (yi as i64).clamp(0, self.h as i64 - 1) as usize;
            let p = self.px[yi * self.w + xi];
            [((p >> 16) & 255) as f32 / 255.0, ((p >> 8) & 255) as f32 / 255.0, (p & 255) as f32 / 255.0, ((p >> 24) & 255) as f32 / 255.0]
        };
        let (a, b, c, d) = (get(x0, y0), get(x0 + 1.0, y0), get(x0, y0 + 1.0), get(x0 + 1.0, y0 + 1.0));
        let mut o = [0.0; 4];
        for i in 0..4 { o[i] = (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy; }
        o
    }
}

fn convert(t: Ty, args: &[Val]) -> Val {
    // vec4(1.0) fills, vec4(v.xy, 0, 1) concatenates, mat2(1.0) is a diagonal
    let mut flat: Vec<f32> = Vec::with_capacity(16);
    for a in args { flat.extend_from_slice(&a.v[..a.n()]); }
    let mut out = Val::zero(t);
    match t {
        Ty::Float => out.v[0] = flat.first().copied().unwrap_or(0.0),
        Ty::Int => out.v[0] = flat.first().copied().unwrap_or(0.0).trunc(),
        Ty::Bool => out.v[0] = if flat.first().copied().unwrap_or(0.0) != 0.0 { 1.0 } else { 0.0 },
        Ty::Vec(n) => {
            let n = n as usize;
            if args.len() == 1 && args[0].n() == 1 { for i in 0..n { out.v[i] = flat[0]; } } else { for i in 0..n.min(flat.len()) { out.v[i] = flat[i]; } }
        }
        Ty::Mat(n) => {
            let n = n as usize;
            if args.len() == 1 && args[0].n() == 1 { for i in 0..n { out.v[i * n + i] = flat[0]; } }
            else if args.len() == 1 && matches!(args[0].ty, Ty::Mat(_)) {
                let Ty::Mat(m) = args[0].ty else { unreachable!() };
                let m = m as usize;
                for c in 0..n { for r in 0..n { out.v[c * n + r] = if c < m && r < m { args[0].v[c * m + r] } else if c == r { 1.0 } else { 0.0 }; } }
            } else { for i in 0..(n * n).min(flat.len()) { out.v[i] = flat[i]; } }
        }
        _ => {}
    }
    out
}

impl Cpu {
    pub fn set_uniform(&mut self, name: &str, vals: &[f32]) -> bool {
        let Some(&(slot, t)) = self.uniforms.get(name) else { return false };
        if let Ty::Arr(..) = t {
            // the numbers fill the items in turn
            let et = t.elem();
            let k = et.size().max(1);
            for i in 0..t.len() {
                let mut v = Val::zero(et);
                for j in 0..k { v.v[j] = vals.get(i * k + j).copied().unwrap_or(0.0); }
                if et == Ty::Int { v.v[0] = v.v[0].trunc(); }
                self.globals[slot + i] = v;
            }
            return true;
        }
        let mut v = Val::zero(t);
        for (i, x) in vals.iter().take(t.size().max(1)).enumerate() { v.v[i] = *x; }
        if t == Ty::Int { v.v[0] = v.v[0].trunc(); }
        self.globals[slot] = v;
        true
    }
    pub fn has_uniform(&self, name: &str) -> bool { self.uniforms.contains_key(name) }

    /// Runs the shader over the whole canvas `src`, writing the result to `dst`.
    pub fn run(&self, w: usize, h: usize, src: &[u32], dst: &mut [u32], images: &[(usize, usize, Vec<u32>)]) {
        if w == 0 || h == 0 { return; }
        let ctx = Ctx { w, h, px: src, images };
        let mut g = self.globals.clone();
        // global initializers run once
        for (slot, e) in &self.global_init { let v = self.eval(e, &mut [], &mut g, &ctx); g[*slot] = v; }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
        let rows_per = h.div_ceil(threads);
        std::thread::scope(|s| {
            for (ti, chunk) in dst.chunks_mut(rows_per * w).enumerate() {
                let (ctx, g0) = (&ctx, &g);
                s.spawn(move || {
                    let mut g = g0.clone();
                    let mut frame = vec![Val::zero(Ty::Float); self.funcs[self.main].slots];
                    for (k, out) in chunk.iter_mut().enumerate() {
                        let i = ti * rows_per * w + k;
                        let (x, y) = (i % w, i / w);
                        g[G_FRAGCOORD] = Val::vec(&[x as f32 + 0.5, (h - 1 - y) as f32 + 0.5, 0.0, 1.0]);
                        g[G_TEXCOORD] = Val::vec(&[(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, 0.0, 1.0]);
                        g[G_VERTCOLOR] = Val::vec(&[1.0, 1.0, 1.0, 1.0]);
                        g[G_FRAGCOLOR] = Val::vec(&[0.0, 0.0, 0.0, 1.0]);
                        match self.exec_block(&self.funcs[self.main].body, &mut frame, &mut g, ctx) {
                            Flow::Discard => *out = src[i],
                            _ => {
                                let c = g[G_FRAGCOLOR].v;
                                let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                                *out = (b(c[3]) << 24) | (b(c[0]) << 16) | (b(c[1]) << 8) | b(c[2]);
                            }
                        }
                    }
                });
            }
        });
    }

    /// Runs the shader for some pixels of a shape (shader() in Processing):
    /// vertColor is the fill, the canvas `src` is what texture2D sees. None
    /// for a pixel the shader discarded.
    pub fn run_points(&self, w: usize, h: usize, src: &[u32], images: &[(usize, usize, Vec<u32>)],
                      pts: &[(usize, usize)], vert_color: [f32; 4]) -> Vec<Option<[f32; 4]>> {
        let ctx = Ctx { w, h, px: src, images };
        let mut g = self.globals.clone();
        for (slot, e) in &self.global_init { let v = self.eval(e, &mut [], &mut g, &ctx); g[*slot] = v; }
        let mut out = vec![None; pts.len()];
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
        let per = pts.len().div_ceil(threads).max(256);
        std::thread::scope(|s| {
            for (chunk, outs) in pts.chunks(per).zip(out.chunks_mut(per)) {
                let (ctx, g0) = (&ctx, &g);
                s.spawn(move || {
                    let mut g = g0.clone();
                    let mut frame = vec![Val::zero(Ty::Float); self.funcs[self.main].slots];
                    for (&(x, y), o) in chunk.iter().zip(outs.iter_mut()) {
                        g[G_FRAGCOORD] = Val::vec(&[x as f32 + 0.5, (h - 1 - y) as f32 + 0.5, 0.0, 1.0]);
                        g[G_TEXCOORD] = Val::vec(&[(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, 0.0, 1.0]);
                        g[G_VERTCOLOR] = Val::vec(&vert_color);
                        g[G_FRAGCOLOR] = Val::vec(&vert_color);
                        *o = match self.exec_block(&self.funcs[self.main].body, &mut frame, &mut g, ctx) {
                            Flow::Discard => None,
                            _ => { let c = g[G_FRAGCOLOR].v; Some([c[0], c[1], c[2], c[3]]) }
                        };
                    }
                });
            }
        });
        out
    }

    /// A shader ready to color one pixel at a time (3D triangles).
    pub fn one_pixel(&self, w: usize, h: usize, src: Vec<u32>, images: Vec<(usize, usize, Vec<u32>)>) -> OnePixel<'_> {
        let mut g = self.globals.clone();
        {
            let ctx = Ctx { w, h, px: &src, images: &images };
            for (slot, e) in &self.global_init { let v = self.eval(e, &mut [], &mut g, &ctx); g[*slot] = v; }
        }
        let frame = vec![Val::zero(Ty::Float); self.funcs[self.main].slots];
        OnePixel { cpu: self, w, h, src, images, g: std::cell::RefCell::new((g, frame)) }
    }

    fn exec_block(&self, b: &[S], f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) -> Flow {
        for s in b {
            match self.exec(s, f, g, ctx) { Flow::Go => {} other => return other }
        }
        Flow::Go
    }
    fn exec(&self, s: &S, f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) -> Flow {
        match s {
            S::E(e) => { self.eval(e, f, g, ctx); }
            S::Init(slot, e) => { let v = self.eval(e, f, g, ctx); f[*slot] = v; }
            S::InitArr(slot, items) => for (k, e) in items.iter().enumerate() { let v = self.eval(e, f, g, ctx); f[*slot + k] = v; },
            S::Zero(slot, t) => f[*slot] = Val::zero(*t),
            S::If(c, a, b) => return if self.eval(c, f, g, ctx).truthy() { self.exec_block(a, f, g, ctx) } else { self.exec_block(b, f, g, ctx) },
            S::Loop(init, cond, step, body, cond_last) => {
                for s in init { self.exec(s, f, g, ctx); }
                let mut guard = 0u32;
                loop {
                    if !*cond_last { if let Some(c) = cond { if !self.eval(c, f, g, ctx).truthy() { break; } } }
                    match self.exec_block(body, f, g, ctx) {
                        Flow::Brk => break,
                        Flow::Ret(v) => return Flow::Ret(v),
                        Flow::Discard => return Flow::Discard,
                        _ => {}
                    }
                    if let Some(st) = step { self.eval(st, f, g, ctx); }
                    if *cond_last { if let Some(c) = cond { if !self.eval(c, f, g, ctx).truthy() { break; } } }
                    guard += 1;
                    if guard > 100_000 { break; } // a shader loop that never ends
                }
            }
            S::Ret(e) => return Flow::Ret(match e { Some(e) => self.eval(e, f, g, ctx), None => Val::zero(Ty::Void) }),
            S::Brk => return Flow::Brk,
            S::Cont => return Flow::Cont,
            S::Discard => return Flow::Discard,
        }
        Flow::Go
    }

    fn read(&self, base: Val, acc: &Acc, ty: Ty, f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) -> Val {
        match acc {
            Acc::Swz(idx, n) => { let mut o = Val::zero(ty); for i in 0..*n as usize { o.v[i] = base.v[idx[i] as usize]; } o }
            Acc::Idx(i) => {
                let i = self.eval(i, f, g, ctx).x().max(0.0) as usize;
                match base.ty {
                    Ty::Mat(n) => { let n = n as usize; let i = i.min(n - 1); Val::vec(&base.v[i * n..i * n + n]) }
                    _ => Val::f(base.v[i.min(base.n().max(1) - 1)]),
                }
            }
        }
    }
    /// The slot an lvalue names, with an array's item picked.
    fn lv_slot(&self, lv: &LV, f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) -> usize {
        match &lv.arr {
            Some((len, i)) => lv.slot + (self.eval(i, f, g, ctx).x().max(0.0) as usize).min(len - 1),
            None => lv.slot,
        }
    }
    fn get_lv(&self, lv: &LV, f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) -> Val {
        let slot = self.lv_slot(lv, f, g, ctx);
        let mut v = if lv.local { f[slot] } else { g[slot] };
        for a in &lv.path {
            let ty = match (a, v.ty) { (Acc::Swz(_, 1), _) | (Acc::Idx(_), Ty::Vec(_)) => Ty::Float, (Acc::Swz(_, n), _) => Ty::Vec(*n), (Acc::Idx(_), Ty::Mat(n)) => Ty::Vec(n), _ => Ty::Float };
            v = self.read(v, a, ty, f, g, ctx);
        }
        v
    }
    fn set_lv(&self, lv: &LV, val: Val, f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) {
        // walk down to find which components of the root the path names
        let slot = self.lv_slot(lv, f, g, ctx);
        let root = if lv.local { f[slot] } else { g[slot] };
        let mut comps: Vec<usize> = (0..root.n()).collect();
        let mut ty = root.ty;
        for a in &lv.path {
            match a {
                Acc::Swz(idx, n) => { comps = (0..*n as usize).map(|i| comps[(idx[i] as usize).min(comps.len() - 1)]).collect(); ty = if *n == 1 { Ty::Float } else { Ty::Vec(*n) }; }
                Acc::Idx(i) => {
                    let i = self.eval(i, f, g, ctx).x().max(0.0) as usize;
                    if let Ty::Mat(n) = ty { let n = n as usize; let i = i.min(n - 1); comps = comps[i * n..i * n + n].to_vec(); ty = Ty::Vec(n as u8); }
                    else { comps = vec![comps[i.min(comps.len() - 1)]]; ty = Ty::Float; }
                }
            }
        }
        let val = if val.n() == 1 && comps.len() > 1 { convert(ty, &[val]) } else { val };
        let target = if lv.local { &mut f[slot] } else { &mut g[slot] };
        for (k, c) in comps.iter().enumerate() { target.v[*c] = val.v[k.min(15)]; }
        if lv.path.is_empty() && target.ty == Ty::Int { target.v[0] = target.v[0].trunc(); }
    }

    fn eval(&self, e: &C, f: &mut [Val], g: &mut Vec<Val>, ctx: &Ctx) -> Val {
        match e {
            C::K(v) => *v,
            C::L(s) => f[*s],
            C::G(s) => g[*s],
            C::ArrIdx(local, base, len, i) => {
                let i = (self.eval(i, f, g, ctx).x().max(0.0) as usize).min(len - 1);
                if *local { f[base + i] } else { g[base + i] }
            }
            C::Acc(b, a, t) => { let b = self.eval(b, f, g, ctx); self.read(b, a, *t, f, g, ctx) }
            C::Neg(x) => { let mut v = self.eval(x, f, g, ctx); for i in 0..v.n() { v.v[i] = -v.v[i]; } v }
            C::Not(x) => Val::b(!self.eval(x, f, g, ctx).truthy()),
            C::And(a, b) => Val::b(self.eval(a, f, g, ctx).truthy() && self.eval(b, f, g, ctx).truthy()),
            C::Or(a, b) => Val::b(self.eval(a, f, g, ctx).truthy() || self.eval(b, f, g, ctx).truthy()),
            C::Tern(c, a, b) => if self.eval(c, f, g, ctx).truthy() { self.eval(a, f, g, ctx) } else { self.eval(b, f, g, ctx) },
            C::Bin(op, a, b, t) => { let (a, b) = (self.eval(a, f, g, ctx), self.eval(b, f, g, ctx)); binop(op, a, b, *t) }
            C::Make(t, args) => {
                let vals: Vec<Val> = args.iter().map(|a| self.eval(a, f, g, ctx)).collect();
                if vals.len() == 1 && vals[0].ty == *t { return vals[0]; }
                convert(*t, &vals)
            }
            C::Bi(b, args) => {
                let mut v = [Val::zero(Ty::Float); 3];
                for (i, a) in args.iter().take(3).enumerate() { v[i] = self.eval(a, f, g, ctx); }
                builtin(*b, &v[..args.len().min(3)], ctx)
            }
            C::Call(fi, args, outs) => {
                let func = &self.funcs[*fi];
                let mut frame = vec![Val::zero(Ty::Float); func.slots.max(func.params)];
                for (i, a) in args.iter().enumerate().take(func.params) { frame[i] = self.eval(a, f, g, ctx); }
                let r = match self.exec_block(&func.body, &mut frame, g, ctx) { Flow::Ret(v) => v, _ => Val::zero(func.ret) };
                for (i, lv) in outs { let v = frame[*i]; self.set_lv(lv, v, f, g, ctx); }
                r
            }
            C::Set(lv, op, x, t) => {
                let mut v = self.eval(x, f, g, ctx);
                if *op != "=" { let cur = self.get_lv(lv, f, g, ctx); v = binop(op, cur, v, *t); }
                if v.ty != *t && !(v.n() == 1 && t.size() == 1) { v = convert(*t, &[v]); }
                self.set_lv(lv, v, f, g, ctx);
                v
            }
            C::IncDec(lv, inc, pre, t) => {
                let cur = self.get_lv(lv, f, g, ctx);
                let next = binop(if *inc { "+" } else { "-" }, cur, Val::f(1.0), *t);
                self.set_lv(lv, next, f, g, ctx);
                if *pre { next } else { cur }
            }
        }
    }
}

fn binop(op: &str, a: Val, b: Val, t: Ty) -> Val {
    match op {
        "==" => return Val::b((0..a.n().max(b.n())).all(|i| a.v[i] == b.v[i])),
        "!=" => return Val::b((0..a.n().max(b.n())).any(|i| a.v[i] != b.v[i])),
        "<" => return Val::b(a.x() < b.x()),
        ">" => return Val::b(a.x() > b.x()),
        "<=" => return Val::b(a.x() <= b.x()),
        ">=" => return Val::b(a.x() >= b.x()),
        "^^" => return Val::b(a.truthy() != b.truthy()),
        _ => {}
    }
    // matrix products
    if op == "*" {
        match (a.ty, b.ty) {
            (Ty::Mat(n), Ty::Vec(_)) => { let n = n as usize; let mut o = Val::zero(Ty::Vec(n as u8)); for r in 0..n { o.v[r] = (0..n).map(|c| a.v[c * n + r] * b.v[c]).sum(); } return o; }
            (Ty::Vec(_), Ty::Mat(n)) => { let n = n as usize; let mut o = Val::zero(Ty::Vec(n as u8)); for c in 0..n { o.v[c] = (0..n).map(|r| a.v[r] * b.v[c * n + r]).sum(); } return o; }
            (Ty::Mat(n), Ty::Mat(_)) => {
                let n = n as usize;
                let mut o = Val::zero(Ty::Mat(n as u8));
                for c in 0..n { for r in 0..n { o.v[c * n + r] = (0..n).map(|k| a.v[k * n + r] * b.v[c * n + k]).sum(); } }
                return o;
            }
            _ => {}
        }
    }
    let n = t.size().max(1);
    let mut o = Val::zero(t);
    let int = t == Ty::Int;
    for i in 0..n {
        let x = if a.n() == 1 { a.v[0] } else { a.v[i] };
        let y = if b.n() == 1 { b.v[0] } else { b.v[i] };
        o.v[i] = match op {
            "+" => x + y, "-" => x - y, "*" => x * y,
            "/" => if int { if y == 0.0 { 0.0 } else { (x / y).trunc() } } else { x / y },
            "%" => if y == 0.0 { 0.0 } else { x % y },
            _ => 0.0,
        };
    }
    o
}

fn builtin(b: Bi, a: &[Val], ctx: &Ctx) -> Val {
    let cw = |a: &[Val], f: &dyn Fn(&[f32]) -> f32| -> Val {
        let n = a.iter().map(|v| v.n()).max().unwrap_or(1);
        let ty = if n == 1 { Ty::Float } else { Ty::Vec(n as u8) };
        let mut o = Val::zero(ty);
        let mut xs = [0.0f32; 3];
        for i in 0..n {
            for (k, v) in a.iter().enumerate() { xs[k] = if v.n() == 1 { v.v[0] } else { v.v[i] }; }
            o.v[i] = f(&xs[..a.len()]);
        }
        o
    };
    let len = |v: &Val| (0..v.n()).map(|i| v.v[i] * v.v[i]).sum::<f32>().sqrt();
    let dot = |x: &Val, y: &Val| (0..x.n()).map(|i| x.v[i] * y.v[i]).sum::<f32>();
    match b {
        Bi::Sin => cw(a, &|x| x[0].sin()), Bi::Cos => cw(a, &|x| x[0].cos()), Bi::Tan => cw(a, &|x| x[0].tan()),
        Bi::Asin => cw(a, &|x| x[0].asin()), Bi::Acos => cw(a, &|x| x[0].acos()), Bi::Atan => cw(a, &|x| x[0].atan()),
        Bi::Atan2 => cw(a, &|x| x[0].atan2(x[1])),
        Bi::Radians => cw(a, &|x| x[0].to_radians()), Bi::Degrees => cw(a, &|x| x[0].to_degrees()),
        Bi::Exp => cw(a, &|x| x[0].exp()), Bi::Log => cw(a, &|x| x[0].ln()), Bi::Exp2 => cw(a, &|x| x[0].exp2()),
        Bi::Log2 => cw(a, &|x| x[0].log2()), Bi::Sqrt => cw(a, &|x| x[0].sqrt()), Bi::InvSqrt => cw(a, &|x| 1.0 / x[0].sqrt()),
        Bi::Abs => cw(a, &|x| x[0].abs()), Bi::Sign => cw(a, &|x| if x[0] > 0.0 { 1.0 } else if x[0] < 0.0 { -1.0 } else { 0.0 }),
        Bi::Floor => cw(a, &|x| x[0].floor()), Bi::Ceil => cw(a, &|x| x[0].ceil()), Bi::Fract => cw(a, &|x| x[0] - x[0].floor()),
        Bi::Mod => cw(a, &|x| x[0] - x[1] * (x[0] / x[1]).floor()),
        Bi::Min => cw(a, &|x| x[0].min(x[1])), Bi::Max => cw(a, &|x| x[0].max(x[1])),
        Bi::Clamp => cw(a, &|x| x[0].max(x[1]).min(x[2])),
        Bi::Mix => cw(a, &|x| x[0] * (1.0 - x[2]) + x[1] * x[2]),
        Bi::Step => cw(a, &|x| if x[1] < x[0] { 0.0 } else { 1.0 }),
        Bi::Smoothstep => cw(a, &|x| { let t = ((x[2] - x[0]) / (x[1] - x[0])).clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }),
        Bi::Pow => cw(a, &|x| x[0].powf(x[1])), Bi::Round => cw(a, &|x| x[0].round()), Bi::Trunc => cw(a, &|x| x[0].trunc()),
        Bi::Sinh => cw(a, &|x| x[0].sinh()), Bi::Cosh => cw(a, &|x| x[0].cosh()), Bi::Tanh => cw(a, &|x| x[0].tanh()),
        Bi::Length => Val::f(len(&a[0])),
        Bi::Distance => { let d = binop("-", a[0], a[1], a[0].ty); Val::f(len(&d)) }
        Bi::Dot => Val::f(dot(&a[0], &a[1])),
        Bi::Cross => { let (x, y) = (a[0].v, a[1].v); Val::vec(&[x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]]) }
        Bi::Normalize => { let l = len(&a[0]); let mut o = a[0]; if l > 0.0 { for i in 0..o.n() { o.v[i] /= l; } } o }
        Bi::Reflect => { let d = dot(&a[1], &a[0]); let mut o = a[0]; for i in 0..o.n() { o.v[i] -= 2.0 * d * a[1].v[i]; } o }
        Bi::Texture => {
            let which = a.first().map(|v| v.v[0].max(0.0) as usize).unwrap_or(0);
            let uv = a.get(1).map(|v| v.v).unwrap_or([0.0; 16]);
            Val::vec(&ctx.sample(which, uv[0], uv[1]))
        }
    }
}

// ── translation to WGSL (libprocessing) ──────────────────────────────────

const WGSL_RESERVED: &[&str] = &["array", "atomic", "bitcast", "break", "case", "const", "continue", "continuing", "default",
    "discard", "else", "enable", "fallthrough", "fn", "for", "if", "let", "loop", "override", "private", "ptr", "return",
    "sampler", "storage", "struct", "switch", "texture", "type", "uniform", "var", "while", "workgroup", "alias", "diagnostic",
    "requires", "module", "self", "super", "target", "params", "sample", "fragment", "in", "out", "f32", "i32", "u32", "bool"];

/// A GLSL name that is also a WGSL word gets a prefix; uniforms keep theirs
/// only when they can.
fn wname(n: &str) -> String {
    if WGSL_RESERVED.contains(&n) || n.starts_with("__") { format!("g_{n}") } else { n.to_string() }
}

/// What the WGSL is for: a filter over the canvas, or a material for shapes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target { Filter, Material }

struct W<'a> {
    target: Target,
    prog: &'a Program,
    sc: Scope,
    uniforms: HashMap<String, Ty>,
    ptr_params: Vec<String>,   // out / inout parameters of the function being written
    out: String,
    tmp: usize,
}

fn lit(v: f64, as_float: bool) -> String {
    if as_float { let s = format!("{v:?}"); if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") } } else { format!("{}", v as i64) }
}

impl W<'_> {
    fn ty(&self, e: &Expr) -> Result<Ty, String> { type_of(e, &self.sc, self.prog) }
    /// The expression, as type `want` (ints become floats where a float is wanted).
    fn ex_as(&mut self, e: &Expr, want: Ty) -> Result<String, String> {
        let t = self.ty(e)?;
        if let Expr::Num(v, _) = e { if matches!(want, Ty::Float | Ty::Vec(_) | Ty::Mat(_)) { return Ok(lit(*v, true)); } }
        let s = self.ex(e)?;
        Ok(match (t, want) {
            (Ty::Int, Ty::Float) | (Ty::Bool, Ty::Float) => format!("f32({s})"),
            (Ty::Float, Ty::Int) | (Ty::Bool, Ty::Int) => format!("i32({s})"),
            (Ty::Int, Ty::Vec(n)) | (Ty::Bool, Ty::Vec(n)) => format!("vec{n}<f32>(f32({s}))"),
            (Ty::Int, Ty::Bool) | (Ty::Float, Ty::Bool) => format!("({s} != {})", if t == Ty::Int { "0" } else { "0.0" }),
            _ => s,
        })
    }
    fn var(&self, n: &str) -> String {
        match n {
            "gl_FragColor" => "glsl_frag_color".into(),
            "gl_FragCoord" => "glsl_frag_coord".into(),
            "vertTexCoord" => "glsl_tex_coord".into(),
            "vertColor" => if self.target == Target::Material { "glsl_vert_color".into() } else { "vec4<f32>(1.0)".into() },
            _ if self.uniforms.contains_key(n) && !self.sc.vars.iter().skip(1).any(|m| m.contains_key(n)) => match n {
                "texOffset" if self.target == Target::Filter => "texel_size()".into(),
                "resolution" if self.target == Target::Filter => "resolution()".into(),
                "texOffset" => "(1.0 / params.glsl_resolution)".into(),
                "resolution" => "params.glsl_resolution".into(),
                _ => match self.uniforms[n] { Ty::Bool => format!("(params.{n} != 0)"), Ty::Arr(..) => format!("glsl_u_{n}"), _ => format!("params.{n}") },
            },
            _ if self.ptr_params.iter().any(|p| p == n) => format!("(*{})", wname(n)),
            _ => wname(n),
        }
    }
    fn ex(&mut self, e: &Expr) -> Result<String, String> {
        Ok(match e {
            Expr::Num(v, int) => lit(*v, !*int),
            Expr::Bool(b) => b.to_string(),
            Expr::Var(n) => self.var(n),
            Expr::Field(b, f) => {
                let bt = self.ty(b)?;
                let base = self.ex(b)?;
                let f: String = f.chars().map(|c| ['x', 'y', 'z', 'w'][swizzle_index(c).unwrap_or(0)]).collect();
                if matches!(bt, Ty::Float | Ty::Int) { if f.len() == 1 { base } else { format!("vec{}<f32>(f32({base}))", f.len()) } } else { format!("{base}.{f}") }
            }
            Expr::Index(b, i) => { let (b, i) = (self.ex(b)?, self.ex_as(i, Ty::Int)?); format!("{b}[{i}]") }
            Expr::Unary("!", x) => format!("(!{})", self.ex_as(x, Ty::Bool)?),
            Expr::Unary(_, x) => format!("(-{})", self.ex(x)?),
            Expr::Binary(op, a, b) => {
                let (ta, tb) = (self.ty(a)?, self.ty(b)?);
                if matches!(*op, "&&" | "||") { return Ok(format!("({} {op} {})", self.ex_as(a, Ty::Bool)?, self.ex_as(b, Ty::Bool)?)); }
                if *op == "^^" { return Ok(format!("({} != {})", self.ex_as(a, Ty::Bool)?, self.ex_as(b, Ty::Bool)?)); }
                // one side float or vector: ints on the other side become floats
                let floaty = |t: Ty| matches!(t, Ty::Float | Ty::Vec(_) | Ty::Mat(_));
                let (sa, sb) = if floaty(ta) || floaty(tb) {
                    let wa = if ta == Ty::Int || ta == Ty::Bool { Ty::Float } else { ta };
                    let wb = if tb == Ty::Int || tb == Ty::Bool { Ty::Float } else { tb };
                    (self.ex_as(a, wa)?, self.ex_as(b, wb)?)
                } else { (self.ex(a)?, self.ex(b)?) };
                if matches!(*op, "==" | "!=") && matches!(ta, Ty::Vec(_)) {
                    return Ok(if *op == "==" { format!("all({sa} == {sb})") } else { format!("any({sa} != {sb})") });
                }
                // a scalar on the left of a vector op is fine in WGSL
                format!("({sa} {op} {sb})")
            }
            Expr::Ternary(c, a, b) => {
                let t = self.ty(e)?;
                let (c, a, b) = (self.ex_as(c, Ty::Bool)?, self.ex_as(a, t)?, self.ex_as(b, t)?);
                format!("select({b}, {a}, {c})")
            }
            Expr::Assign(..) | Expr::IncDec(..) => return Err("put the assignment (or ++ / --) on its own line; WGSL has them only as statements".into()),
            Expr::Call(name, args) => self.call(name, args)?,
        })
    }
    fn call(&mut self, name: &str, args: &[Expr]) -> Result<String, String> {
        if let Some(t) = array_ctor(name) {
            let et = t.elem();
            let parts: Vec<String> = args.iter().map(|a| self.ex_as(a, et)).collect::<Result<_, _>>()?;
            return Ok(format!("{}({})", t.wgsl(), parts.join(", ")));
        }
        if let Some(t) = Ty::parse(name) {
            let n = t.size();
            // casts and constructors: everything inside is float, except int() and bool()
            let inner = match t { Ty::Int => Ty::Int, Ty::Bool => Ty::Bool, _ => Ty::Float };
            let mut parts = Vec::new();
            for a in args {
                let at = self.ty(a)?;
                let want = match at { Ty::Vec(k) => Ty::Vec(k), Ty::Mat(k) => Ty::Mat(k), _ => inner };
                parts.push(self.ex_as(a, want)?);
            }
            return Ok(match t {
                Ty::Float | Ty::Int | Ty::Bool => {
                    let at = self.ty(&args[0])?;
                    let s = parts[0].clone();
                    match (t, at) {
                        (Ty::Bool, _) => s,
                        (_, Ty::Vec(_)) => format!("{}({s}.x)", t.wgsl()),
                        _ => format!("{}({s})", t.wgsl()),
                    }
                }
                Ty::Mat(k) if args.len() == 1 && self.ty(&args[0])?.size() == 1 => {
                    let id: Vec<String> = (0..k).map(|c| format!("vec{k}<f32>({})", (0..k).map(|r| if r == c { "1.0" } else { "0.0" }).collect::<Vec<_>>().join(", "))).collect();
                    format!("(mat{k}x{k}<f32>({}) * {})", id.join(", "), parts[0])
                }
                _ => { let _ = n; format!("{}({})", t.wgsl(), parts.join(", ")) }
            });
        }
        let t = type_of(&Expr::Call(name.to_string(), args.to_vec()), &self.sc, self.prog)?;
        let arg_as = |w: &mut W, i: usize, want: Ty| w.ex_as(&args[i], want);
        let f32_or = |t: Ty| if matches!(t, Ty::Int | Ty::Bool) { Ty::Float } else { t };
        Ok(match name {
            "texture2D" | "texture" => {
                let uv = arg_as(self, 1, Ty::Vec(2))?;
                match args.first() {
                    Some(Expr::Var(s)) if s == "texture" && self.target == Target::Filter => format!("glsl_sample({uv})"),
                    Some(Expr::Var(s)) if self.uniforms.get(s.as_str()) == Some(&Ty::Sampler) =>
                        format!("textureSampleLevel(tex_{s}, {}, {uv}, 0.0)", if self.target == Target::Filter { "texture_sampler".to_string() } else { format!("smp_{s}") }),
                    _ => return Err("texture2D() takes a sampler2D uniform".into()),
                }
            }
            "mod" => {
                let w = f32_or(t);
                let a = arg_as(self, 0, w)?;
                let bt = self.ty(&args[1])?;
                let b = self.ex_as(&args[1], if matches!(bt, Ty::Vec(_)) { bt } else { Ty::Float })?;
                match w {
                    Ty::Vec(n) => { let b = if matches!(bt, Ty::Vec(_)) { b } else { format!("vec{n}<f32>({b})") }; format!("glsl_mod{n}({a}, {b})") }
                    _ => format!("glsl_mod({a}, {b})"),
                }
            }
            "atan" if args.len() == 2 => format!("atan2({}, {})", arg_as(self, 0, f32_or(t))?, arg_as(self, 1, f32_or(t))?),
            "inversesqrt" => format!("inverseSqrt({})", arg_as(self, 0, f32_or(t))?),
            "length" | "normalize" => { let at = f32_or(self.ty(&args[0])?); format!("{name}({})", arg_as(self, 0, at)?) }
            "distance" | "dot" | "reflect" | "cross" => { let at = f32_or(self.ty(&args[0])?); format!("{name}({}, {})", arg_as(self, 0, at)?, arg_as(self, 1, at)?) }
            "mix" | "clamp" | "smoothstep" | "step" | "min" | "max" | "pow" => {
                // WGSL wants the same type everywhere: scalars widen to the vector
                let w = f32_or(t);
                let parts: Vec<String> = (0..args.len()).map(|i| {
                    let at = self.ty(&args[i])?;
                    let s = self.ex_as(&args[i], if matches!(at, Ty::Vec(_)) { at } else { Ty::Float })?;
                    Ok(match (w, at) { (Ty::Vec(n), Ty::Float | Ty::Int) => format!("vec{n}<f32>({s})"), _ => s })
                }).collect::<Result<_, String>>()?;
                format!("{name}({})", parts.join(", "))
            }
            n if component_wise(n) => {
                let w = f32_or(t);
                let parts: Vec<String> = (0..args.len()).map(|i| arg_as(self, i, w)).collect::<Result<_, _>>()?;
                format!("{n}({})", parts.join(", "))
            }
            _ => {
                let f = self.prog.funcs.iter().find(|f| f.name == name && !f.body.is_empty()).ok_or_else(|| format!("unknown function {name}()"))?;
                let params = f.params.clone();
                let mut parts = Vec::new();
                for (i, (pt, _, out)) in params.iter().enumerate() {
                    if *out {
                        let Some(Expr::Var(v)) = args.get(i) else { return Err(format!("{name}(): an out parameter needs a plain variable")) };
                        parts.push(if self.ptr_params.iter().any(|p| p == v) { wname(v) } else { format!("&{}", wname(v)) });
                    } else {
                        parts.push(arg_as(self, i, *pt)?);
                    }
                }
                format!("{}({})", wname(name), parts.join(", "))
            }
        })
    }

    fn line(&mut self, depth: usize, s: &str) { for _ in 0..depth { self.out.push_str("    "); } self.out.push_str(s); self.out.push('\n'); }
    fn block(&mut self, b: &[Stmt], depth: usize) -> Result<(), String> {
        self.sc.vars.push(HashMap::new());
        for s in b { self.stmt(s, depth)?; }
        self.sc.vars.pop();
        Ok(())
    }
    /// An assignment as WGSL statements: several swizzled parts get one line each.
    fn assign(&mut self, op: &str, l: &Expr, r: &Expr, depth: usize) -> Result<(), String> {
        let lt = self.ty(l)?;
        let rhs = if op == "=" { self.ex_as(r, lt)? } else {
            let rt = self.ty(r)?;
            let rr = self.ex_as(r, if matches!(rt, Ty::Vec(_) | Ty::Mat(_)) { rt } else if lt == Ty::Int { Ty::Int } else { Ty::Float })?;
            let cur = self.ex(l)?;
            format!("({cur} {op} {rr})")
        };
        if let Expr::Field(b, f) = l {
            if f.len() > 1 {
                // v.xy = e  becomes  t = e; v.x = t.x; v.y = t.y;
                let base = self.ex(b)?;
                self.tmp += 1;
                let t = format!("glsl_t{}", self.tmp);
                self.line(depth, &format!("let {t} = {rhs};"));
                for (i, c) in f.chars().enumerate() {
                    let comp = ['x', 'y', 'z', 'w'][swizzle_index(c).unwrap_or(0)];
                    self.line(depth, &format!("{base}.{comp} = {t}.{};", ['x', 'y', 'z', 'w'][i]));
                }
                return Ok(());
            }
        }
        let target = self.ex(l)?;
        self.line(depth, &format!("{target} = {rhs};"));
        Ok(())
    }
    fn expr_stmt(&mut self, e: &Expr, depth: usize) -> Result<(), String> {
        match e {
            Expr::Assign(op, l, r) => self.assign(op, l, r, depth),
            Expr::IncDec(op, _, x) => {
                let t = self.ty(x)?;
                let one = Expr::Num(1.0, t == Ty::Int);
                self.assign(op, x, &one, depth)
            }
            Expr::Call(..) => { let s = self.ex(e)?; self.line(depth, &format!("{s};")); Ok(()) }
            _ => { let s = self.ex(e)?; self.line(depth, &format!("_ = {s};")); Ok(()) }
        }
    }
    fn stmt(&mut self, s: &Stmt, depth: usize) -> Result<(), String> {
        match s {
            Stmt::Decl(t, names) => for (n, init) in names {
                let v = match init { Some(e) => self.ex_as(e, *t)?, None => format!("{}()", t.wgsl()) };
                self.sc.set(n, *t);
                self.line(depth, &format!("var {}: {} = {v};", wname(n), t.wgsl()));
            },
            Stmt::Decls(list) => for d in list { self.stmt(d, depth)?; },
            Stmt::Expr(e) => self.expr_stmt(e, depth)?,
            Stmt::If(c, a, b) => {
                let c = self.ex_as(c, Ty::Bool)?;
                self.line(depth, &format!("if ({c}) {{"));
                self.block(std::slice::from_ref(a), depth + 1)?;
                if let Some(b) = b { self.line(depth, "} else {"); self.block(std::slice::from_ref(b), depth + 1)?; }
                self.line(depth, "}");
            }
            Stmt::For(init, cond, step, body) => {
                // as a loop, so any init, condition and step work
                self.line(depth, "{");
                self.sc.vars.push(HashMap::new());
                if let Some(i) = init { self.stmt(i, depth + 1)?; }
                self.line(depth + 1, "loop {");
                if let Some(c) = cond { let c = self.ex_as(c, Ty::Bool)?; self.line(depth + 2, &format!("if (!({c})) {{ break; }}")); }
                self.block(std::slice::from_ref(body), depth + 2)?;
                if let Some(st) = step {
                    self.line(depth + 2, "continuing {");
                    self.expr_stmt(st, depth + 3)?;
                    self.line(depth + 2, "}");
                }
                self.line(depth + 1, "}");
                self.sc.vars.pop();
                self.line(depth, "}");
            }
            Stmt::While(c, body) => {
                let c = self.ex_as(c, Ty::Bool)?;
                self.line(depth, &format!("loop {{ if (!({c})) {{ break; }}"));
                self.block(std::slice::from_ref(body), depth + 1)?;
                self.line(depth, "}");
            }
            Stmt::DoWhile(body, c) => {
                self.line(depth, "loop {");
                self.block(std::slice::from_ref(body), depth + 1)?;
                let c = self.ex_as(c, Ty::Bool)?;
                self.line(depth + 1, &format!("continuing {{ break if !({c}); }}"));
                self.line(depth, "}");
            }
            Stmt::Return(e) => match e {
                Some(e) => {
                    let rt = self.sc.get("__ret").unwrap_or(Ty::Float);
                    let s = self.ex_as(e, rt)?;
                    self.line(depth, &format!("return {s};"));
                }
                None => self.line(depth, "return;"),
            },
            Stmt::Break => self.line(depth, "break;"),
            Stmt::Continue => self.line(depth, "continue;"),
            Stmt::Discard => self.line(depth, "discard;"),
            Stmt::Block(b) => { self.line(depth, "{"); self.block(b, depth + 1)?; self.line(depth, "}"); }
        }
        Ok(())
    }
}

/// to_wgsl(program): a libprocessing filter that does what the GLSL shader does.
/// Uniforms other than texture, texOffset and resolution become fields of
/// `params`, settable by their own names.
pub fn to_wgsl(prog: &Program) -> Result<String, String> { to_wgsl_for(prog, Target::Filter) }

/// to_wgsl_for(program, target): as a filter, or as a material for shapes
/// (shader() in Processing), where vertColor is the shape's fill and
/// vertTexCoord its texture coordinates.
pub fn to_wgsl_for(prog: &Program, target: Target) -> Result<String, String> {
    check(prog)?;
    let mut w = W { target, prog, sc: Scope { vars: vec![HashMap::new()] }, uniforms: HashMap::new(), ptr_params: Vec::new(), out: String::new(), tmp: 0 };
    let group = if target == Target::Filter { 1 } else { 3 };
    if target == Target::Filter {
        w.out.push_str("import processing::filter::{screen_texture, texture_sampler, texel_size, resolution, FullscreenVertexOutput};\n\n");
    } else {
        w.out.push_str("import processing::VertexOutput;\n\n");
    }
    let mut fields: Vec<(Ty, String)> = prog.uniforms.iter()
        .filter(|(t, n)| *t != Ty::Sampler && n != "texOffset" && n != "resolution")
        .cloned().collect();
    // a material doesn't know the canvas size: it's a uniform of its own
    if target == Target::Material {
        fields.push((Ty::Vec(2), "glsl_resolution".into()));
        fields.push((Ty::Vec(4), "glsl_fill".into()));
    }
    for (t, n) in &prog.uniforms {
        w.uniforms.insert(n.clone(), *t);
        w.sc.set(n, *t);
    }
    if !fields.is_empty() {
        w.out.push_str("struct GlslParams {\n");
        for (t, n) in &fields {
            if WGSL_RESERVED.contains(&n.as_str()) { return Err(format!("uniform `{n}`: that name is taken in WGSL, please rename it")); }
            // an array is one field per item (name_0, name_1...): uniform
            // arrays in WGSL need 16-byte steps, and items can be set by name
            let items: Vec<(String, Ty)> = match t {
                Ty::Arr(..) => (0..t.len()).map(|i| (format!("{n}_{i}"), t.elem())).collect(),
                _ => vec![(n.clone(), *t)],
            };
            for (fname, ft) in items {
                let ty = if ft == Ty::Bool { "i32".to_string() } else { ft.wgsl() };
                w.out.push_str(&format!("    {fname}: {ty},\n"));
            }
        }
        w.out.push_str(&format!("}}\n@group({group}) @binding(0) var<uniform> params: GlslParams;\n\n"));
    }
    // images set as uniforms: one texture each, named after the uniform (and
    // its own sampler in a material)
    let mut binding = 1;
    for (t, n) in &prog.uniforms {
        if *t == Ty::Sampler && (n != "texture" || target == Target::Material) {
            w.out.push_str(&format!("@group({group}) @binding({binding}) var tex_{n}: texture_2d<f32>;\n"));
            binding += 1;
            if target == Target::Material {
                w.out.push_str(&format!("@group({group}) @binding({binding}) var smp_{n}: sampler;\n"));
                binding += 1;
            }
        }
    }
    w.out.push_str("var<private> glsl_frag_color: vec4<f32>;\nvar<private> glsl_frag_coord: vec4<f32>;\nvar<private> glsl_tex_coord: vec4<f32>;\nvar<private> glsl_vert_color: vec4<f32>;\n");
    if target == Target::Filter {
        w.out.push_str("fn glsl_sample(uv: vec2<f32>) -> vec4<f32> { return textureSampleLevel(screen_texture, texture_sampler, uv, 0.0); }\n");
    }
    w.out.push_str("fn glsl_mod(x: f32, y: f32) -> f32 { return x - y * floor(x / y); }\n");
    for n in 2..=4 { w.out.push_str(&format!("fn glsl_mod{n}(x: vec{n}<f32>, y: vec{n}<f32>) -> vec{n}<f32> {{ return x - y * floor(x / y); }}\n")); }
    for (t, n, _, _) in &prog.globals {
        w.out.push_str(&format!("var<private> {}: {};\n", wname(n), t.wgsl()));
        w.sc.set(n, *t);
    }
    for (t, n) in &prog.uniforms {
        if let Ty::Arr(..) = t { w.out.push_str(&format!("var<private> glsl_u_{n}: {};\n", t.wgsl())); }
    }
    w.out.push('\n');
    for f in prog.funcs.iter().filter(|f| !f.body.is_empty()) {
        w.sc.vars.push(HashMap::new());
        w.ptr_params.clear();
        let mut ps = Vec::new();
        for (t, n, out) in &f.params {
            w.sc.set(n, *t);
            if *out { w.ptr_params.push(n.clone()); ps.push(format!("{}: ptr<function, {}>", wname(n), t.wgsl())); }
            else { ps.push(format!("{}_in: {}", wname(n), t.wgsl())); }
        }
        w.sc.set("__ret", f.ret);
        let name = if f.name == "main" { "glsl_main".to_string() } else { wname(&f.name) };
        let ret = if f.ret == Ty::Void { String::new() } else { format!(" -> {}", f.ret.wgsl()) };
        w.out.push_str(&format!("fn {name}({}){ret} {{\n", ps.join(", ")));
        // parameters can be changed in GLSL; in WGSL they are copied to vars
        for (t, n, out) in &f.params { if !*out { let line = format!("var {0}: {1} = {0}_in;", wname(n), t.wgsl()); w.line(1, &line); } }
        w.block(&f.body, 1)?;
        w.out.push_str("}\n\n");
        w.sc.vars.pop();
    }
    if target == Target::Filter {
        w.out.push_str("@fragment\nfn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {\n");
        w.out.push_str("    glsl_frag_coord = vec4<f32>(in.position.x, resolution().y - in.position.y, in.position.z, 1.0);\n");
        w.out.push_str("    glsl_tex_coord = vec4<f32>(in.uv, 0.0, 1.0);\n");
        w.out.push_str("    glsl_vert_color = vec4<f32>(1.0);\n");
    } else {
        w.out.push_str("@fragment\nfn fragment(in: VertexOutput) -> @location(0) vec4<f32> {\n");
        w.out.push_str("    glsl_frag_coord = vec4<f32>(in.position.x, params.glsl_resolution.y - in.position.y, in.position.z, 1.0);\n");
        w.out.push_str("    glsl_tex_coord = vec4<f32>(in.uv, 0.0, 1.0);\n");
        w.out.push_str("    glsl_vert_color = params.glsl_fill;\n");
    }
    w.out.push_str("    glsl_frag_color = vec4<f32>(0.0, 0.0, 0.0, 1.0);\n");
    for (t, n) in &prog.uniforms {
        if let Ty::Arr(..) = t {
            let items: Vec<String> = (0..t.len()).map(|i| if t.elem() == Ty::Bool { format!("(params.{n}_{i} != 0)") } else { format!("params.{n}_{i}") }).collect();
            w.line(1, &format!("glsl_u_{n} = {}({});", t.wgsl(), items.join(", ")));
        }
    }
    let globals = prog.globals.clone();
    for (t, n, init, _) in &globals {
        if let Some(e) = init { let v = w.ex_as(e, *t)?; w.line(1, &format!("{} = {v};", wname(n))); }
    }
    w.out.push_str("    glsl_main();\n    return glsl_frag_color;\n}\n");
    Ok(w.out)
}

