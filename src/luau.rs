//! The compiled stylesheet as data, and the only code that writes Luau.
//!
//! Everything outlass compiles ends up as a [`Sheet`]: rules with selectors and typed property
//! values. `--emit json` prints a `Sheet` as JSON ([`to_json`]); [`emit`] turns the same `Sheet`
//! into the Luau module. No JSON can become code: text from a stylesheet (names, selectors,
//! property keys, strings) only ever reaches the Luau inside an escaped string literal, Enum names
//! come from `enums.txt` ([`EnumItem`]), and every value is one of a fixed set of constructors.
//! The one exception is [`Value::Luau`], raw Luau from `luau("...")` that only `--allow-raw-luau`
//! lets into a `Sheet`, and that JSON has no way to hold: [`to_json`] refuses a sheet with any.
//!
//! This module depends on nothing else in the crate, so it can be audited on its own.

use std::fmt::Write as _;
use std::sync::OnceLock;

/// An RGB color, channels 0-255 (may be fractional).
#[derive(Clone, Debug, PartialEq)]
pub struct Color3 {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UDim {
    pub scale: f64,
    pub offset: f64,
}

/// An Enum item from `enums.txt`, the items GUI objects use. It can only be made by looking an
/// item up in that table, so its names come from the binary, never from a stylesheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnumItem {
    enum_type: &'static str,
    item: &'static str,
    value: u32,
}

impl EnumItem {
    /// The item, if `enums.txt` has it.
    pub fn get(enum_type: &str, item: &str) -> Option<EnumItem> {
        enum_table().iter().copied().find(|e| e.enum_type == enum_type && e.item == item)
    }

    /// An item the compiler itself names; every such name is in `enums.txt`.
    pub fn of(enum_type: &str, item: &str) -> EnumItem {
        EnumItem::get(enum_type, item).unwrap_or_else(|| panic!("Enum.{enum_type}.{item} isn't in enums.txt"))
    }

    pub fn enum_type(self) -> &'static str {
        self.enum_type
    }

    pub fn item(self) -> &'static str {
        self.item
    }

    /// The number Roblox saves the item as.
    pub fn value(self) -> u32 {
        self.value
    }
}

fn enum_table() -> &'static [EnumItem] {
    static TABLE: OnceLock<Vec<EnumItem>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut items = Vec::new();
        for line in include_str!("enums.txt").lines().filter(|l| !l.starts_with('#')) {
            let mut words = line.split(' ');
            let Some(enum_type) = words.next() else { continue };
            for word in words {
                if let Some((item, value)) = word.split_once('=')
                    && let Ok(value) = value.parse()
                {
                    items.push(EnumItem { enum_type, item, value });
                }
            }
        }
        items
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct TweenInfo {
    pub time: f64,
    pub easing_style: EnumItem,
    pub easing_direction: EnumItem,
    pub repeat_count: f64,
    pub reverses: bool,
    pub delay_time: f64,
}

impl TweenInfo {
    pub fn new(time: f64, easing_style: &str, easing_direction: &str, delay_time: f64) -> Self {
        TweenInfo {
            time,
            easing_style: EnumItem::of("EasingStyle", easing_style),
            easing_direction: EnumItem::of("EasingDirection", easing_direction),
            repeat_count: 0.0,
            reverses: false,
            delay_time,
        }
    }
}

/// A Roblox property or attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Number(f64),
    String(String),
    /// A design token reference: the string `"$Name"`.
    Token(String),
    Enum(EnumItem),
    Color3(Color3),
    UDim(UDim),
    UDim2(UDim, UDim),
    Vector2(f64, f64),
    /// min x, min y, max x, max y
    Rect(f64, f64, f64, f64),
    NumberRange(f64, f64),
    /// (time, color) keypoints
    ColorSequence(Vec<(f64, Color3)>),
    /// (time, value, envelope) keypoints
    NumberSequence(Vec<(f64, f64, f64)>),
    /// `Font.new(family, weight, style)`; weight and style are FontWeight / FontStyle items.
    Font {
        family: String,
        weight: Option<EnumItem>,
        style: Option<EnumItem>,
    },
    /// `Font.fromEnum(Enum.Font.<item>)`
    FontEnum(EnumItem),
    /// Raw Luau from `luau("...")`, emitted verbatim. Only `--allow-raw-luau` makes one, and no
    /// JSON can hold one.
    Luau(String),
}

impl Value {
    pub fn enum_item(enum_type: &str, item: &str) -> Value {
        Value::Enum(EnumItem::of(enum_type, item))
    }

