//! Lowers evaluated rules to Roblox StyleRules and emits Luau (or debug CSS).

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::approx::{self, ApproxOptions, Decl};
use crate::diag::Diagnostics;
use crate::eval::{OutRule, Sheet, Token};
use crate::luau::{self, LuauOptions};
use crate::query::Target;
use crate::value::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Cascade {
    /// The CSS cascade decides which rule wins: `!important` beats normal declarations, then
    /// `@priority` tiers (like CSS layers; default 0), then specificity, then source order. Every
    /// rule gets its rank in that order as its StyleRule.Priority.
    #[default]
    Css,
    /// Only explicit `@priority` / `--default-priority` values are emitted (lass behaviour);
    /// `!important` is ignored.
    None,
}

/// A rule's position in the CSS cascade; greater wins.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
struct CascadeKey {
    important: bool,
    layer: f64,
    specificity: (u32, u32, u32),
    order: usize,
}

pub struct CodegenOptions {
    pub luau: LuauOptions,
    pub approx: ApproxOptions,
    pub default_priority: Option<f64>,
    pub cascade: Cascade,
    /// `StyleSheet.Name`; also used in the header comment.
    pub sheet_name: String,
    /// Source description for the header comment; `None` omits the header.
    pub header: Option<String>,
}

/// A Roblox-level StyleRule.
#[derive(Debug, Default)]
pub struct RobloxRule {
    pub selector: String,
    pub priority: Option<f64>,
    pub props: Vec<(String, String)>,
    pub attributes: Vec<(String, String)>,
    pub transitions: Vec<(String, String)>,
    pub children: Vec<RobloxRule>,
}

impl RobloxRule {
    fn is_empty(&self) -> bool {
        self.props.is_empty() && self.attributes.is_empty() && self.transitions.is_empty() && self.children.is_empty()
    }
}

fn set(list: &mut Vec<(String, String)>, key: String, value: String) {
    match list.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => list.push((key, value)),
    }
}

/// Whether a declaration name is a CSS property (lowercase/kebab-case) rather than a Roblox property.
pub fn is_css_property(name: &str) -> bool {
    match name.chars().next() {
        Some(c) if c.is_ascii_lowercase() => true,
        Some('-') => !name.starts_with("--"),
        _ => false,
    }
}

fn attributes(tokens: &[Token], opts: &LuauOptions, diag: &mut Diagnostics) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for token in tokens {
        let name = luau::attribute_name(&token.name);
        if name != token.name {
            diag.warn(
                format!("token \"--{}\" becomes attribute \"{name}\" (attribute names may only contain letters, digits and _)", token.name),
                Some(&token.span),
            );
        }
        match luau::value(&token_value(&token.value, &token.name, diag, &token.span), opts) {
            Ok(v) => set(&mut out, name, v),
            Err(e) => diag.warn(format!("token --{}: {e} (ignored)", token.name), Some(&token.span)),
        }
    }
    out
}

/// A custom property holds raw text, exactly as CSS and dart-sass keep it, so a token's value
/// arrives here as an unquoted string. Reading it back as a CSS value is what makes
/// `--Surface: #1f2937` a `Color3` and `--Elevation: 2` a number instead of two Luau syntax
/// errors. Variables are already gone by now (only `#{}` substitutes into raw text), so this only
/// ever sees literals.
fn token_value(v: &Value, name: &str, diag: &mut Diagnostics, span: &crate::diag::Span) -> Value {
    let Value::Str { text, quoted: false } = v else { return v.clone() };
    match crate::eval::evaluate_expression(text) {
        // A bare `1px solid red` is three values where an attribute holds one, so it stays text.
        Ok(Value::List { items, bracketed: false, .. }) if items.len() > 1 => Value::quoted(text.clone()),
        Ok(parsed) => parsed,
        Err(e) => {
            let hint = if text.contains('$') {
                " (a custom property is raw text; write #{$var} to substitute a variable)"
            } else {
                ""
            };
            diag.warn(format!("token --{name}: {}{hint}", e.message), Some(span));
            // Keep the text, quoted: an attribute holding the literal characters is what CSS
            // would do, and it can't turn into a Luau syntax error.
            Value::quoted(text.clone())
        }
    }
}

