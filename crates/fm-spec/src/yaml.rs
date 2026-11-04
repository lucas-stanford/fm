//! Frontmatter YAML: 1.2 core schema (so `on`, `yes` and `no` are strings), duplicate keys refused,
//! and a position for every error. Built on the event parser of `yaml-rust2`.

use std::collections::HashMap;
use std::fmt;

use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use crate::refusal::quote;

/// A YAML value as the validator sees it. Mapping keys are strings and keep their order.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// How many nodes the value holds, itself included.
    pub fn size(&self) -> usize {
        match self {
            Value::Seq(items) => 1 + items.iter().map(Value::size).sum::<usize>(),
            Value::Map(entries) => 1 + entries.iter().map(|(_, v)| 1 + v.size()).sum::<usize>(),
            _ => 1,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The value of `key` in a mapping, `None` when absent or empty (an empty value is absent).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .filter(|v| !v.is_null())
    }
}

/// `JSON.stringify` of the value: how messages show what was written.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) if x.is_finite() => write!(f, "{x}"),
            Value::Float(_) => f.write_str("null"),
            Value::Str(s) => f.write_str(&quote(s)),
            Value::Seq(items) => {
                f.write_str("[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Value::Map(entries) => {
                f.write_str("{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{}:{v}", quote(k))?;
                }
                f.write_str("}")
            }
        }
    }
}

/// A YAML error: what the parser said, and where (1-based, in the YAML text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    /// What fixes it, when the parser knows.
    pub fix: Option<String>,
}

impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )?;
        match &self.fix {
            Some(fix) => write!(f, " — {fix}"),
            None => Ok(()),
        }
    }
}

fn error_at(message: impl Into<String>, mark: &Marker) -> YamlError {
    YamlError {
        message: message.into(),
        line: mark.line(),
        column: mark.col() + 1,
        fix: None,
    }
}

enum Frame {
    Seq {
        items: Vec<Value>,
        anchor: usize,
    },
    Map {
        entries: Vec<(String, Value)>,
        key: Option<String>,
        seen: HashMap<String, Marker>,
        anchor: usize,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Value>,
    root: Option<Value>,
    error: Option<YamlError>,
    /// Nodes copied by aliases so far.
    expanded: usize,
}

/// Frontmatter is a few dozen nodes; this many copied by aliases is an attack, not a file.
const ALIAS_BUDGET: usize = 10_000;
/// Deeper than any frontmatter needs, shallow enough for the parser's recursion.
const MAX_INDENT: usize = 64;

impl Builder {
    fn fail(&mut self, e: YamlError) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    /// Put a finished node where it belongs: a mapping key, a mapping value, an item or the root.
    fn place(&mut self, value: Value, key_text: Option<String>, mark: Marker) {
        match self.stack.last_mut() {
            None => {
                if self.root.is_none() {
                    self.root = Some(value);
                }
            }
            Some(Frame::Seq { items, .. }) => items.push(value),
            Some(Frame::Map {
                entries, key, seen, ..
            }) => match key.take() {
                Some(k) => entries.push((k, value)),
                None => {
                    let Some(k) = key_text else {
                        self.fail(error_at(
                            "A mapping or list cannot be a key — use a plain word as the key",
                            &mark,
                        ));
                        return;
                    };
                    if let Some(first) = seen.get(&k) {
                        let first_line = first.line();
                        let mut e = error_at(
                            format!(
                                "Map keys must be unique: {} is repeated (first on line {first_line})",
                                quote(&k)
                            ),
                            &mark,
                        );
                        e.fix = Some("keep one".to_string());
                        self.fail(e);
                        return;
                    }
                    seen.insert(k.clone(), mark);
                    *key = Some(k);
                }
            },
        }
    }
}

fn is_str_tag(tag: &Option<Tag>) -> bool {
    tag.as_ref().is_some_and(|t| t.suffix == "str")
}

/// The YAML 1.2 core schema for a plain scalar.
fn resolve_plain(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Value::Float(f64::INFINITY);
        }
        "-.inf" | "-.Inf" | "-.INF" => return Value::Float(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => return Value::Float(f64::NAN),
        _ => {}
    }
    if let Some(hex) = s.strip_prefix("0x")
        && !hex.is_empty()
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && let Ok(i) = i64::from_str_radix(hex, 16)
    {
        return Value::Int(i);
    }
    if let Some(oct) = s.strip_prefix("0o")
        && !oct.is_empty()
        && oct.chars().all(|c| ('0'..='7').contains(&c))
        && let Ok(i) = i64::from_str_radix(oct, 8)
    {
        return Value::Int(i);
    }
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return match s.parse::<i64>() {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(s.parse::<f64>().unwrap_or(f64::NAN)),
        };
    }
    if is_core_float(s)
        && let Ok(x) = s.parse::<f64>()
    {
        return Value::Float(x);
    }
    Value::Str(s.to_string())
}

