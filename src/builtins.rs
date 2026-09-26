//! Built-in Sass functions (`math.*`, `color.*`, `string.*`, `list.*`, `map.*`, `meta.*`, and
//! their legacy global aliases).

use crate::value::*;

/// Evaluated call arguments. Names are stored WITHOUT `$` and with `_` normalized to `-`.
pub struct Args {
    pub positional: Vec<Value>,
    pub named: Vec<(String, Value)>,
    /// Tracks which positional slots have already been handed out by `take`, so that fixed
    /// argument indices keep working regardless of the order functions ask for them in.
    consumed: Vec<bool>,
}

fn normalize_name(name: &str) -> String {
    name.replace('_', "-")
}

impl Args {
    pub fn new(positional: Vec<Value>, named: Vec<(String, Value)>) -> Self {
        let named = named.into_iter().map(|(n, v)| (normalize_name(&n), v)).collect();
        let consumed = vec![false; positional.len()];
        Args { positional, named, consumed }
    }

    /// Returns the argument at `index` or named `name` (named wins), without allowing it to be
    /// returned again. None if absent.
    pub fn take(&mut self, index: usize, name: &str) -> Option<Value> {
        let name = normalize_name(name);
        if let Some(pos) = self.named.iter().position(|(n, _)| *n == name) {
            return Some(self.named.remove(pos).1);
        }
        if index < self.positional.len() && !self.consumed[index] {
            self.consumed[index] = true;
            return Some(self.positional[index].clone());
        }
        None
    }

    fn required(&mut self, index: usize, name: &str) -> Result<Value, String> {
        self.take(index, name).ok_or_else(|| format!("Missing argument ${name}."))
    }

    fn number_arg(&mut self, index: usize, name: &str) -> Result<Number, String> {
        let v = self.required(index, name)?;
        as_number(&v, name)
    }

    fn opt_number(&mut self, index: usize, name: &str) -> Result<Option<Number>, String> {
        match self.take(index, name) {
            Some(v) => Ok(Some(as_number(&v, name)?)),
            None => Ok(None),
        }
    }

    fn color_arg(&mut self, index: usize, name: &str) -> Result<Color, String> {
        let v = self.required(index, name)?;
        as_color(&v, name)
    }

    fn string_arg(&mut self, index: usize, name: &str) -> Result<String, String> {
        let v = self.required(index, name)?;
        as_string(&v, name)
    }
}

fn as_number(v: &Value, name: &str) -> Result<Number, String> {
    match v {
        Value::Number(n) => Ok(n.clone()),
        other => Err(format!("${name}: {} is not a number.", other.inspect())),
    }
}

fn as_color(v: &Value, name: &str) -> Result<Color, String> {
    match v {
        Value::Color(c) => Ok(c.clone()),
        other => Err(format!("${name}: {} is not a color.", other.inspect())),
    }
}

fn as_string(v: &Value, name: &str) -> Result<String, String> {
    match v {
        Value::Str { text, .. } => Ok(text.clone()),
        other => Err(format!("${name}: {} is not a string.", other.inspect())),
    }
}

/// True if this value can't participate in "plain number math" and a passthrough CSS call is
/// the right fallback (unquoted strings, `var(...)`/`calc(...)` calls).
fn is_special(v: &Value) -> bool {
    matches!(v, Value::Call { .. }) || matches!(v, Value::Str { quoted: false, .. })
}

fn percent_to_unit(n: &Number, name: &str) -> Result<f64, String> {
    if n.has_unit("%") {
        Ok(n.value / 100.0)
    } else if n.is_unitless() {
        Ok(n.value)
    } else {
        Err(format!("${name}: Expected {} to have no units or \"%\".", n.to_css()))
    }
}

fn channel_percent_or_num(n: &Number, name: &str) -> Result<f64, String> {
    if n.has_unit("%") {
        Ok(n.value / 100.0 * 255.0)
    } else if n.is_unitless() {
        Ok(n.value)
    } else {
        Err(format!("${name}: Expected {} to have no units or \"%\".", n.to_css()))
    }
}

// ---------------------------------------------------------------------------------------------
// math module
// ---------------------------------------------------------------------------------------------

fn to_radians(n: &Number, name: &str) -> Result<f64, String> {
    if n.is_unitless() {
        return Ok(n.value);
    }
    match n.simple_unit() {
        Some(u) if u.eq_ignore_ascii_case("deg") => Ok(n.value.to_radians()),
        Some(u) if u.eq_ignore_ascii_case("rad") => Ok(n.value),
        Some(u) if u.eq_ignore_ascii_case("grad") => Ok(n.value * std::f64::consts::PI / 200.0),
        Some(u) if u.eq_ignore_ascii_case("turn") => Ok(n.value * 2.0 * std::f64::consts::PI),
        _ => Err(format!("${name}: Expected {} to have an angle unit (deg, grad, rad, turn).", n.to_css())),
    }
}

fn from_radians_to_deg(v: f64) -> Number {
    Number::with_unit(v.to_degrees(), "deg")
}

fn math_module(name: &str, mut args: Args) -> Result<Value, String> {
    match name {
        "div" => {
            let a = args.number_arg(0, "number1")?;
            let b = args.number_arg(1, "number2")?;
            Ok(Value::Number(a.div(&b)))
        }
        "clamp" => {
            let min = args.number_arg(0, "min")?;
            let val = args.number_arg(1, "value")?;
            let max = args.number_arg(2, "max")?;
            if !min.is_comparable_to(&val) || !val.is_comparable_to(&max) || !min.is_comparable_to(&max) {
                return Err("Incompatible units for clamp().".into());
            }
            if val.cmp_value(&min) == Some(std::cmp::Ordering::Less) {
                return Ok(Value::Number(min));
            }
            if val.cmp_value(&max) == Some(std::cmp::Ordering::Greater) {
                return Ok(Value::Number(max));
            }
            Ok(Value::Number(val))
        }
        "sqrt" => {
            let n = args.number_arg(0, "number")?;
            if !n.is_unitless() {
                return Err(format!("$number: Expected {} to have no units.", n.to_css()));
            }
            Ok(Value::num(n.value.sqrt()))
        }
        "pow" => {
            let base = args.number_arg(0, "base")?;
            let exp = args.number_arg(1, "exponent")?;
            if !base.is_unitless() || !exp.is_unitless() {
                return Err("$base and $exponent must be unitless.".into());
            }
            Ok(Value::num(base.value.powf(exp.value)))
        }
        "log" => {
            let n = args.number_arg(0, "number")?;
            if !n.is_unitless() {
                return Err(format!("$number: Expected {} to have no units.", n.to_css()));
            }
            let base = args.opt_number(1, "base")?;
            let result = match base {
                Some(b) => n.value.log(b.value),
                None => n.value.ln(),
            };
            Ok(Value::num(result))
        }
        "sin" | "cos" | "tan" => {
            let n = args.number_arg(0, "number")?;
            let rad = to_radians(&n, "number")?;
            let result = match name {
                "sin" => rad.sin(),
                "cos" => rad.cos(),
                _ => rad.tan(),
            };
            Ok(Value::num(result))
        }
        "asin" => {
            let n = args.number_arg(0, "number")?;
            if !n.is_unitless() {
                return Err(format!("$number: Expected {} to have no units.", n.to_css()));
            }
            Ok(Value::Number(from_radians_to_deg(n.value.asin())))
        }
        "acos" => {
            let n = args.number_arg(0, "number")?;
            if !n.is_unitless() {
                return Err(format!("$number: Expected {} to have no units.", n.to_css()));
            }
            Ok(Value::Number(from_radians_to_deg(n.value.acos())))
        }
        "atan" => {
            let n = args.number_arg(0, "number")?;
            if !n.is_unitless() {
                return Err(format!("$number: Expected {} to have no units.", n.to_css()));
            }
            Ok(Value::Number(from_radians_to_deg(n.value.atan())))
        }
        "atan2" => {
            let y = args.number_arg(0, "y")?;
            let x = args.number_arg(1, "x")?;
            if !y.is_comparable_to(&x) {
                return Err("Incompatible units for atan2().".into());
            }
            let xv = y.value_in(x.simple_unit().unwrap_or("")).unwrap_or(x.value);
            let _ = xv;
            // Convert both into the same unit space for the ratio; unitless numbers pass through.
            let yv = y.value;
            let xv2 = if x.is_unitless() || y.is_unitless() {
                x.value
            } else {
                x.value_in(y.simple_unit().unwrap_or("")).unwrap_or(x.value)
            };
            Ok(Value::Number(from_radians_to_deg(yv.atan2(xv2))))
        }
        "hypot" => {
            let mut nums = Vec::new();
            let mut i = 0;
            while let Some(v) = args.take(i, &format!("number{}", i + 1)) {
                nums.push(as_number(&v, "number")?);
                i += 1;
            }
            if nums.is_empty() {
                return Err("At least one argument must be passed.".into());
            }
            let first = &nums[0];
            let mut sum = 0.0;
            for n in &nums {
                let v = first.value_in(n.simple_unit().unwrap_or("")).map(|_| n.value);
                let v = if first.is_unitless() || n.is_unitless() {
                    n.value
                } else {
                    n.value_in(first.simple_unit().unwrap_or("")).unwrap_or(v.unwrap_or(n.value))
                };
                sum += v * v;
            }
            let unit = nums.iter().find_map(|n| n.simple_unit().filter(|u| !u.is_empty())).unwrap_or("");
            Ok(Value::Number(Number::with_unit(sum.sqrt(), unit)))
        }
        "is-unitless" => {
            let n = args.number_arg(0, "number")?;
            Ok(Value::Bool(n.is_unitless()))
        }
        "compatible" => {
            let a = args.number_arg(0, "number1")?;
            let b = args.number_arg(1, "number2")?;
            Ok(Value::Bool(a.is_comparable_to(&b)))
        }
        _ => Err(format!("Unknown math function {name}.")),
    }
}

