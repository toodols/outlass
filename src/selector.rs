//! Selector model: parsing, parent (`&`) resolution, `@extend`, and Roblox serialization.

use crate::diag::{Diagnostics, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    /// Whitespace or `>>`.
    Descendant,
    /// `>`
    Child,
    /// `+`
    Next,
    /// `~`
    Sibling,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Simple {
    /// `&`, optionally followed by a suffix (`&-title`).
    Parent(Option<String>),
    Type(String),
    Universal,
    /// `.name` (a CollectionService tag in Roblox).
    Class(String),
    /// `#name` (an Instance.Name in Roblox).
    Id(String),
    /// `%name`
    Placeholder(String),
    /// `[...]` (raw inner text)
    Attribute(String),
    /// `:name` / `:name(arg)`
    PseudoClass {
        name: String,
        arg: Option<String>,
    },
    /// `::name`
    PseudoElement(String),
    /// `@QueryName` (Roblox StyleQuery reference)
    Query(String),
}

pub type Compound = Vec<Simple>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Comb(Combinator),
    Compound(Compound),
}

/// A complex selector: compounds joined by combinators. May start with a combinator (`> a`) when nested.
pub type Complex = Vec<Part>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectorList(pub Vec<Complex>);

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || !c.is_ascii()
}