    pub fn udim(scale: f64, offset: f64) -> Value {
        Value::UDim(UDim { scale, offset })
    }

    pub fn udim2(xs: f64, xo: f64, ys: f64, yo: f64) -> Value {
        Value::UDim2(UDim { scale: xs, offset: xo }, UDim { scale: ys, offset: yo })
    }
}

/// A StyleRule. Its children are nested StyleRules.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub selector: String,
    pub priority: Option<f64>,
    pub props: Vec<(String, Value)>,
    pub attributes: Vec<(String, Value)>,
    /// Property name (or `*`, the default transition) -> transition.
    pub transitions: Vec<(String, TweenInfo)>,
    pub children: Vec<Rule>,
}

impl Rule {
    pub fn is_empty(&self) -> bool {
        self.props.is_empty() && self.attributes.is_empty() && self.transitions.is_empty() && self.children.is_empty()
    }
}

/// A StyleSheet: its attributes (design tokens), rules and themes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub attributes: Vec<(String, Value)>,
    pub rules: Vec<Rule>,
    /// Theme StyleSheets holding the themed tokens; the first is the one in use. The sheet
    /// derives from it through a StyleDerive named `Theme`, so switching theme is
    /// `sheet.Theme.StyleSheet = sheet.Themes.dark`.
    pub themes: Vec<Theme>,
    /// The user-agent stylesheet: defaults that make fresh Roblox elements start out like CSS
    /// boxes. It's a StyleSheet of its own the sheet derives from, through a StyleDerive named
    /// `UserAgent`, so every rule in the sheet beats it whatever their priorities (measured in
    /// Studio), and deleting `sheet.UserAgent` drops it.
    pub user_agent: Vec<Rule>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Theme {
    pub name: String,
    pub attributes: Vec<(String, Value)>,
}

// ----- Luau -----

/// Writes the Luau module that builds `sheet` and returns it.
pub fn emit(sheet: &Sheet) -> String {
    let mut out = String::new();
    // Fixed text only: nothing from the input goes into a comment.
    let _ = writeln!(out, "-- Generated by outlass {}. Do not edit by hand.\n", env!("CARGO_PKG_VERSION"));
    out.push_str("local sheet = Instance.new(\"StyleSheet\")\n");
    let _ = writeln!(out, "sheet.Name = {}", string(&sheet.name));
    for (name, value) in &sheet.attributes {
        let _ = writeln!(out, "sheet:SetAttribute({}, {})", string(name), expression(value));
    }
    if !sheet.themes.is_empty() {
        out.push_str(
            "\nlocal themes = Instance.new(\"Folder\")\n\
             themes.Name = \"Themes\"\n\
             themes.Parent = sheet\n\
             local theme = Instance.new(\"StyleDerive\")\n\
             theme.Name = \"Theme\"\n",
        );
        for (i, t) in sheet.themes.iter().enumerate() {
            out.push_str("do\n\tlocal t = Instance.new(\"StyleSheet\")\n");
            let _ = writeln!(out, "\tt.Name = {}", string(&t.name));
            for (name, value) in &t.attributes {
                let _ = writeln!(out, "\tt:SetAttribute({}, {})", string(name), expression(value));
            }
            out.push_str("\tt.Parent = themes\n");
            if i == 0 {
                out.push_str("\ttheme.StyleSheet = t\n");
            }
            out.push_str("end\n");
        }
        out.push_str("theme.Parent = sheet\n");
    }
    if !sheet.rules.is_empty() || !sheet.user_agent.is_empty() {
        out.push_str(
            "\nlocal function rule(parent: Instance, selector: string, priority: number?, properties: { [string]: any }?): StyleRule\n\
             \tlocal r = Instance.new(\"StyleRule\")\n\
             \tr.Name = selector\n\
             \tr.Selector = selector\n\
             \tif priority then\n\
             \t\tr.Priority = priority\n\
             \tend\n\
             \tif properties then\n\
             \t\tr:SetProperties(properties)\n\
             \tend\n\
             \tr.Parent = parent\n\
             \treturn r\n\
             end\n",
        );
    }
    for rule in &sheet.rules {
        out.push('\n');
        emit_rule(&mut out, rule, "sheet", 0);
    }
    if !sheet.user_agent.is_empty() {
        out.push_str(
            "\n-- The user-agent stylesheet: defaults under every rule above, like a browser's. Delete\n\
             -- sheet.UserAgent to go without it.\n\
             local userAgent = Instance.new(\"StyleSheet\")\n\
             userAgent.Name = \"UserAgent\"\n",
        );
        for rule in &sheet.user_agent {
            out.push('\n');
            emit_rule(&mut out, rule, "userAgent", 0);
        }
        out.push_str(
            "\nlocal derive = Instance.new(\"StyleDerive\")\n\
             derive.Name = \"UserAgent\"\n\
             derive.StyleSheet = userAgent\n\
             userAgent.Parent = derive\n\
             derive.Parent = sheet\n",
        );
    }
    out.push_str("\nreturn sheet\n");
    out
}