// Global math-related functions (percentage, round, ceil, floor, abs, min, max, random, unit,
// unitless, comparable).
fn math_global(name: &str, mut args: Args) -> Option<Result<Value, String>> {
    match name {
        "percentage" => Some((|| {
            let n = args.number_arg(0, "number")?;
            if !n.is_unitless() {
                return Err(format!("$number: Expected {} to have no units.", n.to_css()));
            }
            Ok(Value::Number(Number::with_unit(n.value * 100.0, "%")))
        })()),
        "round" | "ceil" | "floor" | "abs" => Some((|| {
            let v = args.required(0, "number")?;
            if is_special(&v) {
                return Ok(Value::Call { name: name.to_string(), args: vec![v] });
            }
            let n = as_number(&v, "number")?;
            let result = match name {
                "round" => n.value.round(),
                "ceil" => n.value.ceil(),
                "floor" => n.value.floor(),
                _ => n.value.abs(),
            };
            Ok(Value::Number(Number { value: result, numer: n.numer.clone(), denom: n.denom.clone() }))
        })()),
        "min" | "max" => Some((|| {
            let mut vals = Vec::new();
            let mut i = 0;
            while let Some(v) = args.take(i, &format!("number{}", i + 1)) {
                vals.push(v);
                i += 1;
            }
            if vals.is_empty() {
                return Err("At least one argument must be passed.".into());
            }
            if vals.iter().any(is_special) {
                return Ok(Value::Call { name: name.to_string(), args: vals });
            }
            let mut nums = Vec::new();
            for v in &vals {
                match v {
                    Value::Number(n) => nums.push(n.clone()),
                    // Not a plain number at all (e.g. a string that slipped through `is_special`):
                    // fall back to a passthrough CSS call rather than erroring.
                    _ => return Ok(Value::Call { name: name.to_string(), args: vals }),
                }
            }
            let mut best = nums[0].clone();
            for n in &nums[1..] {
                let cmp = match best.cmp_value(n) {
                    Some(c) => c,
                    // Incompatible units (e.g. `min(50%, 10px)`): not a Sass error, just not
                    // something we can resolve at compile time either — pass it through as CSS.
                    None => return Ok(Value::Call { name: name.to_string(), args: vals }),
                };
                let take_this =
                    if name == "min" { cmp == std::cmp::Ordering::Greater } else { cmp == std::cmp::Ordering::Less };
                if take_this {
                    best = n.clone();
                }
            }
            Ok(Value::Number(best))
        })()),
        "random" => Some((|| {
            let limit = args.opt_number(0, "limit")?;
            match limit {
                None => Ok(Value::num(next_random_f64())),
                Some(n) => {
                    let max = n.value.round() as i64;
                    if max < 1 {
                        return Err(format!("$limit: Must be greater than 0, was {}.", n.to_css()));
                    }
                    let r = (next_random_f64() * max as f64).floor() as i64 + 1;
                    Ok(Value::num(r.min(max) as f64))
                }
            }
        })()),
        "unit" => Some((|| {
            let n = args.number_arg(0, "number")?;
            Ok(Value::quoted(n.unit_string()))
        })()),
        "unitless" => Some((|| {
            let n = args.number_arg(0, "number")?;
            Ok(Value::Bool(n.is_unitless()))
        })()),
        "comparable" => Some((|| {
            let a = args.number_arg(0, "number1")?;
            let b = args.number_arg(1, "number2")?;
            Ok(Value::Bool(a.is_comparable_to(&b)))
        })()),
        _ => None,
    }
}

fn next_random_f64() -> f64 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }
    STATE.with(|state| {
        let mut x = state.get();
        if x == 0 {
            let nanos =
                SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E3779B97F4A7C15);
            x = nanos ^ 0x2545F4914F6CDD1D;
            if x == 0 {
                x = 0x9E3779B97F4A7C15;
            }
        }
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state.set(x);
        (x >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    })
}

// ---------------------------------------------------------------------------------------------
// color module / globals
// ---------------------------------------------------------------------------------------------

fn rgb_channel(v: &Value, name: &str) -> Result<f64, String> {
    let n = as_number(v, name)?;
    channel_percent_or_num(&n, name)
}

fn make_rgb(mut args: Args) -> Result<Value, String> {
    // First peek at the first argument (by either name) to decide which form this is.
    let first = args.take(0, "red").or_else(|| args.take(0, "color"));
    let is_color_alpha_form = matches!(first, Some(Value::Color(_)));
    let mut raw: Vec<Value> = first.into_iter().collect();
    if is_color_alpha_form {
        raw.extend(args.take(1, "alpha"));
    } else {
        raw.extend(args.take(1, "green"));
        raw.extend(args.take(2, "blue"));
        raw.extend(args.take(3, "alpha"));
    }
    if raw.iter().any(is_special) {
        return Ok(Value::Call { name: "rgb".into(), args: raw });
    }
    // color + alpha form
    if raw.len() == 2
        && let Value::Color(c) = &raw[0]
    {
        let a = as_number(&raw[1], "alpha")?;
        let alpha = percent_to_unit(&a, "alpha")?;
        return Ok(Value::Color(Color::rgba(c.r, c.g, c.b, alpha)));
    }
    if raw.len() == 3 || raw.len() == 4 {
        let r = rgb_channel(&raw[0], "red")?;
        let g = rgb_channel(&raw[1], "green")?;
        let b = rgb_channel(&raw[2], "blue")?;
        let a = if raw.len() == 4 { percent_to_unit(&as_number(&raw[3], "alpha")?, "alpha")? } else { 1.0 };
        return Ok(Value::Color(Color::rgba(r, g, b, a)));
    }
    Err("Expected 1, 2, 3, or 4 arguments.".into())
}

fn make_hsl(mut args: Args) -> Result<Value, String> {
    let first = args.take(0, "hue").or_else(|| args.take(0, "color"));
    let is_color_alpha_form = matches!(first, Some(Value::Color(_)));
    let mut raw: Vec<Value> = first.into_iter().collect();
    if is_color_alpha_form {
        raw.extend(args.take(1, "alpha"));
    } else {
        raw.extend(args.take(1, "saturation"));
        raw.extend(args.take(2, "lightness"));
        raw.extend(args.take(3, "alpha"));
    }
    if raw.iter().any(is_special) {
        return Ok(Value::Call { name: "hsl".into(), args: raw });
    }
    if raw.len() == 2
        && let Value::Color(c) = &raw[0]
    {
        let a = as_number(&raw[1], "alpha")?;
        let alpha = percent_to_unit(&a, "alpha")?;
        return Ok(Value::Color(Color::rgba(c.r, c.g, c.b, alpha)));
    }
    if raw.len() == 3 || raw.len() == 4 {
        let h = as_number(&raw[0], "hue")?;
        let s = as_number(&raw[1], "saturation")?;
        let l = as_number(&raw[2], "lightness")?;
        let a = if raw.len() == 4 { percent_to_unit(&as_number(&raw[3], "alpha")?, "alpha")? } else { 1.0 };
        return Ok(Value::Color(Color::from_hsl(h.value, s.value, l.value, a)));
    }
    Err("Expected 1, 2, 3, or 4 arguments.".into())
}

fn color_channel_global(name: &str, mut args: Args) -> Result<Value, String> {
    let c = args.color_arg(0, "color")?;
    match name {
        "red" => Ok(Value::num(c.r.round())),
        "green" => Ok(Value::num(c.g.round())),
        "blue" => Ok(Value::num(c.b.round())),
        "hue" => Ok(Value::Number(Number::with_unit(c.to_hsl().0, "deg"))),
        "saturation" => Ok(Value::Number(Number::with_unit(c.to_hsl().1, "%"))),
        "lightness" => Ok(Value::Number(Number::with_unit(c.to_hsl().2, "%"))),
        "alpha" | "opacity" => Ok(Value::num(c.a)),
        _ => unreachable!(),
    }
}

fn mix_colors(c1: &Color, c2: &Color, weight: f64) -> Color {
    // dart-sass alpha-weighted mixing algorithm.
    let w = weight * 2.0 - 1.0;
    let a = c1.a - c2.a;
    let w1 = if w * a == -1.0 { (w + 1.0) / 2.0 } else { (((w + a) / (1.0 + w * a)) + 1.0) / 2.0 };
    let w2 = 1.0 - w1;
    Color::rgba(
        c1.r * w1 + c2.r * w2,
        c1.g * w1 + c2.g * w2,
        c1.b * w1 + c2.b * w2,
        c1.a * weight + c2.a * (1.0 - weight),
    )
}

fn color_mix(mut args: Args) -> Result<Value, String> {
    let c1 = args.color_arg(0, "color1")?;
    let c2 = args.color_arg(1, "color2")?;
    let weight = args.opt_number(2, "weight")?.map(|n| percent_to_unit(&n, "weight")).transpose()?.unwrap_or(0.5);
    Ok(Value::Color(mix_colors(&c1, &c2, weight)))
}

fn adjust_hue_color(c: &Color, degrees: f64) -> Color {
    let (h, s, l) = c.to_hsl();
    Color::from_hsl(h + degrees, s, l, c.a)
}

fn lighten_color(c: &Color, amount: f64) -> Color {
    let (h, s, l) = c.to_hsl();
    Color::from_hsl(h, s, (l + amount).clamp(0.0, 100.0), c.a)
}

fn darken_color(c: &Color, amount: f64) -> Color {
    let (h, s, l) = c.to_hsl();
    Color::from_hsl(h, s, (l - amount).clamp(0.0, 100.0), c.a)
}

fn saturate_color(c: &Color, amount: f64) -> Color {
    let (h, s, l) = c.to_hsl();
    Color::from_hsl(h, (s + amount).clamp(0.0, 100.0), l, c.a)
}

fn desaturate_color(c: &Color, amount: f64) -> Color {
    let (h, s, l) = c.to_hsl();
    Color::from_hsl(h, (s - amount).clamp(0.0, 100.0), l, c.a)
}

