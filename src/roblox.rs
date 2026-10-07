//! Converting SassScript values into typed Roblox values (see `luau::Value`).
//!
//! Only values of a known shape get through: literals, `Enum.<Type>.<Item>`, token references and
//! a fixed list of Roblox constructors. Anything else is an error, so no stylesheet text ever
//! reaches the generated Luau as code.

use crate::luau::{self, Color3};
use crate::value::{Color, ListSep, Number, Value};

/// Pixels per `rem`/`em`, a browser's default font size.
pub const REM_PX: f64 = 16.0;

#[derive(Clone, Debug, Default)]
pub struct ValueOptions {
    /// Whether `luau("...")` may insert raw Luau.
    pub allow_raw_luau: bool,
}

/// The Roblox constructors outlass understands, for error messages.
const CONSTRUCTORS: &str = "UDim.new, UDim2.new/fromScale/fromOffset, Vector2.new, Rect.new, NumberRange.new, \
     Color3.new/fromRGB/fromHex/fromHSV, ColorSequence.new, ColorSequenceKeypoint.new, NumberSequence.new, \
     NumberSequenceKeypoint.new, Font.new/fromName/fromId/fromEnum";

pub fn color3(c: &Color) -> luau::Value {
    luau::Value::Color3(to_color3(c))
}

fn to_color3(c: &Color) -> Color3 {
    Color3 { r: c.r, g: c.g, b: c.b }
}

