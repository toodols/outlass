//! SassScript runtime values.

use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListSep {
    Space,
    Comma,
    Slash,
    /// Empty or single-element lists whose separator hasn't been decided.
    Undecided,
}

impl ListSep {
    pub fn name(self) -> &'static str {
        match self {
            ListSep::Space | ListSep::Undecided => "space",
            ListSep::Comma => "comma",
            ListSep::Slash => "slash",
        }
    }

    fn joiner(self) -> &'static str {
        match self {
            ListSep::Space | ListSep::Undecided => " ",
            ListSep::Comma => ", ",
            ListSep::Slash => " / ",
        }
    }
}

/// A number with (possibly compound) units, e.g. `10px`, `50%`, `2px*em/s`.
#[derive(Clone, Debug)]
pub struct Number {
    pub value: f64,
    pub numer: Vec<String>,
    pub denom: Vec<String>,
}

/// Precision used by Sass when comparing and printing numbers.
pub const EPSILON: f64 = 1e-10;

pub fn fuzzy_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < EPSILON
}

/// Returns (category, factor-to-canonical-unit) for known convertible units.
fn unit_info(unit: &str) -> Option<(&'static str, f64)> {
    let u = unit.to_ascii_lowercase();
    Some(match u.as_str() {
        "px" => ("length", 1.0),
        "in" => ("length", 96.0),
        "cm" => ("length", 96.0 / 2.54),
        "mm" => ("length", 96.0 / 25.4),
        "q" => ("length", 96.0 / 101.6),
        "pt" => ("length", 96.0 / 72.0),
        "pc" => ("length", 16.0),
        "deg" => ("angle", 1.0),
        "grad" => ("angle", 0.9),
        "rad" => ("angle", 180.0 / std::f64::consts::PI),
        "turn" => ("angle", 360.0),
        "s" => ("time", 1.0),
        "ms" => ("time", 0.001),
        "hz" => ("frequency", 1.0),
        "khz" => ("frequency", 1000.0),
        "dpi" => ("resolution", 1.0),
        "dpcm" => ("resolution", 2.54),
        "dppx" => ("resolution", 96.0),
        _ => return None,
    })
}

/// Factor to multiply a value in `from` by to express it in `to`, if convertible.
pub fn conversion_factor(from: &str, to: &str) -> Option<f64> {
    if from.eq_ignore_ascii_case(to) {
        return Some(1.0);
    }
    let (cat_a, fa) = unit_info(from)?;
    let (cat_b, fb) = unit_info(to)?;
    (cat_a == cat_b).then_some(fa / fb)
}

impl Number {
    pub fn new(value: f64) -> Self {
        Number { value, numer: Vec::new(), denom: Vec::new() }
    }

    pub fn with_unit(value: f64, unit: &str) -> Self {
        if unit.is_empty() {
            return Number::new(value);
        }
        Number { value, numer: vec![unit.to_string()], denom: Vec::new() }
    }

    pub fn is_unitless(&self) -> bool {
        self.numer.is_empty() && self.denom.is_empty()
    }

    /// The single unit of this number ("" if unitless), or `None` for compound units.
    pub fn simple_unit(&self) -> Option<&str> {
        match (self.numer.len(), self.denom.len()) {
            (0, 0) => Some(""),
            (1, 0) => Some(&self.numer[0]),
            _ => None,
        }
    }

    pub fn has_unit(&self, unit: &str) -> bool {
        self.simple_unit().is_some_and(|u| u.eq_ignore_ascii_case(unit))
    }

    /// Human readable unit string, e.g. `px`, `px*em`, `px/s`.
    pub fn unit_string(&self) -> String {
        let mut s = self.numer.join("*");
        if !self.denom.is_empty() {
            if s.is_empty() {
                s.push('1');
            }
            s.push('/');
            s.push_str(&self.denom.join("*"));
        }
        s
    }

    /// Converts this number to `unit` (a simple unit, "" for unitless).
    /// A unitless number converts to anything.
    pub fn value_in(&self, unit: &str) -> Option<f64> {
        if self.is_unitless() {
            return Some(self.value);
        }
        let own = self.simple_unit()?;
        if unit.is_empty() {
            return None;
        }
        conversion_factor(own, unit).map(|f| self.value * f)
    }

