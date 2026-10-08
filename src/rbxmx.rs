//! The compiled stylesheet as a Roblox XML model (`.rbxmx`): StyleSheet and StyleRule instances
//! whose properties are data, which Studio and Rojo load as they are. Nothing in it runs as code.
//!
//! A StyleRule's properties, its transitions and every instance's attributes are binary blobs
//! (`PropertiesSerialize`, `PropertyTransitionsSerialize`, `AttributesSerialize`) in Roblox's
//! attribute encoding, base64 in the XML. The layouts here were read off models Studio saved.

use std::fmt::Write as _;

use crate::luau::{Rule, Sheet, TweenInfo, Value};

/// The Enum items GUI objects use and the numbers they're saved as (see the file's header).
const ENUMS: &str = include_str!("enums.txt");

/// Writes the model. The second value lists what a model file can't hold, which was left out.
pub fn write(sheet: &Sheet) -> Result<(String, Vec<String>), String> {
    let mut w = Writer::default();
    w.out.push_str("<roblox version=\"4\">\n");
    w.open("StyleSheet", 1);
    w.string("Name", &sheet.name, 3)?;
    w.blob("AttributesSerialize", &attributes(&sheet.attributes)?, 3);
    w.close_properties(2);
    if !sheet.themes.is_empty() {
        w.open("Folder", 2);
        w.string("Name", "Themes", 4)?;
        w.close_properties(3);
        let mut first = None;
        for theme in &sheet.themes {
            let referent = w.open("StyleSheet", 3);
            first.get_or_insert(referent);
            w.string("Name", &theme.name, 5)?;
            w.blob("AttributesSerialize", &attributes(&theme.attributes)?, 5);
            w.close_properties(4);
            w.close_item(3);
        }
        w.close_item(2);
        w.open("StyleDerive", 2);
        w.string("Name", "Theme", 4)?;
        let _ = writeln!(w.out, "{}<Ref name=\"StyleSheet\">RBX{}</Ref>", "\t".repeat(4), first.unwrap_or_default());
        w.close_properties(3);
        w.close_item(2);
    }
    for rule in &sheet.rules {
        w.rule(rule, 2)?;
    }
    if !sheet.user_agent.is_empty() {
        // The user-agent StyleSheet lives inside the StyleDerive that points at it.
        w.open("StyleDerive", 2);
        w.string("Name", "UserAgent", 4)?;
        let _ = writeln!(w.out, "{}<Ref name=\"StyleSheet\">RBX{}</Ref>", "\t".repeat(4), w.next);
        w.close_properties(3);
        w.open("StyleSheet", 3);
        w.string("Name", "UserAgent", 5)?;
        w.close_properties(4);
        for rule in &sheet.user_agent {
            w.rule(rule, 4)?;
        }
        w.close_item(3);
        w.close_item(2);
    }
    w.close_item(1);
    w.out.push_str("</roblox>\n");
    Ok((w.out, w.dropped))
}

#[derive(Default)]
struct Writer {
    out: String,
    next: usize,
    dropped: Vec<String>,
}

impl Writer {
    /// Opens an item and its properties; returns its referent number.
    fn open(&mut self, class: &str, depth: usize) -> usize {
        let referent = self.next;
        self.next += 1;
        let pad = "\t".repeat(depth);
        let _ = writeln!(self.out, "{pad}<Item class=\"{class}\" referent=\"RBX{referent}\">\n{pad}\t<Properties>");
        referent
    }

    fn close_properties(&mut self, depth: usize) {
        let _ = writeln!(self.out, "{}</Properties>", "\t".repeat(depth));
    }

    fn close_item(&mut self, depth: usize) {
        let _ = writeln!(self.out, "{}</Item>", "\t".repeat(depth));
    }

    fn string(&mut self, name: &str, value: &str, depth: usize) -> Result<(), String> {
        let _ = writeln!(self.out, "{}<string name=\"{name}\">{}</string>", "\t".repeat(depth), xml(value)?);
        Ok(())
    }

    fn blob(&mut self, name: &str, bytes: &[u8], depth: usize) {
        let _ =
            writeln!(self.out, "{}<BinaryString name=\"{name}\">{}</BinaryString>", "\t".repeat(depth), base64(bytes));
    }

