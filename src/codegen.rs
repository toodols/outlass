//! Lowers evaluated rules to Roblox StyleRules and emits Luau (or debug CSS).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;

use crate::approx::{self, ApproxOptions, Decl};
use crate::diag::Diagnostics;
use crate::eval::{self, OutDecl, OutRule, Sheet, Syntax, Token};
use crate::fs::MemoryFs;
use crate::luau::{self, Rule};
use crate::query::Target;
use crate::roblox::{self, ValueOptions};
use crate::value::Value;

/// A rule's position in the CSS cascade; greater wins.
#[derive(Clone, PartialEq, PartialOrd)]
struct CascadeKey {
    important: bool,
    layer: Vec<i64>,
    specificity: (u32, u32, u32),
    order: usize,
}

#[derive(Clone)]
pub struct CodegenOptions {
    pub values: ValueOptions,
    pub approx: ApproxOptions,
    /// `StyleSheet.Name`; also used in the header comment.
    pub sheet_name: String,
    /// Source description for the header comment; `None` omits the header.
    pub header: Option<String>,
    /// `--tags`: CollectionService tag (or `#Name`) → the GuiObject classes it's used on.
    pub tags: HashMap<String, Vec<String>>,
    /// Emit the user-agent stylesheet: defaults (RichText, AutomaticSize, transparent backgrounds,
    /// ...) that make fresh Roblox elements start out like CSS boxes, in a StyleSheet the sheet
    /// derives from.
    pub user_agent_styles: bool,
}