    /// Whether the two numbers can be added/compared.
    pub fn is_comparable_to(&self, other: &Number) -> bool {
        if self.is_unitless() || other.is_unitless() {
            return true;
        }
        self.convert_units_of(other).is_some()
    }

    /// Expresses `other`'s value in `self`'s units. Unitless numbers take on the other's units.
    fn convert_units_of(&self, other: &Number) -> Option<f64> {
        if other.is_unitless() || self.is_unitless() {
            return Some(other.value);
        }
        if self.numer.len() != other.numer.len() || self.denom.len() != other.denom.len() {
            return None;
        }
        let mut factor = 1.0;
        let mut used = vec![false; other.numer.len()];
        for u in &self.numer {
            let (i, f) = other
                .numer
                .iter()
                .enumerate()
                .filter(|(i, _)| !used[*i])
                .find_map(|(i, o)| conversion_factor(o, u).map(|f| (i, f)))?;
            used[i] = true;
            factor *= f;
        }
        let mut used = vec![false; other.denom.len()];
        for u in &self.denom {
            let (i, f) = other
                .denom
                .iter()
                .enumerate()
                .filter(|(i, _)| !used[*i])
                .find_map(|(i, o)| conversion_factor(o, u).map(|f| (i, f)))?;
            used[i] = true;
            factor /= f;
        }
        Some(other.value * factor)
    }

    fn additive(&self, other: &Number, op: &str, f: impl Fn(f64, f64) -> f64) -> Result<Number, String> {
        if self.is_unitless() {
            return Ok(Number {
                value: f(self.value, other.value),
                numer: other.numer.clone(),
                denom: other.denom.clone(),
            });
        }
        match self.convert_units_of(other) {
            Some(v) => Ok(Number { value: f(self.value, v), numer: self.numer.clone(), denom: self.denom.clone() }),
            None => Err(format!("Incompatible units {} and {} for `{op}`", self.unit_string(), other.unit_string())),
        }
    }

    pub fn add(&self, other: &Number) -> Result<Number, String> {
        self.additive(other, "+", |a, b| a + b)
    }

    pub fn sub(&self, other: &Number) -> Result<Number, String> {
        self.additive(other, "-", |a, b| a - b)
    }

    pub fn rem(&self, other: &Number) -> Result<Number, String> {
        // Sass modulo takes the sign of the divisor.
        self.additive(other, "%", |a, b| {
            let r = a % b;
            if r != 0.0 && (r < 0.0) != (b < 0.0) { r + b } else { r }
        })
    }

    pub fn mul(&self, other: &Number) -> Number {
        let mut n = Number {
            value: self.value * other.value,
            numer: [self.numer.clone(), other.numer.clone()].concat(),
            denom: [self.denom.clone(), other.denom.clone()].concat(),
        };
        n.cancel_units();
        n
    }

    pub fn div(&self, other: &Number) -> Number {
        let mut n = Number {
            value: self.value / other.value,
            numer: [self.numer.clone(), other.denom.clone()].concat(),
            denom: [self.denom.clone(), other.numer.clone()].concat(),
        };
        n.cancel_units();
        n
    }

    fn cancel_units(&mut self) {
        let mut i = 0;
        while i < self.numer.len() {
            let found =
                self.denom.iter().enumerate().find_map(|(j, d)| conversion_factor(&self.numer[i], d).map(|f| (j, f)));
            if let Some((j, f)) = found {
                self.value *= f;
                self.numer.remove(i);
                self.denom.remove(j);
            } else {
                i += 1;
            }
        }
    }

    /// Compares two numbers, converting units. `None` if incompatible.
    pub fn cmp_value(&self, other: &Number) -> Option<std::cmp::Ordering> {
        let b = self.convert_units_of(other)?;
        if fuzzy_eq(self.value, b) { Some(std::cmp::Ordering::Equal) } else { self.value.partial_cmp(&b) }
    }

    pub fn sass_eq(&self, other: &Number) -> bool {
        if self.is_unitless() != other.is_unitless() {
            return false;
        }
        self.cmp_value(other) == Some(std::cmp::Ordering::Equal)
    }

