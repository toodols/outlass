//! CSS `@media` / `@container` conditions and their Roblox StyleQuery equivalents.
//!
//! A query compiles to either a built-in Roblox query (`@ViewportDisplaySizeSmall`,
//! `@PreferredInputTouch`, ...) or a custom `StyleQuery` pseudo-instance whose conditions
//! (`MinSize`, `MaxSize`, `AspectRatioRange`, ...) are checked against its parent's AbsoluteSize.
//! Rules inside the query get the `@Name` selector prefix.

use crate::luau;

/// Where a query's size conditions are measured.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// `@media`: the viewport, i.e. the ScreenGui.
    Viewport,
    /// `@container [name]`: the nearest element declaring `container-type` / `container-name`.
    Container(Option<String>),
}

/// Conditions that must all hold (a query without commas).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conditions {
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub max_width: Option<f64>,
    pub max_height: Option<f64>,
    pub min_aspect: Option<f64>,
    pub max_aspect: Option<f64>,
    /// `Enum.PreferredInput` item.
    pub preferred_input: Option<&'static str>,
    pub reduced_motion: Option<bool>,
    /// `Enum.DisplaySize` item.
    pub display_size: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub target: Target,
    pub conditions: Conditions,
}

/// Pixels per `em`/`rem` in query lengths (CSS uses the initial font size, 16px).
const EM_PX: f64 = 16.0;

fn length(text: &str) -> Result<f64, String> {
    let t = text.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let value: f64 = num.parse().map_err(|_| format!("expected a length, got `{t}`"))?;
    match unit.trim() {
        "" | "px" => Ok(value),
        "em" | "rem" => Ok(value * EM_PX),
        other => Err(format!("unit `{other}` isn't supported in queries (use px)")),
    }
}

fn ratio(text: &str) -> Result<f64, String> {
    match text.split_once('/') {
        Some((a, b)) => {
            let a: f64 = a.trim().parse().map_err(|_| format!("bad ratio `{text}`"))?;
            let b: f64 = b.trim().parse().map_err(|_| format!("bad ratio `{text}`"))?;
            if b == 0.0 { Err(format!("bad ratio `{text}`")) } else { Ok(a / b) }
        }
        None => text.trim().parse().map_err(|_| format!("bad ratio `{text}`")),
    }
}

fn keep_max(slot: &mut Option<f64>, v: f64) {
    *slot = Some(slot.map_or(v, |old| old.max(v)));
}

fn keep_min(slot: &mut Option<f64>, v: f64) {
    *slot = Some(slot.map_or(v, |old| old.min(v)));
}

impl Conditions {
    /// Sets `feature` >= / <= `value` (`min` = lower bound).
    fn bound(&mut self, feature: &str, value: &str, min: bool) -> Result<(), String> {
        match feature {
            "width" | "inline-size" | "device-width" => {
                let v = length(value)?;
                if min { keep_max(&mut self.min_width, v) } else { keep_min(&mut self.max_width, v) }
            }
            "height" | "block-size" | "device-height" => {
                let v = length(value)?;
                if min { keep_max(&mut self.min_height, v) } else { keep_min(&mut self.max_height, v) }
            }
            "aspect-ratio" | "device-aspect-ratio" => {
                let v = ratio(value)?;
                if min { keep_max(&mut self.min_aspect, v) } else { keep_min(&mut self.max_aspect, v) }
            }
            other => return Err(format!("`{other}` can't be used as a range in a Roblox query")),
        }
        Ok(())
    }

    fn feature(&mut self, name: &str, value: &str, target: &Target) -> Result<(), String> {
        let value = value.trim();
        if let Some(feature) = name.strip_prefix("min-") {
            return self.bound(feature, value, true);
        }
        if let Some(feature) = name.strip_prefix("max-") {
            return self.bound(feature, value, false);
        }
        let media_only = |this: &str| -> Result<(), String> {
            if matches!(target, Target::Container(_)) {
                Err(format!("`{this}` is a media feature; it can't be used in @container"))
            } else {
                Ok(())
            }
        };
        match (name, value) {
            ("width" | "inline-size" | "height" | "block-size" | "aspect-ratio", _) => {
                self.bound(name, value, true)?;
                self.bound(name, value, false)?;
            }
            ("orientation", "landscape") => keep_max(&mut self.min_aspect, 1.0),
            ("orientation", "portrait") => keep_min(&mut self.max_aspect, 1.0),
            // Roblox's documented CSS equivalents.
            ("pointer", "fine") | ("hover", "hover") | ("any-hover", "hover") => {
                media_only(name)?;
                self.preferred_input = Some("KeyboardAndMouse");
            }
            ("pointer", "coarse") | ("hover", "none") | ("any-hover", "none") => {
                media_only(name)?;
                self.preferred_input = Some("Touch");
            }
            ("any-pointer", "coarse") => {
                media_only(name)?;
                self.preferred_input = Some("Gamepad");
            }
            ("prefers-reduced-motion", "reduce") => {
                media_only(name)?;
                self.reduced_motion = Some(true);
            }
            ("prefers-reduced-motion", "no-preference") => {
                media_only(name)?;
                self.reduced_motion = Some(false);
            }
            _ => return Err(format!("`({name}: {value})` has no Roblox StyleQuery equivalent")),
        }
        Ok(())
    }