fn set<T>(list: &mut Vec<(String, T)>, key: String, value: T) {
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

/// The attributes for tokens. `referenced` names the length tokens a UDim property references
/// (see referenced_tokens), which have to be UDim attributes.
fn attributes(
    tokens: &[Token],
    opts: &ValueOptions,
    referenced: &HashMap<String, Value>,
    diag: &mut Diagnostics,
) -> Vec<(String, luau::Value)> {
    let mut out = Vec::new();
    for token in tokens {
        let name = roblox::attribute_name(&token.name);
        if name != token.name {
            diag.warn(
                format!("token \"--{}\" becomes attribute \"{name}\" (attribute names may only contain letters, digits and _)", token.name),
                Some(&token.span),
            );
        }
        let value = token_value(&token.value, &token.name, diag, &token.span);
        // Roblox reads a number attribute in a UDim property as the offset, but only once: it
        // never sees the token change again (measured in Studio), so a theme couldn't change it.
        // A UDim attribute stays live, and a percentage needs one anyway, for its scale.
        if let Value::Number(n) = &value
            && (n.has_unit("%") || referenced.contains_key(&token.name))
        {
            let udim = if n.has_unit("%") {
                luau::Value::udim(n.value / 100.0, 0.0)
            } else {
                match roblox::number_value(n) {
                    Ok(px) => luau::Value::udim(0.0, px),
                    Err(e) => {
                        diag.warn(format!("token --{}: {e} (ignored)", token.name), Some(&token.span));
                        continue;
                    }
                }
            };
            set(&mut out, name, udim);
            continue;
        }
        match roblox::value(&value, opts) {
            Ok(v) => set(&mut out, name, v),
            Err(e) if roblox::uses_raw_luau(&value) => {
                diag.error(format!("token --{}: {e}", token.name), Some(&token.span))
            }
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
        // So does a CSS function such as `linear-gradient(...)`, which has no attribute type. Its
        // uses are compiled in instead (see inline_tokens).
        Ok(Value::Call { name, .. }) if name.contains('-') => Value::quoted(text.clone()),
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

/// Custom properties whose uses are compiled in rather than referenced. A `var()` becomes a
/// `"$Token"` reference, which Roblox resolves at run time (so themes can change it), but only an
/// opaque color token works that way everywhere outlass translates CSS: a font family, a length or
/// a gradient has to be known at compile time to become a FontFace, a UDim or a UIGradient, and a
/// Color3 attribute has no alpha, so a translucent color's transparency would be lost. So the
/// `:root` value of every other token is substituted into the declarations that use it. A rule
/// that redefines the token compiles those declarations again with its own value (see
/// redefined_token_uses).
fn inline_tokens(sheet: &Sheet) -> HashMap<String, Value> {
    sheet.tokens.iter().filter_map(|t| inline_value(t).map(|v| (t.name.clone(), v))).collect()
}

/// A token's value to compile into the declarations that use it, or `None` for an opaque color,
/// which stays a `"$Name"` reference (see inline_tokens).
fn inline_value(t: &Token) -> Option<Value> {
    let v = token_value(&t.value, &t.name, &mut Diagnostics::default(), &t.span);
    let v = match &v {
        // Kept as text for the attribute; the declaration needs the value itself.
        Value::Str { text, quoted: true } => crate::eval::evaluate_expression(text).unwrap_or(v),
        _ => v,
    };
    (!matches!(&v, Value::Color(c) if c.a >= 1.0)).then_some(v)
}

/// The tokens rule `index`'s CSS declarations can compile in: the `:root` ones, overridden by
/// those set on weaker rules for the same elements and on the rule itself, as CSS would cascade
/// them.
fn rule_tokens(sheet: &Sheet, index: usize, root: &HashMap<String, Value>) -> HashMap<String, Value> {
    let mut tokens = root.clone();
    for i in weaker_rules(sheet, index).into_iter().chain([index]) {
        for t in &sheet.rules[i].tokens {
            match inline_value(t) {
                Some(v) => tokens.insert(t.name.clone(), v),
                None => tokens.remove(&t.name),
            };
        }
    }
    tokens
}

/// The declarations of weaker rules for the same elements that use a token this rule sets. CSS
/// resolves `var()` for each element, so `.a.b { --gap: 10px }` changes the padding that
/// `.a { padding: var(--gap) }` gives it; a compiled-in value has to be compiled again for `.a.b`.
fn redefined_token_uses(sheet: &Sheet, index: usize, tokens: &HashMap<String, Value>) -> Vec<OutDecl> {
    fn uses(v: &Value, names: &[&str]) -> bool {
        match v {
            Value::Call { name, args } if name == "var" => {
                args.first()
                    .and_then(|a| a.as_str())
                    .and_then(|a| a.strip_prefix("--"))
                    .is_some_and(|t| names.contains(&t))
                    || args.iter().skip(1).any(|a| uses(a, names))
            }
            Value::Call { args, .. } => args.iter().any(|a| uses(a, names)),
            Value::List { items, .. } => items.iter().any(|i| uses(i, names)),
            _ => false,
        }
    }
    // A `"$Name"` reference only sees the tokens of the rule it's in, so references are re-emitted
    // too: the redefining rule's copy resolves against its own attribute.
    let _ = tokens;
    let names: Vec<&str> = sheet.rules[index].tokens.iter().map(|t| t.name.as_str()).collect();
    if names.is_empty() {
        return Vec::new();
    }
    weaker_rules(sheet, index)
        .into_iter()
        .flat_map(|i| sheet.rules[i].decls.iter())
        .filter(|d| uses(&d.value, &names))
        .cloned()
        .collect()
}

/// The font family an interface's root gives all its text (`ScreenGui { font: 14px Gotham }`, as a
/// page's body does), if a rule for a root names one.
fn root_family(sheet: &Sheet) -> Option<String> {
    use crate::selector::{Part, Simple};
    const ROOTS: &[&str] = &["ScreenGui", "BillboardGui", "SurfaceGui", "LayerCollector"];
    let is_root = |rule: &OutRule| {
        !rule.query
            && rule.parent.is_none()
            && !rule.selector.0.is_empty()
            && rule.selector.0.iter().all(|complex| {
                matches!(complex.as_slice(), [Part::Compound(c)] if matches!(c.as_slice(), [Simple::Type(t)] if ROOTS.contains(&t.as_str())))
            })
    };
    sheet
        .rules
        .iter()
        .rev()
        .filter(|r| is_root(r))
        .find_map(|r| approx::declared_family(&r.decls.iter().map(to_decl).collect::<Vec<_>>()))
}

/// The CSS properties whose length tokens stay `"$Name"` references (see ApproxOptions::tokens).
const TOKEN_PROPERTIES: &[&str] = &[
    "border-radius",
    "gap",
    "row-gap",
    "column-gap",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "padding-inline",
    "padding-block",
];

/// Whether a token's value is a length a UDim can hold.
fn is_length_token(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_unitless() || n.has_unit("%") || n.has_unit("em") || n.has_unit("rem") || n.value_in("px").is_some())
}

/// The stylesheet's length tokens (and the ones only a theme sets) that a `border-radius`,
/// `padding` or `gap` declaration, or a UDim property, uses, by name, with their values.
fn referenced_tokens(sheet: &Sheet) -> HashMap<String, Value> {
    fn uses(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Call { name, args } if name == "var" => {
                if let Some(t) = args.first().and_then(|a| a.as_str()).and_then(|a| a.strip_prefix("--")) {
                    out.push(t.to_string());
                }
            }
            Value::List { items, .. } => items.iter().for_each(|i| uses(i, out)),
            _ => {}
        }
    }
    // Roblox UDim properties written out reference a token the same way.
    const UDIM_PROPERTIES: &[&str] =
        &["CornerRadius", "Padding", "PaddingTop", "PaddingRight", "PaddingBottom", "PaddingLeft"];
    let mut used = Vec::new();
    for decl in sheet.rules.iter().flat_map(|r| r.decls.iter()) {
        if TOKEN_PROPERTIES.contains(&decl.name.as_str()) || UDIM_PROPERTIES.contains(&decl.name.as_str()) {
            uses(&decl.value, &mut used);
        }
    }
    let mut out = HashMap::new();
    for t in sheet.tokens.iter().chain(sheet.themes.iter().flat_map(|(_, ts)| ts.iter())) {
        if used.contains(&t.name)
            && let Some(v) = inline_value(t)
            && is_length_token(&v)
        {
            out.entry(t.name.clone()).or_insert(v);
        }
    }
    out
}

/// A CSS declaration's value with its tokens compiled in, except the length tokens in `keep`,
/// which `border-radius`, `padding` and `gap` reference by name.
fn resolve_css(name: &str, v: &Value, tokens: &HashMap<String, Value>, keep: &[String]) -> Value {
    if keep.is_empty() || !TOKEN_PROPERTIES.contains(&name) {
        return inline_vars(v, tokens);
    }
    let compiled: HashMap<String, Value> =
        tokens.iter().filter(|(k, _)| !keep.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    inline_vars(v, &compiled)
}

/// The referenced tokens a rule can keep as `"$Name"`: not ones a weaker rule for the same elements
/// sets, which the reference couldn't see (it only sees the sheet's tokens and its own rule's).
fn kept_tokens(sheet: &Sheet, index: usize, referenced: &HashMap<String, Value>) -> Vec<String> {
    let shadowed: Vec<&str> = weaker_rules(sheet, index)
        .into_iter()
        .flat_map(|i| sheet.rules[i].tokens.iter().map(|t| t.name.as_str()))
        .collect();
    referenced.keys().filter(|k| !shadowed.contains(&k.as_str())).cloned().collect()
}

/// A theme changes a token at run time only where Roblox looks it up: a colour, or a length that
/// `border-radius`, `padding` or `gap` uses whole. Anywhere else the token is compiled in with the
/// default theme's value, which deserves a warning.
fn warn_compiled_theme_tokens(sheet: &Sheet, referenced: &HashMap<String, Value>, diag: &mut Diagnostics) {
    fn vars(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Call { name, args } if name == "var" => {
                if let Some(t) = args.first().and_then(|a| a.as_str()).and_then(|a| a.strip_prefix("--")) {
                    out.push(t.to_string());
                }
                args.iter().skip(1).for_each(|a| vars(a, out));
            }
            Value::Call { args, .. } => args.iter().for_each(|a| vars(a, out)),
            Value::List { items, .. } => items.iter().for_each(|i| vars(i, out)),
            _ => {}
        }
    }
    let themed: Vec<&Token> = sheet.themes.iter().flat_map(|(_, ts)| ts.iter()).collect();
    let mut warned: Vec<String> = Vec::new();
    for decl in sheet.rules.iter().flat_map(|r| r.decls.iter()).filter(|d| is_css_property(&d.name)) {
        let mut used = Vec::new();
        vars(&decl.value, &mut used);
        for name in used {
            let Some(token) = themed.iter().find(|t| t.name == name) else { continue };
            let looked_up = inline_value(token).is_none()
                || (TOKEN_PROPERTIES.contains(&decl.name.as_str()) && referenced.contains_key(&name));
            if looked_up || warned.contains(&name) {
                continue;
            }
            diag.warn(
                format!(
                    "`--{name}` changes with the theme, but `{}` compiles it in, so it keeps the default theme's value",
                    decl.name
                ),
                Some(&decl.span),
            );
            warned.push(name);
        }
    }
}

/// `@font-face` names in `font-family` and `font` replaced by the font assets they stand for.
fn with_font_faces(sheet: &Sheet) -> Sheet {
    fn substitute(v: &Value, faces: &[(String, String)]) -> Value {
        match v {
            Value::Str { text, .. } => match faces.iter().rev().find(|(name, _)| name.eq_ignore_ascii_case(text)) {
                Some((_, asset)) => Value::quoted(asset.clone()),
                None => v.clone(),
            },
            Value::List { items, sep, bracketed } => Value::List {
                items: items.iter().map(|i| substitute(i, faces)).collect(),
                sep: *sep,
                bracketed: *bracketed,
            },
            _ => v.clone(),
        }
    }
    let mut out = sheet.clone();
    for decl in out.rules.iter_mut().flat_map(|r| r.decls.iter_mut()) {
        if decl.name == "font-family" || decl.name == "font" {
            decl.value = substitute(&decl.value, &sheet.font_faces);
        }
    }
    out
}

/// Substitutes the inlined tokens' values for their `var()` references, anywhere in `v`.
fn inline_vars(v: &Value, tokens: &HashMap<String, Value>) -> Value {
    if tokens.is_empty() {
        return v.clone();
    }
    match v {
        Value::Call { name, args } if name == "var" => {
            let token = args.first().and_then(|a| a.as_str()).and_then(|a| a.strip_prefix("--"));
            match token.and_then(|t| tokens.get(t)) {
                Some(value) => value.clone(),
                // `var(--x, fallback)` for an unknown token falls back, as in CSS.
                None if token.is_some_and(|t| !tokens.contains_key(t)) && args.len() > 1 => {
                    inline_vars(&args[1], tokens)
                }
                None => v.clone(),
            }
        }
        Value::Call { name, args } => {
            Value::Call { name: name.clone(), args: args.iter().map(|a| inline_vars(a, tokens)).collect() }
        }
        Value::List { items, sep, bracketed } => Value::List {
            items: items.iter().map(|i| inline_vars(i, tokens)).collect(),
            sep: *sep,
            bracketed: *bracketed,
        },
        _ => v.clone(),
    }
}

/// `--strict`: a percentage size needs a parent whose size on that axis is known. Under a parent
/// sized by its content, Roblox resolves it to 0, or inflates the parent in a feedback loop when
/// that parent grows along a flex line; CSS would treat it as `auto`. A stylesheet only knows an
/// element's parent when a child combinator names it (`.card > .bar`), so that's what's checked,
/// against the rules written for exactly that parent selector.
fn check_percentages_under_content(sheet: &Sheet, diag: &mut Diagnostics) {
    use crate::selector::{Combinator, Part, SelectorList};
    let is_percent = |v: &Value| match v {
        Value::Number(n) => n.has_unit("%"),
        Value::Call { name, .. } if name == "calc" => v.inspect().contains('%'),
        _ => false,
    };
    for rule in sheet.rules.iter().filter(|r| !r.query) {
        for (axis, edges) in [("width", ["left", "right"]), ("height", ["top", "bottom"])] {
            let Some(decl) = rule.decls.iter().rev().find(|d| d.name == axis) else { continue };
            if !is_percent(&decl.value) {
                continue;
            }
            for complex in &rule.selector.0 {
                let Some(at) = complex.iter().rposition(|p| matches!(p, Part::Comb(Combinator::Child))) else {
                    continue;
                };
                let parent = SelectorList(vec![complex[..at].to_vec()]);
                let parent_rules: Vec<&OutRule> =
                    sheet.rules.iter().filter(|r| !r.query && r.selector.0.iter().any(|c| *c == parent.0[0])).collect();
                if parent_rules.is_empty() {
                    continue;
                }
                let decls = || parent_rules.iter().flat_map(|r| r.decls.iter());
                let sized = decls().rfind(|d| d.name == axis).is_some_and(|d| {
                    !d.value.as_str().is_some_and(|s| {
                        matches!(
                            s.to_ascii_lowercase().as_str(),
                            "auto" | "fit-content" | "max-content" | "min-content"
                        )
                    })
                });
                let stretched =
                    edges.iter().all(|e| decls().any(|d| d.name == *e)) || decls().any(|d| d.name == "inset");
                if !sized && !stretched {
                    diag.error(
                        format!(
                            "strict: `{axis}: {}` is a percentage of `{}`, which is sized by its content; Roblox resolves \
                             it to 0 (or inflates the parent) where CSS treats it as `auto`; give the parent a `{axis}`",
                            decl.value.to_css().unwrap_or_default(),
                            parent.to_css()
                        ),
                        Some(&decl.span),
                    );
                }
            }
        }
    }
}

/// `--strict`: a `1fr` track is a share of the grid's own size. A grid sized by its content has
/// none to share: CSS then sizes the tracks from their contents, while Roblox's cells are a
/// fraction of whatever the grid ends up as. The grid's size may come from any rule for the same
/// elements (one with the same selector, or a weaker one that matches them all).
fn check_fractional_grids(sheet: &Sheet, diag: &mut Diagnostics) {
    let is_fractional = |v: &Value| {
        let text = v.to_css().unwrap_or_else(|_| v.inspect());
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
            .any(|t| t.strip_suffix("fr").is_some_and(|n| !n.is_empty() && n.parse::<f64>().is_ok()))
    };
    let is_length = |v: &Value| match v {
        Value::Number(_) => true,
        Value::Call { name, .. } => name == "calc",
        _ => false,
    };
    for rule in sheet.rules.iter().filter(|r| !r.query) {
        let relevant: Vec<&OutRule> = sheet
            .rules
            .iter()
            .filter(|o| {
                !o.query
                    && o.parent == rule.parent
                    && (o.selector == rule.selector || o.selector.covers(&rule.selector))
            })
            .collect();
        let decls = || relevant.iter().flat_map(|r| r.decls.iter());
        for (tracks, axis, edges) in
            [("grid-template-columns", "width", ["left", "right"]), ("grid-template-rows", "height", ["top", "bottom"])]
        {
            let Some(d) = rule.decls.iter().rev().find(|d| d.name == tracks) else { continue };
            if !is_fractional(&d.value) {
                continue;
            }
            let sized = decls().any(|d| d.name == axis && is_length(&d.value))
                || edges.iter().all(|e| decls().any(|d| d.name == *e))
                || decls().any(|d| d.name == "inset");
            if !sized {
                diag.error(
                    format!(
                        "strict: `fr` tracks share out the grid's {axis}, but this grid is sized by its content, where \
                         CSS sizes the tracks from their contents and Roblox can't; give the grid a `{axis}` \
                         (e.g. `100%`)"
                    ),
                    Some(&d.span),
                );
            }
        }
    }
}

/// The declarations that apply to rule `index`'s elements from weaker rules matching all of them
/// (`.bar` for `.bar.left`), in cascade order, keeping the properties `keep` names.
fn weaker_decls(sheet: &Sheet, index: usize, keep: impl Fn(&str) -> bool) -> Vec<Decl> {
    weaker_rules(sheet, index)
        .into_iter()
        .flat_map(|i| sheet.rules[i].decls.iter())
        .filter(|d| keep(&d.name))
        .map(to_decl)
        .collect()
}

/// The rules that apply to all of rule `index`'s elements and lose to it in the cascade, weakest
/// first (`.bar` for `.bar.left`).
fn weaker_rules(sheet: &Sheet, index: usize) -> Vec<usize> {
    let rule = &sheet.rules[index];
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
    // Cascade order: lower specificity first, then source order.
    weaker.sort_by_key(|(i, other)| (other.selector.specificity(), *i));
    weaker.into_iter().map(|(i, _)| i).collect()
}

fn to_decl(d: &crate::eval::OutDecl) -> Decl {
    Decl { name: d.name.clone(), value: d.value.clone(), span: Some(d.span.clone()) }
}

/// CSS flex items shrink to fit their line (`flex-shrink: 1` is the initial value); a Roblox item
/// only does with a UIFlexItem, whose Shrink mode splits the overflow by size as CSS does
/// (measured in Studio). A flex row gives its children one through `> GuiObject::UIFlexItem`,
/// below every author rule: an item's own `flex`/`flex-grow`/`flex-shrink` override it property by
/// property, and so keep the shrink unless they set one. A rule that turns a weaker rule's row
/// into something else turns the shrink back off.
fn flex_shrink_defaults(sheet: &Sheet, priorities: &HashMap<(usize, bool), Option<f64>>) -> Vec<Rule> {
    const NAMES: &[&str] = &["display", "flex-direction", "overflow", "overflow-x", "overflow-y"];
    let top = priorities.values().filter_map(|p| *p).fold(0.0, f64::max);
    let mut out = Vec::new();
    for (idx, rule) in sheet.rules.iter().enumerate() {
        if rule.query || rule.selector.0.is_empty() || !rule.decls.iter().any(|d| NAMES.contains(&d.name.as_str())) {
            continue;
        }
        let weaker = weaker_decls(sheet, idx, |n| NAMES.contains(&n));
        let mut all = weaker.clone();
        all.extend(rule.decls.iter().filter(|d| NAMES.contains(&d.name.as_str())).map(to_decl));
        let props = if approx::shrinks_items(&all) {
            vec![
                ("FlexMode".to_string(), luau::Value::enum_item("UIFlexMode", "Shrink")),
                ("ShrinkRatio".to_string(), luau::Value::Number(1.0)),
            ]
        } else if approx::shrinks_items(&weaker) {
            vec![
                ("FlexMode".to_string(), luau::Value::enum_item("UIFlexMode", "None")),
                ("ShrinkRatio".to_string(), luau::Value::Number(0.0)),
            ]
        } else {
            continue;
        };
        // Author rules are ranked from 1 up: these take the same order below them.
        let priority =
            priorities.get(&(idx, false)).or(priorities.get(&(idx, true))).copied().flatten().map(|p| p - top - 1.0);
        let selector = split_selector_list(&rule.selector.to_roblox(&mut Diagnostics::default(), None))
            .iter()
            .map(|s| format!("{s} > GuiObject::UIFlexItem"))
            .collect::<Vec<_>>()
            .join(", ");
        let prefixes: Vec<Option<String>> = match rule.parent {
            Some(container) => sheet.rules[container].queries.iter().map(|q| Some(q.name())).collect(),
            None => vec![None],
        };
        for prefix in prefixes {
            let selector = match prefix {
                Some(name) => prefix_selector(&selector, &format!("@{name} ")),
                None => selector.clone(),
            };
            out.push(Rule { selector, priority, props: props.clone(), ..Default::default() });
        }
    }
    out
}

/// `--strict`: an element with no `width` fills a block parent in CSS, but fits its content in
/// Roblox. As with percentages, a stylesheet only knows the parent through a child combinator.
fn check_block_children_without_width(sheet: &Sheet, diag: &mut Diagnostics) {
    use crate::selector::{Combinator, Part, SelectorList};
    const NAMES: &[&str] = &["width", "position", "display", "left", "right", "inset"];
    for (idx, rule) in sheet.rules.iter().enumerate() {
        if rule.query || !rule.decls.iter().any(|d| is_css_property(&d.name)) {
            continue;
        }
        for complex in &rule.selector.0 {
            let Some(at) = complex.iter().rposition(|p| matches!(p, Part::Comb(Combinator::Child))) else {
                continue;
            };
            let parent = SelectorList(vec![complex[..at].to_vec()]);
            let parent_rules: Vec<&OutRule> =
                sheet.rules.iter().filter(|r| !r.query && r.selector.0.iter().any(|c| *c == parent.0[0])).collect();
            let parent_display: Vec<Decl> =
                parent_rules.iter().flat_map(|r| r.decls.iter().filter(|d| d.name == "display").map(to_decl)).collect();
            if parent_rules.is_empty() || approx::is_flex_or_grid(&parent_display) {
                continue;
            }
            let mut decls = weaker_decls(sheet, idx, |n| NAMES.contains(&n));
            decls.extend(rule.decls.iter().filter(|d| NAMES.contains(&d.name.as_str())).map(to_decl));
            let value = |name: &str| {
                decls
                    .iter()
                    .rfind(|d| d.name == name)
                    .map(|d| d.value.to_css().unwrap_or_default().to_ascii_lowercase())
            };
            let sized = value("width").is_some()
                || value("inset").is_some()
                || (value("left").is_some() && value("right").is_some());
            let out_of_flow = value("position").is_some_and(|p| p == "absolute" || p == "fixed");
            let inline = value("display").is_some_and(|d| d.starts_with("inline") || d == "none");
            if sized || out_of_flow || inline {
                continue;
            }
            diag.error(
                format!(
                    "strict: `{}` has no `width`, so it fills its block parent `{}` in CSS but fits its content in \
                     Roblox; give it a `width` (`100%` to fill)",
                    rule.selector.to_css(),
                    parent.to_css()
                ),
                Some(&rule.span),
            );
        }
    }
}

/// Converts evaluated rules into the Roblox StyleRule tree.
pub fn lower(sheet: &Sheet, opts: &CodegenOptions, diag: &mut Diagnostics) -> luau::Sheet {
    let substituted;
    let sheet = if sheet.font_faces.is_empty() {
        sheet
    } else {
        substituted = with_font_faces(sheet);
        &substituted
    };
    let referenced = referenced_tokens(sheet);
    let opts = &CodegenOptions {
        approx: ApproxOptions {
            tokens: referenced.clone(),
            inherited_family: root_family(sheet),
            ..opts.approx.clone()
        },
        ..opts.clone()
    };
    let (sheet_attributes, themes) = sheet_and_theme_attributes(sheet, opts, diag);
    warn_compiled_theme_tokens(sheet, &referenced, diag);

    let mut rules: Vec<Rule> = Vec::new();
    let hides_anything = sheet.rules.iter().any(|r| r.decls.iter().any(hides));
    let parts = cascade_parts(sheet);
    let mut priorities = cascade_priorities(sheet, &parts);
    let root_tokens = inline_tokens(sheet);
    // The user-agent sheet's rules match every element directly, so nothing needs passing down.
    let inherited_text =
        if opts.approx.user_agent { Vec::new() } else { inherited_text_rules(sheet, &priorities, &root_tokens, opts) };
    // Inherited values rank below every author rule.
    let offset = inherited_text.len() as f64;
    for p in priorities.values_mut() {
        *p = p.map(|p| p + offset);
    }
    if opts.approx.strict && opts.approx.groups.contains(&approx::Group::Size) {
        check_percentages_under_content(sheet, diag);
    }
    if opts.approx.strict && opts.approx.groups.contains(&approx::Group::Layout) {
        check_fractional_grids(sheet, diag);
    }
    if opts.approx.strict && opts.approx.groups.contains(&approx::Group::Size) {
        check_block_children_without_width(sheet, diag);
    }

    for (idx, rule) in sheet.rules.iter().enumerate() {
        if rule.selector.0.is_empty() || rule.query {
            continue;
        }
        // Rules inside @media / @container / @Query get the query's `@Name` prefix, once per alternative.
        let prefixes: Vec<Option<String>> = match rule.parent {
            Some(container) => sheet.rules[container].queries.iter().map(|q| Some(q.name())).collect(),
            None => vec![None],
        };
        let tokens = rule_tokens(sheet, idx, &root_tokens);
        let keep = kept_tokens(sheet, idx, &referenced);
        if let Some(base) = rule.selector.strip_pseudo_element("placeholder") {
            let priority = priorities.get(&(idx, false)).or(priorities.get(&(idx, true))).copied().flatten();
            let selector = base.to_roblox(diag, Some(&rule.span));
            let props = placeholder_props(rule, &tokens, opts, diag);
            for prefix in &prefixes {
                let selector = match prefix {
                    Some(name) => prefix_selector(&selector, &format!("@{name} ")),
                    None => selector.clone(),
                };
                rules.push(Rule { selector, priority, props: props.clone(), ..Default::default() });
            }
            continue;
        }
        let selector = rule.selector.to_roblox(diag, Some(&rule.span));
        for (part, important) in &parts[idx] {
            let priority = priorities.get(&(idx, *important)).copied().flatten();
            let mut part = part.clone();
            if !important {
                part.decls.splice(0..0, redefined_token_uses(sheet, idx, &tokens));
            }
            let part = &part;
            let inherited = inherited_decls(sheet, idx, &part.decls);
            let mut lowered = lower_rule(part, &inherited, &tokens, &keep, selector.clone(), priority, opts, diag);
            if let Some(classes) = rule.selector.subject_classes(&opts.tags)
                && let Some(main) = lowered.iter_mut().find(|r| !r.selector.contains("::"))
            {
                drop_missing_properties(main, part, &classes, opts, diag);
            }
            if !hides_anything {
                drop_generated_visible(part, &mut lowered);
            }
            for prefix in &prefixes {
                for r in &lowered {
                    let selector = match prefix {
                        Some(name) => prefix_selector(&r.selector, &format!("@{name} ")),
                        None => r.selector.clone(),
                    };
                    rules.push(Rule {
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
    rules.splice(0..0, inherited_text);
    let definitions = query_definitions(sheet, diag);
    rules.splice(0..0, definitions);

    if opts.approx.groups.contains(&approx::Group::Layout) {
        rules.splice(0..0, flex_shrink_defaults(sheet, &priorities));
    }
    let user_agent = if opts.user_agent_styles { user_agent_rules(opts) } else { Vec::new() };
    luau::Sheet { name: opts.sheet_name.clone(), attributes: sheet_attributes, rules, themes, user_agent }
}

/// The user-agent stylesheet, `user-agent.scss`: defaults below every author rule, like a
/// browser's. Any rule that sets the same property overrides them (e.g. `RichText: false`), since
/// these live in a sheet the author's derives from. It's compiled like any stylesheet, with
/// `$approx-<group>` set for each approximation group.
pub const USER_AGENT_SCSS: &str = include_str!("user-agent.scss");

fn user_agent_rules(opts: &CodegenOptions) -> Vec<Rule> {
    let defines = approx::Group::concrete()
        .iter()
        .map(|g| (format!("approx-{}", g.name()), Value::Bool(opts.approx.groups.contains(g))))
        .collect();
    let eval_opts = eval::Options { defines, fs: Rc::new(MemoryFs::new()), ..eval::Options::default() };
    let mut diag = Diagnostics::default();
    let sheet =
        eval::compile_source(USER_AGENT_SCSS, Path::new("user-agent.scss"), Syntax::Scss, &eval_opts, &mut diag)
            .unwrap_or_else(|e| panic!("user-agent.scss doesn't compile: {e}"));
    let ua_opts = CodegenOptions {
        approx: ApproxOptions { user_agent: true, strict: false, ..opts.approx.clone() },
        user_agent_styles: false,
        header: None,
        tags: HashMap::new(),
        ..opts.clone()
    };
    let rules = lower(&sheet, &ua_opts, &mut diag).rules;
    debug_assert!(diag.items.is_empty(), "user-agent.scss: {:?}", diag.items);
    rules
}

/// The stylesheet's own attributes, and its themes. A sheet's own attribute beats the one on the
/// theme it derives from (measured in Studio), so a themed token lives only on the themes: the
/// default theme gets its `:root` value and every other theme starts from those.
fn sheet_and_theme_attributes(
    sheet: &Sheet,
    opts: &CodegenOptions,
    diag: &mut Diagnostics,
) -> (Vec<(String, luau::Value)>, Vec<luau::Theme>) {
    let base = attributes(&sheet.tokens, &opts.values, &opts.approx.tokens, diag);
    if sheet.themes.is_empty() {
        return (base, Vec::new());
    }
    let themed: Vec<String> =
        sheet.themes.iter().flat_map(|(_, ts)| ts.iter().map(|t| roblox::attribute_name(&t.name))).collect();
    let (default, own): (Vec<_>, Vec<_>) = base.into_iter().partition(|(name, _)| themed.contains(name));
    let mut themes = vec![luau::Theme { name: "default".into(), attributes: default }];
    for (name, tokens) in &sheet.themes {
        let overrides = attributes(tokens, &opts.values, &opts.approx.tokens, diag);
        let index = match themes.iter().position(|t| t.name == *name) {
            Some(i) => i,
            None => {
                themes.push(luau::Theme { name: name.clone(), attributes: themes[0].attributes.clone() });
                themes.len() - 1
            }
        };
        for (k, v) in overrides {
            set(&mut themes[index].attributes, k, v);
        }
    }
    (own, themes)
}

/// The GuiObject classes a stylesheet can select, used for the user-agent defaults. Every one of
/// them starts out 0x0.
pub const GUI_OBJECT_CLASSES: &[&str] = &[
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
fn query_definitions(sheet: &Sheet, diag: &mut Diagnostics) -> Vec<Rule> {
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
            out.push(Rule { selector, props: q.conditions(), ..Default::default() });
        }
    }
    out
}

/// Splits each rule into the parts that take part in the cascade: its normal declarations (with
/// its tokens) and its `!important` declarations as a separate part.
fn cascade_parts(sheet: &Sheet) -> Vec<Vec<(OutRule, bool)>> {
    sheet
        .rules
        .iter()
        .map(|rule| {
            if rule.query || rule.selector.0.is_empty() || (rule.decls.is_empty() && rule.tokens.is_empty()) {
                return Vec::new();
            }
            if !rule.decls.iter().any(|d| d.important) {
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

/// StyleRule.Priority for each (rule index, important) part: its rank (1, 2, ...) in cascade order,
/// so the rule CSS would apply always has the higher priority.
fn cascade_priorities(sheet: &Sheet, parts: &[Vec<(OutRule, bool)>]) -> HashMap<(usize, bool), Option<f64>> {
    let mut keys: Vec<((usize, bool), CascadeKey)> = parts
        .iter()
        .enumerate()
        .flat_map(|(idx, p)| {
            p.iter().map(move |(rule, important)| {
                let key = CascadeKey {
                    important: *important,
                    layer: layer_key(sheet, &rule.layer, *important),
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

/// Where a rule's `@layer` sits in the cascade, compared level by level: each level ranks its
/// layers in the order they were first named, and the level's own rules (outside any of those
/// layers) after all of them, as in CSS. `!important` declarations reverse that order.
fn layer_key(sheet: &Sheet, layer: &[String], important: bool) -> Vec<i64> {
    let mut key: Vec<i64> = (1..=layer.len())
        .map(|n| {
            let siblings = sheet.layers.iter().filter(|l| l.len() == n && l[..n - 1] == layer[..n - 1]);
            siblings.into_iter().position(|l| l[..] == layer[..n]).unwrap_or(0) as i64
        })
        .collect();
    key.push(i64::MAX);
    if important {
        key.iter_mut().for_each(|k| *k = -*k);
    }
    key
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
fn drop_generated_visible(rule: &OutRule, lowered: &mut Vec<Rule>) {
    if rule.decls.iter().any(|d| d.name == "Visible") {
        return;
    }
    for r in lowered.iter_mut().filter(|r| !r.selector.contains("::")) {
        r.props.retain(|(k, v)| !(k == "Visible" && *v == luau::Value::Bool(true)));
    }
    lowered.retain(|r| !r.is_empty());
}

/// `border: none` disables the element's `::UIStroke`, which creates a (disabled) UIStroke on every
/// matched element. That only matters if some rule turns a stroke on; otherwise drop those rules.
fn drop_unneeded_stroke_resets(rules: &mut Vec<Rule>) {
    fn is_stroke_reset(rule: &Rule) -> bool {
        rule.selector.contains("::UIStroke")
            && rule.transitions.is_empty()
            && rule.children.is_empty()
            && rule.props.iter().all(|(k, v)| k == "Enabled" && *v == luau::Value::Bool(false))
    }
    fn any_real_stroke(rules: &[Rule]) -> bool {
        rules.iter().any(|r| (r.selector.contains("::UIStroke") && !is_stroke_reset(r)) || any_real_stroke(&r.children))
    }
    fn remove(rules: &mut Vec<Rule>) {
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
    // Position, Size, AnchorPoint, AutomaticSize, Rotation, and UISizeConstraint (which also caps
    // an explicit size, unless the element grows along a flex line)
    &[
        "position",
        "left",
        "top",
        "right",
        "bottom",
        "inset",
        "width",
        "height",
        "transform",
        "translate",
        "rotate",
        "scale",
        "margin",
        "margin-top",
        "margin-right",
        "margin-bottom",
        "margin-left",
        "margin-inline",
        "margin-block",
        "min-width",
        "min-height",
        "max-width",
        "max-height",
        "flex",
        "flex-grow",
        "box-sizing",
        "contain",
        // a scroll container's own size can't be capped (see translate_size)
        "overflow",
        "overflow-x",
        "overflow-y",
    ],
    // FontFace, and UIPadding, which holds the half-leading of the text's line-height as well as
    // the padding
    &[
        "font",
        "font-family",
        "font-weight",
        "font-style",
        "padding",
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
        "padding-inline",
        "padding-block",
        "line-height",
        "font-size",
        // A CSS border takes up room inside the box, which the padding makes (see border_inset)
        "border",
        "border-width",
        "border-style",
        // whether the padding is inside the size (checked under --strict)
        "box-sizing",
    ],
    // UIListLayout axes depend on the direction; `align-content` depends on whether it's a container
    &[
        "display",
        "flex-direction",
        "flex-wrap",
        "justify-content",
        "align-items",
        "align-content",
        "gap",
        "row-gap",
        "column-gap",
    ],
    // transparency combines color alpha, opacity, gradients and masks
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
        // multiplying puts the background color on the picture
        "background-blend-mode",
        "border-image",
        "border-image-source",
        // a gradient's rotation depends on the element's shape
        "width",
        "height",
    ],
    // Image, ScaleType and the slices
    &[
        "object-fit",
        "background-size",
        "background-repeat",
        "border-image",
        "border-image-source",
        "border-image-slice",
        "border-image-width",
    ],
    // ScrollingDirection / AutomaticCanvasSize cover both axes
    &["overflow", "overflow-x", "overflow-y"],
    // transition longhands combine by index
    &["transition", "transition-property", "transition-duration", "transition-timing-function", "transition-delay"],
];

/// The composite groups a property belongs to. Most belong to one; a few feed two Roblox
/// properties that cascade separately (a border is a UIStroke and room inside the UIPadding).
fn composite_groups(name: &str) -> Vec<&'static [&'static str]> {
    let bare = name.strip_prefix("-webkit-").unwrap_or(name);
    COMPOSITE_GROUPS.iter().copied().filter(|group| group.contains(&bare)).collect()
}

/// Per-axis cascade for composite properties. When a rule sets part of a composite group (say
/// `left`), the parts it doesn't set (`top`) still apply to its elements from weaker rules that
/// match all of them (`.bar` for `.bar.left`), exactly as CSS would cascade them. Those declarations
/// are returned so the rule's Roblox `Position` can be computed from all of them.
fn inherited_decls(sheet: &Sheet, index: usize, decls: &[OutDecl]) -> Vec<Decl> {
    let groups: Vec<&[&str]> = {
        let mut groups: Vec<&[&str]> = Vec::new();
        for decl in decls.iter().filter(|d| is_css_property(&d.name)) {
            for group in composite_groups(&decl.name) {
                if !groups.iter().any(|g| std::ptr::eq(*g, group)) {
                    groups.push(group);
                }
            }
        }
        groups
    };
    if groups.is_empty() {
        return Vec::new();
    }
    weaker_decls(sheet, index, |name| {
        is_css_property(name) && composite_groups(name).iter().any(|g| groups.iter().any(|x| std::ptr::eq(*x, *g)))
    })
}

/// Keeps only the properties (and pseudo-instance properties and transitions) that `keys` has.
fn restrict_to(mut full: approx::Translated, keys: &approx::Translated) -> approx::Translated {
    fn has<T>(list: &[(String, T)], key: &str) -> bool {
        list.iter().any(|(k, _)| k == key)
    }
    fn pseudo_has<T>(list: &[(String, Vec<(String, T)>)], class: &str, key: &str) -> bool {
        list.iter().any(|(c, props)| c == class && has(props, key))
    }
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

/// `::placeholder { color: gray }`: a TextBox's placeholder text, which Roblox colors with
/// PlaceholderColor3.
fn placeholder_props(
    rule: &OutRule,
    tokens: &HashMap<String, Value>,
    opts: &CodegenOptions,
    diag: &mut Diagnostics,
) -> Vec<(String, luau::Value)> {
    let mut props = Vec::new();
    for d in &rule.decls {
        let name = if d.name == "color" { "PlaceholderColor3" } else { d.name.as_str() };
        if is_css_property(name) {
            diag.warn(format!("`{}` has no Roblox equivalent on a placeholder (ignored)", d.name), Some(&d.span));
            continue;
        }
        match roblox::value(&inline_vars(&d.value, tokens), &opts.values) {
            Ok(v) => set(&mut props, name.to_string(), v),
            Err(e) => diag.warn(format!("{}: {e} (ignored)", d.name), Some(&d.span)),
        }
    }
    props
}

/// CSS properties an element's descendants inherit, among those outlass translates to text
/// properties.
const INHERITED: &[&str] = &[
    "color",
    "text-fill-color",
    "font",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "line-height",
    "text-align",
    "white-space",
    "text-wrap",
];

/// The text properties inherited declarations become.
const INHERITED_TEXT_PROPERTIES: &[&str] =
    &["TextColor3", "TextTransparency", "FontFace", "TextSize", "LineHeight", "TextXAlignment", "TextWrapped"];

fn is_inherited(name: &str) -> bool {
    INHERITED.contains(&name.strip_prefix("-webkit-").unwrap_or(name))
}

/// CSS text properties are inherited: `.card { color: white }` colors the text of everything
/// inside the card. Roblox styles each element on its own, so a rule that sets them also gets a
/// rule for the text elements under it (`.card >> TextLabel`). Inherited values lose to every
/// rule that styles the text element itself, so these rank between the user-agent defaults and
/// the author rules. CSS takes the nearest ancestor's value, which a selector can't express; of
/// two ancestors' rules, the stronger one wins.
fn inherited_text_rules(
    sheet: &Sheet,
    priorities: &HashMap<(usize, bool), Option<f64>>,
    root_tokens: &HashMap<String, Value>,
    opts: &CodegenOptions,
) -> Vec<Rule> {
    if !opts.approx.groups.contains(&approx::Group::Text) && !opts.approx.groups.contains(&approx::Group::Color) {
        return Vec::new();
    }
    let mut found: Vec<(f64, Rule)> = Vec::new();
    for (idx, rule) in sheet.rules.iter().enumerate() {
        if rule.query || rule.selector.0.is_empty() || !rule.decls.iter().any(|d| is_inherited(&d.name)) {
            continue;
        }
        let tokens = rule_tokens(sheet, idx, root_tokens);
        let resolve = |d: Decl| Decl { value: inline_vars(&d.value, &tokens), ..d };
        // Gradient text (`background-clip: text; color: transparent`) only tints the element's
        // own text in Roblox, so its transparent color would hide the text inside it.
        let clips_text = rule.decls.iter().any(|d| {
            d.name.ends_with("background-clip") && d.value.as_str().is_some_and(|s| s.eq_ignore_ascii_case("text"))
        });
        let own: Vec<Decl> = rule
            .decls
            .iter()
            .filter(|d| is_inherited(&d.name) && !(clips_text && d.name.ends_with("color")))
            .map(to_decl)
            .map(resolve)
            .collect();
        if own.is_empty() {
            continue;
        }
        let mut all: Vec<Decl> = weaker_decls(sheet, idx, is_inherited).into_iter().map(resolve).collect();
        all.extend(own.iter().cloned());
        // Warnings were given when the rule itself was translated.
        let mut silent = Diagnostics::default();
        let produced = approx::translate(&own, &opts.approx, &mut silent).props;
        let props: Vec<(String, luau::Value)> = approx::translate(&all, &opts.approx, &mut silent)
            .props
            .into_iter()
            .filter(|(k, _)| INHERITED_TEXT_PROPERTIES.contains(&k.as_str()) && produced.iter().any(|(p, _)| p == k))
            .collect();
        if props.is_empty() {
            continue;
        }
        let base = rule.selector.to_roblox(&mut Diagnostics::default(), None);
        let selector = split_selector_list(&base)
            .into_iter()
            .filter(|s| !s.contains("::"))
            .flat_map(|s| ["TextLabel", "TextButton", "TextBox"].map(|class| format!("{s} >> {class}")))
            .collect::<Vec<_>>()
            .join(", ");
        if selector.is_empty() {
            continue;
        }
        let order = priorities.get(&(idx, false)).or(priorities.get(&(idx, true))).copied().flatten().unwrap_or(0.0);
        let prefixes: Vec<Option<String>> = match rule.parent {
            Some(container) => sheet.rules[container].queries.iter().map(|q| Some(q.name())).collect(),
            None => vec![None],
        };
        for prefix in prefixes {
            let selector = match prefix {
                Some(name) => prefix_selector(&selector, &format!("@{name} ")),
                None => selector.clone(),
            };
            found.push((order, Rule { selector, props: props.clone(), ..Default::default() }));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.into_iter().enumerate().map(|(rank, (_, rule))| Rule { priority: Some(rank as f64 + 1.0), ..rule }).collect()
}

/// The classes that have a property, for the properties only some GuiObject classes have; `None`
/// for one every GuiObject has, or one outlass doesn't know.
fn property_owners(property: &str) -> Option<&'static [&'static str]> {
    const TEXT: &[&str] = &["TextLabel", "TextButton", "TextBox"];
    const IMAGE: &[&str] = &["ImageLabel", "ImageButton"];
    Some(match property {
        "Text"
        | "TextColor3"
        | "TextTransparency"
        | "TextSize"
        | "TextScaled"
        | "TextWrapped"
        | "TextTruncate"
        | "TextXAlignment"
        | "TextYAlignment"
        | "FontFace"
        | "Font"
        | "LineHeight"
        | "RichText"
        | "TextStrokeColor3"
        | "TextStrokeTransparency"
        | "MaxVisibleGraphemes" => TEXT,
        "PlaceholderColor3" | "PlaceholderText" | "ClearTextOnFocus" | "MultiLine" | "TextEditable" => &["TextBox"],
        "Image" | "ImageColor3" | "ImageTransparency" | "ImageRectOffset" | "ImageRectSize" | "ScaleType"
        | "TileSize" | "SliceCenter" | "SliceScale" | "ResampleMode" => IMAGE,
        "HoverImage" | "PressedImage" => &["ImageButton"],
        "AutoButtonColor" | "Modal" => &["TextButton", "ImageButton"],
        "CanvasSize"
        | "AutomaticCanvasSize"
        | "CanvasPosition"
        | "ScrollingEnabled"
        | "ScrollingDirection"
        | "ScrollBarThickness"
        | "ScrollBarImageColor3"
        | "ScrollBarImageTransparency"
        | "VerticalScrollBarInset"
        | "HorizontalScrollBarInset"
        | "ElasticBehavior" => &["ScrollingFrame"],
        "GroupTransparency" | "GroupColor3" => &["CanvasGroup"],
        _ => return None,
    })
}

/// With `--tags`, a rule whose elements can only be certain classes keeps only the properties
/// those classes have. A Roblox property written out, or a CSS declaration none of whose
/// properties are left, is warned about; an inherited text property isn't, since it still styles
/// the text inside.
fn drop_missing_properties(
    main: &mut Rule,
    rule: &OutRule,
    classes: &[String],
    opts: &CodegenOptions,
    diag: &mut Diagnostics,
) {
    let has = |property: &str| {
        property_owners(property).is_none_or(|owners| classes.iter().any(|c| owners.contains(&c.as_str())))
    };
    let which = if classes.is_empty() { "nothing".to_string() } else { classes.join(" or ") };
    for d in &rule.decls {
        let dropped: Vec<String> = if is_css_property(&d.name) {
            if is_inherited(&d.name) {
                continue;
            }
            let alone = approx::translate(&[to_decl(d)], &opts.approx, &mut Diagnostics::default()).props;
            if alone.is_empty() || alone.iter().any(|(p, _)| has(p)) {
                continue;
            }
            alone.into_iter().map(|(p, _)| p).collect()
        } else if has(&d.name) {
            continue;
        } else {
            vec![d.name.clone()]
        };
        diag.warn(
            format!("`{}` sets {}, which {which} doesn't have (ignored)", d.name, dropped.join(", ")),
            Some(&d.span),
        );
    }
    main.props.retain(|(p, _)| has(p));
    main.transitions.retain(|(p, _)| p == "*" || has(p));
}

/// Marks a property in `own_only`, which only records which properties a rule produces.
const PLACEHOLDER: luau::Value = luau::Value::Bool(false);

#[allow(clippy::too_many_arguments)]
fn lower_rule(
    rule: &OutRule,
    inherited: &[Decl],
    tokens: &HashMap<String, Value>,
    keep: &[String],
    selector: String,
    priority: Option<f64>,
    opts: &CodegenOptions,
    diag: &mut Diagnostics,
) -> Vec<Rule> {
    let mut main = Rule { selector, priority, ..Default::default() };
    main.attributes = attributes(&rule.tokens, &opts.values, &opts.approx.tokens, diag);

    let mut css: Vec<Decl> = inherited
        .iter()
        .map(|d| Decl {
            name: d.name.clone(),
            value: resolve_css(&d.name, &d.value, tokens, keep),
            span: d.span.clone(),
        })
        .collect();
    let mut explicit: Vec<(String, luau::Value)> = Vec::new();
    let mut explicit_transitions: Vec<(String, luau::TweenInfo)> = Vec::new();
    for decl in &rule.decls {
        if CONTAINER_PROPERTIES.contains(&decl.name.as_str()) {
            // Marks the element as a query container; see query_definitions.
        } else if is_css_property(&decl.name) {
            let value = resolve_css(&decl.name, &decl.value, tokens, keep);
            css.push(Decl { name: decl.name.clone(), value, span: Some(decl.span.clone()) });
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
            match roblox::value(&decl.value, &opts.values) {
                Ok(v) => set(&mut explicit, decl.name.clone(), v),
                Err(e) if roblox::uses_raw_luau(&decl.value) => {
                    diag.error(format!("{}: {e}", decl.name), Some(&decl.span))
                }
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
            .map(|d| Decl {
                name: d.name.clone(),
                value: resolve_css(&d.name, &d.value, tokens, keep),
                span: Some(d.span.clone()),
            })
            .collect();
        let mut own_only = approx::translate(&own, &opts.approx, &mut Diagnostics::default());
        // `opacity` scales what the weaker rules paint, so it recomputes their transparencies too:
        // `.a:hover { opacity: 0.5 }` fades `.a`'s background and border.
        if own.iter().any(|d| d.name == "opacity") {
            for prop in approx::css_property_targets("opacity") {
                own_only.set_prop(prop, PLACEHOLDER);
            }
            for (class, prop) in approx::css_pseudo_targets("opacity") {
                own_only.set_pseudo_prop(class, prop, PLACEHOLDER);
            }
        }
        // TextSize depends on the family as well as the size, and LineHeight and the half-leading
        // padding on all three and the line-height: `.mono { font-family: ... }` resizes the text.
        if own.iter().any(|d| matches!(d.name.as_str(), "font" | "font-family" | "font-size" | "line-height")) {
            own_only.set_prop("TextSize", PLACEHOLDER);
            own_only.set_prop("LineHeight", PLACEHOLDER);
            own_only.set_pseudo_prop("UIPadding", "PaddingTop", PLACEHOLDER);
            own_only.set_pseudo_prop("UIPadding", "PaddingBottom", PLACEHOLDER);
        }
        // A border image's slices are measured in its picture: `.p:hover { border-image-source: ... }`
        // keeps the slicing `.p` gives it.
        if own.iter().any(|d| d.name.starts_with("border-image")) {
            for prop in ["Image", "ScaleType", "SliceCenter", "SliceScale"] {
                own_only.set_prop(prop, PLACEHOLDER);
            }
        }
        // A background color multiplied into a picture is its ImageColor3.
        if own.iter().any(|d| matches!(d.name.as_str(), "background" | "background-color" | "background-blend-mode")) {
            own_only.set_prop("ImageColor3", PLACEHOLDER);
        }
        // Scrolling lifts the cap that keeps a sized element out of a flex line's stretch:
        // `.log.scrolls { overflow-y: auto }` needs its canvas to outgrow `.log`'s height.
        if own.iter().any(|d| d.name.starts_with("overflow")) {
            own_only.set_pseudo_prop("UISizeConstraint", "MaxSize", PLACEHOLDER);
        }
        // The border's inset is part of every side's padding: `.card.flat { border: none }` gives
        // the room back.
        if own.iter().any(|d| matches!(d.name.as_str(), "border" | "border-width" | "border-style")) {
            for side in ["PaddingTop", "PaddingRight", "PaddingBottom", "PaddingLeft"] {
                own_only.set_pseudo_prop("UIPadding", side, PLACEHOLDER);
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
    let pseudo_rules: Vec<Rule> = pseudo
        .into_iter()
        .map(|(class, props)| {
            let transitions = translated
                .pseudo_transitions
                .iter()
                .find(|(c, _)| *c == class)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            // A `"$Name"` reference only sees its own rule's tokens, so the pseudo-instance rule
            // gets a copy of each one of the rule's it uses.
            let attributes = main
                .attributes
                .iter()
                .filter(|(name, _)| props.iter().any(|(_, v)| matches!(v, luau::Value::Token(t) if t == name)))
                .cloned()
                .collect();
            Rule {
                selector: rule.selector.with_pseudo_instance(&class).to_roblox(&mut Diagnostics::default(), None),
                priority,
                props,
                attributes,
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
    let lowered = lower(sheet, opts, diag);
    let emit_options = luau::EmitOptions {
        header: opts.header.as_ref().map(|source| {
            format!("Generated by outlass {} from {source}. Do not edit by hand.", env!("CARGO_PKG_VERSION"))
        }),
        allow_raw_luau: opts.values.allow_raw_luau,
    };
    match luau::emit(&lowered, &emit_options) {
        Ok(code) => code,
        Err(e) => {
            diag.error(format!("can't emit Luau: {e}"), None);
            String::new()
        }
    }
}

/// The lowered stylesheet as a Roblox model file (see `rbxmx::write`).
pub fn emit_rbxmx(sheet: &Sheet, opts: &CodegenOptions, diag: &mut Diagnostics) -> String {
    match crate::rbxmx::write(&lower(sheet, opts, diag)) {
        Ok((model, dropped)) => {
            for d in dropped {
                diag.warn(d, None);
            }
            model
        }
        Err(e) => {
            diag.error(format!("can't write the model file: {e}"), None);
            String::new()
        }
    }
}

/// The lowered stylesheet as JSON (see `luau::to_json`).
pub fn emit_json(sheet: &Sheet, opts: &CodegenOptions, diag: &mut Diagnostics) -> String {
    luau::to_json(&lower(sheet, opts, diag))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn options(groups: Vec<approx::Group>) -> CodegenOptions {
        CodegenOptions {
            values: ValueOptions { allow_raw_luau: false },
            approx: ApproxOptions {
                groups,
                strict: false,
                tokens: HashMap::new(),
                inherited_family: None,
                user_agent: false,
            },
            sheet_name: "test".into(),
            header: None,
            tags: HashMap::new(),
            user_agent_styles: true,
        }
    }

    #[test]
    fn the_user_agent_sheet_styles_every_gui_object_class() {
        let rules = user_agent_rules(&options(approx::Group::expand(&[approx::Group::All])));
        let all = GUI_OBJECT_CLASSES.join(", ");
        assert!(rules.iter().any(|r| r.selector == all), "no `{all}` rule in {rules:?}");
    }

    #[test]
    fn the_user_agent_sheet_compiles_cleanly_with_and_without_every_group() {
        // `user_agent_rules` panics on an error and asserts there are no warnings.
        let all = user_agent_rules(&options(approx::Group::expand(&[approx::Group::All])));
        let none = user_agent_rules(&options(Vec::new()));
        assert_eq!(none.len(), 1, "{none:?}");
        assert_eq!(none[0].props, vec![("RichText".to_string(), luau::Value::Bool(true))]);
        assert_eq!(all.len(), 5, "{all:?}");
    }

    #[test]
    fn the_user_agent_sheet_sizes_from_content_without_overriding_size() {
        // Nothing ranks below the user-agent sheet, so `fit-content` needn't zero a weaker Size,
        // and its text properties aren't copied down to descendants.
        let rules = user_agent_rules(&options(approx::Group::expand(&[approx::Group::All])));
        let every_class = rules.iter().find(|r| r.selector == GUI_OBJECT_CLASSES.join(", ")).unwrap();
        assert!(every_class.props.iter().any(|(k, _)| k == "AutomaticSize"), "{every_class:?}");
        assert!(rules.iter().all(|r| r.props.iter().all(|(k, _)| k != "Size")), "{rules:?}");
        assert!(rules.iter().all(|r| !r.selector.contains(">>")), "{rules:?}");
    }
}