/// Converts evaluated rules into the Roblox StyleRule tree.
pub fn lower(sheet: &Sheet, opts: &CodegenOptions, diag: &mut Diagnostics) -> (Vec<(String, String)>, Vec<RobloxRule>) {
    let sheet_attributes = attributes(&sheet.tokens, &opts.luau, diag);

    let mut rules: Vec<RobloxRule> = Vec::new();
    let hides_anything = sheet.rules.iter().any(|r| r.decls.iter().any(hides));
    let parts = cascade_parts(sheet, opts, diag);
    let priorities = cascade_priorities(sheet, &parts, opts);

    for (idx, rule) in sheet.rules.iter().enumerate() {
        if rule.selector.0.is_empty() || rule.query {
            continue;
        }
        let selector = rule.selector.to_roblox(diag, Some(&rule.span));
        // Rules inside @media / @container / @Query get the query's `@Name` prefix, once per alternative.
        let prefixes: Vec<Option<String>> = match rule.parent {
            Some(container) => sheet.rules[container].queries.iter().map(|q| Some(q.name())).collect(),
            None => vec![None],
        };
        for (part, important) in &parts[idx] {
            let priority = priorities.get(&(idx, *important)).copied().flatten();
            let inherited = inherited_decls(sheet, idx);
            let mut lowered = lower_rule(part, &inherited, selector.clone(), priority, opts, diag);
            if !hides_anything {
                drop_generated_visible(part, &mut lowered);
            }
            for prefix in &prefixes {
                for r in &lowered {
                    let selector = match prefix {
                        Some(name) => prefix_selector(&r.selector, &format!("@{name} ")),
                        None => r.selector.clone(),
                    };
                    rules.push(RobloxRule {
                        selector,
                        priority: r.priority,
                        props: r.props.clone(),
                        attributes: r.attributes.clone(),
                        transitions: r.transitions.clone(),
                        children: Vec::new(),
                    });
                }
            }
        }
    }
    drop_unneeded_stroke_resets(&mut rules);
    let definitions = query_definitions(sheet, diag);
    rules.splice(0..0, definitions);

    // The user-agent stylesheet: defaults below every author rule, like a browser's. Any rule that
    // sets the same property overrides them (e.g. `RichText: false`).
    let lowest = match opts.cascade {
        Cascade::Css => 0.0,
        Cascade::None => lowest_priority(&rules).min(0.0) - 1.0,
    };
    // A fresh GuiObject is 0x0, where a CSS box with no size given takes one from its content.
    // AutomaticSize is a floor rather than a replacement — it never shrinks an element below its
    // Size — and `--approx=size` turns every CSS `width`/`height` into Size *and* an explicit
    // AutomaticSize, so a rule that sizes an element switches this straight back off.
    if opts.approx.groups.contains(&approx::Group::Size) {
        rules.insert(
            0,
            RobloxRule {
                selector: GUI_OBJECT_CLASSES.join(", "),
                priority: Some(lowest),
                props: vec![("AutomaticSize".to_string(), "Enum.AutomaticSize.XY".to_string())],
                ..Default::default()
            },
        );
    }
    // A CSS scroll container's scrollable area is its content; a ScrollingFrame's canvas is a fixed
    // `{0, 0}, {2, 0}` unless told otherwise. CanvasSize is a floor under AutomaticCanvasSize, so
    // it's zeroed rather than left at twice the frame's height.
    if opts.approx.groups.contains(&approx::Group::Visibility) {
        rules.insert(
            0,
            RobloxRule {
                selector: "ScrollingFrame".to_string(),
                priority: Some(lowest),
                props: vec![
                    ("AutomaticCanvasSize".to_string(), "Enum.AutomaticSize.XY".to_string()),
                    ("CanvasSize".to_string(), "UDim2.new()".to_string()),
                ],
                ..Default::default()
            },
        );
    }
    let mut text_defaults = vec![("RichText".to_string(), "true".to_string())];
    // CSS text wraps unless `white-space: nowrap` says otherwise; Roblox text runs off the edge
    // unless TextWrapped says otherwise.
    if opts.approx.groups.contains(&approx::Group::Text) {
        text_defaults.push(("TextWrapped".to_string(), "true".to_string()));
    }
    rules.insert(
        0,
        RobloxRule {
            selector: "TextLabel, TextButton, TextBox".to_string(),
            priority: Some(lowest),
            props: text_defaults,
            ..Default::default()
        },
    );
    (sheet_attributes, rules)
}