    /// Range syntax: `width >= 600px`, `600px <= width`, `400px <= width <= 800px`.
    fn range(&mut self, text: &str) -> Result<(), String> {
        let mut parts: Vec<(String, Option<&str>)> = Vec::new();
        let mut rest = text;
        loop {
            let op_at = rest.find(['<', '>', '=']);
            let Some(at) = op_at else {
                parts.push((rest.trim().to_string(), None));
                break;
            };
            let op_len = if rest[at + 1..].starts_with('=') { 2 } else { 1 };
            parts.push((rest[..at].trim().to_string(), Some(&rest[at..at + op_len])));
            rest = &rest[at + op_len..];
        }
        let is_feature = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        // Pairs of (left, op, right); strict comparisons are treated as inclusive (pixels are whole).
        for pair in parts.windows(2) {
            let (left, Some(op)) = (&pair[0].0, pair[0].1) else { continue };
            let right = &pair[1].0;
            let (feature, value, feature_is_left) = if is_feature(left) {
                (left.as_str(), right.as_str(), true)
            } else if is_feature(right) {
                (right.as_str(), left.as_str(), false)
            } else {
                return Err(format!("can't read the range `{text}`"));
            };
            match (op, feature_is_left) {
                ("=", _) => {
                    self.bound(feature, value, true)?;
                    self.bound(feature, value, false)?;
                }
                (">=" | ">", true) | ("<=" | "<", false) => self.bound(feature, value, true)?,
                ("<=" | "<", true) | (">=" | ">", false) => self.bound(feature, value, false)?,
                _ => return Err(format!("can't read the range `{text}`")),
            }
        }
        Ok(())
    }

    /// Both sets of conditions (nested queries).
    pub fn and(&self, other: &Conditions) -> Conditions {
        let mut c = self.clone();
        for (slot, v) in [
            (&mut c.min_width, other.min_width),
            (&mut c.min_height, other.min_height),
            (&mut c.min_aspect, other.min_aspect),
        ] {
            if let Some(v) = v {
                keep_max(slot, v);
            }
        }
        for (slot, v) in [
            (&mut c.max_width, other.max_width),
            (&mut c.max_height, other.max_height),
            (&mut c.max_aspect, other.max_aspect),
        ] {
            if let Some(v) = v {
                keep_min(slot, v);
            }
        }
        c.preferred_input = other.preferred_input.or(c.preferred_input);
        c.reduced_motion = other.reduced_motion.or(c.reduced_motion);
        c.display_size = other.display_size.or(c.display_size);
        c
    }

    fn has_size(&self) -> bool {
        self.min_width.is_some()
            || self.min_height.is_some()
            || self.max_width.is_some()
            || self.max_height.is_some()
            || self.min_aspect.is_some()
            || self.max_aspect.is_some()
    }

    /// The equivalent built-in viewport query, when the size conditions are exactly one of the
    /// breakpoints Roblox documents for ViewportDisplaySize.
    fn display_size_equivalent(&self) -> Option<&'static str> {
        if self.min_height.is_some()
            || self.max_height.is_some()
            || self.min_aspect.is_some()
            || self.max_aspect.is_some()
        {
            return None;
        }
        match (self.min_width, self.max_width) {
            (None, Some(600.0)) => Some("Small"),
            (Some(601.0), Some(1200.0)) => Some("Medium"),
            (Some(1201.0), None) => Some("Large"),
            _ => None,
        }
    }
}

impl Query {
    /// The built-in Roblox query this is exactly equivalent to, if any.
    pub fn builtin(&self) -> Option<String> {
        if self.target != Target::Viewport {
            return None;
        }
        let c = &self.conditions;
        let display = if c.has_size() { c.display_size_equivalent() } else { c.display_size };
        let features = [c.preferred_input.is_some(), c.reduced_motion.is_some(), display.is_some()];
        if features.iter().filter(|f| **f).count() != 1 || (c.has_size() && display.is_none()) {
            return None;
        }
        Some(match (c.preferred_input, c.reduced_motion, display) {
            (Some(input), _, _) => format!("PreferredInput{input}"),
            (_, Some(true), _) => "ReducedMotionEnabledTrue".to_string(),
            (_, Some(false), _) => "ReducedMotionEnabledFalse".to_string(),
            (_, _, Some(size)) => format!("ViewportDisplaySize{size}"),
            _ => unreachable!(),
        })
    }