pub fn parse(text: &str) -> Result<SelectorList, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut list = Vec::new();
    let mut complex: Complex = Vec::new();
    let mut compound: Compound = Vec::new();
    let mut pending_ws = false;

    let name = |i: &mut usize| -> String {
        let start = *i;
        while *i < chars.len() && (is_name_char(chars[*i]) || (chars[*i] == '\\' && *i + 1 < chars.len())) {
            if chars[*i] == '\\' {
                *i += 1;
            }
            *i += 1;
        }
        chars[start..*i].iter().collect()
    };

    fn flush(compound: &mut Compound, complex: &mut Complex) {
        if !compound.is_empty() {
            complex.push(Part::Compound(std::mem::take(compound)));
        }
    }

    fn push_comb(comb: Combinator, compound: &mut Compound, complex: &mut Complex) -> Result<(), String> {
        flush(compound, complex);
        match complex.last_mut() {
            Some(Part::Comb(existing)) => {
                // A descendant (whitespace) next to an explicit combinator is just spacing.
                if *existing == Combinator::Descendant {
                    *existing = comb;
                } else if comb != Combinator::Descendant {
                    return Err("Consecutive combinators are not supported".into());
                }
            }
            _ => complex.push(Part::Comb(comb)),
        }
        Ok(())
    }

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            pending_ws = true;
            i += 1;
            continue;
        }
        if pending_ws && !matches!(c, ',' | '>' | '+' | '~') && !(complex.is_empty() && compound.is_empty()) {
            push_comb(Combinator::Descendant, &mut compound, &mut complex)?;
        }
        pending_ws = false;
        match c {
            ',' => {
                flush(&mut compound, &mut complex);
                if matches!(complex.last(), Some(Part::Comb(Combinator::Descendant))) {
                    complex.pop();
                }
                if complex.is_empty() {
                    return Err("Expected selector before \",\"".into());
                }
                list.push(std::mem::take(&mut complex));
                i += 1;
            }
            '>' => {
                if chars.get(i + 1) == Some(&'>') {
                    i += 2;
                    push_comb(Combinator::Descendant, &mut compound, &mut complex)?;
                    // Mark explicitly so following whitespace doesn't matter.
                } else {
                    i += 1;
                    push_comb(Combinator::Child, &mut compound, &mut complex)?;
                }
            }
            '+' => {
                i += 1;
                push_comb(Combinator::Next, &mut compound, &mut complex)?;
            }
            '~' => {
                i += 1;
                push_comb(Combinator::Sibling, &mut compound, &mut complex)?;
            }
            '&' => {
                i += 1;
                let suffix = name(&mut i);
                if !compound.is_empty() {
                    return Err("\"&\" may only be used at the beginning of a compound selector".into());
                }
                compound.push(Simple::Parent((!suffix.is_empty()).then_some(suffix)));
            }
            '*' => {
                i += 1;
                compound.push(Simple::Universal);
            }
            '.' | '#' | '%' | '@' => {
                i += 1;
                let n = name(&mut i);
                if n.is_empty() {
                    return Err(format!("Expected name after \"{c}\""));
                }
                compound.push(match c {
                    '.' => Simple::Class(n),
                    '#' => Simple::Id(n),
                    '%' => Simple::Placeholder(n),
                    _ => Simple::Query(n),
                });
            }
            '[' => {
                let start = i + 1;
                let mut depth = 1;
                i += 1;
                while i < chars.len() && depth > 0 {
                    match chars[i] {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth > 0 {
                    return Err("Expected \"]\"".into());
                }
                compound.push(Simple::Attribute(chars[start..i - 1].iter().collect()));
            }
            ':' => {
                let element = chars.get(i + 1) == Some(&':');
                i += if element { 2 } else { 1 };
                let n = name(&mut i);
                if n.is_empty() {
                    return Err("Expected pseudo-class name after \":\"".into());
                }
                let mut arg = None;
                if chars.get(i) == Some(&'(') {
                    let start = i + 1;
                    let mut depth = 1;
                    i += 1;
                    while i < chars.len() && depth > 0 {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    if depth > 0 {
                        return Err("Expected \")\"".into());
                    }
                    arg = Some(chars[start..i - 1].iter().collect::<String>().trim().to_string());
                }
                if element {
                    compound.push(Simple::PseudoElement(n));
                } else {
                    compound.push(Simple::PseudoClass { name: n, arg });
                }
            }
            c if is_name_char(c) || c == '\\' => {
                let n = name(&mut i);
                compound.push(Simple::Type(n));
            }
            other => return Err(format!("Unexpected \"{other}\" in selector")),
        }
    }
    flush(&mut compound, &mut complex);
    if matches!(complex.last(), Some(Part::Comb(Combinator::Descendant))) {
        complex.pop();
    }
    if complex.is_empty() {
        return Err(if list.is_empty() { "Expected selector".into() } else { "Expected selector after \",\"".into() });
    }
    if matches!(complex.last(), Some(Part::Comb(_))) {
        return Err("Selector can't end with a combinator".into());
    }
    list.push(complex);
    Ok(SelectorList(list))
}

fn contains_parent(complex: &Complex) -> bool {
    complex.iter().any(|p| matches!(p, Part::Compound(c) if c.iter().any(|s| matches!(s, Simple::Parent(_)))))
}

impl SelectorList {
    pub fn contains_parent(&self) -> bool {
        self.0.iter().any(contains_parent)
    }

    /// Resolves `&` against `parent`. Without `&`, the child is nested as a descendant
    /// (or joined with its leading combinator, e.g. `> a`).
    pub fn resolve(&self, parent: Option<&SelectorList>) -> Result<SelectorList, String> {
        let Some(parent) = parent else {
            if self.contains_parent() {
                return Err("Top-level selectors may not contain the parent selector \"&\".".into());
            }
            return Ok(self.clone());
        };
        let mut out = Vec::new();
        for p in &parent.0 {
            for child in &self.0 {
                if contains_parent(child) {
                    out.push(substitute_parent(child, p)?);
                } else {
                    let mut joined = p.clone();
                    if !matches!(child.first(), Some(Part::Comb(_))) {
                        joined.push(Part::Comb(Combinator::Descendant));
                    }
                    joined.extend(child.iter().cloned());
                    out.push(joined);
                }
            }
        }
        Ok(SelectorList(out))
    }

    /// Whether any complex selector references a placeholder (`%name`).
    pub fn without_placeholders(&self) -> SelectorList {
        SelectorList(
            self.0
                .iter()
                .filter(|c| {
                    !c.iter().any(
                        |p| matches!(p, Part::Compound(c) if c.iter().any(|s| matches!(s, Simple::Placeholder(_)))),
                    )
                })
                .cloned()
                .collect(),
        )
    }

    /// `:root` (alone) — used for stylesheet-level tokens.
    pub fn is_root(&self) -> bool {
        self.0.len() == 1
            && matches!(self.0[0].as_slice(), [Part::Compound(c)] if matches!(c.as_slice(), [Simple::PseudoClass { name, arg: None }] if name.eq_ignore_ascii_case("root")))
    }

    /// CSS specificity (ids, classes/attributes/pseudo-classes/queries, types/pseudo-elements), maxed over the list.
    pub fn specificity(&self) -> (u32, u32, u32) {
        self.0
            .iter()
            .map(|complex| {
                let mut s = (0, 0, 0);
                for part in complex {
                    if let Part::Compound(c) = part {
                        for simple in c {
                            match simple {
                                Simple::Id(_) => s.0 += 1,
                                Simple::Class(_)
                                | Simple::Attribute(_)
                                | Simple::PseudoClass { .. }
                                | Simple::Placeholder(_)
                                | Simple::Query(_) => s.1 += 1,
                                Simple::Type(_) | Simple::PseudoElement(_) => s.2 += 1,
                                Simple::Universal | Simple::Parent(_) => {}
                            }
                        }
                    }
                }
                s
            })
            .max()
            .unwrap_or_default()
    }

    /// Whether every element matched by `stronger` is also matched by `self` (conservative:
    /// `false` when unsure). `.bar` covers `.bar.left`, `.x .bar:hover` and `.moon` covers `.dawn .moon`.
    pub fn covers(&self, stronger: &SelectorList) -> bool {
        !stronger.0.is_empty() && stronger.0.iter().all(|s| self.0.iter().any(|w| complex_covers(w, s)))
    }

    /// Appends `::Name` to every complex selector.
    pub fn with_pseudo_instance(&self, name: &str) -> SelectorList {
        let mut out = self.clone();
        for complex in &mut out.0 {
            if let Some(Part::Compound(c)) = complex.last_mut() {
                c.push(Simple::PseudoElement(name.to_string()));
            }
        }
        out
    }

    /// Plain CSS serialization.
    pub fn to_css(&self) -> String {
        self.0.iter().map(|c| complex_to_string(c, false, &mut None)).collect::<Vec<_>>().join(", ")
    }

    /// Roblox selector syntax: descendant combinator is `>>`, CSS pseudo-classes are mapped to
    /// Roblox GuiStates. Unsupported constructs are kept verbatim with a warning.
    pub fn to_roblox(&self, diag: &mut Diagnostics, span: Option<&Span>) -> String {
        let mut ctx = Some((diag, span));
        self.0.iter().map(|c| complex_to_string(c, true, &mut ctx)).collect::<Vec<_>>().join(", ")
    }
}

type WarnCtx<'a, 'b> = Option<(&'a mut Diagnostics, Option<&'b Span>)>;

fn warn(ctx: &mut WarnCtx, msg: String) {
    if let Some((diag, span)) = ctx {
        diag.warn(msg, *span);
    }
}

/// CSS pseudo-class → Roblox `Enum.GuiState` name.
pub fn roblox_pseudo_class(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "hover" => "Hover",
        "active" | "press" | "pressed" => "Press",
        "disabled" | "noninteractable" => "NonInteractable",
        "idle" => "Idle",
        _ => return None,
    })
}