fn emit_rule(out: &mut String, rule: &Rule, parent: &str, depth: usize) {
    let indent = "\t".repeat(depth);
    let needs_handle = !rule.attributes.is_empty() || !rule.transitions.is_empty() || !rule.children.is_empty();
    // The property table sits one level deeper inside a `do` block.
    let body = if needs_handle { "\t".repeat(depth + 1) } else { indent.clone() };
    let priority = rule.priority.map(number).unwrap_or_else(|| "nil".into());
    let props = if rule.props.is_empty() {
        "nil".to_string()
    } else {
        let mut s = String::from("{\n");
        for (k, v) in &rule.props {
            let _ = writeln!(s, "{body}\t{} = {},", table_key(k), expression(v));
        }
        s.push_str(&body);
        s.push('}');
        s
    };
    let call = format!("rule({parent}, {}, {priority}, {props})", string(&rule.selector));
    if !needs_handle {
        let _ = writeln!(out, "{indent}{call}");
        return;
    }
    let var = format!("r{}", depth + 1);
    let _ = writeln!(out, "{indent}do");
    let _ = writeln!(out, "{body}local {var} = {call}");
    for (name, value) in &rule.attributes {
        let _ = writeln!(out, "{body}{var}:SetAttribute({}, {})", string(name), expression(value));
    }
    let (defaults, specific): (Vec<_>, Vec<_>) = rule.transitions.iter().partition(|(k, _)| k == "*");
    if !specific.is_empty() {
        let _ = writeln!(out, "{body}{var}:SetPropertyTransitions({{");
        for (k, t) in specific {
            let _ = writeln!(out, "{body}\t{} = {},", table_key(k), tween_info(t));
        }
        let _ = writeln!(out, "{body}}})");
    }
    if let Some((_, t)) = defaults.last() {
        let _ = writeln!(out, "{body}{var}:SetDefaultPropertyTransition({})", tween_info(t));
    }
    for child in &rule.children {
        emit_rule(out, child, &var, depth + 1);
    }
    let _ = writeln!(out, "{indent}end");
}

/// A value as a Luau expression.
pub fn expression(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(*n),
        Value::String(s) => string(s),
        Value::Token(name) => string(&format!("${name}")),
        Value::Enum(e) => enum_item(*e),
        Value::Color3(c) => color3(c),
        Value::UDim(u) => format!("UDim.new({}, {})", number(u.scale), number(u.offset)),
        Value::UDim2(x, y) => {
            format!("UDim2.new({}, {}, {}, {})", number(x.scale), number(x.offset), number(y.scale), number(y.offset))
        }
        Value::Vector2(x, y) => format!("Vector2.new({}, {})", number(*x), number(*y)),
        Value::Rect(a, b, c, d) => format!("Rect.new({}, {}, {}, {})", number(*a), number(*b), number(*c), number(*d)),
        Value::NumberRange(min, max) => format!("NumberRange.new({}, {})", number(*min), number(*max)),
        Value::ColorSequence(keypoints) => {
            let keypoints: Vec<String> = keypoints
                .iter()
                .map(|(t, c)| format!("ColorSequenceKeypoint.new({}, {})", number(*t), color3(c)))
                .collect();
            format!("ColorSequence.new({{{}}})", keypoints.join(", "))
        }
        Value::NumberSequence(keypoints) => {
            let keypoints: Vec<String> = keypoints
                .iter()
                .map(|(t, v, e)| {
                    if *e == 0.0 {
                        format!("NumberSequenceKeypoint.new({}, {})", number(*t), number(*v))
                    } else {
                        format!("NumberSequenceKeypoint.new({}, {}, {})", number(*t), number(*v), number(*e))
                    }
                })
                .collect();
            format!("NumberSequence.new({{{}}})", keypoints.join(", "))
        }
        Value::Font { family, weight, style } => {
            let mut args = vec![string(family)];
            if weight.is_some() || style.is_some() {
                args.push(enum_item(weight.unwrap_or(EnumItem::of("FontWeight", "Regular"))));
            }
            if let Some(style) = style {
                args.push(enum_item(*style));
            }
            format!("Font.new({})", args.join(", "))
        }
        Value::FontEnum(item) => format!("Font.fromEnum({})", enum_item(*item)),
        Value::Luau(code) => code.clone(),
    }
}