/// `[-+]? ( \. [0-9]+ | [0-9]+ ( \. [0-9]* )? ) ( [eE] [-+]? [0-9]+ )?`
fn is_core_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - f;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let e = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e {
            return false;
        }
    }
    i == b.len()
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        if self.error.is_some() {
            return;
        }
        match ev {
            Event::Scalar(text, style, anchor, tag) => {
                let value = if style != TScalarStyle::Plain || is_str_tag(&tag) {
                    Value::Str(text.clone())
                } else {
                    resolve_plain(&text)
                };
                if anchor > 0 {
                    self.anchors.insert(anchor, value.clone());
                }
                self.place(value, Some(text), mark);
            }
            Event::Alias(id) => {
                let Some(size) = self.anchors.get(&id).map(Value::size) else {
                    self.fail(error_at("Unknown alias", &mark));
                    return;
                };
                // Aliases of aliases grow exponentially: bound the copies, as YAML libraries do.
                self.expanded += size;
                if self.expanded > ALIAS_BUDGET {
                    let mut e = error_at("Too many aliases", &mark);
                    e.fix = Some("write the values out instead of repeating anchors".to_string());
                    self.fail(e);
                    return;
                }
                let value = self.anchors[&id].clone();
                let key = match &value {
                    Value::Str(s) => Some(s.clone()),
                    Value::Seq(_) | Value::Map(_) => None,
                    other => Some(other.to_string()),
                };
                self.place(value, key, mark);
            }
            Event::SequenceStart(anchor, _) => self.stack.push(Frame::Seq {
                items: Vec::new(),
                anchor,
            }),
            Event::MappingStart(anchor, _) => self.stack.push(Frame::Map {
                entries: Vec::new(),
                key: None,
                seen: HashMap::new(),
                anchor,
            }),
            Event::SequenceEnd | Event::MappingEnd => {
                let (value, anchor) = match self.stack.pop() {
                    Some(Frame::Seq { items, anchor }) => (Value::Seq(items), anchor),
                    Some(Frame::Map {
                        entries, anchor, ..
                    }) => (Value::Map(entries), anchor),
                    None => return,
                };
                if anchor > 0 {
                    self.anchors.insert(anchor, value.clone());
                }
                self.place(value, None, mark);
            }
            Event::Nothing
            | Event::StreamStart
            | Event::StreamEnd
            | Event::DocumentStart
            | Event::DocumentEnd => {}
        }
    }
}