fn complex_to_string(complex: &Complex, roblox: bool, ctx: &mut WarnCtx) -> String {
    let mut s = String::new();
    for part in complex {
        match part {
            Part::Comb(comb) => {
                let text = match comb {
                    Combinator::Descendant if roblox => " >> ",
                    Combinator::Descendant => " ",
                    Combinator::Child => " > ",
                    Combinator::Next => " + ",
                    Combinator::Sibling => " ~ ",
                };
                if roblox && matches!(comb, Combinator::Next | Combinator::Sibling) {
                    warn(ctx, format!("sibling combinator \"{}\" is not supported by Roblox selectors", text.trim()));
                }
                if s.is_empty() {
                    s.push_str(text.trim_start());
                } else {
                    s.push_str(text);
                }
            }
            Part::Compound(compound) => {
                for simple in compound {
                    match simple {
                        Simple::Parent(suffix) => {
                            s.push('&');
                            if let Some(x) = suffix {
                                s.push_str(x);
                            }
                        }
                        Simple::Type(n) => s.push_str(n),
                        Simple::Universal => {
                            if roblox {
                                warn(ctx, "the universal selector \"*\" is not supported by Roblox selectors".into());
                            }
                            s.push('*');
                        }
                        Simple::Class(n) => {
                            s.push('.');
                            s.push_str(n);
                        }
                        Simple::Id(n) => {
                            s.push('#');
                            s.push_str(n);
                        }
                        Simple::Placeholder(n) => {
                            s.push('%');
                            s.push_str(n);
                        }
                        Simple::Attribute(a) => {
                            if roblox {
                                warn(ctx, format!("attribute selector \"[{a}]\" is not supported by Roblox selectors"));
                            }
                            s.push('[');
                            s.push_str(a);
                            s.push(']');
                        }
                        Simple::PseudoClass { name, arg } => {
                            s.push(':');
                            if roblox {
                                match roblox_pseudo_class(name) {
                                    Some(state) if arg.is_none() => s.push_str(state),
                                    _ => {
                                        warn(
                                            ctx,
                                            format!(
                                                "pseudo-class \":{name}\" has no Roblox equivalent (Roblox supports :Hover, :Press, :NonInteractable, :Idle)"
                                            ),
                                        );
                                        s.push_str(name);
                                    }
                                }
                            } else {
                                s.push_str(name);
                            }
                            if let Some(a) = arg {
                                s.push('(');
                                s.push_str(a);
                                s.push(')');
                            }
                        }
                        Simple::PseudoElement(n) => {
                            if roblox && n.starts_with(|c: char| c.is_ascii_lowercase()) {
                                warn(
                                    ctx,
                                    format!(
                                        "pseudo-element \"::{n}\" isn't a Roblox pseudo-instance (expected a class like ::UICorner)"
                                    ),
                                );
                            }
                            s.push_str("::");
                            s.push_str(n);
                        }
                        Simple::Query(n) => {
                            s.push('@');
                            s.push_str(n);
                        }
                    }
                }
            }
        }
    }
    s
}