fn grayscale_color(c: &Color) -> Color {
    let (h, _, l) = c.to_hsl();
    Color::from_hsl(h, 0.0, l, c.a)
}

fn complement_color(c: &Color) -> Color {
    let (h, s, l) = c.to_hsl();
    Color::from_hsl(h + 180.0, s, l, c.a)
}

fn invert_color(c: &Color, weight: f64) -> Color {
    let inv = Color::rgba(255.0 - c.r, 255.0 - c.g, 255.0 - c.b, c.a);
    mix_colors(&inv, c, weight)
}

fn ie_hex_str(c: &Color) -> String {
    let a = (c.a.clamp(0.0, 1.0) * 255.0).round() as u8;
    let r = c.r.round() as u8;
    let g = c.g.round() as u8;
    let b = c.b.round() as u8;
    format!("#{a:02X}{r:02X}{g:02X}{b:02X}")
}

/// Shared implementation of `adjust`/`scale`/`change` (color module) and their global aliases
/// `adjust-color`/`scale-color`/`change-color`.
fn adjust_color(mut args: Args) -> Result<Value, String> {
    let c = args.color_arg(0, "color")?;
    let mut r = c.r;
    let mut g = c.g;
    let mut b = c.b;
    let (mut h, mut s, mut l) = c.to_hsl();
    let mut a = c.a;
    if let Some(v) = args.take(0, "red") {
        r = (r + as_number(&v, "red")?.value).clamp(0.0, 255.0);
    }
    if let Some(v) = args.take(0, "green") {
        g = (g + as_number(&v, "green")?.value).clamp(0.0, 255.0);
    }
    if let Some(v) = args.take(0, "blue") {
        b = (b + as_number(&v, "blue")?.value).clamp(0.0, 255.0);
    }
    if let Some(v) = args.take(0, "hue") {
        h += as_number(&v, "hue")?.value;
    }
    if let Some(v) = args.take(0, "saturation") {
        s = (s + as_number(&v, "saturation")?.value).clamp(0.0, 100.0);
    }
    if let Some(v) = args.take(0, "lightness") {
        l = (l + as_number(&v, "lightness")?.value).clamp(0.0, 100.0);
    }
    if let Some(v) = args.take(0, "alpha") {
        let n = as_number(&v, "alpha")?;
        let delta = percent_to_unit(&n, "alpha")?;
        a = (a + delta).clamp(0.0, 1.0);
    }
    let used_rgb = r != c.r || g != c.g || b != c.b;
    let used_hsl = h != c.to_hsl().0 || s != c.to_hsl().1 || l != c.to_hsl().2;
    let base = if used_hsl && !used_rgb { Color::from_hsl(h, s, l, a) } else { Color::rgba(r, g, b, a) };
    Ok(Value::Color(base))
}

fn scale_color(mut args: Args) -> Result<Value, String> {
    let c = args.color_arg(0, "color")?;
    let scale = |cur: f64, max: f64, pct: f64| {
        let delta = if pct >= 0.0 { max - cur } else { cur };
        cur + delta * pct
    };
    let mut r = c.r;
    let mut g = c.g;
    let mut b = c.b;
    let (mut h, mut s, mut l) = c.to_hsl();
    let _ = h;
    let mut a = c.a;
    let mut used_rgb = false;
    let mut used_hsl = false;
    if let Some(v) = args.take(0, "red") {
        let pct = percent_to_unit(&as_number(&v, "red")?, "red")?;
        r = scale(r, 255.0, pct).clamp(0.0, 255.0);
        used_rgb = true;
    }
    if let Some(v) = args.take(0, "green") {
        let pct = percent_to_unit(&as_number(&v, "green")?, "green")?;
        g = scale(g, 255.0, pct).clamp(0.0, 255.0);
        used_rgb = true;
    }
    if let Some(v) = args.take(0, "blue") {
        let pct = percent_to_unit(&as_number(&v, "blue")?, "blue")?;
        b = scale(b, 255.0, pct).clamp(0.0, 255.0);
        used_rgb = true;
    }
    if let Some(v) = args.take(0, "saturation") {
        let pct = percent_to_unit(&as_number(&v, "saturation")?, "saturation")?;
        s = scale(s, 100.0, pct).clamp(0.0, 100.0);
        used_hsl = true;
    }
    if let Some(v) = args.take(0, "lightness") {
        let pct = percent_to_unit(&as_number(&v, "lightness")?, "lightness")?;
        l = scale(l, 100.0, pct).clamp(0.0, 100.0);
        used_hsl = true;
    }
    if let Some(v) = args.take(0, "alpha") {
        let pct = percent_to_unit(&as_number(&v, "alpha")?, "alpha")?;
        a = scale(a, 1.0, pct).clamp(0.0, 1.0);
    }
    h = c.to_hsl().0;
    if used_hsl && !used_rgb {
        Ok(Value::Color(Color::from_hsl(h, s, l, a)))
    } else {
        Ok(Value::Color(Color::rgba(r, g, b, a)))
    }
}

fn change_color(mut args: Args) -> Result<Value, String> {
    let c = args.color_arg(0, "color")?;
    let mut r = c.r;
    let mut g = c.g;
    let mut b = c.b;
    let (mut h, mut s, mut l) = c.to_hsl();
    let mut a = c.a;
    let mut used_rgb = false;
    let mut used_hsl = false;
    if let Some(v) = args.take(0, "red") {
        r = channel_percent_or_num(&as_number(&v, "red")?, "red")?;
        used_rgb = true;
    }
    if let Some(v) = args.take(0, "green") {
        g = channel_percent_or_num(&as_number(&v, "green")?, "green")?;
        used_rgb = true;
    }
    if let Some(v) = args.take(0, "blue") {
        b = channel_percent_or_num(&as_number(&v, "blue")?, "blue")?;
        used_rgb = true;
    }
    if let Some(v) = args.take(0, "hue") {
        h = as_number(&v, "hue")?.value;
        used_hsl = true;
    }
    if let Some(v) = args.take(0, "saturation") {
        s = as_number(&v, "saturation")?.value;
        used_hsl = true;
    }
    if let Some(v) = args.take(0, "lightness") {
        l = as_number(&v, "lightness")?.value;
        used_hsl = true;
    }
    if let Some(v) = args.take(0, "alpha") {
        a = percent_to_unit(&as_number(&v, "alpha")?, "alpha")?;
    }
    if used_hsl && !used_rgb {
        Ok(Value::Color(Color::from_hsl(h, s, l, a)))
    } else {
        Ok(Value::Color(Color::rgba(r, g, b, a)))
    }
}

fn color_module(name: &str, args: Args) -> Result<Value, String> {
    match name {
        "adjust" => adjust_color(args),
        "scale" => scale_color(args),
        "change" => change_color(args),
        "mix" => color_mix(args),
        "invert" => {
            let mut args = args;
            let c = args.color_arg(0, "color")?;
            let weight =
                args.opt_number(1, "weight")?.map(|n| percent_to_unit(&n, "weight")).transpose()?.unwrap_or(1.0);
            Ok(Value::Color(invert_color(&c, weight)))
        }
        "complement" => Ok(Value::Color(complement_color(&args_first_color(args, "color")?))),
        "grayscale" => Ok(Value::Color(grayscale_color(&args_first_color(args, "color")?))),
        "lighten" | "darken" | "saturate" | "desaturate" | "adjust-hue" | "opacify" | "fade-in" | "transparentize"
        | "fade-out" => global_color_adjust(name, args),
        "ie-hex-str" => Ok(Value::quoted(ie_hex_str(&args_first_color(args, "color")?))),
        "red" | "green" | "blue" | "hue" | "saturation" | "lightness" | "alpha" => color_channel_global(name, args),
        "whiteness" => {
            let c = args_first_color(args, "color")?;
            let w = c.r.min(c.g).min(c.b) / 255.0 * 100.0;
            Ok(Value::Number(Number::with_unit(w, "%")))
        }
        "blackness" => {
            let c = args_first_color(args, "color")?;
            let bl = 100.0 - c.r.max(c.g).max(c.b) / 255.0 * 100.0;
            Ok(Value::Number(Number::with_unit(bl, "%")))
        }
        _ => Err(format!("Unknown color function {name}.")),
    }
}

fn args_first_color(mut args: Args, name: &str) -> Result<Color, String> {
    args.color_arg(0, name)
}

