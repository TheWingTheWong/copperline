//! Minimal S-expression reader for KiCad files.
//!
//! KiCad's board format is a tree of lists whose first element is a symbol naming the
//! node, e.g. `(pad "1" smd rect (at 0 0) (size 1 1))`. Strings are double-quoted with
//! backslash escapes; everything else is an atom. We keep atoms as raw text and parse
//! numbers lazily, which keeps the reader small and lossless enough for our needs.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Sexp {
    /// Unquoted symbol or number, kept as text.
    Atom(String),
    /// Quoted string with escapes resolved.
    Str(String),
    List(Vec<Sexp>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub offset: usize,
    pub msg: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s-expression error at byte {}: {}", self.offset, self.msg)
    }
}

impl std::error::Error for ParseError {}

impl Sexp {
    /// Name of a list node (its first atom), if any.
    pub fn name(&self) -> Option<&str> {
        match self {
            Sexp::List(items) => match items.first() {
                Some(Sexp::Atom(a)) => Some(a.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn items(&self) -> &[Sexp] {
        match self {
            Sexp::List(items) => items,
            _ => &[],
        }
    }

    /// Arguments of a list node (everything after the name).
    pub fn args(&self) -> &[Sexp] {
        let items = self.items();
        if items.is_empty() { items } else { &items[1..] }
    }

    /// Child list nodes with the given name.
    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.args().iter().filter(move |c| c.name() == Some(name))
    }

    /// First child list node with the given name.
    pub fn child(&self, name: &str) -> Option<&Sexp> {
        self.args().iter().find(|c| c.name() == Some(name))
    }

    /// Text of an atom or string.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sexp::Atom(s) | Sexp::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Sexp::Atom(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// Numeric argument `i` (0-based, after the name).
    pub fn arg_num(&self, i: usize) -> Option<f64> {
        self.args().get(i).and_then(|a| a.num())
    }

    /// Text argument `i` (0-based, after the name).
    pub fn arg_text(&self, i: usize) -> Option<&str> {
        self.args().get(i).and_then(|a| a.text())
    }

    /// True if any argument is the bare atom `flag` (e.g. `hide`, `locked`).
    pub fn has_atom(&self, flag: &str) -> bool {
        self.args().iter().any(|a| matches!(a, Sexp::Atom(s) if s == flag))
    }
}

/// Parse one top-level expression from `src`.
pub fn parse(src: &str) -> Result<Sexp, ParseError> {
    let bytes = src.as_bytes();
    let mut pos = 0usize;
    skip_ws(bytes, &mut pos);
    let v = parse_at(bytes, &mut pos)?;
    Ok(v)
}

fn skip_ws(b: &[u8], pos: &mut usize) {
    while *pos < b.len() && b[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
}

fn parse_at(b: &[u8], pos: &mut usize) -> Result<Sexp, ParseError> {
    // Iterative to survive very deep or very long files without recursion limits.
    let mut stack: Vec<Vec<Sexp>> = Vec::new();
    loop {
        skip_ws(b, pos);
        if *pos >= b.len() {
            return Err(ParseError { offset: *pos, msg: "unexpected end of input" });
        }
        let value = match b[*pos] {
            b'(' => {
                *pos += 1;
                stack.push(Vec::new());
                continue;
            }
            b')' => {
                *pos += 1;
                match stack.pop() {
                    Some(items) => Sexp::List(items),
                    None => return Err(ParseError { offset: *pos - 1, msg: "unbalanced ')'" }),
                }
            }
            b'"' => Sexp::Str(read_string(b, pos)?),
            _ => {
                let start = *pos;
                while *pos < b.len() && !b[*pos].is_ascii_whitespace() && b[*pos] != b'(' && b[*pos] != b')' {
                    *pos += 1;
                }
                Sexp::Atom(String::from_utf8_lossy(&b[start..*pos]).into_owned())
            }
        };
        match stack.last_mut() {
            Some(top) => top.push(value),
            None => return Ok(value),
        }
    }
}

fn read_string(b: &[u8], pos: &mut usize) -> Result<String, ParseError> {
    let start = *pos;
    *pos += 1; // opening quote
    let mut out = Vec::new();
    while *pos < b.len() {
        match b[*pos] {
            b'"' => {
                *pos += 1;
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            b'\\' if *pos + 1 < b.len() => {
                let c = b[*pos + 1];
                out.push(match c {
                    b'n' => b'\n',
                    b't' => b'\t',
                    other => other,
                });
                *pos += 2;
            }
            c => {
                out.push(c);
                *pos += 1;
            }
        }
    }
    Err(ParseError { offset: start, msg: "unterminated string" })
}

/// Quote a string for writing into a KiCad file.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_lists_strings_and_atoms() {
        let s = parse(r#"(pad "1" smd rect (at 1.5 -2 90) (net 3 "GND\"x"))"#).unwrap();
        assert_eq!(s.name(), Some("pad"));
        assert_eq!(s.arg_text(0), Some("1"));
        assert!(s.has_atom("smd"));
        let at = s.child("at").unwrap();
        assert_eq!(at.arg_num(0), Some(1.5));
        assert_eq!(at.arg_num(1), Some(-2.0));
        assert_eq!(at.arg_num(2), Some(90.0));
        let net = s.child("net").unwrap();
        assert_eq!(net.arg_num(0), Some(3.0));
        assert_eq!(net.arg_text(1), Some("GND\"x"));
    }

    #[test]
    fn rejects_unbalanced_input() {
        assert!(parse("(a (b)").is_err());
        assert!(parse(")").is_err());
        assert!(parse("(a \"open)").is_err());
    }

    #[test]
    fn quote_round_trips() {
        for s in ["plain", "with \"quotes\"", "back\\slash", "/net{x}"] {
            let q = quote(s);
            assert_eq!(parse(&q).unwrap().text(), Some(s));
        }
    }
}
