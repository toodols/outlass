//! Converting SassScript values into Luau source expressions.

use crate::value::{Color, ListSep, Number, Value, format_number};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorFormat {
    /// `Color3.fromRGB(255, 128, 0)`
    Rgb,
    /// `Color3.fromHex("#ff8000")`
    Hex,
    /// `Color3.new(1, 0.5, 0)`
    Float,
}

#[derive(Clone, Debug)]
pub struct LuauOptions {
    pub color_format: ColorFormat,
    /// Pixels per `rem`/`em` when converting font-relative lengths.
    pub rem_px: f64,
}

impl Default for LuauOptions {
    fn default() -> Self {
        LuauOptions { color_format: ColorFormat::Rgb, rem_px: 16.0 }
    }
}

/// Formats a plain number as a Luau literal.
pub fn number(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "math.huge".into() } else { "-math.huge".into() };
    }
    if v.is_nan() {
        return "0/0".into();
    }
    format_number(v)
}

/// Quotes text as a Luau string literal.
pub fn string(text: &str) -> String {
    let mut s = String::with_capacity(text.len() + 2);
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\{}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// Whether `name` can be used as a bare table key (`Name = value`).
pub fn is_identifier(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not",
        "or", "repeat", "return", "then", "true", "until", "while", "continue",
    ];
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&name)
}

/// A table key: `Name` or `["odd name"]`.
pub fn table_key(name: &str) -> String {
    if is_identifier(name) { name.to_string() } else { format!("[{}]", string(name)) }
}

/// A Color3 constructor (alpha is ignored; callers map it to a transparency property).
pub fn color3(c: &Color, format: ColorFormat) -> String {
    match format {
        ColorFormat::Rgb => {
            format!("Color3.fromRGB({}, {}, {})", c.r.round() as u8, c.g.round() as u8, c.b.round() as u8)
        }
        ColorFormat::Hex => {
            format!("Color3.fromHex(\"#{:02x}{:02x}{:02x}\")", c.r.round() as u8, c.g.round() as u8, c.b.round() as u8)
        }
        ColorFormat::Float => format!(
            "Color3.new({}, {}, {})",
            number(round_to(c.r / 255.0, 4)),
            number(round_to(c.g / 255.0, 4)),
            number(round_to(c.b / 255.0, 4))
        ),
    }
}

pub fn round_to(v: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (v * f).round() / f
}

pub fn udim(scale: f64, offset: f64) -> String {
    format!("UDim.new({}, {})", number(scale), number(offset))
}

pub fn udim2(xs: f64, xo: f64, ys: f64, yo: f64) -> String {
    format!("UDim2.new({}, {}, {}, {})", number(xs), number(xo), number(ys), number(yo))
}

pub fn vector2(x: f64, y: f64) -> String {
    format!("Vector2.new({}, {})", number(x), number(y))
}

/// Converts a number to a plain Luau number, interpreting CSS units:
/// `px` → as-is, `%` → fraction (`50%` → `0.5`), angles → degrees, times → seconds,
/// `em`/`rem` → pixels, absolute lengths → pixels.
pub fn number_value(n: &Number, opts: &LuauOptions) -> Result<f64, String> {
    let Some(unit) = n.simple_unit() else {
        return Err(format!("{} has compound units and can't be converted to a Luau number", n.to_css()));
    };
    let lower = unit.to_ascii_lowercase();
    Ok(match lower.as_str() {
        "" | "px" => n.value,
        "%" => n.value / 100.0,
        "em" | "rem" => n.value * opts.rem_px,
        _ => {
            for target in ["px", "deg", "s"] {
                if let Some(v) = n.value_in(target) {
                    return Ok(v);
                }
            }
            return Err(format!("unit `{unit}` has no Luau equivalent (in {})", n.to_css()));
        }
    })
}