fn global_color_adjust(name: &str, mut args: Args) -> Result<Value, String> {
    // `grayscale($number)`/`invert($number)`/`saturate($number)` are CSS filter functions.
    if matches!(name, "grayscale" | "invert" | "saturate") {
        if let Some(v) = args.take(0, "color") {
            if matches!(v, Value::Number(_)) || is_special(&v) {
                let mut call_args = vec![v];
                call_args.extend(args.take(1, "amount"));
                return Ok(Value::Call { name: name.to_string(), args: call_args });
            }
            let c = as_color(&v, "color")?;
            return match name {
                "grayscale" => Ok(Value::Color(grayscale_color(&c))),
                "invert" => {
                    let weight = args
                        .opt_number(0, "weight")?
                        .map(|n| percent_to_unit(&n, "weight"))
                        .transpose()?
                        .unwrap_or(1.0);
                    Ok(Value::Color(invert_color(&c, weight)))
                }
                "saturate" => {
                    let amt = args.number_arg(0, "amount")?;
                    Ok(Value::Color(saturate_color(&c, percent_to_unit(&amt, "amount")? * 100.0)))
                }
                _ => unreachable!(),
            };
        }
        return Err("Missing argument $color.".into());
    }
    let c = args.color_arg(0, "color")?;
    match name {
        "lighten" => {
            let amt = args.number_arg(1, "amount")?;
            Ok(Value::Color(lighten_color(&c, percent_to_unit(&amt, "amount")? * 100.0)))
        }
        "darken" => {
            let amt = args.number_arg(1, "amount")?;
            Ok(Value::Color(darken_color(&c, percent_to_unit(&amt, "amount")? * 100.0)))
        }
        "desaturate" => {
            let amt = args.number_arg(1, "amount")?;
            Ok(Value::Color(desaturate_color(&c, percent_to_unit(&amt, "amount")? * 100.0)))
        }
        "adjust-hue" => {
            let amt = args.number_arg(1, "degrees")?;
            Ok(Value::Color(adjust_hue_color(&c, amt.value)))
        }
        "opacify" | "fade-in" => {
            let amt = args.number_arg(1, "amount")?;
            let delta = percent_to_unit(&amt, "amount")?;
            Ok(Value::Color(Color::rgba(c.r, c.g, c.b, (c.a + delta).clamp(0.0, 1.0))))
        }
        "transparentize" | "fade-out" => {
            let amt = args.number_arg(1, "amount")?;
            let delta = percent_to_unit(&amt, "amount")?;
            Ok(Value::Color(Color::rgba(c.r, c.g, c.b, (c.a - delta).clamp(0.0, 1.0))))
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------------------------
// string module / globals
// ---------------------------------------------------------------------------------------------

fn char_len(s: &str) -> i64 {
    s.chars().count() as i64
}

/// Converts a 1-based (possibly negative) Sass string index into a 0-based char index, clamped.
fn resolve_index(idx: i64, len: i64) -> i64 {
    if idx < 0 { (len + idx + 1).max(0) } else { idx }
}

fn string_module(name: &str, mut args: Args) -> Result<Value, String> {
    match name {
        "quote" => {
            let s = args.string_arg(0, "string")?;
            Ok(Value::quoted(s))
        }
        "unquote" => {
            let s = args.string_arg(0, "string")?;
            Ok(Value::str(s))
        }
        "length" => {
            let s = args.string_arg(0, "string")?;
            Ok(Value::num(char_len(&s) as f64))
        }
        "to-upper-case" | "to-lower-case" => {
            let v = args.required(0, "string")?;
            let quoted = matches!(v, Value::Str { quoted: true, .. });
            let s = as_string(&v, "string")?;
            let out = if name == "to-upper-case" { s.to_uppercase() } else { s.to_lowercase() };
            Ok(preserve_quote(&quoted, out))
        }
        "unique-id" => Ok(Value::str(gen_unique_id())),
        "index" => {
            let s = args.string_arg(0, "string")?;
            let sub = args.string_arg(1, "substring")?;
            match s.find(&sub) {
                Some(byte_idx) => {
                    let char_idx = s[..byte_idx].chars().count() as f64 + 1.0;
                    Ok(Value::num(char_idx))
                }
                None => Ok(Value::Null),
            }
        }
        "insert" => {
            let v = args.required(0, "string")?;
            let quoted = matches!(v, Value::Str { quoted: true, .. });
            let s = as_string(&v, "string")?;
            let insert = args.string_arg(1, "insert")?;
            let idx = args.number_arg(2, "index")?.value.round() as i64;
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let pos;
            if idx > 0 {
                pos = (idx - 1).min(len);
            } else if idx < 0 {
                pos = (len + idx + 1).clamp(0, len);
            } else {
                pos = 0;
            }
            let pos = pos.clamp(0, len) as usize;
            let mut out: String = chars[..pos].iter().collect();
            out.push_str(&insert);
            out.push_str(&chars[pos..].iter().collect::<String>());
            Ok(preserve_quote(&quoted, out))
        }
        "slice" => {
            let v = args.required(0, "string")?;
            let quoted = matches!(v, Value::Str { quoted: true, .. });
            let s = as_string(&v, "string")?;
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let start = args.number_arg(1, "start-at")?.value.round() as i64;
            let end = args.opt_number(2, "end-at")?.map(|n| n.value.round() as i64).unwrap_or(-1);
            if len == 0 {
                return Ok(preserve_quote(&quoted, String::new()));
            }
            let mut s_idx = resolve_index(start, len);
            let mut e_idx = resolve_index(end, len);
            if start == 0 {
                s_idx = 1;
            }
            if end == 0 {
                e_idx = 0;
            }
            let s_idx = s_idx.clamp(1, len);
            let e_idx = e_idx.clamp(0, len);
            if e_idx < s_idx {
                return Ok(preserve_quote(&quoted, String::new()));
            }
            let slice: String = chars[(s_idx - 1) as usize..e_idx as usize].iter().collect();
            Ok(preserve_quote(&quoted, slice))
        }
        "split" => {
            let s = args.string_arg(0, "string")?;
            let sep = args.string_arg(1, "separator")?;
            let limit = args.opt_number(2, "limit")?.map(|n| n.value.round() as i64);
            let parts: Vec<&str> = if sep.is_empty() {
                return Ok(Value::list(s.chars().map(|c| Value::quoted(c.to_string())).collect(), ListSep::Comma));
            } else {
                match limit {
                    Some(n) if n > 0 => s.splitn(n as usize, sep.as_str()).collect(),
                    _ => s.split(sep.as_str()).collect(),
                }
            };
            Ok(Value::list(parts.into_iter().map(Value::quoted).collect(), ListSep::Comma))
        }
        _ => Err(format!("Unknown string function {name}.")),
    }
}

fn preserve_quote(quoted: &bool, s: String) -> Value {
    if *quoted { Value::quoted(s) } else { Value::str(s) }
}

fn gen_unique_id() -> String {
    let n = (next_random_f64() * (36u64.pow(8)) as f64) as u64;
    let mut s = String::from("u");
    let mut n = n;
    let alphabet = "0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::new();
    if n == 0 {
        buf.push(b'0');
    }
    while n > 0 {
        buf.push(alphabet.as_bytes()[(n % 36) as usize]);
        n /= 36;
    }
    while buf.len() < 8 {
        buf.push(b'0');
    }
    s.push_str(&String::from_utf8(buf).unwrap());
    s
}

fn string_global(name: &str, args: Args) -> Option<Result<Value, String>> {
    match name {
        "quote" | "unquote" | "unique-id" | "to-upper-case" | "to-lower-case" => Some(string_module(name, args)),
        "str-length" => Some(string_module("length", args)),
        "str-index" => Some(string_module("index", args)),
        "str-insert" => Some(string_module("insert", args)),
        "str-slice" => Some(string_module("slice", args)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// list module / globals
// ---------------------------------------------------------------------------------------------

fn list_len(v: &Value) -> i64 {
    v.as_list().len() as i64
}

fn resolve_list_index(idx: i64, len: i64, name: &str) -> Result<usize, String> {
    if idx == 0 {
        return Err("$n: List index may not be 0.".to_string());
    }
    let real = if idx < 0 { len + idx + 1 } else { idx };
    if real < 1 || real > len {
        return Err(format!("${name}: Invalid index {idx} for a list with {len} elements."));
    }
    Ok((real - 1) as usize)
}

fn list_module(name: &str, mut args: Args) -> Result<Value, String> {
    match name {
        "length" => {
            let v = args.required(0, "list")?;
            Ok(Value::num(list_len(&v) as f64))
        }
        "nth" => {
            let v = args.required(0, "list")?;
            let items = v.as_list();
            let idx = args.number_arg(1, "n")?.value.round() as i64;
            let i = resolve_list_index(idx, items.len() as i64, "n")?;
            Ok(items[i].clone())
        }
        "set-nth" => {
            let v = args.required(0, "list")?;
            let mut items = v.as_list();
            let idx = args.number_arg(1, "n")?.value.round() as i64;
            let i = resolve_list_index(idx, items.len() as i64, "n")?;
            let new_val = args.required(2, "value")?;
            items[i] = new_val;
            Ok(Value::List {
                items,
                sep: v.separator(),
                bracketed: matches!(&v, Value::List{bracketed,..} if *bracketed),
            })
        }
        "join" => {
            let l1 = args.required(0, "list1")?;
            let l2 = args.take(1, "list2").unwrap_or(Value::empty_list());
            let sep_arg = args.take(2, "separator");
            let bracketed_arg = args.take(3, "bracketed");
            let items1 = l1.as_list();
            let items2 = l2.as_list();
            let sep = match sep_arg.as_ref().and_then(|v| v.as_str()) {
                Some("comma") => ListSep::Comma,
                Some("space") => ListSep::Space,
                Some("slash") => ListSep::Slash,
                _ => {
                    if !items1.is_empty() {
                        l1.separator()
                    } else if !items2.is_empty() {
                        l2.separator()
                    } else {
                        ListSep::Space
                    }
                }
            };
            let bracketed = match bracketed_arg {
                Some(Value::Bool(b)) => b,
                Some(v) if v.is_truthy() => true,
                Some(_) => false,
                None => matches!(&l1, Value::List { bracketed: true, .. }),
            };
            let mut items = items1;
            items.extend(items2);
            Ok(Value::List { items, sep, bracketed })
        }
        "append" => {
            let l = args.required(0, "list")?;
            let val = args.required(1, "val")?;
            let sep_arg = args.take(2, "separator");
            let mut items = l.as_list();
            let sep = match sep_arg.as_ref().and_then(|v| v.as_str()) {
                Some("comma") => ListSep::Comma,
                Some("space") => ListSep::Space,
                Some("slash") => ListSep::Slash,
                _ => l.separator(),
            };
            items.push(val);
            Ok(Value::List { items, sep, bracketed: matches!(&l, Value::List{bracketed,..} if *bracketed) })
        }
        "zip" => {
            let mut all: Vec<Vec<Value>> = Vec::new();
            let mut i = 0;
            while let Some(v) = args.take(i, &format!("list{}", i + 1)) {
                all.push(v.as_list());
                i += 1;
            }
            if all.is_empty() {
                return Ok(Value::empty_list());
            }
            let min_len = all.iter().map(|v| v.len()).min().unwrap_or(0);
            let mut out = Vec::new();
            for idx in 0..min_len {
                let tuple: Vec<Value> = all.iter().map(|l| l[idx].clone()).collect();
                out.push(Value::list(tuple, ListSep::Space));
            }
            Ok(Value::list(out, ListSep::Comma))
        }
        "index" => {
            let l = args.required(0, "list")?;
            let val = args.required(1, "value")?;
            let items = l.as_list();
            match items.iter().position(|v| v.sass_eq(&val)) {
                Some(i) => Ok(Value::num((i + 1) as f64)),
                None => Ok(Value::Null),
            }
        }
        "is-bracketed" => {
            let l = args.required(0, "list")?;
            Ok(Value::Bool(matches!(l, Value::List { bracketed: true, .. })))
        }
        "separator" => {
            let l = args.required(0, "list")?;
            Ok(Value::str(l.separator().name()))
        }
        "slash" => {
            let mut items = Vec::new();
            let mut i = 0;
            while let Some(v) = args.take(i, &format!("arg{}", i + 1)) {
                items.push(v);
                i += 1;
            }
            if items.len() < 2 {
                return Err("At least 2 arguments must be passed.".into());
            }
            Ok(Value::list(items, ListSep::Slash))
        }
        _ => Err(format!("Unknown list function {name}.")),
    }
}

// ---------------------------------------------------------------------------------------------
// map module / globals
// ---------------------------------------------------------------------------------------------

fn as_pairs(v: &Value, name: &str) -> Result<Vec<(Value, Value)>, String> {
    v.as_map().ok_or_else(|| format!("${name}: {} is not a map.", v.inspect()))
}

fn map_get_nested<'a>(pairs: &'a [(Value, Value)], keys: &[Value]) -> Option<&'a Value> {
    let (first, rest) = keys.split_first()?;
    let v = pairs.iter().find(|(k, _)| k.sass_eq(first)).map(|(_, v)| v)?;
    if rest.is_empty() {
        Some(v)
    } else {
        match v {
            Value::Map(p) => map_get_nested(p, rest),
            _ => None,
        }
    }
}

fn map_module(name: &str, mut args: Args) -> Result<Value, String> {
    match name {
        "get" => {
            let m = args.required(0, "map")?;
            let pairs = as_pairs(&m, "map")?;
            let mut keys = Vec::new();
            let mut i = 1;
            while let Some(v) = args.take(i, &format!("key{}", i)) {
                keys.push(v);
                i += 1;
            }
            if keys.is_empty() {
                return Err("Missing argument $key.".into());
            }
            Ok(map_get_nested(&pairs, &keys).cloned().unwrap_or(Value::Null))
        }
        "has-key" => {
            let m = args.required(0, "map")?;
            let pairs = as_pairs(&m, "map")?;
            let mut keys = Vec::new();
            let mut i = 1;
            while let Some(v) = args.take(i, &format!("key{}", i)) {
                keys.push(v);
                i += 1;
            }
            Ok(Value::Bool(map_get_nested(&pairs, &keys).is_some()))
        }
        "keys" => {
            let m = args.required(0, "map")?;
            let pairs = as_pairs(&m, "map")?;
            Ok(Value::list(pairs.into_iter().map(|(k, _)| k).collect(), ListSep::Comma))
        }
        "values" => {
            let m = args.required(0, "map")?;
            let pairs = as_pairs(&m, "map")?;
            Ok(Value::list(pairs.into_iter().map(|(_, v)| v).collect(), ListSep::Comma))
        }
        "merge" => {
            let m = args.required(0, "map")?;
            let mut pairs = as_pairs(&m, "map")?;
            // Collect remaining args: could be [map2] or [key.., map2] for nested merge.
            let mut rest = Vec::new();
            let mut i = 1;
            while let Some(v) = args.take(i, &format!("arg{}", i)) {
                rest.push(v);
                i += 1;
            }
            if rest.is_empty() {
                return Err("Missing argument $map2.".into());
            }
            let map2 = rest.pop().unwrap();
            let map2_pairs = as_pairs(&map2, "map2")?;
            let keys = rest;
            merge_at(&mut pairs, &keys, &map2_pairs)?;
            Ok(Value::Map(pairs))
        }
        "deep-merge" => {
            let m = args.required(0, "map")?;
            let pairs = as_pairs(&m, "map")?;
            let m2 = args.required(1, "map2")?;
            let pairs2 = as_pairs(&m2, "map2")?;
            Ok(Value::Map(deep_merge(&pairs, &pairs2)))
        }
        "remove" => {
            let m = args.required(0, "map")?;
            let mut pairs = as_pairs(&m, "map")?;
            let mut i = 1;
            while let Some(k) = args.take(i, &format!("key{}", i)) {
                pairs.retain(|(pk, _)| !pk.sass_eq(&k));
                i += 1;
            }
            Ok(Value::Map(pairs))
        }
        "deep-remove" => {
            let m = args.required(0, "map")?;
            let mut pairs = as_pairs(&m, "map")?;
            let mut keys = Vec::new();
            let mut i = 1;
            while let Some(v) = args.take(i, &format!("key{}", i)) {
                keys.push(v);
                i += 1;
            }
            if keys.is_empty() {
                return Err("Missing argument $key.".into());
            }
            deep_remove(&mut pairs, &keys);
            Ok(Value::Map(pairs))
        }
        "set" => {
            let m = args.required(0, "map")?;
            let mut pairs = as_pairs(&m, "map")?;
            let mut rest = Vec::new();
            let mut i = 1;
            while let Some(v) = args.take(i, &format!("arg{}", i)) {
                rest.push(v);
                i += 1;
            }
            if rest.len() < 2 {
                return Err("Missing argument $value.".into());
            }
            let value = rest.pop().unwrap();
            let keys = rest;
            set_at(&mut pairs, &keys, value);
            Ok(Value::Map(pairs))
        }
        _ => Err(format!("Unknown map function {name}.")),
    }
}

fn merge_at(pairs: &mut Vec<(Value, Value)>, keys: &[Value], map2: &[(Value, Value)]) -> Result<(), String> {
    if keys.is_empty() {
        for (k, v) in map2 {
            set_key(pairs, k.clone(), v.clone());
        }
        return Ok(());
    }
    let (first, rest) = keys.split_first().unwrap();
    let existing = pairs.iter().find(|(k, _)| k.sass_eq(first)).map(|(_, v)| v.clone());
    let mut sub = match existing {
        Some(Value::Map(p)) => p,
        Some(_) | None => Vec::new(),
    };
    merge_at(&mut sub, rest, map2)?;
    set_key(pairs, first.clone(), Value::Map(sub));
    Ok(())
}

fn set_at(pairs: &mut Vec<(Value, Value)>, keys: &[Value], value: Value) {
    if keys.is_empty() {
        return;
    }
    let (first, rest) = keys.split_first().unwrap();
    if rest.is_empty() {
        set_key(pairs, first.clone(), value);
        return;
    }
    let existing = pairs.iter().find(|(k, _)| k.sass_eq(first)).map(|(_, v)| v.clone());
    let mut sub = match existing {
        Some(Value::Map(p)) => p,
        _ => Vec::new(),
    };
    set_at(&mut sub, rest, value);
    set_key(pairs, first.clone(), Value::Map(sub));
}

fn set_key(pairs: &mut Vec<(Value, Value)>, key: Value, value: Value) {
    if let Some(entry) = pairs.iter_mut().find(|(k, _)| k.sass_eq(&key)) {
        entry.1 = value;
    } else {
        pairs.push((key, value));
    }
}

fn deep_merge(a: &[(Value, Value)], b: &[(Value, Value)]) -> Vec<(Value, Value)> {
    let mut out = a.to_vec();
    for (k, v) in b {
        let existing = out.iter().find(|(ek, _)| ek.sass_eq(k)).map(|(_, ev)| ev.clone());
        let merged = match (existing, v) {
            (Some(Value::Map(ep)), Value::Map(vp)) => Value::Map(deep_merge(&ep, vp)),
            _ => v.clone(),
        };
        set_key(&mut out, k.clone(), merged);
    }
    out
}

fn deep_remove(pairs: &mut Vec<(Value, Value)>, keys: &[Value]) {
    if keys.len() == 1 {
        pairs.retain(|(k, _)| !k.sass_eq(&keys[0]));
        return;
    }
    if let Some(entry) = pairs.iter_mut().find(|(k, _)| k.sass_eq(&keys[0]))
        && let Value::Map(sub) = &mut entry.1
    {
        deep_remove(sub, &keys[1..]);
    }
}

// ---------------------------------------------------------------------------------------------
// meta module
// ---------------------------------------------------------------------------------------------

fn meta_module(name: &str, mut args: Args) -> Result<Value, String> {
    match name {
        "type-of" => {
            let v = args.required(0, "value")?;
            Ok(Value::str(v.type_name()))
        }
        "inspect" => {
            let v = args.required(0, "value")?;
            Ok(Value::str(v.inspect()))
        }
        "feature-exists" => {
            let s = args.string_arg(0, "feature")?;
            let known = matches!(
                s.as_str(),
                "global-variable-shadowing"
                    | "extend-selector-pseudoclass"
                    | "units-level-3"
                    | "at-error"
                    | "custom-property"
            );
            Ok(Value::Bool(known))
        }
        _ => Err(format!("Unknown meta function {name}.")),
    }
}

// ---------------------------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------------------------

/// Calls a builtin. `module` is Some("math") for `math.div(...)` etc, or None for the global
/// (legacy) name. Returns None if no such builtin exists.
pub fn call(module: Option<&str>, name: &str, args: Args) -> Option<Result<Value, String>> {
    let name = normalize_name(name);
    match module {
        Some("math") => {
            if math_module_names().contains(&name.as_str()) {
                Some(math_module(&name, args))
            } else {
                None
            }
        }
        Some("color") => {
            if color_module_names().contains(&name.as_str()) {
                Some(color_module(&name, args))
            } else {
                None
            }
        }
        Some("string") => {
            if string_module_names().contains(&name.as_str()) {
                Some(string_module(&name, args))
            } else {
                None
            }
        }
        Some("list") => {
            if list_module_names().contains(&name.as_str()) {
                Some(list_module(&name, args))
            } else {
                None
            }
        }
        Some("map") => {
            if map_module_names().contains(&name.as_str()) {
                Some(map_module(&name, args))
            } else {
                None
            }
        }
        Some("meta") => {
            if meta_module_names().contains(&name.as_str()) {
                Some(meta_module(&name, args))
            } else {
                None
            }
        }
        Some("selector") => None,
        Some(_) => None,
        None => call_global(&name, args),
    }
}

fn math_module_names() -> &'static [&'static str] {
    &[
        "div",
        "clamp",
        "sqrt",
        "pow",
        "log",
        "sin",
        "cos",
        "tan",
        "asin",
        "acos",
        "atan",
        "atan2",
        "hypot",
        "is-unitless",
        "compatible",
    ]
}
fn color_module_names() -> &'static [&'static str] {
    &[
        "adjust",
        "scale",
        "change",
        "mix",
        "invert",
        "complement",
        "grayscale",
        "ie-hex-str",
        "red",
        "green",
        "blue",
        "hue",
        "saturation",
        "lightness",
        "alpha",
        "whiteness",
        "blackness",
    ]
}
fn string_module_names() -> &'static [&'static str] {
    &["quote", "unquote", "length", "index", "insert", "slice", "to-upper-case", "to-lower-case", "unique-id", "split"]
}
fn list_module_names() -> &'static [&'static str] {
    &["length", "nth", "set-nth", "join", "append", "zip", "index", "is-bracketed", "separator", "slash"]
}
fn map_module_names() -> &'static [&'static str] {
    &["get", "has-key", "keys", "values", "merge", "deep-merge", "remove", "deep-remove", "set"]
}
fn meta_module_names() -> &'static [&'static str] {
    &["type-of", "inspect", "feature-exists"]
}