    pub fn to_css(&self) -> String {
        let mut s = format_number(self.value);
        s.push_str(&self.unit_string());
        s
    }
}

/// Formats a number like Sass: up to 10 decimal places, no trailing zeros, no `-0`.
pub fn format_number(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "infinity".into() } else { "-infinity".into() };
    }
    if v.is_nan() {
        return "NaN".into();
    }
    if fuzzy_eq(v, v.round()) {
        let r = v.round();
        return if r == 0.0 { "0".into() } else { format!("{r}") };
    }
    let mut s = format!("{v:.10}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

/// An RGBA color. Channels are 0-255 (may be fractional), alpha is 0-1.
#[derive(Clone, Debug, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub fn rgba(r: f64, g: f64, b: f64, a: f64) -> Self {
        Color { r: r.clamp(0.0, 255.0), g: g.clamp(0.0, 255.0), b: b.clamp(0.0, 255.0), a: a.clamp(0.0, 1.0) }
    }

    /// Hue in degrees [0, 360), saturation and lightness in percent [0, 100].
    pub fn to_hsl(&self) -> (f64, f64, f64) {
        let r = self.r / 255.0;
        let g = self.g / 255.0;
        let b = self.b / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let l = (max + min) / 2.0;
        let h = if delta == 0.0 {
            0.0
        } else if max == r {
            60.0 * (((g - b) / delta).rem_euclid(6.0))
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        let s = if delta == 0.0 { 0.0 } else { delta / (1.0 - (2.0 * l - 1.0).abs()) };
        (h.rem_euclid(360.0), s * 100.0, l * 100.0)
    }

    /// Hue in degrees, saturation and lightness in percent.
    pub fn from_hsl(h: f64, s: f64, l: f64, a: f64) -> Self {
        let h = h.rem_euclid(360.0) / 360.0;
        let s = (s / 100.0).clamp(0.0, 1.0);
        let l = (l / 100.0).clamp(0.0, 1.0);
        let m2 = if l <= 0.5 { l * (s + 1.0) } else { l + s - l * s };
        let m1 = l * 2.0 - m2;
        let hue = |mut h: f64| {
            if h < 0.0 {
                h += 1.0;
            }
            if h > 1.0 {
                h -= 1.0;
            }
            if h * 6.0 < 1.0 {
                m1 + (m2 - m1) * h * 6.0
            } else if h * 2.0 < 1.0 {
                m2
            } else if h * 3.0 < 2.0 {
                m1 + (m2 - m1) * (2.0 / 3.0 - h) * 6.0
            } else {
                m1
            }
        };
        Color::rgba(hue(h + 1.0 / 3.0) * 255.0, hue(h) * 255.0, hue(h - 1.0 / 3.0) * 255.0, a)
    }

    pub fn to_css(&self) -> String {
        let r = self.r.round() as u8;
        let g = self.g.round() as u8;
        let b = self.b.round() as u8;
        if fuzzy_eq(self.a, 1.0) {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("rgba({r}, {g}, {b}, {})", format_number(self.a))
        }
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (without the `#`).
    pub fn from_hex(hex: &str) -> Option<Color> {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|d| d as f64 * 17.0);
        let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|d| d as f64);
        match hex.len() {
            3 => Some(Color::rgba(digit(0)?, digit(1)?, digit(2)?, 1.0)),
            4 => Some(Color::rgba(digit(0)?, digit(1)?, digit(2)?, digit(3)? / 255.0)),
            6 => Some(Color::rgba(pair(0)?, pair(2)?, pair(4)?, 1.0)),
            8 => Some(Color::rgba(pair(0)?, pair(2)?, pair(4)?, pair(6)? / 255.0)),
            _ => None,
        }
    }

    pub fn from_name(name: &str) -> Option<Color> {
        let lower = name.to_ascii_lowercase();
        if lower == "transparent" {
            return Some(Color::rgba(0.0, 0.0, 0.0, 0.0));
        }
        NAMED_COLORS.iter().find(|(n, _)| *n == lower).map(|(_, hex)| {
            Color::rgba(((hex >> 16) & 0xff) as f64, ((hex >> 8) & 0xff) as f64, (hex & 0xff) as f64, 1.0)
        })
    }
}