/// Converts an evaluated value into a Luau expression for a Roblox property.
///
/// * unquoted strings are emitted verbatim (they are Luau expressions such as `Enum.Font.Gotham`)
/// * quoted strings become Luau strings
/// * colors become `Color3` constructors (alpha dropped)
/// * `var(--Name)` / `token(Name)` become the token reference string `"$Name"`
/// * `luau("code")` emits `code` verbatim
/// * bracketed lists and maps become Luau tables
/// * other function calls (e.g. `UDim2.new(...)`) are emitted as calls with converted arguments
pub fn value(v: &Value, opts: &LuauOptions) -> Result<String, String> {
    Ok(match v {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(number_value(n, opts)?),
        Value::Str { text, quoted: true } => string(text),
        Value::Str { text, quoted: false } => text.clone(),
        Value::Color(c) => color3(c, opts.color_format),
        Value::List { items, sep, bracketed } => {
            let parts = items.iter().map(|i| value(i, opts)).collect::<Result<Vec<_>, _>>()?;
            if *bracketed {
                format!("{{{}}}", parts.join(", "))
            } else {
                match sep {
                    ListSep::Comma => parts.join(", "),
                    ListSep::Slash => parts.join(" / "),
                    _ => parts.join(" "),
                }
            }
        }
        Value::Map(pairs) => {
            let parts = pairs
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::Str { text, .. } => table_key(text),
                        other => format!("[{}]", value(other, opts)?),
                    };
                    Ok(format!("{key} = {}", value(v, opts)?))
                })
                .collect::<Result<Vec<_>, String>>()?;
            format!("{{{}}}", parts.join(", "))
        }
        Value::Function(name) => return Err(format!("function reference `{name}` can't be used as a property value")),
        Value::Call { name, args } => call(name, args, opts)?,
    })
}

/// Roblox attribute names may only contain letters, digits and `_`; other characters become `_`.
pub fn attribute_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// Returns the token name referenced by `var(--Name)` or `token(Name)`, if `v` is one.
pub fn token_reference(v: &Value) -> Option<String> {
    let Value::Call { name, args } = v else { return None };
    let arg = args.first()?.as_str()?;
    match name.as_str() {
        "var" => arg.strip_prefix("--").map(attribute_name),
        "token" => Some(attribute_name(arg.trim_start_matches('$'))),
        _ => None,
    }
}

fn call(name: &str, args: &[Value], opts: &LuauOptions) -> Result<String, String> {
    if let Some(token) = token_reference(&Value::Call { name: name.to_string(), args: args.to_vec() }) {
        return Ok(string(&format!("${token}")));
    }
    match name {
        "var" => Err("var() must reference a custom property, e.g. var(--Accent)".into()),
        "luau" | "lua" => match args {
            [Value::Str { text, .. }] => Ok(text.clone()),
            _ => Err(format!("{name}() takes exactly one string of Luau code")),
        },
        "url" => match args {
            [Value::Str { text, .. }] => Ok(string(text)),
            _ => Err("url() takes exactly one argument".into()),
        },
        "calc" | "min" | "max" | "clamp" | "env" | "attr" => Err(format!(
            "CSS {name}() can't be evaluated here; use plain arithmetic, or enable --approx for CSS properties"
        )),
        // A Luau constructor like `UDim.new(...)` passes through; a CSS function like
        // `linear-gradient(...)` can't be a Luau call at all.
        _ if name.contains('-') => Err(format!("CSS {name}() has no Luau equivalent here")),
        _ => {
            let parts = args.iter().map(|a| value(a, opts)).collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{name}({})", parts.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_values() {
        let o = LuauOptions::default();
        assert_eq!(value(&Value::num_unit(50.0, "%"), &o).unwrap(), "0.5");
        assert_eq!(value(&Value::num_unit(500.0, "ms"), &o).unwrap(), "0.5");
        assert_eq!(value(&Value::quoted("a\"b"), &o).unwrap(), "\"a\\\"b\"");
        let call = Value::Call { name: "var".into(), args: vec![Value::str("--Accent")] };
        assert_eq!(value(&call, &o).unwrap(), "\"$Accent\"");
        let udim = Value::Call { name: "UDim.new".into(), args: vec![Value::num(0.0), Value::num_unit(4.0, "px")] };
        assert_eq!(value(&udim, &o).unwrap(), "UDim.new(0, 4)");
        assert_eq!(table_key("BackgroundColor3"), "BackgroundColor3");
        assert_eq!(table_key("end"), "[\"end\"]");
    }
}