/// Converts a number to a plain Luau number, interpreting CSS units:
/// `px` → as-is, `%` → fraction (`50%` → `0.5`), angles → degrees, times → seconds,
/// `em`/`rem` → pixels, absolute lengths → pixels.
pub fn number_value(n: &Number) -> Result<f64, String> {
    let Some(unit) = n.simple_unit() else {
        return Err(format!("{} has compound units and can't be converted to a Luau number", n.to_css()));
    };
    let lower = unit.to_ascii_lowercase();
    Ok(match lower.as_str() {
        "" | "px" => n.value,
        "%" => n.value / 100.0,
        "em" | "rem" => n.value * REM_PX,
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

/// Converts an evaluated value into a Roblox property or attribute value.
///
/// * quoted strings stay strings; unquoted ones must be `Enum.<Type>.<Item>` or `math.huge`
/// * colors become `Color3`s (alpha dropped)
/// * `var(--Name)` / `token(Name)` become the token reference `"$Name"`
/// * Roblox constructors like `UDim2.new(...)` become the value they construct
/// * `luau("code")` is raw Luau, refused unless `--allow-raw-luau`
pub fn value(v: &Value, opts: &ValueOptions) -> Result<luau::Value, String> {
    Ok(match v {
        Value::Null => return Err("null isn't a Roblox value".into()),
        Value::Bool(b) => luau::Value::Bool(*b),
        Value::Number(n) => luau::Value::Number(number_value(n)?),
        Value::Str { text, quoted: true } => luau::Value::String(text.clone()),
        Value::Str { text, quoted: false } => keyword(text)?,
        Value::Color(c) => color3(c),
        // `Font.new "rbxasset://..."`: Luau's call syntax for a single string argument.
        Value::List { items, sep: ListSep::Space, bracketed: false }
            if matches!(items.as_slice(), [Value::Str { quoted: false, .. }, Value::Str { quoted: true, .. }]) =>
        {
            call(items[0].as_str().unwrap_or_default(), &items[1..], opts)?
        }
        Value::List { .. } | Value::Map(_) => {
            return Err(format!("`{}` is a list, which isn't a Roblox value", v.inspect()));
        }
        Value::Function(name) => return Err(format!("function reference `{name}` can't be used as a property value")),
        Value::Call { name, args } => call(name, args, opts)?,
    })
}

/// An unquoted word: an Enum item or one of a few constants.
fn keyword(text: &str) -> Result<luau::Value, String> {
    match text {
        "math.huge" => return Ok(luau::Value::Number(f64::INFINITY)),
        "-math.huge" => return Ok(luau::Value::Number(f64::NEG_INFINITY)),
        "Vector2.zero" => return Ok(luau::Value::Vector2(0.0, 0.0)),
        "Vector2.one" => return Ok(luau::Value::Vector2(1.0, 1.0)),
        _ => {}
    }
    if let Some(rest) = text.strip_prefix("Enum.")
        && let Some((enum_type, item)) = rest.split_once('.')
        && luau::is_identifier(enum_type)
        && luau::is_identifier(item)
    {
        return Ok(luau::Value::enum_item(enum_type, item));
    }
    Err(format!(
        "`{text}` isn't a Roblox value; quote it (\"{text}\") for a string, or write an Enum like Enum.Font.Gotham"
    ))
}

/// Roblox attribute names may only contain letters, digits and `_`; other characters become `_`.
pub fn attribute_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// Whether `v` calls `luau()` anywhere.
pub fn uses_raw_luau(v: &Value) -> bool {
    match v {
        Value::Call { name, args } => name == "luau" || name == "lua" || args.iter().any(uses_raw_luau),
        Value::List { items, .. } => items.iter().any(uses_raw_luau),
        Value::Map(pairs) => pairs.iter().any(|(k, v)| uses_raw_luau(k) || uses_raw_luau(v)),
        _ => false,
    }
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

fn call(name: &str, args: &[Value], opts: &ValueOptions) -> Result<luau::Value, String> {
    if let Some(token) = token_reference(&Value::Call { name: name.to_string(), args: args.to_vec() }) {
        return Ok(luau::Value::Token(token));
    }
    let nums = |range: std::ops::RangeInclusive<usize>| -> Result<Vec<f64>, String> {
        if !range.contains(&args.len()) {
            let count = if range.start() == range.end() {
                range.start().to_string()
            } else {
                format!("{} to {}", range.start(), range.end())
            };
            return Err(format!("{name}() takes {count} numbers, got {}", args.len()));
        }
        let mut out = args
            .iter()
            .map(|a| match a {
                Value::Number(n) => number_value(n),
                other => match value(other, opts)? {
                    luau::Value::Number(n) => Ok(n),
                    _ => Err(format!("{name}() takes numbers, got `{}`", other.inspect())),
                },
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.resize(*range.end(), 0.0);
        Ok(out)
    };
    let string_arg = |i: usize| -> Result<String, String> {
        match args.get(i) {
            Some(Value::Str { text, quoted: true }) => Ok(text.clone()),
            Some(other) => Err(format!("{name}() takes a string, got `{}`", other.inspect())),
            None => Err(format!("{name}() is missing its string argument")),
        }
    };
    Ok(match name {
        "var" => return Err("var() must reference a custom property, e.g. var(--Accent)".into()),
        "luau" | "lua" => match args {
            [Value::Str { text, .. }] if opts.allow_raw_luau => luau::Value::Luau(text.clone()),
            [Value::Str { .. }] => {
                return Err(format!(
                    "{name}() inserts raw Luau, which can run any code; pass --allow-raw-luau to allow it \
                     (only for stylesheets you trust)"
                ));
            }
            _ => return Err(format!("{name}() takes exactly one string of Luau code")),
        },
        "url" => match args {
            [Value::Str { text, .. }] => luau::Value::String(text.clone()),
            _ => return Err("url() takes exactly one argument".into()),
        },
        "calc" | "min" | "max" | "clamp" | "env" | "attr" => {
            return Err(format!(
                "CSS {name}() can't be evaluated here; use plain arithmetic, or enable --approx for CSS properties"
            ));
        }
        "UDim.new" => {
            let n = nums(0..=2)?;
            luau::Value::udim(n[0], n[1])
        }
        "UDim2.new" => {
            let n = if args.is_empty() { vec![0.0; 4] } else { nums(4..=4)? };
            luau::Value::udim2(n[0], n[1], n[2], n[3])
        }
        "UDim2.fromScale" => {
            let n = nums(2..=2)?;
            luau::Value::udim2(n[0], 0.0, n[1], 0.0)
        }
        "UDim2.fromOffset" => {
            let n = nums(2..=2)?;
            luau::Value::udim2(0.0, n[0], 0.0, n[1])
        }
        "Vector2.new" => {
            let n = nums(0..=2)?;
            luau::Value::Vector2(n[0], n[1])
        }
        "Rect.new" => {
            let n = nums(4..=4)?;
            luau::Value::Rect(n[0], n[1], n[2], n[3])
        }
        "NumberRange.new" => {
            let n = nums(1..=2)?;
            luau::Value::NumberRange(n[0], if args.len() == 2 { n[1] } else { n[0] })
        }
        "Color3.new" | "Color3.fromRGB" | "Color3.fromHex" | "Color3.fromHSV" => {
            luau::Value::Color3(color_call(name, args)?)
        }
        "ColorSequence.new" => luau::Value::ColorSequence(match args {
            [Value::List { items, .. }] => {
                items.iter().map(|k| color_keypoint(k, opts)).collect::<Result<Vec<_>, _>>()?
            }
            [c] => {
                let c = color_arg(c, opts)?;
                vec![(0.0, c.clone()), (1.0, c)]
            }
            [a, b] => vec![(0.0, color_arg(a, opts)?), (1.0, color_arg(b, opts)?)],
            _ => return Err("ColorSequence.new() takes a color, two colors or a list of keypoints".into()),
        }),
        "NumberSequence.new" => luau::Value::NumberSequence(match args {
            [Value::List { items, .. }] => items.iter().map(number_keypoint).collect::<Result<Vec<_>, _>>()?,
            [_] => {
                let n = nums(1..=1)?;
                vec![(0.0, n[0], 0.0), (1.0, n[0], 0.0)]
            }
            [_, _] => {
                let n = nums(2..=2)?;
                vec![(0.0, n[0], 0.0), (1.0, n[1], 0.0)]
            }
            _ => return Err("NumberSequence.new() takes a number, two numbers or a list of keypoints".into()),
        }),
        "ColorSequenceKeypoint.new" | "NumberSequenceKeypoint.new" => {
            return Err(format!("{name}() is only valid inside a list given to {}.new()", &name[..name.len() - 12]));
        }
        "Font.new" | "Font.fromName" | "Font.fromId" => {
            let family = match name {
                "Font.new" => string_arg(0)?,
                "Font.fromName" => format!("rbxasset://fonts/families/{}.json", string_arg(0)?),
                _ => match args.first() {
                    Some(Value::Number(n)) if n.is_unitless() && n.value.fract() == 0.0 && n.value >= 0.0 => {
                        format!("rbxassetid://{}", n.value as u64)
                    }
                    _ => return Err("Font.fromId() takes an asset id".into()),
                },
            };
            if args.len() > 3 {
                return Err(format!("{name}() takes at most 3 arguments"));
            }
            let item = |i: usize, enum_type: &str| -> Result<Option<String>, String> {
                let Some(arg) = args.get(i) else { return Ok(None) };
                match value(arg, opts)? {
                    luau::Value::Enum { enum_type: t, item } if t == enum_type => Ok(Some(item)),
                    _ => Err(format!("{name}() expects an Enum.{enum_type} item, got `{}`", arg.inspect())),
                }
            };
            luau::Value::Font { family, weight: item(1, "FontWeight")?, style: item(2, "FontStyle")? }
        }
        "Font.fromEnum" => match args.first().map(|a| value(a, opts)).transpose()? {
            Some(luau::Value::Enum { enum_type, item }) if enum_type == "Font" && args.len() == 1 => {
                luau::Value::FontEnum(item)
            }
            _ => return Err("Font.fromEnum() takes an Enum.Font item".into()),
        },
        // A CSS function like `linear-gradient(...)` can't be a Luau call at all.
        _ if name.contains('-') => return Err(format!("CSS {name}() has no Luau equivalent here")),
        _ => return Err(format!("`{name}()` isn't a Roblox constructor outlass knows (it knows {CONSTRUCTORS})")),
    })
}

fn color_arg(v: &Value, opts: &ValueOptions) -> Result<Color3, String> {
    match value(v, opts)? {
        luau::Value::Color3(c) => Ok(c),
        _ => Err(format!("expected a color, got `{}`", v.inspect())),
    }
}

fn color_call(name: &str, args: &[Value]) -> Result<Color3, String> {
    let nums = || -> Result<[f64; 3], String> {
        if args.len() > 3 {
            return Err(format!("{name}() takes at most 3 numbers"));
        }
        let mut out = [0.0; 3];
        for (slot, a) in out.iter_mut().zip(args) {
            *slot = match a {
                Value::Number(n) => number_value(n)?,
                other => return Err(format!("{name}() takes numbers, got `{}`", other.inspect())),
            };
        }
        Ok(out)
    };
    Ok(match name {
        "Color3.new" => {
            let [r, g, b] = nums()?;
            Color3 { r: r * 255.0, g: g * 255.0, b: b * 255.0 }
        }
        "Color3.fromRGB" => {
            let [r, g, b] = nums()?;
            Color3 { r, g, b }
        }
        "Color3.fromHSV" => {
            let [h, s, v] = nums()?;
            hsv(h, s, v)
        }
        _ => {
            let [Value::Str { text, .. }] = args else { return Err("Color3.fromHex() takes one string".into()) };
            hex(text).ok_or_else(|| format!("Color3.fromHex(): `{text}` isn't a hex color"))?
        }
    })
}

fn hex(text: &str) -> Option<Color3> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(f64::from);
    match digits.len() {
        3 => {
            let d: Vec<String> = digits.chars().map(|c| format!("{c}{c}")).collect();
            Some(Color3 { r: channel(&d[0])?, g: channel(&d[1])?, b: channel(&d[2])? })
        }
        6 => Some(Color3 { r: channel(&digits[0..2])?, g: channel(&digits[2..4])?, b: channel(&digits[4..6])? }),
        _ => None,
    }
}

/// HSV (each 0-1, as Color3.fromHSV takes them) to RGB 0-255.
fn hsv(h: f64, s: f64, v: f64) -> Color3 {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Color3 { r: (r + m) * 255.0, g: (g + m) * 255.0, b: (b + m) * 255.0 }
}

fn keypoint_args<'a>(v: &'a Value, name: &str) -> Result<&'a [Value], String> {
    match v {
        Value::Call { name: n, args } if n == name => Ok(args),
        other => Err(format!("expected {name}(...), got `{}`", other.inspect())),
    }
}

fn color_keypoint(v: &Value, opts: &ValueOptions) -> Result<(f64, Color3), String> {
    match keypoint_args(v, "ColorSequenceKeypoint.new")? {
        [Value::Number(t), c] => Ok((number_value(t)?, color_arg(c, opts)?)),
        _ => Err("ColorSequenceKeypoint.new() takes a time and a color".into()),
    }
}

fn number_keypoint(v: &Value) -> Result<(f64, f64, f64), String> {
    let args = keypoint_args(v, "NumberSequenceKeypoint.new")?;
    let n = args
        .iter()
        .map(|a| match a {
            Value::Number(n) => number_value(n),
            other => Err(format!("NumberSequenceKeypoint.new() takes numbers, got `{}`", other.inspect())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    match n.as_slice() {
        [t, v] => Ok((*t, *v, 0.0)),
        [t, v, e] => Ok((*t, *v, *e)),
        _ => Err("NumberSequenceKeypoint.new() takes a time, a value and an optional envelope".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Vec<Value>) -> Value {
        Value::Call { name: name.into(), args }
    }

    #[test]
    fn converts_values() {
        let o = ValueOptions::default();
        assert_eq!(value(&Value::num_unit(50.0, "%"), &o).unwrap(), luau::Value::Number(0.5));
        assert_eq!(value(&Value::num_unit(500.0, "ms"), &o).unwrap(), luau::Value::Number(0.5));
        assert_eq!(value(&Value::quoted("a\"b"), &o).unwrap(), luau::Value::String("a\"b".into()));
        assert_eq!(value(&call("var", vec![Value::str("--Accent")]), &o).unwrap(), luau::Value::Token("Accent".into()));
        let udim = call("UDim.new", vec![Value::num(0.0), Value::num_unit(4.0, "px")]);
        assert_eq!(value(&udim, &o).unwrap(), luau::Value::udim(0.0, 4.0));
        assert_eq!(value(&Value::str("Enum.Font.Gotham"), &o).unwrap(), luau::Value::enum_item("Font", "Gotham"));
        let hex = call("Color3.fromHex", vec![Value::quoted("#ff8000")]);
        assert_eq!(value(&hex, &o).unwrap(), luau::Value::Color3(Color3 { r: 255.0, g: 128.0, b: 0.0 }));
    }

    #[test]
    fn refuses_anything_that_is_not_a_value() {
        let o = ValueOptions::default();
        // Unknown calls, raw words and words that only look like Enums.
        assert!(value(&call("require", vec![Value::num(1.0)]), &o).is_err());
        assert!(value(&call("game.GetService", vec![Value::str("x")]), &o).is_err());
        assert!(value(&Value::str("a)require(4)"), &o).is_err());
        assert!(value(&Value::str("Enum.Font.Gotham)require(1)--"), &o).is_err());
        assert!(value(&Value::str("Enum.Font"), &o).is_err());
        // Raw Luau only with permission.
        let raw = call("luau", vec![Value::quoted("require(1)")]);
        assert!(value(&raw, &o).is_err());
        let allowed = ValueOptions { allow_raw_luau: true, ..o };
        assert_eq!(value(&raw, &allowed).unwrap(), luau::Value::Luau("require(1)".into()));
    }
}