    /// The `@Name` used in selectors: the built-in name, or a readable name for a custom query.
    pub fn name(&self) -> String {
        if let Some(builtin) = self.builtin() {
            return builtin;
        }
        let fmt = |v: f64| luau::number(v).replace(['.', '-'], "_");
        let c = &self.conditions;
        let mut name = match &self.target {
            Target::Viewport => "Media".to_string(),
            Target::Container(None) => "Container".to_string(),
            Target::Container(Some(n)) => {
                let pascal: String = n
                    .split(['-', '_'])
                    .map(|w| {
                        let mut cs = w.chars();
                        cs.next().map(|f| f.to_ascii_uppercase().to_string() + cs.as_str()).unwrap_or_default()
                    })
                    .collect();
                format!("Container{}", crate::roblox::attribute_name(&pascal))
            }
        };
        let parts = [
            ("MinWidth", c.min_width),
            ("MaxWidth", c.max_width),
            ("MinHeight", c.min_height),
            ("MaxHeight", c.max_height),
            ("MinAspect", c.min_aspect),
            ("MaxAspect", c.max_aspect),
        ];
        for (label, v) in parts {
            if let Some(v) = v {
                name.push_str(label);
                name.push_str(&fmt(v));
            }
        }
        if let Some(input) = c.preferred_input {
            name.push_str(input);
        }
        if let Some(reduced) = c.reduced_motion {
            name.push_str(if reduced { "ReducedMotion" } else { "FullMotion" });
        }
        if let Some(size) = c.display_size {
            name.push_str(size);
        }
        name
    }

    /// The StyleQuery conditions as (property, Luau value) pairs.
    pub fn conditions(&self) -> Vec<(String, luau::Value)> {
        let c = &self.conditions;
        let mut out = Vec::new();
        if c.min_width.is_some() || c.min_height.is_some() {
            out.push(("MinSize".into(), luau::Value::Vector2(c.min_width.unwrap_or(0.0), c.min_height.unwrap_or(0.0))));
        }
        if c.max_width.is_some() || c.max_height.is_some() {
            out.push((
                "MaxSize".into(),
                luau::Value::Vector2(c.max_width.unwrap_or(f64::INFINITY), c.max_height.unwrap_or(f64::INFINITY)),
            ));
        }
        if c.min_aspect.is_some() || c.max_aspect.is_some() {
            out.push((
                "AspectRatioRange".into(),
                luau::Value::NumberRange(c.min_aspect.unwrap_or(0.0), c.max_aspect.unwrap_or(f64::INFINITY)),
            ));
        }
        if let Some(input) = c.preferred_input {
            out.push(("PreferredInput".into(), luau::Value::enum_item("PreferredInput", input)));
        }
        if let Some(reduced) = c.reduced_motion {
            out.push(("ReducedMotionEnabled".into(), luau::Value::Bool(reduced)));
        }
        if let Some(size) = c.display_size {
            out.push(("ViewportDisplaySize".into(), luau::Value::enum_item("DisplaySize", size)));
        }
        out
    }

    /// Both queries at once (a query nested in another). `None` when they can't be combined into a
    /// single StyleQuery: different containers, or a media query inside a container query.
    pub fn and(&self, inner: &Query) -> Option<Query> {
        if self.target != inner.target {
            return None;
        }
        Some(Query { target: self.target.clone(), conditions: self.conditions.and(&inner.conditions) })
    }
}

/// Parses one comma-free alternative: `screen and (min-width: 600px) and (orientation: portrait)`.
fn parse_alternative(text: &str, target: Target) -> Result<Option<Query>, String> {
    let mut conditions = Conditions::default();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('(') {
            let close = after.find(')').ok_or_else(|| format!("missing `)` in `{text}`"))?;
            let inner = after[..close].trim();
            if inner.contains(['<', '>', '=']) {
                conditions.range(inner)?;
            } else if let Some((name, value)) = inner.split_once(':') {
                conditions.feature(name.trim(), value, &target)?;
            } else {
                return Err(format!("`({inner})` has no Roblox StyleQuery equivalent"));
            }
            rest = after[close + 1..].trim_start();
            continue;
        }
        let word_end = rest.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(rest.len());
        let word = &rest[..word_end];
        match word {
            "and" | "only" | "all" | "screen" => {}
            "print" | "speech" => return Ok(None), // never matches in a game
            "not" | "or" => return Err(format!("`{word}` isn't supported in Roblox queries")),
            other => return Err(format!("unexpected `{other}` in query `{text}`")),
        }
        rest = rest[word_end..].trim_start();
    }
    Ok(Some(Query { target, conditions }))
}