/// Compares right to left: every compound of `weak` must be a subset of the aligned compound of
/// `strong` with the same combinators; `weak` may have fewer ancestors. The subject compounds must
/// target the same pseudo-instance (`.a` does not cover `.a::UICorner`).
fn complex_covers(weak: &Complex, strong: &Complex) -> bool {
    let pseudo_elements =
        |c: &Compound| c.iter().filter(|s| matches!(s, Simple::PseudoElement(_))).cloned().collect::<Vec<_>>();
    if weak.len() > strong.len() {
        return false;
    }
    for (i, (w, s)) in weak.iter().rev().zip(strong.iter().rev()).enumerate() {
        match (w, s) {
            (Part::Compound(w), Part::Compound(s)) => {
                if !w.iter().all(|simple| s.contains(simple)) {
                    return false;
                }
                if i == 0 && pseudo_elements(w) != pseudo_elements(s) {
                    return false;
                }
            }
            (Part::Comb(a), Part::Comb(b)) if a == b => {}
            _ => return false,
        }
    }
    // A leading combinator in `weak` would need a matching ancestor it doesn't have.
    !matches!(weak.first(), Some(Part::Comb(_))) || weak.len() == strong.len()
}

fn substitute_parent(child: &Complex, parent: &Complex) -> Result<Complex, String> {
    let mut out: Complex = Vec::new();
    for part in child {
        match part {
            Part::Compound(compound) if matches!(compound.first(), Some(Simple::Parent(_))) => {
                let Some(Simple::Parent(suffix)) = compound.first() else { unreachable!() };
                let mut resolved = parent.clone();
                let Some(Part::Compound(last)) = resolved.last_mut() else {
                    return Err("Parent selector must end with a compound selector".into());
                };
                if let Some(suffix) = suffix {
                    match last.last_mut() {
                        Some(Simple::Type(n) | Simple::Class(n) | Simple::Id(n) | Simple::Placeholder(n)) => {
                            n.push_str(suffix)
                        }
                        Some(Simple::PseudoClass { name, arg: None }) => name.push_str(suffix),
                        _ => return Err(format!("Can't append suffix \"{suffix}\" to the parent selector")),
                    }
                }
                last.extend(compound[1..].iter().cloned());
                out.extend(resolved);
            }
            other => out.push(other.clone()),
        }
    }
    Ok(out)
}