fn call_global(name: &str, args: Args) -> Option<Result<Value, String>> {
    match name {
        // math globals
        "percentage" | "round" | "ceil" | "floor" | "abs" | "min" | "max" | "random" | "unit" | "unitless"
        | "comparable" => math_global(name, args),
        // color globals
        "rgb" | "rgba" => Some(make_rgb(args)),
        "hsl" | "hsla" => Some(make_hsl(args)),
        "red" | "green" | "blue" | "hue" | "saturation" | "lightness" | "alpha" | "opacity" => {
            Some(color_channel_global_smart(name, args))
        }
        "mix" => Some(color_mix(args)),
        "lighten" | "darken" | "saturate" | "desaturate" | "adjust-hue" | "opacify" | "fade-in" | "transparentize"
        | "fade-out" => Some(global_color_adjust(name, args)),
        "grayscale" => Some(global_color_adjust(name, args)),
        "complement" => Some((|| Ok(Value::Color(complement_color(&args_first_color(args, "color")?))))()),
        "invert" => Some(global_color_adjust(name, args)),
        "adjust-color" => Some(adjust_color(args)),
        "scale-color" => Some(scale_color(args)),
        "change-color" => Some(change_color(args)),
        "ie-hex-str" => Some((|| Ok(Value::quoted(ie_hex_str(&args_first_color(args, "color")?))))()),
        // string globals
        "quote" | "unquote" | "unique-id" | "str-length" | "str-index" | "str-insert" | "str-slice"
        | "to-upper-case" | "to-lower-case" => string_global(name, args),
        // list globals
        "length" | "nth" | "set-nth" | "join" | "append" | "zip" | "index" | "is-bracketed" | "list-separator" => {
            Some(list_global(name, args))
        }
        // map globals
        "map-get" | "map-merge" | "map-remove" | "map-keys" | "map-values" | "map-has-key" => {
            Some(map_global(name, args))
        }
        // meta globals
        "type-of" | "inspect" | "feature-exists" => Some(meta_module(name, args)),
        _ => None,
    }
}