    fn rule(&mut self, rule: &Rule, depth: usize) -> Result<(), String> {
        let inner = depth + 2;
        self.open("StyleRule", depth);
        self.string("Name", &rule.selector, inner)?;
        self.string("Selector", &rule.selector, inner)?;
        if let Some(p) = rule.priority {
            let _ = writeln!(self.out, "{}<int name=\"Priority\">{}</int>", "\t".repeat(inner), p.round() as i32);
        }
        // Roblox requires the properties blob even when it's empty.
        self.blob("PropertiesSerialize", &attributes(&rule.props)?, inner);
        let (defaults, transitions): (Vec<_>, Vec<_>) = rule.transitions.iter().partition(|(k, _)| k == "*");
        if !defaults.is_empty() {
            self.dropped.push(format!(
                "`{}`: a default transition (`all`) isn't saved in a model file; list the properties instead",
                rule.selector
            ));
        }
        if !transitions.is_empty() {
            self.blob("PropertyTransitionsSerialize", &property_transitions(&transitions)?, inner);
        }
        if !rule.attributes.is_empty() {
            self.blob("AttributesSerialize", &attributes(&rule.attributes)?, inner);
        }
        self.close_properties(depth + 1);
        for child in &rule.children {
            self.rule(child, depth + 1)?;
        }
        self.close_item(depth);
        Ok(())
    }
}