/// Parse one YAML document. `Ok(Value::Null)` for an empty one.
pub fn parse_yaml(text: &str) -> Result<Value, YamlError> {
    // Each block level needs deeper indentation, and the parser recurses per level: refuse a
    // depth no frontmatter has before it can exhaust the stack.
    for (i, line) in text.lines().enumerate() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > MAX_INDENT {
            return Err(YamlError {
                message: format!("Indented {indent} spaces"),
                line: i + 1,
                column: indent + 1,
                fix: Some(format!(
                    "frontmatter is indented {MAX_INDENT} spaces at most"
                )),
            });
        }
    }
    let mut builder = Builder::default();
    let mut parser = Parser::new_from_str(text);
    if let Err(e) = parser.load(&mut builder, false) {
        return Err(error_at(capitalize(e.info()), e.marker()));
    }
    if let Some(e) = builder.error {
        return Err(e);
    }
    Ok(builder.root.unwrap_or(Value::Null))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(text: &str) -> Vec<(String, Value)> {
        match parse_yaml(text).unwrap() {
            Value::Map(m) => m,
            other => panic!("not a map: {other:?}"),
        }
    }

    #[test]
    fn keeps_on_yes_and_no_as_strings() {
        let m = map("on:\n  manual: true\nenabled: no\nx: yes\ny: off");
        assert_eq!(m[0].0, "on");
        assert_eq!(
            m[0].1,
            Value::Map(vec![("manual".into(), Value::Bool(true))])
        );
        assert_eq!(m[1].1, Value::Str("no".into()));
        assert_eq!(m[2].1, Value::Str("yes".into()));
        assert_eq!(m[3].1, Value::Str("off".into()));
    }

    #[test]
    fn resolves_the_core_schema() {
        let m = map("a: 42\nb: \"42\"\nc: 1.5\nd: ~\ne:\nf: 0x1f\ng: 1_000\nh: 'true'\ni: !!str 7");
        let v: Vec<&Value> = m.iter().map(|(_, v)| v).collect();
        assert_eq!(v[0], &Value::Int(42));
        assert_eq!(v[1], &Value::Str("42".into()));
        assert_eq!(v[2], &Value::Float(1.5));
        assert_eq!(v[3], &Value::Null);
        assert_eq!(v[4], &Value::Null);
        assert_eq!(v[5], &Value::Int(31));
        assert_eq!(v[6], &Value::Str("1_000".into()));
        assert_eq!(v[7], &Value::Str("true".into()));
        assert_eq!(v[8], &Value::Str("7".into()));
    }

    #[test]
    fn refuses_duplicate_keys_with_their_position() {
        let e = parse_yaml("name: x\non:\n  manual: true\non:\n  schedule: x").unwrap_err();
        assert!(e.message.starts_with("Map keys must be unique"), "{e}");
        assert_eq!((e.line, e.column), (4, 1));
        let nested = parse_yaml("on:\n  manual: true\n  manual: true").unwrap_err();
        assert_eq!(nested.line, 3);
    }

    #[test]
    fn reports_an_unquoted_cron_as_an_error() {
        let e = parse_yaml("on:\n  schedule:\n    cron: */15 * * * *").unwrap_err();
        assert_eq!(e.line, 3, "{e}");
    }

    #[test]
    fn follows_anchors_and_aliases() {
        let m = map("a: &x hello\nb: *x");
        assert_eq!(m[1].1, Value::Str("hello".into()));
    }

    #[test]
    fn refuses_nesting_deep_enough_to_exhaust_the_stack() {
        let mut block = String::new();
        for i in 0..2000 {
            block.push_str(&" ".repeat(i));
            block.push_str("a:\n");
        }
        let e = parse_yaml(&block).unwrap_err();
        assert_eq!(e.line, 66, "{e}");
        let mut worst = String::new();
        for i in 0..MAX_INDENT {
            worst.push_str(&" ".repeat(i));
            worst.push_str("a:\n");
        }
        worst.push_str(&" ".repeat(MAX_INDENT));
        worst.push_str(&"[".repeat(10_000));
        // Run on a thread with the small stack a spawned worker gets, debug build included.
        let handle = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || parse_yaml(&worst).is_err())
            .unwrap();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn bounds_alias_expansion() {
        let mut text = String::from("a: &a [x, x, x, x, x, x, x, x, x, x]\n");
        for (prev, name) in ["a", "b", "c", "d", "e", "f", "g", "h"]
            .windows(2)
            .map(|w| (w[0], w[1]))
        {
            let items = vec![format!("*{prev}"); 10].join(", ");
            text.push_str(&format!("{name}: &{name} [{items}]\n"));
        }
        let e = parse_yaml(&text).unwrap_err();
        assert!(e.message.starts_with("Too many aliases"), "{e}");
    }

    #[test]
    fn empty_text_is_null() {
        assert_eq!(parse_yaml("").unwrap(), Value::Null);
        assert_eq!(parse_yaml("# only a comment\n").unwrap(), Value::Null);
    }

    #[test]
    fn displays_like_json_stringify() {
        assert_eq!(
            parse_yaml("[\"0 9 * * *\", 1, true, ~]")
                .unwrap()
                .to_string(),
            "[\"0 9 * * *\",1,true,null]"
        );
        assert_eq!(parse_yaml("{a: 1}").unwrap().to_string(), "{\"a\":1}");
    }

    #[test]
    fn get_treats_empty_as_absent() {
        let v = parse_yaml("a:\nb: 1").unwrap();
        assert_eq!(v.get("a"), None);
        assert_eq!(v.get("b"), Some(&Value::Int(1)));
        assert_eq!(v.get("c"), None);
    }
}