/// `@media` params → alternatives (comma-separated queries apply when any matches).
pub fn parse_media(params: &str) -> Result<Vec<Query>, String> {
    let mut out = Vec::new();
    for alt in params.to_ascii_lowercase().split(',') {
        if let Some(q) = parse_alternative(alt, Target::Viewport)? {
            out.push(q);
        }
    }
    Ok(out)
}

/// `@container [name] (conditions)`.
pub fn parse_container(params: &str) -> Result<Vec<Query>, String> {
    let params = params.trim();
    let (name, conditions) = match params.find('(') {
        Some(0) | None => (None, params),
        Some(at) => (Some(params[..at].trim().to_string()), &params[at..]),
    };
    let mut out = Vec::new();
    for alt in conditions.to_ascii_lowercase().split(',') {
        if let Some(q) = parse_alternative(alt, Target::Container(name.clone()))? {
            out.push(q);
        }
    }
    Ok(out)
}

/// A built-in query written directly as an at-rule (`@PreferredInputTouch { ... }`), as a Query
/// when it's one we can combine with others.
pub fn builtin_by_name(name: &str) -> Option<Query> {
    let mut c = Conditions::default();
    match name {
        "PreferredInputKeyboardAndMouse" => c.preferred_input = Some("KeyboardAndMouse"),
        "PreferredInputTouch" => c.preferred_input = Some("Touch"),
        "PreferredInputGamepad" => c.preferred_input = Some("Gamepad"),
        "ReducedMotionEnabledTrue" => c.reduced_motion = Some(true),
        "ReducedMotionEnabledFalse" => c.reduced_motion = Some(false),
        "ViewportDisplaySizeSmall" => c.display_size = Some("Small"),
        "ViewportDisplaySizeMedium" => c.display_size = Some("Medium"),
        "ViewportDisplaySizeLarge" => c.display_size = Some("Large"),
        _ => return None,
    }
    Some(Query { target: Target::Viewport, conditions: c })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_media_features_map_to_builtins() {
        let name = |q: &str| parse_media(q).unwrap()[0].name();
        assert_eq!(name("(max-width: 600px)"), "ViewportDisplaySizeSmall");
        assert_eq!(name("(min-width: 601px) and (max-width: 1200px)"), "ViewportDisplaySizeMedium");
        assert_eq!(name("(min-width: 1201px)"), "ViewportDisplaySizeLarge");
        assert_eq!(name("(pointer: fine)"), "PreferredInputKeyboardAndMouse");
        assert_eq!(name("(pointer: coarse)"), "PreferredInputTouch");
        assert_eq!(name("(any-pointer: coarse)"), "PreferredInputGamepad");
        assert_eq!(name("(prefers-reduced-motion: reduce)"), "ReducedMotionEnabledTrue");
        assert_eq!(name("screen and (prefers-reduced-motion: no-preference)"), "ReducedMotionEnabledFalse");
    }

    #[test]
    fn other_queries_become_custom_style_queries() {
        let q = &parse_media("(width >= 900px) and (orientation: landscape)").unwrap()[0];
        assert!(q.builtin().is_none());
        assert_eq!(q.name(), "MediaMinWidth900MinAspect1");
        assert_eq!(
            q.conditions().iter().map(|(k, v)| (k.clone(), luau::render(v))).collect::<Vec<_>>(),
            vec![
                ("MinSize".to_string(), "Vector2.new(900, 0)".to_string()),
                ("AspectRatioRange".to_string(), "NumberRange.new(1, math.huge)".to_string())
            ]
        );
        let q = &parse_media("(400px <= width <= 800px)").unwrap()[0];
        assert_eq!(q.conditions.min_width, Some(400.0));
        assert_eq!(q.conditions.max_width, Some(800.0));
        let q = &parse_media("(pointer: coarse) and (max-width: 500px)").unwrap()[0];
        assert_eq!(q.name(), "MediaMaxWidth500Touch");
    }

    #[test]
    fn containers() {
        let q = &parse_container("(min-width: 400px)").unwrap()[0];
        assert_eq!(q.target, Target::Container(None));
        assert_eq!(q.name(), "ContainerMinWidth400");
        let q = &parse_container("side-bar (max-width: 20rem)").unwrap()[0];
        assert_eq!(q.name(), "ContainerSideBarMaxWidth320");
        assert!(parse_container("(pointer: coarse)").is_err());
    }

    #[test]
    fn alternatives_and_unsupported() {
        assert_eq!(parse_media("(max-width: 600px), (orientation: portrait)").unwrap().len(), 2);
        assert!(parse_media("print").unwrap().is_empty());
        assert!(parse_media("not screen").is_err());
        assert!(parse_media("(color)").is_err());
    }
}