/// Applies `@extend`: for each complex selector containing `target` (a single simple selector),
/// adds a copy where `target` is replaced by the extender.
pub fn extend(list: &SelectorList, target: &Simple, extender: &SelectorList) -> Option<SelectorList> {
    let mut added = Vec::new();
    for complex in &list.0 {
        for (idx, part) in complex.iter().enumerate() {
            let Part::Compound(compound) = part else { continue };
            if !compound.contains(target) {
                continue;
            }
            for ext in &extender.0 {
                let Some(Part::Compound(ext_last)) = ext.last() else { continue };
                // Replace the target in place with the extender's simples; type selectors go first.
                let mut merged: Compound = Vec::new();
                for s in compound {
                    if s == target {
                        merged.extend(ext_last.iter().filter(|e| !compound.contains(e)).cloned());
                    } else {
                        merged.push(s.clone());
                    }
                }
                if let Some(pos) = merged.iter().position(|s| matches!(s, Simple::Type(_))) {
                    let t = merged.remove(pos);
                    merged.retain(|m| !matches!(m, Simple::Universal));
                    merged.insert(0, t);
                }
                if merged.iter().filter(|s| matches!(s, Simple::Type(_))).count() > 1 {
                    continue; // e.g. `a` extended by `b` can't unify
                }
                let mut new: Complex = Vec::new();
                new.extend(ext[..ext.len() - 1].iter().cloned());
                if !new.is_empty() && idx > 0 {
                    // Both have ancestors: keep the extender's context before the original's.
                    new.push(Part::Comb(Combinator::Descendant));
                }
                new.extend(complex[..idx].iter().cloned());
                new.push(Part::Compound(merged));
                new.extend(complex[idx + 1..].iter().cloned());
                if !list.0.contains(&new) && !added.contains(&new) {
                    added.push(new);
                }
            }
        }
    }
    if added.is_empty() {
        return None;
    }
    let mut out = list.clone();
    out.0.extend(added);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roblox(s: &SelectorList) -> String {
        s.to_roblox(&mut Diagnostics::default(), None)
    }

    #[test]
    fn nesting() {
        let parent = parse(".a, .b").unwrap();
        assert_eq!(roblox(&parse(".c").unwrap().resolve(Some(&parent)).unwrap()), ".a >> .c, .b >> .c");
        assert_eq!(roblox(&parse("&:hover").unwrap().resolve(Some(&parent)).unwrap()), ".a:Hover, .b:Hover");
        assert_eq!(
            roblox(&parse("> TextLabel").unwrap().resolve(Some(&parent)).unwrap()),
            ".a > TextLabel, .b > TextLabel"
        );
        assert_eq!(roblox(&parse("&-title").unwrap().resolve(Some(&parse(".card").unwrap())).unwrap()), ".card-title");
        assert_eq!(
            roblox(&parse("&::UICorner").unwrap().resolve(Some(&parse("Frame").unwrap())).unwrap()),
            "Frame::UICorner"
        );
        assert_eq!(roblox(&parse(".x &").unwrap().resolve(Some(&parse(".y").unwrap())).unwrap()), ".x >> .y");
    }

    #[test]
    fn descendant_forms() {
        assert_eq!(roblox(&parse("Frame  >>  TextLabel").unwrap()), "Frame >> TextLabel");
        assert_eq!(roblox(&parse("Frame TextLabel:active").unwrap()), "Frame >> TextLabel:Press");
        assert_eq!(roblox(&parse("Frame>TextLabel").unwrap()), "Frame > TextLabel");
    }

    #[test]
    fn covering() {
        let covers = |w: &str, s: &str| parse(w).unwrap().covers(&parse(s).unwrap());
        assert!(covers(".bar", ".bar.left"));
        assert!(covers(".bar", ".x > .bar:hover"));
        assert!(covers(".moon", ".dawn .moon"));
        assert!(covers(".a, .b", ".b.c"));
        assert!(!covers(".bar.left", ".bar"));
        assert!(!covers(".a", ".a::UICorner"));
        assert!(!covers(".x > .a", ".y > .a"));
        assert!(!covers(".a", ".a, .b"));
    }

    #[test]
    fn extending() {
        let rule = parse(".btn:hover").unwrap();
        let out = extend(&rule, &Simple::Class("btn".into()), &parse(".primary").unwrap()).unwrap();
        assert_eq!(out.to_css(), ".btn:hover, .primary:hover");
        let placeholder = parse("%base").unwrap();
        let out = extend(&placeholder, &Simple::Placeholder("base".into()), &parse("TextButton").unwrap()).unwrap();
        assert_eq!(out.without_placeholders().to_css(), "TextButton");
    }
}