fn tween_info(t: &TweenInfo) -> String {
    let mut args = vec![number(t.time), enum_item(t.easing_style), enum_item(t.easing_direction)];
    if t.repeat_count != 0.0 || t.reverses || t.delay_time != 0.0 {
        args.push(number(t.repeat_count));
        args.push(t.reverses.to_string());
    }
    if t.delay_time != 0.0 {
        args.push(number(t.delay_time));
    }
    format!("TweenInfo.new({})", args.join(", "))
}

/// `Enum.<type>.<item>`, with the names from `enums.txt`.
fn enum_item(e: EnumItem) -> String {
    format!("Enum.{}.{}", e.enum_type(), e.item())
}

fn color3(c: &Color3) -> String {
    let byte = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    format!("Color3.fromRGB({}, {}, {})", byte(c.r), byte(c.g), byte(c.b))
}

/// A number as a Luau literal: digits, `-`, `.` and `e` only (or `math.huge`, `0/0`).
pub fn number(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "math.huge".into() } else { "-math.huge".into() };
    }
    if v.is_nan() {
        return "0/0".into();
    }
    if (v - v.round()).abs() < 1e-10 {
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

/// Quotes text as a Luau string literal. A Luau string only ends at its closing quote or a line
/// break, so escaping `"`, `\` and every control character keeps the text inside it.
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
            c if c.is_control() => {
                let _ = write!(s, "\\u{{{:x}}}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// A table key, always a string literal: a property name is the stylesheet's text.
fn table_key(name: &str) -> String {
    format!("[{}]", string(name))
}

// ----- JSON -----

enum Json {
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

fn obj(fields: Vec<(&str, Json)>) -> Json {
    Json::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// A number field. JSON has no infinity or NaN, so those are the strings `"inf"`, `"-inf"`, `"nan"`.
fn num(v: f64) -> Json {
    if v.is_finite() {
        Json::Number(v)
    } else if v.is_nan() {
        Json::String("nan".into())
    } else {
        Json::String(if v > 0.0 { "inf" } else { "-inf" }.into())
    }
}

fn str(s: &str) -> Json {
    Json::String(s.to_string())
}

fn typed(name: &str, mut fields: Vec<(&str, Json)>) -> Json {
    fields.insert(0, ("type", str(name)));
    obj(fields)
}

fn color_json(c: &Color3) -> Json {
    obj(vec![("r", num(c.r)), ("g", num(c.g)), ("b", num(c.b))])
}

fn udim_json(u: &UDim) -> Json {
    obj(vec![("scale", num(u.scale)), ("offset", num(u.offset))])
}

fn tween_json(t: &TweenInfo) -> Json {
    typed(
        "TweenInfo",
        vec![
            ("time", num(t.time)),
            ("easingStyle", str(t.easing_style.item())),
            ("easingDirection", str(t.easing_direction.item())),
            ("repeatCount", num(t.repeat_count)),
            ("reverses", Json::Bool(t.reverses)),
            ("delayTime", num(t.delay_time)),
        ],
    )
}

/// Booleans, finite numbers and strings are plain JSON; everything else is an object whose
/// `type` names the Roblox type.
fn value_json(v: &Value) -> Json {
    match v {
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) if n.is_finite() => Json::Number(*n),
        Value::Number(n) => typed("number", vec![("value", num(*n))]),
        Value::String(s) => str(s),
        Value::Token(name) => typed("token", vec![("name", str(name))]),
        Value::Enum(e) => typed("Enum", vec![("enum", str(e.enum_type())), ("item", str(e.item()))]),
        Value::Color3(c) => typed("Color3", vec![("r", num(c.r)), ("g", num(c.g)), ("b", num(c.b))]),
        Value::UDim(u) => typed("UDim", vec![("scale", num(u.scale)), ("offset", num(u.offset))]),
        Value::UDim2(x, y) => typed("UDim2", vec![("x", udim_json(x)), ("y", udim_json(y))]),
        Value::Vector2(x, y) => typed("Vector2", vec![("x", num(*x)), ("y", num(*y))]),
        Value::Rect(a, b, c, d) => typed(
            "Rect",
            vec![
                ("min", obj(vec![("x", num(*a)), ("y", num(*b))])),
                ("max", obj(vec![("x", num(*c)), ("y", num(*d))])),
            ],
        ),
        Value::NumberRange(min, max) => typed("NumberRange", vec![("min", num(*min)), ("max", num(*max))]),
        Value::ColorSequence(keypoints) => typed(
            "ColorSequence",
            vec![(
                "keypoints",
                Json::Array(
                    keypoints.iter().map(|(t, c)| obj(vec![("time", num(*t)), ("color", color_json(c))])).collect(),
                ),
            )],
        ),
        Value::NumberSequence(keypoints) => typed(
            "NumberSequence",
            vec![(
                "keypoints",
                Json::Array(
                    keypoints
                        .iter()
                        .map(|(t, v, e)| obj(vec![("time", num(*t)), ("value", num(*v)), ("envelope", num(*e))]))
                        .collect(),
                ),
            )],
        ),
        Value::Font { family, weight, style } => {
            let mut fields = vec![("family", str(family))];
            if let Some(w) = weight {
                fields.push(("weight", str(w.item())));
            }
            if let Some(s) = style {
                fields.push(("style", str(s.item())));
            }
            typed("Font", fields)
        }
        Value::FontEnum(item) => typed("Font", vec![("enum", str(item.item()))]),
        Value::Luau(_) => unreachable!("to_json refuses a sheet with raw Luau before writing any value"),
    }
}

fn map_json<T>(entries: &[(String, T)], f: impl Fn(&T) -> Json) -> Json {
    Json::Object(entries.iter().map(|(k, v)| (k.clone(), f(v))).collect())
}

fn rule_json(rule: &Rule) -> Json {
    let mut fields = vec![("selector", str(&rule.selector))];
    if let Some(p) = rule.priority {
        fields.push(("priority", num(p)));
    }
    if !rule.props.is_empty() {
        fields.push(("properties", map_json(&rule.props, value_json)));
    }
    if !rule.attributes.is_empty() {
        fields.push(("attributes", map_json(&rule.attributes, value_json)));
    }
    if !rule.transitions.is_empty() {
        fields.push(("transitions", map_json(&rule.transitions, tween_json)));
    }
    if !rule.children.is_empty() {
        fields.push(("children", Json::Array(rule.children.iter().map(rule_json).collect())));
    }
    obj(fields)
}

/// The sheet as a JSON document.
pub fn to_json(sheet: &Sheet) -> Result<String, String> {
    if uses_raw_luau(sheet) {
        return Err("JSON holds the stylesheet as data, so it can't hold raw Luau from luau()".into());
    }
    let doc = obj(vec![
        ("version", Json::Number(1.0)),
        ("name", str(&sheet.name)),
        ("attributes", map_json(&sheet.attributes, value_json)),
        ("rules", Json::Array(sheet.rules.iter().map(rule_json).collect())),
        ("userAgent", Json::Array(sheet.user_agent.iter().map(rule_json).collect())),
        (
            "themes",
            Json::Array(
                sheet
                    .themes
                    .iter()
                    .map(|t| obj(vec![("name", str(&t.name)), ("attributes", map_json(&t.attributes, value_json))]))
                    .collect(),
            ),
        ),
    ]);
    let mut out = String::new();
    write_json(&mut out, &doc, 0);
    out.push('\n');
    Ok(out)
}

/// Whether any value in `sheet` is raw Luau.
fn uses_raw_luau(sheet: &Sheet) -> bool {
    fn rule(r: &Rule) -> bool {
        r.props.iter().chain(&r.attributes).any(|(_, v)| matches!(v, Value::Luau(_))) || r.children.iter().any(rule)
    }
    let attributes = sheet.attributes.iter().chain(sheet.themes.iter().flat_map(|t| &t.attributes));
    attributes.into_iter().any(|(_, v)| matches!(v, Value::Luau(_)))
        || sheet.rules.iter().chain(&sheet.user_agent).any(rule)
}

/// Pretty-prints `v`, keeping any array or object that fits within 100 columns on one line.
fn write_json(out: &mut String, v: &Json, depth: usize) {
    let mut flat = String::new();
    write_flat(&mut flat, v);
    if flat.len() + depth * 2 <= 100 {
        out.push_str(&flat);
        return;
    }
    let pad = "  ".repeat(depth + 1);
    match v {
        Json::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad);
                write_json(out, item, depth + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            let _ = write!(out, "{}]", "  ".repeat(depth));
        }
        Json::Object(fields) => {
            out.push_str("{\n");
            for (i, (k, item)) in fields.iter().enumerate() {
                out.push_str(&pad);
                json_string(out, k);
                out.push_str(": ");
                write_json(out, item, depth + 1);
                out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
            }
            let _ = write!(out, "{}}}", "  ".repeat(depth));
        }
        _ => out.push_str(&flat),
    }
}

fn write_flat(out: &mut String, v: &Json) {
    match v {
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => out.push_str(&number(*n)),
        Json::String(s) => json_string(out, s),
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_flat(out, item);
            }
            out.push(']');
        }
        Json::Object(fields) => {
            out.push('{');
            for (i, (k, item)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                json_string(out, k);
                out.push_str(": ");
                write_flat(out, item);
            }
            out.push('}');
        }
    }
}

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A value as Luau with the default options, for tests.
#[cfg(test)]
pub fn render(v: &Value) -> String {
    expression(v)
}

#[cfg(test)]
pub fn render_tween(t: &TweenInfo) -> String {
    tween_info(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_cannot_break_out() {
        assert_eq!(string("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(string("\u{1}2"), r#""\u{1}2""#);
        assert_eq!(string("é\u{7f}"), r#""é\u{7f}""#);
    }

    #[test]
    fn enum_items_come_only_from_the_table() {
        assert_eq!(expression(&Value::enum_item("Font", "Gotham")), "Enum.Font.Gotham");
        assert!(EnumItem::get("Font", "x) require(1)").is_none());
        assert!(EnumItem::get("Workspace", "Gravity").is_none());
        assert_eq!(EnumItem::of("AutomaticSize", "XY").value(), 3);
    }

    #[test]
    fn user_text_only_reaches_the_luau_inside_string_literals() {
        // A property name, a selector, an attribute and a sheet name that would each be code if
        // written bare.
        let hostile = "x\"]) require(1) --";
        let sheet = Sheet {
            name: hostile.into(),
            attributes: vec![(hostile.into(), Value::String(hostile.into()))],
            rules: vec![Rule {
                selector: hostile.into(),
                props: vec![(hostile.into(), Value::Number(1.0))],
                transitions: vec![(hostile.into(), TweenInfo::new(1.0, "Quad", "Out", 0.0))],
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = emit(&sheet);
        assert!(!out.contains("x\"]"), "{out}");
        assert_eq!(out.matches(r#""x\"]) require(1) --""#).count(), 6, "{out}");
    }

    #[test]
    fn raw_luau_reaches_luau_but_never_json() {
        let sheet = Sheet {
            rules: vec![Rule {
                selector: ".a".into(),
                props: vec![("Text".into(), Value::Luau("game.Players.LocalPlayer.Name".into()))],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(emit(&sheet).contains(r#"["Text"] = game.Players.LocalPlayer.Name,"#));
        assert!(to_json(&sheet).is_err());
        let in_user_agent = Sheet { user_agent: sheet.rules.clone(), ..Default::default() };
        assert!(to_json(&in_user_agent).is_err());
    }

    #[test]
    fn the_header_holds_no_input_text() {
        let sheet = Sheet { name: "a\nrequire(1)".into(), ..Default::default() };
        let out = emit(&sheet);
        let header = format!("-- Generated by outlass {}. Do not edit by hand.\n\n", env!("CARGO_PKG_VERSION"));
        assert!(out.starts_with(&header), "{out}");
        assert!(out.contains(r#"sheet.Name = "a\nrequire(1)""#), "{out}");
    }

    #[test]
    fn json_marks_types_and_non_finite_numbers() {
        let sheet = Sheet {
            name: "s".into(),
            attributes: vec![],
            rules: vec![Rule {
                selector: ".a".into(),
                priority: Some(1.0),
                props: vec![
                    ("Size".into(), Value::udim2(0.5, 10.0, 0.0, 0.0)),
                    ("MaxSize".into(), Value::Vector2(f64::INFINITY, 4.0)),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let json = to_json(&sheet).unwrap();
        assert!(
            json.contains(
                r#""Size": {"type": "UDim2", "x": {"scale": 0.5, "offset": 10}, "y": {"scale": 0, "offset": 0}}"#
            ),
            "{json}"
        );
        assert!(json.contains(r#""MaxSize": {"type": "Vector2", "x": "inf", "y": 4}"#), "{json}");
    }
}