/// Text for XML. Roblox models can't hold control characters other than tab and line breaks.
fn xml(text: &str) -> Result<String, String> {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            '\t' | '\n' | '\r' => s.push(c),
            c if c.is_control() => return Err(format!("{text:?} has a control character a model file can't hold")),
            c => s.push(c),
        }
    }
    Ok(s)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                s.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[derive(Default)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend(v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn i32(&mut self, v: f64) {
        self.0.extend((v.round() as i32).to_le_bytes());
    }
    fn f32(&mut self, v: f64) {
        self.0.extend((v as f32).to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend(v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend(s.as_bytes());
    }
}

/// Roblox's attribute encoding: a count, then each entry's name, type byte and value.
fn attributes(entries: &[(String, Value)]) -> Result<Vec<u8>, String> {
    let mut b = Bytes::default();
    b.u32(entries.len() as u32);
    for (name, value) in entries {
        b.str(name);
        attribute(&mut b, value).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(b.0)
}

fn attribute(b: &mut Bytes, v: &Value) -> Result<(), String> {
    let color = |b: &mut Bytes, c: &crate::luau::Color3| {
        b.f32(c.r / 255.0);
        b.f32(c.g / 255.0);
        b.f32(c.b / 255.0);
    };
    match v {
        Value::String(s) => {
            b.u8(0x02);
            b.str(s);
        }
        Value::Token(name) => {
            b.u8(0x02);
            b.str(&format!("${name}"));
        }
        Value::Bool(v) => {
            b.u8(0x03);
            b.u8(u8::from(*v));
        }
        Value::Number(n) => {
            b.u8(0x06);
            b.f64(*n);
        }
        Value::UDim(u) => {
            b.u8(0x09);
            b.f32(u.scale);
            b.i32(u.offset);
        }
        Value::UDim2(x, y) => {
            b.u8(0x0A);
            b.f32(x.scale);
            b.i32(x.offset);
            b.f32(y.scale);
            b.i32(y.offset);
        }
        Value::Color3(c) => {
            b.u8(0x0F);
            color(b, c);
        }
        Value::Vector2(x, y) => {
            b.u8(0x10);
            b.f32(*x);
            b.f32(*y);
        }
        Value::Enum { enum_type, item } => {
            b.u8(0x15);
            b.str(enum_type);
            b.u32(enum_value(enum_type, item)?);
        }
        Value::NumberSequence(keypoints) => {
            b.u8(0x17);
            b.u32(keypoints.len() as u32);
            for (time, value, envelope) in keypoints {
                b.f32(*envelope);
                b.f32(*time);
                b.f32(*value);
            }
        }
        Value::ColorSequence(keypoints) => {
            b.u8(0x19);
            b.u32(keypoints.len() as u32);
            for (time, c) in keypoints {
                b.f32(0.0);
                b.f32(*time);
                color(b, c);
            }
        }
        Value::NumberRange(min, max) => {
            b.u8(0x1B);
            b.f32(*min);
            b.f32(*max);
        }
        Value::Rect(a, c, d, e) => {
            b.u8(0x1C);
            for v in [a, c, d, e] {
                b.f32(*v);
            }
        }
        Value::Font { family, weight, style } => {
            b.u8(0x21);
            b.u16(enum_value("FontWeight", weight.as_deref().unwrap_or("Regular"))? as u16);
            b.u8(enum_value("FontStyle", style.as_deref().unwrap_or("Normal"))? as u8);
            b.str(family);
            // The cached face, which Roblox fills in when it loads the font.
            b.str("");
        }
        Value::FontEnum(_) => return Err("a model file can't hold Font.fromEnum(); use Font.new()".into()),
        Value::Luau(_) => return Err("a model file can't hold raw Luau".into()),
    }
    Ok(())
}

/// The number Roblox saves an Enum item as.
fn enum_value(enum_type: &str, item: &str) -> Result<u32, String> {
    ENUMS
        .lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|line| {
            let mut words = line.split(' ');
            (words.next() == Some(enum_type)).then(|| {
                words
                    .find_map(|w| w.split_once('=').filter(|(name, _)| *name == item).and_then(|(_, v)| v.parse().ok()))
            })
        })
        .flatten()
        .ok_or_else(|| format!("Enum.{enum_type}.{item} isn't an Enum item outlass can write to a model file"))
}

/// `PropertyTransitionsSerialize`: a version, a count, then each property's name and TweenInfo.
fn property_transitions(transitions: &[&(String, TweenInfo)]) -> Result<Vec<u8>, String> {
    let mut b = Bytes::default();
    b.u16(2);
    b.u32(transitions.len() as u32);
    for (name, t) in transitions {
        b.str(name);
        b.u8(0x24);
        b.f32(t.time);
        b.f32(t.delay_time);
        b.i32(t.repeat_count);
        b.u32(enum_value("EasingStyle", &t.easing_style)?);
        b.u32(enum_value("EasingDirection", &t.easing_direction)?);
        b.u8(u8::from(t.reverses));
    }
    Ok(b.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::luau::{Color3, UDim};

    /// Bytes Studio saved for the same values (see the module docs).
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn values_match_what_studio_saves() {
        let entries = vec![
            ("Size".to_string(), Value::UDim2(UDim { scale: 0.5, offset: 4.0 }, UDim { scale: 0.0, offset: 20.0 })),
            ("BackgroundColor3".to_string(), Value::Color3(Color3 { r: 255.0, g: 127.5, b: 0.0 })),
            ("M_enum".to_string(), Value::enum_item("ScaleType", "Crop")),
            ("CornerRadius".to_string(), Value::Token("radius".into())),
        ];
        assert_eq!(
            hex(&attributes(&entries).unwrap()),
            "04000000\
             0400000053697a650a0000003f040000000000000014000000\
             100000004261636b67726f756e64436f6c6f72330f0000803f0000003f00000000\
             060000004d5f656e756d15090000005363616c655479706504000000\
             0c000000436f726e6572526164697573020700000024726164697573"
        );
        let transition = (
            "Z".to_string(),
            TweenInfo {
                time: 0.25,
                easing_style: "Back".into(),
                easing_direction: "InOut".into(),
                repeat_count: 2.0,
                reverses: true,
                delay_time: 0.5,
            },
        );
        assert_eq!(
            hex(&property_transitions(&[&transition]).unwrap()),
            "020001000000010000005a240000803e0000003f02000000020000000200000001"
        );
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0, 0, 0, 0]), "AAAAAA==");
    }

    #[test]
    fn text_is_escaped_and_code_refused() {
        assert_eq!(xml("a > b & \"c\"").unwrap(), "a &gt; b &amp; &quot;c&quot;");
        assert!(xml("a\u{1}").is_err());
        assert!(attributes(&[("X".into(), Value::Luau("require(1)".into()))]).is_err());
    }
}