/// A SassScript value.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    Str {
        text: String,
        quoted: bool,
    },
    Color(Color),
    List {
        items: Vec<Value>,
        sep: ListSep,
        bracketed: bool,
    },
    /// Ordered map; keys are unique under `sass_eq`.
    Map(Vec<(Value, Value)>),
    /// A first-class function reference (from `get-function`).
    Function(String),
    /// A plain (non-Sass) function call kept as-is: CSS functions like `url(...)`, `var(--x)`,
    /// `calc(...)`, or Luau expressions like `UDim2.new(0, 10, 0, 20)`.
    Call {
        name: String,
        args: Vec<Value>,
    },
}

impl Value {
    pub fn str(text: impl Into<String>) -> Value {
        Value::Str { text: text.into(), quoted: false }
    }

    pub fn quoted(text: impl Into<String>) -> Value {
        Value::Str { text: text.into(), quoted: true }
    }

    pub fn num(v: f64) -> Value {
        Value::Number(Number::new(v))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn num_unit(v: f64, unit: &str) -> Value {
        Value::Number(Number::with_unit(v, unit))
    }

    pub fn list(items: Vec<Value>, sep: ListSep) -> Value {
        Value::List { items, sep, bracketed: false }
    }

    pub fn empty_list() -> Value {
        Value::list(Vec::new(), ListSep::Undecided)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::Str { .. } | Value::Call { .. } => "string",
            Value::Color(_) => "color",
            Value::List { .. } => "list",
            Value::Map(_) => "map",
            Value::Function(_) => "function",
        }
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Null | Value::Bool(false))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Treats the value as a list: lists are themselves, maps are lists of `key value` pairs,
    /// everything else is a single-element list.
    pub fn as_list(&self) -> Vec<Value> {
        match self {
            Value::List { items, .. } => items.clone(),
            Value::Map(pairs) => {
                pairs.iter().map(|(k, v)| Value::list(vec![k.clone(), v.clone()], ListSep::Space)).collect()
            }
            other => vec![other.clone()],
        }
    }