fn color_channel_global_smart(name: &str, mut args: Args) -> Result<Value, String> {
    if name == "alpha" {
        // `alpha($color)` (one positional color arg) vs legacy `alpha(opacity=NN)` IE syntax we
        // don't support; just treat as the color channel getter.
        if !args.positional.is_empty() || args.named.iter().any(|(n, _)| n == "color") {
            return color_channel_global("alpha", args);
        }
    }
    if name == "opacity" {
        // CSS `opacity` property value passthrough is not a function call; if arg isn't a color,
        // pass through.
        if let Some(v) = args.take(0, "color") {
            if let Value::Color(c) = &v {
                return Ok(Value::num(c.a));
            }
            return Ok(Value::Call { name: "opacity".into(), args: vec![v] });
        }
        return Err("Missing argument $color.".into());
    }
    color_channel_global(name, args)
}

fn list_global(name: &str, args: Args) -> Result<Value, String> {
    match name {
        "list-separator" => list_module("separator", args),
        other => list_module(other, args),
    }
}

fn map_global(name: &str, args: Args) -> Result<Value, String> {
    match name {
        "map-get" => map_module("get", args),
        "map-merge" => map_module("merge", args),
        "map-remove" => map_module("remove", args),
        "map-keys" => map_module("keys", args),
        "map-values" => map_module("values", args),
        "map-has-key" => map_module("has-key", args),
        _ => unreachable!(),
    }
}

/// True if `call(module, name, ..)` would find a builtin.
pub fn exists(module: Option<&str>, name: &str) -> bool {
    let name = normalize_name(name);
    match module {
        Some("math") => math_module_names().contains(&name.as_str()),
        Some("color") => color_module_names().contains(&name.as_str()),
        Some("string") => string_module_names().contains(&name.as_str()),
        Some("list") => list_module_names().contains(&name.as_str()),
        Some("map") => map_module_names().contains(&name.as_str()),
        Some("meta") => meta_module_names().contains(&name.as_str()),
        Some(_) => false,
        None => global_names().contains(&name.as_str()),
    }
}

fn global_names() -> &'static [&'static str] {
    &[
        "percentage",
        "round",
        "ceil",
        "floor",
        "abs",
        "min",
        "max",
        "random",
        "unit",
        "unitless",
        "comparable",
        "rgb",
        "rgba",
        "hsl",
        "hsla",
        "red",
        "green",
        "blue",
        "hue",
        "saturation",
        "lightness",
        "alpha",
        "opacity",
        "mix",
        "lighten",
        "darken",
        "saturate",
        "desaturate",
        "adjust-hue",
        "opacify",
        "fade-in",
        "transparentize",
        "fade-out",
        "grayscale",
        "invert",
        "complement",
        "adjust-color",
        "scale-color",
        "change-color",
        "ie-hex-str",
        "quote",
        "unquote",
        "unique-id",
        "str-length",
        "str-index",
        "str-insert",
        "str-slice",
        "to-upper-case",
        "to-lower-case",
        "length",
        "nth",
        "set-nth",
        "join",
        "append",
        "zip",
        "index",
        "is-bracketed",
        "list-separator",
        "map-get",
        "map-merge",
        "map-remove",
        "map-keys",
        "map-values",
        "map-has-key",
        "type-of",
        "inspect",
        "feature-exists",
    ]
}

/// Documentation entry for one builtin.
pub struct BuiltinDoc {
    pub module: &'static str,
    pub name: &'static str,
    pub global: Option<&'static str>,
    pub signature: &'static str,
    pub summary: &'static str,
}