/// The GuiObject classes a stylesheet can select, used for the user-agent defaults. Every one of
/// them starts out 0x0.
const GUI_OBJECT_CLASSES: &[&str] = &[
    "Frame",
    "TextLabel",
    "TextButton",
    "TextBox",
    "ImageLabel",
    "ImageButton",
    "ScrollingFrame",
    "CanvasGroup",
    "VideoFrame",
    "ViewportFrame",
];

/// Splits a Roblox selector list at top-level commas.
fn split_selector_list(selector: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in selector.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(selector[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(selector[start..].trim());
    parts
}

/// `@Name ` in front of every selector in the list.
fn prefix_selector(selector: &str, prefix: &str) -> String {
    split_selector_list(selector).iter().map(|s| format!("{prefix}{s}")).collect::<Vec<_>>().join(", ")
}

/// CSS declarations that mark an element as a query container. They configure outlass itself and
/// aren't translated to properties.
const CONTAINER_PROPERTIES: &[&str] = &["container", "container-type", "container-name"];

/// The names a rule's element is a container under, if it's a container at all:
/// `container-type: inline-size` → unnamed; `container-name: a b` / `container: a / size` → a, b.
fn container_names(rule: &OutRule) -> Option<Vec<String>> {
    let text =
        |name: &str| rule.decls.iter().rev().find(|d| d.name == name).map(|d| d.value.to_css().unwrap_or_default());
    let mut names: Vec<String> = Vec::new();
    let mut is_container = false;
    if let Some(shorthand) = text("container") {
        let (name_part, kind) = shorthand.split_once('/').unwrap_or((&shorthand, "size"));
        names.extend(name_part.split_whitespace().filter(|n| *n != "none").map(str::to_string));
        is_container |= kind.trim() != "normal";
    }
    if let Some(kind) = text("container-type") {
        is_container = kind.trim() != "normal";
    }
    if let Some(list) = text("container-name") {
        names = list.split_whitespace().filter(|n| *n != "none").map(str::to_string).collect();
        is_container |= !names.is_empty();
    }
    is_container.then_some(names)
}

/// One `<element>::StyleQuery #Name { conditions }` rule per custom query: on the ScreenGui for
/// @media, on each matching container for @container.
fn query_definitions(sheet: &Sheet, diag: &mut Diagnostics) -> Vec<RobloxRule> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for rule in sheet.rules.iter().filter(|r| r.query) {
        for q in &rule.queries {
            let crate::eval::QueryRef::Known(q) = q else { continue };
            let name = q.name();
            if q.builtin().is_some() || seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let hosts: Vec<String> = match &q.target {
                Target::Viewport => vec!["ScreenGui".to_string()],
                Target::Container(wanted) => sheet
                    .rules
                    .iter()
                    .filter(|r| !r.query && r.parent.is_none())
                    .filter(|r| {
                        container_names(r).is_some_and(|names| wanted.as_ref().is_none_or(|w| names.contains(w)))
                    })
                    .flat_map(|r| {
                        split_selector_list(&r.selector.to_roblox(&mut Diagnostics::default(), None))
                            .into_iter()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect(),
            };
            if hosts.is_empty() {
                let which = match &q.target {
                    Target::Container(Some(n)) => format!("with `container-name: {n}`"),
                    _ => "with `container-type`".to_string(),
                };
                diag.warn(
                    format!("no element is declared a container {which}, so @container rules for {name} never apply"),
                    Some(&rule.span),
                );
                continue;
            }
            let selector = hosts.iter().map(|h| format!("{h}::StyleQuery #{name}")).collect::<Vec<_>>().join(", ");
            out.push(RobloxRule { selector, props: q.conditions(), ..Default::default() });
        }
    }
    out
}

fn lowest_priority(rules: &[RobloxRule]) -> f64 {
    rules.iter().map(|r| r.priority.unwrap_or(0.0).min(lowest_priority(&r.children))).fold(f64::INFINITY, f64::min)
}

/// Splits each rule into the parts that take part in the cascade: its normal declarations (with
/// its tokens) and, under the CSS cascade, its `!important` declarations as a separate part.
fn cascade_parts(sheet: &Sheet, opts: &CodegenOptions, diag: &mut Diagnostics) -> Vec<Vec<(OutRule, bool)>> {
    let mut warned = false;
    sheet
        .rules
        .iter()
        .map(|rule| {
            if rule.query || rule.selector.0.is_empty() || (rule.decls.is_empty() && rule.tokens.is_empty()) {
                return Vec::new();
            }
            let has_important = rule.decls.iter().any(|d| d.important);
            if opts.cascade == Cascade::None || !has_important {
                if has_important && !warned {
                    let span = rule.decls.iter().find(|d| d.important).map(|d| d.span.clone());
                    diag.warn("!important needs the CSS cascade (--cascade css) and is ignored", span.as_ref());
                    warned = true;
                }
                return vec![(rule.clone(), false)];
            }
            let mut normal = rule.clone();
            normal.decls.retain(|d| !d.important);
            let mut important = rule.clone();
            important.decls.retain(|d| d.important);
            important.tokens.clear();
            let mut parts = Vec::new();
            if !normal.decls.is_empty() || !normal.tokens.is_empty() {
                parts.push((normal, false));
            }
            parts.push((important, true));
            parts
        })
        .collect()
}

/// StyleRule.Priority for each (rule index, important) part. Under the CSS cascade every part gets
/// its rank (1, 2, ...) in cascade order, so the rule CSS would apply always has the higher priority.
fn cascade_priorities(
    sheet: &Sheet,
    parts: &[Vec<(OutRule, bool)>],
    opts: &CodegenOptions,
) -> HashMap<(usize, bool), Option<f64>> {
    let layer_of = |rule: &OutRule| rule.priority.or(opts.default_priority);
    if opts.cascade == Cascade::None {
        return parts
            .iter()
            .enumerate()
            .flat_map(|(idx, p)| p.iter().map(move |(rule, important)| ((idx, *important), layer_of(rule))))
            .collect();
    }
    let mut keys: Vec<((usize, bool), CascadeKey)> = parts
        .iter()
        .enumerate()
        .flat_map(|(idx, p)| {
            p.iter().map(move |(rule, important)| {
                let key = CascadeKey {
                    important: *important,
                    layer: layer_of(rule).unwrap_or(0.0),
                    specificity: sheet.rules[idx].selector.specificity(),
                    order: idx,
                };
                ((idx, *important), key)
            })
        })
        .collect();
    keys.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    keys.into_iter().enumerate().map(|(rank, (id, _))| (id, Some(rank as f64 + 1.0))).collect()
}

/// Whether a declaration can make an element invisible.
fn hides(decl: &crate::eval::OutDecl) -> bool {
    let value = decl.value.to_css().unwrap_or_default().to_ascii_lowercase();
    match decl.name.as_str() {
        "display" => value == "none",
        "visibility" => value == "hidden" || value == "collapse",
        "Visible" => value == "false",
        _ => false,
    }
}

/// `display: flex` (or `visibility: visible`) sets `Visible = true`, which only matters for undoing
/// another rule's `display: none`. With nothing in the sheet hiding elements, it would only fight
/// scripts that set `Visible`, so it's dropped. Explicit `Visible: true` declarations are kept.
fn drop_generated_visible(rule: &OutRule, lowered: &mut Vec<RobloxRule>) {
    if rule.decls.iter().any(|d| d.name == "Visible") {
        return;
    }
    for r in lowered.iter_mut().filter(|r| !r.selector.contains("::")) {
        r.props.retain(|(k, v)| !(k == "Visible" && v == "true"));
    }
    lowered.retain(|r| !r.is_empty());
}

/// `border: none` disables the element's `::UIStroke`, which creates a (disabled) UIStroke on every
/// matched element. That only matters if some rule turns a stroke on; otherwise drop those rules.
fn drop_unneeded_stroke_resets(rules: &mut Vec<RobloxRule>) {
    fn is_stroke_reset(rule: &RobloxRule) -> bool {
        rule.selector.contains("::UIStroke")
            && rule.transitions.is_empty()
            && rule.children.is_empty()
            && rule.props.iter().all(|(k, v)| k == "Enabled" && v == "false")
    }
    fn any_real_stroke(rules: &[RobloxRule]) -> bool {
        rules.iter().any(|r| (r.selector.contains("::UIStroke") && !is_stroke_reset(r)) || any_real_stroke(&r.children))
    }
    fn remove(rules: &mut Vec<RobloxRule>) {
        rules.retain(|r| !is_stroke_reset(r));
        for r in rules {
            remove(&mut r.children);
        }
    }
    if !any_real_stroke(rules) {
        remove(rules);
    }
}

/// CSS properties that outlass folds into one Roblox property (or that are interpreted relative to
/// each other). CSS cascades each of these separately, but Roblox has e.g. a single `Position`.
const COMPOSITE_GROUPS: &[&[&str]] = &[
    // Position, Size, AnchorPoint, AutomaticSize, Rotation
    &["position", "left", "top", "right", "bottom", "inset", "width", "height", "transform"],
    // FontFace
    &["font", "font-family", "font-weight", "font-style"],
    // UISizeConstraint
    &["min-width", "min-height", "max-width", "max-height"],
    // UIListLayout axes depend on the direction
    &["display", "flex-direction", "flex-wrap", "justify-content", "align-items", "gap", "row-gap", "column-gap"],
    // transparency combines colour alpha, opacity, gradients and masks
    &[
        "opacity",
        "color",
        "background",
        "background-color",
        "background-image",
        "background-clip",
        "mask",
        "mask-image",
        "border",
        "border-color",
        "outline",
        "outline-color",
        "scrollbar-color",
    ],
    // ScrollingDirection / AutomaticCanvasSize cover both axes
    &["overflow", "overflow-x", "overflow-y"],
    // transition longhands combine by index
    &["transition", "transition-property", "transition-duration", "transition-timing-function", "transition-delay"],
];

fn composite_group(name: &str) -> Option<&'static [&'static str]> {
    let bare = name.strip_prefix("-webkit-").unwrap_or(name);
    COMPOSITE_GROUPS.iter().copied().find(|group| group.contains(&bare))
}

/// Per-axis cascade for composite properties. When a rule sets part of a composite group (say
/// `left`), the parts it doesn't set (`top`) still apply to its elements from weaker rules that
/// match all of them (`.bar` for `.bar.left`), exactly as CSS would cascade them. Those declarations
/// are returned so the rule's Roblox `Position` can be computed from all of them.
fn inherited_decls(sheet: &Sheet, index: usize) -> Vec<Decl> {
    let rule = &sheet.rules[index];
    let groups: Vec<&[&str]> = {
        let mut groups: Vec<&[&str]> = Vec::new();
        for decl in rule.decls.iter().filter(|d| is_css_property(&d.name)) {
            if let Some(group) = composite_group(&decl.name)
                && !groups.iter().any(|g| std::ptr::eq(*g, group))
            {
                groups.push(group);
            }
        }
        groups
    };
    if groups.is_empty() {
        return Vec::new();
    }
    let mut weaker: Vec<(usize, &OutRule)> = sheet
        .rules
        .iter()
        .enumerate()
        .filter(|(i, other)| {
            *i != index
                && !other.query
                // A rule inside a query only applies when the query does.
                && (other.parent.is_none() || other.parent == rule.parent)
                && other.selector.covers(&rule.selector)
                // Identical selectors: only earlier rules come first in the cascade.
                && (other.selector != rule.selector || *i < index)
        })
        .collect();
    // Cascade order: lower specificity first, then source order; the rule's own decls come last.
    weaker.sort_by_key(|(i, other)| (other.selector.specificity(), *i));
    weaker
        .into_iter()
        .flat_map(|(_, other)| other.decls.iter())
        .filter(|d| {
            is_css_property(&d.name)
                && composite_group(&d.name).is_some_and(|g| groups.iter().any(|x| std::ptr::eq(*x, g)))
        })
        .map(|d| Decl { name: d.name.clone(), value: d.value.clone(), span: Some(d.span.clone()) })
        .collect()
}

/// Keeps only the properties (and pseudo-instance properties and transitions) that `keys` has.
fn restrict_to(mut full: approx::Translated, keys: &approx::Translated) -> approx::Translated {
    let has = |list: &[(String, String)], key: &str| list.iter().any(|(k, _)| k == key);
    let pseudo_has = |list: &[(String, Vec<(String, String)>)], class: &str, key: &str| {
        list.iter().any(|(c, props)| c == class && has(props, key))
    };
    full.props.retain(|(k, _)| has(&keys.props, k));
    full.transitions.retain(|(k, _)| has(&keys.transitions, k));
    for (class, props) in &mut full.pseudo {
        props.retain(|(k, _)| pseudo_has(&keys.pseudo, class, k));
    }
    full.pseudo.retain(|(_, props)| !props.is_empty());
    for (class, props) in &mut full.pseudo_transitions {
        props.retain(|(k, _)| pseudo_has(&keys.pseudo_transitions, class, k));
    }
    full.pseudo_transitions.retain(|(_, props)| !props.is_empty());
    full
}

fn lower_rule(
    rule: &OutRule,
    inherited: &[Decl],
    selector: String,
    priority: Option<f64>,
    opts: &CodegenOptions,
    diag: &mut Diagnostics,
) -> Vec<RobloxRule> {
    let mut main = RobloxRule { selector, priority, ..Default::default() };
    main.attributes = attributes(&rule.tokens, &opts.luau, diag);

    let mut css: Vec<Decl> =
        inherited.iter().map(|d| Decl { name: d.name.clone(), value: d.value.clone(), span: d.span.clone() }).collect();
    let mut explicit: Vec<(String, String)> = Vec::new();
    let mut explicit_transitions: Vec<(String, String)> = Vec::new();
    for decl in &rule.decls {
        if CONTAINER_PROPERTIES.contains(&decl.name.as_str()) {
            // Marks the element as a query container; see query_definitions.
        } else if is_css_property(&decl.name) {
            css.push(Decl { name: decl.name.clone(), value: decl.value.clone(), span: Some(decl.span.clone()) });
        } else if decl.name.eq_ignore_ascii_case("Transition") {
            match approx::roblox_transitions(&decl.value) {
                Ok(list) => {
                    for (k, v) in list {
                        set(&mut explicit_transitions, k, v);
                    }
                }
                Err(e) => diag.warn(format!("Transition: {e} (ignored)"), Some(&decl.span)),
            }
        } else {
            if let crate::value::Value::Color(c) = &decl.value
                && c.a < 1.0
            {
                diag.warn(
                        format!(
                            "{}: Color3 has no alpha channel, so the alpha of {} is dropped (set a Transparency property, or use a CSS property with --approx)",
                            decl.name,
                            decl.value.to_css().unwrap_or_default()
                        ),
                        Some(&decl.span),
                    );
            }
            match luau::value(&decl.value, &opts.luau) {
                Ok(v) => set(&mut explicit, decl.name.clone(), v),
                Err(e) => diag.warn(format!("{}: {e} (ignored)", decl.name), Some(&decl.span)),
            }
        }
    }

    let mut out = Vec::new();
    let translated = if css.is_empty() {
        Default::default()
    } else if inherited.is_empty() {
        approx::translate(&css, &opts.approx, diag)
    } else {
        // Inherited declarations only complete the Roblox properties this rule's own declarations
        // produce (e.g. the `top` half of `Position`); they never add properties of their own, which
        // could compete with sibling state rules that CSS would let win.
        let own: Vec<Decl> = rule
            .decls
            .iter()
            .filter(|d| is_css_property(&d.name))
            .map(|d| Decl { name: d.name.clone(), value: d.value.clone(), span: Some(d.span.clone()) })
            .collect();
        let mut own_only = approx::translate(&own, &opts.approx, &mut Diagnostics::default());
        // `opacity` scales what the weaker rules paint, so it recomputes their transparencies too:
        // `.a:hover { opacity: 0.5 }` fades `.a`'s background and border.
        if own.iter().any(|d| d.name == "opacity") {
            for prop in approx::css_property_targets("opacity") {
                own_only.set_prop(prop, String::new());
            }
            for (class, prop) in approx::css_pseudo_targets("opacity") {
                own_only.set_pseudo_prop(class, prop, String::new());
            }
        }
        restrict_to(approx::translate(&css, &opts.approx, diag), &own_only)
    };
    // Explicit Roblox properties win over approximations of the same property.
    for (k, v) in translated.props {
        if !explicit.iter().any(|(e, _)| *e == k) {
            set(&mut main.props, k, v);
        }
    }
    for (k, v) in explicit {
        set(&mut main.props, k, v);
    }
    for (k, v) in translated.transitions.into_iter().chain(explicit_transitions) {
        set(&mut main.transitions, k, v);
    }
    // Pseudo-instance rules, each with the transitions for its properties. A rule that only carries
    // transitions still creates the pseudo-instance, so that's limited to classes whose defaults
    // change nothing (a default UIStroke draws an outline, a default UICorner rounds, ...).
    const NEUTRAL_PSEUDO_CLASSES: &[&str] = &["UIScale", "UIPadding", "UISizeConstraint"];
    let mut pseudo = translated.pseudo;
    for (class, _) in &translated.pseudo_transitions {
        if !pseudo.iter().any(|(c, _)| c == class) && NEUTRAL_PSEUDO_CLASSES.contains(&class.as_str()) {
            pseudo.push((class.clone(), Vec::new()));
        }
    }
    let pseudo_rules: Vec<RobloxRule> = pseudo
        .into_iter()
        .map(|(class, props)| {
            let transitions = translated
                .pseudo_transitions
                .iter()
                .find(|(c, _)| *c == class)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            RobloxRule {
                selector: rule.selector.with_pseudo_instance(&class).to_roblox(&mut Diagnostics::default(), None),
                priority,
                props,
                transitions,
                ..Default::default()
            }
        })
        .collect();
    if !main.is_empty() {
        out.push(main);
    }
    out.extend(pseudo_rules);
    out
}

pub fn emit_luau(sheet: &Sheet, opts: &CodegenOptions, diag: &mut Diagnostics) -> String {
    let (sheet_attributes, rules) = lower(sheet, opts, diag);
    let mut out = String::new();
    if let Some(source) = &opts.header {
        let _ =
            writeln!(out, "-- Generated by outlass {} from {source}. Do not edit by hand.", env!("CARGO_PKG_VERSION"));
        out.push('\n');
    }
    out.push_str("local sheet = Instance.new(\"StyleSheet\")\n");
    let _ = writeln!(out, "sheet.Name = {}", luau::string(&opts.sheet_name));
    for (name, value) in &sheet_attributes {
        let _ = writeln!(out, "sheet:SetAttribute({}, {value})", luau::string(name));
    }
    if !rules.is_empty() {
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
    for rule in &rules {
        out.push('\n');
        emit_rule(&mut out, rule, "sheet", 0);
    }
    out.push_str("\nreturn sheet\n");
    out
}

fn emit_rule(out: &mut String, rule: &RobloxRule, parent: &str, depth: usize) {
    let indent = "\t".repeat(depth);
    let priority = rule.priority.map(luau::number).unwrap_or_else(|| "nil".into());
    let props = if rule.props.is_empty() {
        "nil".to_string()
    } else {
        let mut s = String::from("{\n");
        for (k, v) in &rule.props {
            let _ = writeln!(s, "{indent}\t{} = {v},", luau::table_key(k));
        }
        s.push_str(&indent);
        s.push('}');
        s
    };
    let call = format!("rule({parent}, {}, {priority}, {props})", luau::string(&rule.selector));
    let needs_handle = !rule.attributes.is_empty() || !rule.transitions.is_empty() || !rule.children.is_empty();
    if !needs_handle {
        let _ = writeln!(out, "{indent}{call}");
        return;
    }
    let var = format!("r{}", depth + 1);
    let inner = "\t".repeat(depth + 1);
    let _ = writeln!(out, "{indent}do");
    // Re-indent the property table for the extra `do` level.
    let call = call.replace(&format!("\n{indent}"), &format!("\n{inner}"));
    let _ = writeln!(out, "{inner}local {var} = {call}");
    for (name, value) in &rule.attributes {
        let _ = writeln!(out, "{inner}{var}:SetAttribute({}, {value})", luau::string(name));
    }
    let (defaults, specific): (Vec<_>, Vec<_>) = rule.transitions.iter().partition(|(k, _)| k == "*");
    if !specific.is_empty() {
        let _ = writeln!(out, "{inner}{var}:SetPropertyTransitions({{");
        for (k, v) in specific {
            let _ = writeln!(out, "{inner}\t{} = {v},", luau::table_key(k));
        }
        let _ = writeln!(out, "{inner}}})");
    }
    if let Some((_, v)) = defaults.last() {
        let _ = writeln!(out, "{inner}{var}:SetDefaultPropertyTransition({v})");
    }
    for child in &rule.children {
        emit_rule(out, child, &var, depth + 1);
    }
    let _ = writeln!(out, "{indent}end");
}

/// Debug output: the evaluated stylesheet as flat CSS (before Roblox lowering).
pub fn emit_css(sheet: &Sheet) -> String {
    let mut out = String::new();
    if !sheet.tokens.is_empty() {
        out.push_str(":root {\n");
        for t in &sheet.tokens {
            let _ = writeln!(out, "  --{}: {};", t.name, t.value.to_css().unwrap_or_else(|_| t.value.inspect()));
        }
        out.push_str("}\n");
    }
    let mut depth_of: HashMap<usize, usize> = HashMap::new();
    let mut open: Vec<usize> = Vec::new();
    for (idx, rule) in sheet.rules.iter().enumerate() {
        while let Some(&top) = open.last() {
            if Some(top) == rule.parent {
                break;
            }
            open.pop();
            let _ = writeln!(out, "{}}}", "  ".repeat(open.len()));
        }
        let depth = rule.parent.map(|p| depth_of.get(&p).copied().unwrap_or(0) + 1).unwrap_or(0);
        depth_of.insert(idx, depth);
        let pad = "  ".repeat(depth);
        if rule.query {
            let _ = writeln!(out, "{pad}{} {{", rule.selector.to_css());
            open.push(idx);
            continue;
        }
        if rule.selector.0.is_empty() || (rule.decls.is_empty() && rule.tokens.is_empty()) {
            continue;
        }
        let _ = writeln!(out, "{pad}{} {{", rule.selector.to_css());
        for t in &rule.tokens {
            let _ = writeln!(out, "{pad}  --{}: {};", t.name, t.value.to_css().unwrap_or_else(|_| t.value.inspect()));
        }
        for d in &rule.decls {
            let value = d.value.to_css().unwrap_or_else(|_| d.value.inspect());
            let bang = if d.important { " !important" } else { "" };
            let _ = writeln!(out, "{pad}  {}: {value}{bang};", d.name);
        }
        let _ = writeln!(out, "{pad}}}");
    }
    while open.pop().is_some() {
        let _ = writeln!(out, "{}}}", "  ".repeat(open.len()));
    }
    out
}