    pub fn separator(&self) -> ListSep {
        match self {
            Value::List { sep, .. } => *sep,
            Value::Map(p) if !p.is_empty() => ListSep::Comma,
            _ => ListSep::Space,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }

    /// The text of a string (quoted or not).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str { text, .. } => Some(text),
            _ => None,
        }
    }

    /// Sass `==`.
    pub fn sass_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a.sass_eq(b),
            (Value::Str { text: a, .. }, Value::Str { text: b, .. }) => a == b,
            (Value::Color(a), Value::Color(b)) => {
                fuzzy_eq(a.r, b.r) && fuzzy_eq(a.g, b.g) && fuzzy_eq(a.b, b.b) && fuzzy_eq(a.a, b.a)
            }
            (Value::List { items: a, sep: sa, bracketed: ba }, Value::List { items: b, sep: sb, bracketed: bb }) => {
                ba == bb
                    && a.len() == b.len()
                    && (a.len() <= 1 || sa == sb)
                    && a.iter().zip(b).all(|(x, y)| x.sass_eq(y))
            }
            (Value::List { items, .. }, Value::Map(m)) | (Value::Map(m), Value::List { items, .. }) => {
                items.is_empty() && m.is_empty()
            }
            (Value::Map(a), Value::Map(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.iter().any(|(k2, v2)| k.sass_eq(k2) && v.sass_eq(v2)))
            }
            (Value::Function(a), Value::Function(b)) => a == b,
            (Value::Call { .. }, _) | (_, Value::Call { .. }) => self.to_css().ok() == other.to_css().ok(),
            _ => false,
        }
    }

    /// Converts to a map if possible (empty lists are empty maps).
    pub fn as_map(&self) -> Option<Vec<(Value, Value)>> {
        match self {
            Value::Map(p) => Some(p.clone()),
            Value::List { items, .. } if items.is_empty() => Some(Vec::new()),
            _ => None,
        }
    }

    /// Serializes the value as CSS text. Quoted strings keep their quotes.
    pub fn to_css(&self) -> Result<String, String> {
        Ok(match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_css(),
            Value::Str { text, quoted: true } => quote_string(text),
            Value::Str { text, quoted: false } => text.clone(),
            Value::Color(c) => c.to_css(),
            Value::List { items, sep, bracketed } => {
                let parts = items
                    .iter()
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        let s = v.to_css()?;
                        // Nested comma lists inside comma lists need parens to survive a round trip.
                        Ok(match v {
                            Value::List { sep: ListSep::Comma, items, bracketed: false }
                                if *sep == ListSep::Comma && items.len() > 1 =>
                            {
                                format!("({s})")
                            }
                            _ => s,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let inner = parts.join(sep.joiner());
                if *bracketed { format!("[{inner}]") } else { inner }
            }
            Value::Map(_) => return Err(format!("{} isn't a valid CSS value.", self.inspect())),
            Value::Function(name) => return Err(format!("get-function(\"{name}\") isn't a valid CSS value.")),
            Value::Call { name, args } => {
                let args = args.iter().map(|a| a.to_css()).collect::<Result<Vec<_>, _>>()?;
                format!("{name}({})", args.join(", "))
            }
        })
    }

    /// Text used for `#{}` interpolation: like `to_css` but strings lose their quotes.
    pub fn to_interp(&self) -> Result<String, String> {
        match self {
            Value::Str { text, .. } => Ok(text.clone()),
            Value::Null => Ok(String::new()),
            other => other.to_css(),
        }
    }

    /// Debug representation (as `inspect()` / `@debug` print it).
    pub fn inspect(&self) -> String {
        match self {
            Value::Null => "null".into(),
            Value::List { items, sep, bracketed } => {
                if items.is_empty() {
                    return if *bracketed { "[]".into() } else { "()".into() };
                }
                let inner = items.iter().map(|v| v.inspect()).collect::<Vec<_>>().join(sep.joiner());
                if *bracketed {
                    format!("[{inner}]")
                } else if items.len() == 1 && *sep == ListSep::Comma {
                    format!("({inner},)")
                } else {
                    inner
                }
            }
            Value::Map(pairs) => {
                let mut s = String::from("(");
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    let _ = write!(s, "{}: {}", k.inspect(), v.inspect());
                }
                s.push(')');
                s
            }
            Value::Function(name) => format!("get-function(\"{name}\")"),
            other => other.to_css().unwrap_or_default(),
        }
    }
}

pub fn quote_string(text: &str) -> String {
    let quote = if text.contains('"') && !text.contains('\'') { '\'' } else { '"' };
    let mut s = String::with_capacity(text.len() + 2);
    s.push(quote);
    for c in text.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\a "),
            c if c == quote => {
                s.push('\\');
                s.push(c);
            }
            c => s.push(c),
        }
    }
    s.push(quote);
    s
}

/// CSS named colors (lowercase name, 0xRRGGBB).
pub const NAMED_COLORS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_math() {
        let a = Number::with_unit(1.0, "in");
        let b = Number::with_unit(48.0, "px");
        assert_eq!(a.add(&b).unwrap().to_css(), "1.5in");
        assert_eq!(b.div(&Number::with_unit(2.0, "px")).to_css(), "24");
        assert!(Number::with_unit(1.0, "px").add(&Number::with_unit(1.0, "%")).is_err());
        assert_eq!(Number::with_unit(1.0, "s").add(&Number::with_unit(500.0, "ms")).unwrap().to_css(), "1.5s");
    }

    #[test]
    fn hsl_roundtrip() {
        let c = Color::rgba(51.0, 102.0, 153.0, 1.0);
        let (h, s, l) = c.to_hsl();
        let back = Color::from_hsl(h, s, l, 1.0);
        assert!(fuzzy_eq(back.r.round(), 51.0) && fuzzy_eq(back.g.round(), 102.0) && fuzzy_eq(back.b.round(), 153.0));
    }

    #[test]
    fn number_format() {
        assert_eq!(format_number(1.0 / 3.0), "0.3333333333");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(12.50), "12.5");
    }
}