pub static BUILTINS: &[BuiltinDoc] = &[
    // math
    BuiltinDoc {
        module: "math",
        name: "div",
        global: None,
        signature: "($number1, $number2)",
        summary: "Divides two numbers.",
    },
    BuiltinDoc {
        module: "math",
        name: "percentage",
        global: Some("percentage"),
        signature: "($number)",
        summary: "Converts a unitless number to a percentage.",
    },
    BuiltinDoc {
        module: "math",
        name: "round",
        global: Some("round"),
        signature: "($number)",
        summary: "Rounds to the nearest whole number.",
    },
    BuiltinDoc {
        module: "math",
        name: "ceil",
        global: Some("ceil"),
        signature: "($number)",
        summary: "Rounds up to the next whole number.",
    },
    BuiltinDoc {
        module: "math",
        name: "floor",
        global: Some("floor"),
        signature: "($number)",
        summary: "Rounds down to the previous whole number.",
    },
    BuiltinDoc { module: "math", name: "abs", global: Some("abs"), signature: "($number)", summary: "Absolute value." },
    BuiltinDoc {
        module: "math",
        name: "min",
        global: Some("min"),
        signature: "($numbers...)",
        summary: "Smallest of the given numbers.",
    },
    BuiltinDoc {
        module: "math",
        name: "max",
        global: Some("max"),
        signature: "($numbers...)",
        summary: "Largest of the given numbers.",
    },
    BuiltinDoc {
        module: "math",
        name: "clamp",
        global: None,
        signature: "($min, $value, $max)",
        summary: "Clamps a number between a min and max.",
    },
    BuiltinDoc { module: "math", name: "sqrt", global: None, signature: "($number)", summary: "Square root." },
    BuiltinDoc {
        module: "math",
        name: "pow",
        global: None,
        signature: "($base, $exponent)",
        summary: "Raises a number to a power.",
    },
    BuiltinDoc {
        module: "math",
        name: "log",
        global: None,
        signature: "($number, $base: null)",
        summary: "Logarithm.",
    },
    BuiltinDoc { module: "math", name: "sin", global: None, signature: "($number)", summary: "Sine." },
    BuiltinDoc { module: "math", name: "cos", global: None, signature: "($number)", summary: "Cosine." },
    BuiltinDoc { module: "math", name: "tan", global: None, signature: "($number)", summary: "Tangent." },
    BuiltinDoc {
        module: "math",
        name: "asin",
        global: None,
        signature: "($number)",
        summary: "Arcsine, returns degrees.",
    },
    BuiltinDoc {
        module: "math",
        name: "acos",
        global: None,
        signature: "($number)",
        summary: "Arccosine, returns degrees.",
    },
    BuiltinDoc {
        module: "math",
        name: "atan",
        global: None,
        signature: "($number)",
        summary: "Arctangent, returns degrees.",
    },
    BuiltinDoc {
        module: "math",
        name: "atan2",
        global: None,
        signature: "($y, $x)",
        summary: "Two-argument arctangent, returns degrees.",
    },
    BuiltinDoc {
        module: "math",
        name: "hypot",
        global: None,
        signature: "($numbers...)",
        summary: "Length of the n-dimensional hypotenuse.",
    },
    BuiltinDoc {
        module: "math",
        name: "random",
        global: Some("random"),
        signature: "($limit: null)",
        summary: "Random number.",
    },
    BuiltinDoc {
        module: "math",
        name: "unit",
        global: Some("unit"),
        signature: "($number)",
        summary: "Returns a number's unit(s) as a string.",
    },
    BuiltinDoc {
        module: "math",
        name: "unitless",
        global: Some("unitless"),
        signature: "($number)",
        summary: "Whether a number has no units.",
    },
    BuiltinDoc {
        module: "math",
        name: "compatible",
        global: Some("comparable"),
        signature: "($number1, $number2)",
        summary: "Whether two numbers can be added/compared.",
    },
    BuiltinDoc {
        module: "math",
        name: "is-unitless",
        global: None,
        signature: "($number)",
        summary: "Whether a number has no units.",
    },
    // color
    BuiltinDoc {
        module: "color",
        name: "rgb",
        global: Some("rgb"),
        signature: "($red, $green, $blue, $alpha: 1)",
        summary: "Creates a color from RGB(A) channels.",
    },
    BuiltinDoc {
        module: "color",
        name: "hsl",
        global: Some("hsl"),
        signature: "($hue, $saturation, $lightness, $alpha: 1)",
        summary: "Creates a color from HSL(A) channels.",
    },
    BuiltinDoc {
        module: "color",
        name: "red",
        global: Some("red"),
        signature: "($color)",
        summary: "Red channel (0-255).",
    },
    BuiltinDoc {
        module: "color",
        name: "green",
        global: Some("green"),
        signature: "($color)",
        summary: "Green channel (0-255).",
    },
    BuiltinDoc {
        module: "color",
        name: "blue",
        global: Some("blue"),
        signature: "($color)",
        summary: "Blue channel (0-255).",
    },
    BuiltinDoc { module: "color", name: "hue", global: Some("hue"), signature: "($color)", summary: "Hue in degrees." },
    BuiltinDoc {
        module: "color",
        name: "saturation",
        global: Some("saturation"),
        signature: "($color)",
        summary: "HSL saturation as a percentage.",
    },
    BuiltinDoc {
        module: "color",
        name: "lightness",
        global: Some("lightness"),
        signature: "($color)",
        summary: "HSL lightness as a percentage.",
    },
    BuiltinDoc {
        module: "color",
        name: "whiteness",
        global: None,
        signature: "($color)",
        summary: "HWB whiteness as a percentage.",
    },
    BuiltinDoc {
        module: "color",
        name: "blackness",
        global: None,
        signature: "($color)",
        summary: "HWB blackness as a percentage.",
    },
    BuiltinDoc {
        module: "color",
        name: "alpha",
        global: Some("alpha"),
        signature: "($color)",
        summary: "Alpha channel (0-1).",
    },
    BuiltinDoc {
        module: "color",
        name: "mix",
        global: Some("mix"),
        signature: "($color1, $color2, $weight: 50%)",
        summary: "Mixes two colors.",
    },
    BuiltinDoc {
        module: "color",
        name: "lighten",
        global: Some("lighten"),
        signature: "($color, $amount)",
        summary: "Increases HSL lightness.",
    },
    BuiltinDoc {
        module: "color",
        name: "darken",
        global: Some("darken"),
        signature: "($color, $amount)",
        summary: "Decreases HSL lightness.",
    },
    BuiltinDoc {
        module: "color",
        name: "saturate",
        global: Some("saturate"),
        signature: "($color, $amount)",
        summary: "Increases HSL saturation.",
    },
    BuiltinDoc {
        module: "color",
        name: "desaturate",
        global: Some("desaturate"),
        signature: "($color, $amount)",
        summary: "Decreases HSL saturation.",
    },
    BuiltinDoc {
        module: "color",
        name: "adjust-hue",
        global: Some("adjust-hue"),
        signature: "($color, $degrees)",
        summary: "Rotates the hue.",
    },
    BuiltinDoc {
        module: "color",
        name: "grayscale",
        global: Some("grayscale"),
        signature: "($color)",
        summary: "Removes all saturation.",
    },
    BuiltinDoc {
        module: "color",
        name: "complement",
        global: Some("complement"),
        signature: "($color)",
        summary: "Rotates the hue by 180deg.",
    },
    BuiltinDoc {
        module: "color",
        name: "invert",
        global: Some("invert"),
        signature: "($color, $weight: 100%)",
        summary: "Inverts a color.",
    },
    BuiltinDoc {
        module: "color",
        name: "opacify",
        global: Some("opacify"),
        signature: "($color, $amount)",
        summary: "Increases the alpha channel.",
    },
    BuiltinDoc {
        module: "color",
        name: "transparentize",
        global: Some("transparentize"),
        signature: "($color, $amount)",
        summary: "Decreases the alpha channel.",
    },
    BuiltinDoc {
        module: "color",
        name: "adjust",
        global: Some("adjust-color"),
        signature: "($color, $red.., $green.., $blue.., $hue.., $saturation.., $lightness.., $alpha..)",
        summary: "Adjusts color channels by fixed amounts.",
    },
    BuiltinDoc {
        module: "color",
        name: "scale",
        global: Some("scale-color"),
        signature: "($color, $red.., $green.., $blue.., $saturation.., $lightness.., $alpha..)",
        summary: "Scales color channels fluidly.",
    },
    BuiltinDoc {
        module: "color",
        name: "change",
        global: Some("change-color"),
        signature: "($color, $red.., $green.., $blue.., $hue.., $saturation.., $lightness.., $alpha..)",
        summary: "Sets color channels to new values.",
    },
    BuiltinDoc {
        module: "color",
        name: "ie-hex-str",
        global: Some("ie-hex-str"),
        signature: "($color)",
        summary: "Returns an IE-compatible #AARRGGBB string.",
    },
    // string
    BuiltinDoc {
        module: "string",
        name: "quote",
        global: Some("quote"),
        signature: "($string)",
        summary: "Adds quotes to a string.",
    },
    BuiltinDoc {
        module: "string",
        name: "unquote",
        global: Some("unquote"),
        signature: "($string)",
        summary: "Removes quotes from a string.",
    },
    BuiltinDoc {
        module: "string",
        name: "length",
        global: Some("str-length"),
        signature: "($string)",
        summary: "Number of characters.",
    },
    BuiltinDoc {
        module: "string",
        name: "index",
        global: Some("str-index"),
        signature: "($string, $substring)",
        summary: "1-based index of a substring, or null.",
    },
    BuiltinDoc {
        module: "string",
        name: "insert",
        global: Some("str-insert"),
        signature: "($string, $insert, $index)",
        summary: "Inserts a string at an index.",
    },
    BuiltinDoc {
        module: "string",
        name: "slice",
        global: Some("str-slice"),
        signature: "($string, $start-at, $end-at: -1)",
        summary: "Extracts a substring.",
    },
    BuiltinDoc {
        module: "string",
        name: "to-upper-case",
        global: Some("to-upper-case"),
        signature: "($string)",
        summary: "Converts to upper case.",
    },
    BuiltinDoc {
        module: "string",
        name: "to-lower-case",
        global: Some("to-lower-case"),
        signature: "($string)",
        summary: "Converts to lower case.",
    },
    BuiltinDoc {
        module: "string",
        name: "unique-id",
        global: Some("unique-id"),
        signature: "()",
        summary: "Generates an unused identifier.",
    },
    BuiltinDoc {
        module: "string",
        name: "split",
        global: None,
        signature: "($string, $separator, $limit: null)",
        summary: "Splits a string into a list.",
    },
    // list
    BuiltinDoc {
        module: "list",
        name: "length",
        global: Some("length"),
        signature: "($list)",
        summary: "Number of elements.",
    },
    BuiltinDoc {
        module: "list",
        name: "nth",
        global: Some("nth"),
        signature: "($list, $n)",
        summary: "Element at a (possibly negative) index.",
    },
    BuiltinDoc {
        module: "list",
        name: "set-nth",
        global: Some("set-nth"),
        signature: "($list, $n, $value)",
        summary: "Replaces the element at an index.",
    },
    BuiltinDoc {
        module: "list",
        name: "join",
        global: Some("join"),
        signature: "($list1, $list2, $separator: auto, $bracketed: auto)",
        summary: "Concatenates two lists.",
    },
    BuiltinDoc {
        module: "list",
        name: "append",
        global: Some("append"),
        signature: "($list, $val, $separator: auto)",
        summary: "Appends a value to a list.",
    },
    BuiltinDoc {
        module: "list",
        name: "zip",
        global: Some("zip"),
        signature: "($lists...)",
        summary: "Combines lists element-wise.",
    },
    BuiltinDoc {
        module: "list",
        name: "index",
        global: Some("index"),
        signature: "($list, $value)",
        summary: "1-based index of a value, or null.",
    },
    BuiltinDoc {
        module: "list",
        name: "is-bracketed",
        global: Some("is-bracketed"),
        signature: "($list)",
        summary: "Whether a list has square brackets.",
    },
    BuiltinDoc {
        module: "list",
        name: "separator",
        global: Some("list-separator"),
        signature: "($list)",
        summary: "The list's separator (space/comma/slash).",
    },
    BuiltinDoc {
        module: "list",
        name: "slash",
        global: None,
        signature: "($elements...)",
        summary: "Builds a slash-separated list.",
    },
    // map
    BuiltinDoc {
        module: "map",
        name: "get",
        global: Some("map-get"),
        signature: "($map, $key, $keys...)",
        summary: "Looks up a (possibly nested) key.",
    },
    BuiltinDoc {
        module: "map",
        name: "merge",
        global: Some("map-merge"),
        signature: "($map, $keys..., $map2)",
        summary: "Merges two maps (shallow, at an optional nested path).",
    },
    BuiltinDoc {
        module: "map",
        name: "remove",
        global: Some("map-remove"),
        signature: "($map, $keys...)",
        summary: "Removes keys from a map.",
    },
    BuiltinDoc { module: "map", name: "keys", global: Some("map-keys"), signature: "($map)", summary: "List of keys." },
    BuiltinDoc {
        module: "map",
        name: "values",
        global: Some("map-values"),
        signature: "($map)",
        summary: "List of values.",
    },
    BuiltinDoc {
        module: "map",
        name: "has-key",
        global: Some("map-has-key"),
        signature: "($map, $key, $keys...)",
        summary: "Whether a (possibly nested) key exists.",
    },
    BuiltinDoc {
        module: "map",
        name: "deep-merge",
        global: None,
        signature: "($map1, $map2)",
        summary: "Recursively merges two maps.",
    },
    BuiltinDoc {
        module: "map",
        name: "deep-remove",
        global: None,
        signature: "($map, $keys...)",
        summary: "Recursively removes a nested key.",
    },
    BuiltinDoc {
        module: "map",
        name: "set",
        global: None,
        signature: "($map, $keys..., $value)",
        summary: "Sets a (possibly nested) key to a value.",
    },
    // meta
    BuiltinDoc {
        module: "meta",
        name: "type-of",
        global: Some("type-of"),
        signature: "($value)",
        summary: "The type of a value.",
    },
    BuiltinDoc {
        module: "meta",
        name: "inspect",
        global: Some("inspect"),
        signature: "($value)",
        summary: "A debug string representation of a value.",
    },
    BuiltinDoc {
        module: "meta",
        name: "feature-exists",
        global: Some("feature-exists"),
        signature: "($feature)",
        summary: "Whether a Sass feature is supported.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(vals: Vec<Value>) -> Args {
        Args::new(vals, Vec::new())
    }

    #[allow(dead_code)]
    fn named(vals: Vec<(&str, Value)>) -> Args {
        Args::new(Vec::new(), vals.into_iter().map(|(n, v)| (n.to_string(), v)).collect())
    }

    fn css(v: Result<Value, String>) -> String {
        v.expect("ok").to_css().expect("css")
    }

    #[test]
    fn math_basic() {
        assert_eq!(
            css(call(Some("math"), "div", pos(vec![Value::num_unit(10.0, "px"), Value::num(2.0)])).unwrap()),
            "5px"
        );
        assert_eq!(css(call(None, "percentage", pos(vec![Value::num(0.25)])).unwrap()), "25%");
        assert_eq!(css(call(None, "round", pos(vec![Value::num(4.5)])).unwrap()), "5");
        assert_eq!(css(call(None, "ceil", pos(vec![Value::num(4.1)])).unwrap()), "5");
        assert_eq!(css(call(None, "floor", pos(vec![Value::num(4.9)])).unwrap()), "4");
        assert_eq!(css(call(None, "abs", pos(vec![Value::num(-3.0)])).unwrap()), "3");
        assert_eq!(
            css(call(None, "min", pos(vec![Value::num_unit(1.0, "px"), Value::num_unit(3.0, "px")])).unwrap()),
            "1px"
        );
        assert_eq!(
            css(call(None, "max", pos(vec![Value::num_unit(1.0, "px"), Value::num_unit(3.0, "px")])).unwrap()),
            "3px"
        );
    }

    #[test]
    fn math_min_passthrough() {
        let r = call(None, "min", pos(vec![Value::num_unit(50.0, "%"), Value::num_unit(10.0, "px")])).unwrap().unwrap();
        assert!(matches!(r, Value::Call { .. }));
        assert_eq!(r.to_css().unwrap(), "min(50%, 10px)");
    }

    #[test]
    fn math_trig() {
        let r = call(Some("math"), "sin", pos(vec![Value::num_unit(0.0, "deg")])).unwrap().unwrap();
        assert_eq!(r.to_css().unwrap(), "0");
        let r = call(Some("math"), "sqrt", pos(vec![Value::num(16.0)])).unwrap().unwrap();
        assert_eq!(r.to_css().unwrap(), "4");
    }

    #[test]
    fn math_clamp() {
        assert_eq!(
            css(call(Some("math"), "clamp", pos(vec![Value::num(0.0), Value::num(5.0), Value::num(10.0)])).unwrap()),
            "5"
        );
        assert_eq!(
            css(call(Some("math"), "clamp", pos(vec![Value::num(0.0), Value::num(-5.0), Value::num(10.0)])).unwrap()),
            "0"
        );
    }

    fn hex(s: &str) -> Value {
        Value::Color(Color::from_hex(s).unwrap())
    }

    #[test]
    fn color_lighten_darken() {
        assert_eq!(
            css(call(None, "lighten", pos(vec![hex("800000"), Value::num_unit(20.0, "%")])).unwrap()),
            "#e60000"
        );
        assert_eq!(css(call(None, "darken", pos(vec![hex("b37399"), Value::num_unit(20.0, "%")])).unwrap()), "#7c4465");
    }

    #[test]
    fn color_mix() {
        assert_eq!(css(call(None, "mix", pos(vec![hex("036"), hex("d2e1dd")])).unwrap()), "#698aa2");
    }

    #[test]
    fn color_adjust_hue() {
        assert_eq!(
            css(call(None, "adjust-hue", pos(vec![hex("6b717f"), Value::num_unit(60.0, "deg")])).unwrap()),
            "#796b7f"
        );
    }

    #[test]
    fn color_rgba_alpha() {
        let r = call(None, "rgba", pos(vec![hex("ff0000"), Value::num(0.5)])).unwrap().unwrap();
        if let Value::Color(c) = r {
            assert!(fuzzy_eq(c.a, 0.5));
        } else {
            panic!("expected color");
        }
    }

    #[test]
    fn color_rgb_passthrough() {
        let r = call(
            None,
            "rgb",
            pos(vec![
                Value::Call { name: "var".into(), args: vec![Value::str("--r")] },
                Value::num(0.0),
                Value::num(0.0),
            ]),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(r, Value::Call { .. }));
    }

    #[test]
    fn color_channels() {
        assert_eq!(css(call(None, "red", pos(vec![hex("ff8000")])).unwrap()), "255");
        assert_eq!(css(call(None, "green", pos(vec![hex("ff8000")])).unwrap()), "128");
    }

    #[test]
    fn string_fns() {
        assert_eq!(
            css(call(None, "str-slice", pos(vec![Value::quoted("abcd"), Value::num(2.0), Value::num(3.0)])).unwrap()),
            "\"bc\""
        );
        assert_eq!(css(call(Some("string"), "to-upper-case", pos(vec![Value::quoted("abc")])).unwrap()), "\"ABC\"");
        assert_eq!(css(call(None, "str-length", pos(vec![Value::quoted("hello")])).unwrap()), "5");
        assert!(matches!(
            call(Some("string"), "index", pos(vec![Value::quoted("hello"), Value::quoted("xyz")])).unwrap().unwrap(),
            Value::Null
        ));
    }

    #[test]
    fn list_fns() {
        let l = Value::list(vec![Value::str("a"), Value::str("b"), Value::str("c")], ListSep::Space);
        assert_eq!(css(call(None, "nth", pos(vec![l.clone(), Value::num(-1.0)])).unwrap()), "c");
        assert_eq!(css(call(None, "length", pos(vec![l.clone()])).unwrap()), "3");
        let joined = call(
            Some("list"),
            "join",
            pos(vec![
                Value::list(vec![Value::num(1.0)], ListSep::Space),
                Value::list(vec![Value::num(2.0)], ListSep::Space),
            ]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(joined.to_css().unwrap(), "1 2");
    }

    #[test]
    fn map_fns() {
        let m = Value::Map(vec![(Value::str("a"), Value::num(1.0)), (Value::str("b"), Value::num(2.0))]);
        assert_eq!(css(call(None, "map-get", pos(vec![m.clone(), Value::str("b")])).unwrap()), "2");
        assert!(matches!(call(None, "map-get", pos(vec![m.clone(), Value::str("z")])).unwrap().unwrap(), Value::Null));
        assert_eq!(css(call(Some("map"), "has-key", pos(vec![m.clone(), Value::str("a")])).unwrap()), "true");
    }

    #[test]
    fn meta_fns() {
        assert_eq!(css(call(None, "type-of", pos(vec![Value::num_unit(12.0, "px")])).unwrap()), "number");
        assert_eq!(css(call(Some("meta"), "type-of", pos(vec![Value::num_unit(12.0, "px")])).unwrap()), "number");
    }

    #[test]
    fn exists_table_consistency() {
        for doc in BUILTINS {
            let module_ok = exists(Some(doc.module), doc.name);
            let global_ok = doc.global.is_some_and(|g| exists(None, g));
            assert!(module_ok || global_ok, "neither {}.{} nor its global alias (if any) exist", doc.module, doc.name);
            if doc.global.is_none() {
                assert!(module_ok, "module fn {}.{} should exist since it has no global alias", doc.module, doc.name);
            }
        }
        assert!(!exists(None, "not-a-real-function"));
        assert!(!exists(Some("math"), "not-a-real-function"));
        assert!(!exists(Some("color"), "div"));
        assert!(!exists(None, "div"));
    }

    #[test]
    fn underscore_dash_equivalence() {
        assert!(exists(None, "map_get"));
        assert!(exists(Some("map"), "has_key"));
    }
}
