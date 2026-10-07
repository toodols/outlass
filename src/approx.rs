//! CSS -> Roblox property approximations.
//!
//! When `--approx[=<groups>]` is passed, CSS properties that have no direct Roblox equivalent are
//! translated into near-equivalent `GuiObject` properties, and into properties on pseudo-instances
//! (phantom children created via a `::ClassName` selector suffix, e.g. `Frame::UICorner`).

use crate::diag::{Diagnostics, Span};
use crate::luau;
use crate::roblox;
use crate::value::*;

// ---------------------------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------------------------

/// Groups of approximations, selectable on the CLI via `--approx=a,b,c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    All,
    Color,
    Opacity,
    Text,
    Size,
    Position,
    Box,
    Layout,
    Visibility,
    Transition,
}

const CONCRETE_GROUPS: [Group; 9] = [
    Group::Color,
    Group::Opacity,
    Group::Text,
    Group::Size,
    Group::Position,
    Group::Box,
    Group::Layout,
    Group::Visibility,
    Group::Transition,
];

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::All => "all",
            Group::Color => "color",
            Group::Opacity => "opacity",
            Group::Text => "text",
            Group::Size => "size",
            Group::Position => "position",
            Group::Box => "box",
            Group::Layout => "layout",
            Group::Visibility => "visibility",
            Group::Transition => "transition",
        }
    }

    /// Expands `All` into every concrete group; deduplicates while preserving order.
    pub fn expand(groups: &[Group]) -> Vec<Group> {
        let source: Vec<Group> = if groups.contains(&Group::All) { CONCRETE_GROUPS.to_vec() } else { groups.to_vec() };
        let mut out = Vec::new();
        for g in source {
            if !out.contains(&g) {
                out.push(g);
            }
        }
        out
    }

    /// Help text generated from the `PROPERTIES` table: the CSS properties this group covers.
    fn generated_help(self) -> String {
        if self == Group::All {
            return "every group".to_string();
        }
        let mut names: Vec<&str> = PROPERTIES.iter().filter(|p| p.group == self).map(|p| p.css).collect();
        names.dedup();
        names.join(", ")
    }
}

impl clap::ValueEnum for Group {
    fn value_variants<'a>() -> &'a [Self] {
        &[
            Group::All,
            Group::Color,
            Group::Opacity,
            Group::Text,
            Group::Size,
            Group::Position,
            Group::Box,
            Group::Layout,
            Group::Visibility,
            Group::Transition,
        ]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.name()).help(self.generated_help()))
    }
}

// ---------------------------------------------------------------------------------------------
// Public API types
// ---------------------------------------------------------------------------------------------

/// The font family used when font-weight/font-style are set without font-family.
const DEFAULT_FONT: &str = "rbxasset://fonts/families/SourceSansPro.json";

#[derive(Clone)]
pub struct ApproxOptions {
    /// Already expanded (no `All`).
    pub groups: Vec<Group>,
    /// Warn about CSS that compiles to a different layout than a browser gives it (`--strict`).
    pub strict: bool,
}

#[derive(Clone)]
pub struct Decl {
    pub name: String,
    pub value: Value,
    pub span: Option<Span>,
}

#[derive(Default, Debug)]
pub struct Translated {
    /// Roblox property name -> Luau expression, for the rule itself. Order = first produced.
    pub props: Vec<(String, luau::Value)>,
    /// Pseudo-instance class name -> its properties. Order = first produced.
    pub pseudo: Vec<(String, Vec<(String, luau::Value)>)>,
    /// Roblox property name (or "*" meaning the default transition) -> Luau TweenInfo expression.
    pub transitions: Vec<(String, luau::TweenInfo)>,
    /// Transitions for properties of pseudo-instances (e.g. `transition: transform` animating
    /// `UIScale.Scale`): pseudo-instance class -> (property, TweenInfo expression). Emitted on the
    /// `<selector>::<Class>` rule by codegen.
    pub pseudo_transitions: Vec<(String, Vec<(String, luau::TweenInfo)>)>,
}

impl Translated {
    pub(crate) fn set_prop(&mut self, name: &str, expr: luau::Value) {
        if let Some(entry) = self.props.iter_mut().find(|(n, _)| n == name) {
            entry.1 = expr;
        } else {
            self.props.push((name.to_string(), expr));
        }
    }

    pub(crate) fn set_pseudo_prop(&mut self, class: &str, name: &str, expr: luau::Value) {
        let bucket = if let Some(pos) = self.pseudo.iter().position(|(n, _)| n == class) {
            &mut self.pseudo[pos].1
        } else {
            self.pseudo.push((class.to_string(), Vec::new()));
            let last = self.pseudo.len() - 1;
            &mut self.pseudo[last].1
        };
        if let Some(entry) = bucket.iter_mut().find(|(n, _)| n == name) {
            entry.1 = expr;
        } else {
            bucket.push((name.to_string(), expr));
        }
    }

    fn set_pseudo_transition(&mut self, class: &str, name: &str, expr: luau::TweenInfo) {
        let bucket = if let Some(pos) = self.pseudo_transitions.iter().position(|(n, _)| n == class) {
            &mut self.pseudo_transitions[pos].1
        } else {
            self.pseudo_transitions.push((class.to_string(), Vec::new()));
            let last = self.pseudo_transitions.len() - 1;
            &mut self.pseudo_transitions[last].1
        };
        match bucket.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = expr,
            None => bucket.push((name.to_string(), expr)),
        }
    }

    fn set_transition(&mut self, prop: &str, expr: luau::TweenInfo) {
        if let Some(entry) = self.transitions.iter_mut().find(|(n, _)| n == prop) {
            entry.1 = expr;
        } else {
            self.transitions.push((prop.to_string(), expr));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Property documentation / dispatch table
// ---------------------------------------------------------------------------------------------

pub struct PropDoc {
    pub css: &'static str,
    pub group: Group,
    pub roblox: &'static str,
    pub notes: &'static str,
}

pub static PROPERTIES: &[PropDoc] = &[
    // color
    PropDoc { css: "color", group: Group::Color, roblox: "TextColor3 (+ TextTransparency)", notes: "" },
    PropDoc {
        css: "background-color",
        group: Group::Color,
        roblox: "BackgroundColor3 (+ BackgroundTransparency)",
        notes: "",
    },
    PropDoc {
        css: "background",
        group: Group::Color,
        roblox: "BackgroundColor3/BackgroundTransparency or UIGradient (::UIGradient)",
        notes: "a solid color, a linear-gradient(), or a repeating-linear-gradient() painted over the color",
    },
    PropDoc {
        css: "background-image",
        group: Group::Color,
        roblox: "UIGradient (::UIGradient) or Image",
        notes: "linear-gradient(), repeating-linear-gradient() or url(); a repeating gradient is written out stop \
                by stop, so its stops need percentage positions and only ~20 keypoints fit",
    },
    PropDoc {
        css: "background-size",
        group: Group::Color,
        roblox: "ScaleType / TileSize",
        notes: "for an image: cover → Crop, contain → Fit, a size → Tile at that size",
    },
    PropDoc {
        css: "background-repeat",
        group: Group::Color,
        roblox: "ScaleType",
        notes: "for an image: no-repeat with a background-size stretches instead of tiling",
    },
    PropDoc {
        css: "object-fit",
        group: Group::Color,
        roblox: "ScaleType",
        notes: "fill → Stretch, contain/scale-down → Fit, cover → Crop",
    },
    PropDoc {
        css: "image-rendering",
        group: Group::Color,
        roblox: "ResampleMode",
        notes: "pixelated/crisp-edges → Pixelated",
    },
    PropDoc {
        css: "background-clip",
        group: Group::Color,
        roblox: "UIGradient on the text (::UIGradient), TextColor3",
        notes: "`background-clip: text` with a linear-gradient() background tints the text (gradient text)",
    },
    PropDoc {
        css: "-webkit-text-fill-color",
        group: Group::Color,
        roblox: "TextColor3",
        notes: "overrides `color`, as in browsers; `transparent` is part of the gradient-text idiom",
    },
    PropDoc {
        css: "mask-image",
        group: Group::Color,
        roblox: "UIGradient.Transparency (::UIGradient)",
        notes: "a linear-gradient() (repeating or not) whose stop alphas become the transparency; shares the \
                UIGradient with a background gradient",
    },
    PropDoc {
        css: "mask",
        group: Group::Color,
        roblox: "UIGradient.Transparency (::UIGradient)",
        notes: "like mask-image",
    },
    // opacity
    PropDoc {
        css: "opacity",
        group: Group::Opacity,
        roblox: "GroupTransparency, BackgroundTransparency, TextTransparency, ImageTransparency",
        notes: "1 - opacity; on a CanvasGroup it fades the element with its children, like CSS. Combines with color \
                alphas and also scales a border's UIStroke Transparency and a `scrollbar-color`. \
                BackgroundTransparency is only set when the rule (or a weaker rule matching the same elements) \
                sets a background, since CSS scales the background rather than replacing it",
    },
    // text
    PropDoc {
        css: "font-size",
        group: Group::Text,
        roblox: "TextSize",
        notes: "TextSize is a line's height and font-size the em, so with a built-in font-family it's scaled by \
                the family's line-to-em ratio (13px Roboto Mono is TextSize 17), rounded to whole pixels",
    },
    PropDoc {
        css: "font-family",
        group: Group::Text,
        roblox: "FontFace",
        notes: "the first family Roblox has, like a browser; warns about names that aren't built-in fonts. \
                `rbxassetid://` ids (uploaded fonts) are used as given",
    },
    PropDoc { css: "font-weight", group: Group::Text, roblox: "FontFace", notes: "" },
    PropDoc { css: "font-style", group: Group::Text, roblox: "FontFace", notes: "" },
    PropDoc {
        css: "font",
        group: Group::Text,
        roblox: "FontFace, TextSize",
        notes: "shorthand: [style] [weight] size[/line-height] family",
    },
    PropDoc { css: "text-align", group: Group::Text, roblox: "TextXAlignment", notes: "" },
    PropDoc {
        css: "vertical-align",
        group: Group::Text,
        roblox: "TextYAlignment",
        notes: "outlass-only: CSS ignores it on a block, so prefer `align-content`",
    },
    PropDoc {
        css: "align-content",
        group: Group::Text,
        roblox: "TextYAlignment",
        notes: "on an element that isn't a flex or grid container, aligns its text vertically, as CSS aligns a \
                block's content",
    },
    PropDoc {
        css: "line-height",
        group: Group::Text,
        roblox: "LineHeight, UIPadding (::UIPadding)",
        notes: "Roblox spaces only the lines after the first, so with a known font-size the element is also \
                padded by the half-leading, (line-height x font-size - TextSize) / 2, above and below, as CSS \
                does. Text inside the element inherits LineHeight, but not the padding",
    },
    PropDoc { css: "white-space", group: Group::Text, roblox: "TextWrapped", notes: "" },
    PropDoc { css: "text-wrap", group: Group::Text, roblox: "TextWrapped", notes: "" },
    PropDoc { css: "text-overflow", group: Group::Text, roblox: "TextTruncate", notes: "" },
    PropDoc { css: "content", group: Group::Text, roblox: "Text", notes: "a string; replaces the element's text" },
    PropDoc {
        css: "-webkit-text-stroke",
        group: Group::Text,
        roblox: "UIStroke (::UIStroke)",
        notes: "`<width> <color>`; the color defaults to the rule's `color` (currentColor), and a width of 0 \
                disables the stroke. Loses to a border in the same rule",
    },
    PropDoc {
        css: "-webkit-text-stroke-width",
        group: Group::Text,
        roblox: "UIStroke.Thickness (::UIStroke)",
        notes: "",
    },
    PropDoc { css: "-webkit-text-stroke-color", group: Group::Text, roblox: "UIStroke.Color (::UIStroke)", notes: "" },
    // size
    PropDoc {
        css: "box-sizing",
        group: Group::Size,
        roblox: "(implied)",
        notes: "Roblox always sizes the border box (padding is inside the Size), so `border-box` is accepted and \
                `content-box` warns",
    },
    PropDoc {
        css: "width",
        group: Group::Size,
        roblox: "Size / AutomaticSize",
        notes: "px → offset, % → scale, calc(% ± px) → both; auto/fit-content → AutomaticSize. Size covers both \
                axes, so if height isn't set in the same rule it becomes automatic (with a warning)",
    },
    PropDoc {
        css: "height",
        group: Group::Size,
        roblox: "Size / AutomaticSize",
        notes: "like width; if width isn't set in the same rule it becomes automatic (with a warning)",
    },
    PropDoc { css: "min-width", group: Group::Size, roblox: "UISizeConstraint (::UISizeConstraint)", notes: "" },
    PropDoc { css: "min-height", group: Group::Size, roblox: "UISizeConstraint (::UISizeConstraint)", notes: "" },
    PropDoc {
        css: "max-width",
        group: Group::Size,
        roblox: "UISizeConstraint (::UISizeConstraint)",
        notes: "an element sized by its content grows past it unless it's text; on a ScrollingFrame it caps the                 canvas too (both errors under --strict)",
    },
    PropDoc {
        css: "max-height",
        group: Group::Size,
        roblox: "UISizeConstraint (::UISizeConstraint)",
        notes: "like max-width",
    },
    PropDoc {
        css: "aspect-ratio",
        group: Group::Size,
        roblox: "UIAspectRatioConstraint (::UIAspectRatioConstraint)",
        notes: "",
    },
    // position
    PropDoc {
        css: "left",
        group: Group::Position,
        roblox: "Position",
        notes: "with `right` and no width, the element stretches between them (Size)",
    },
    PropDoc { css: "top", group: Group::Position, roblox: "Position", notes: "with `bottom` and no height, stretches" },
    PropDoc { css: "right", group: Group::Position, roblox: "Position / AnchorPoint", notes: "anchors the right edge" },
    PropDoc {
        css: "bottom",
        group: Group::Position,
        roblox: "Position / AnchorPoint",
        notes: "anchors the bottom edge",
    },
    PropDoc {
        css: "inset",
        group: Group::Position,
        roblox: "Position / Size",
        notes: "shorthand like padding; `inset: 0` fills the parent",
    },
    PropDoc {
        css: "transform",
        group: Group::Position,
        roblox: "Position/AnchorPoint/Rotation, UIScale (::UIScale)",
        notes: "translate/translateX/translateY/rotate/scale only",
    },
    PropDoc { css: "translate", group: Group::Position, roblox: "Position/AnchorPoint", notes: "" },
    PropDoc { css: "rotate", group: Group::Position, roblox: "Rotation", notes: "" },
    PropDoc { css: "scale", group: Group::Position, roblox: "UIScale (::UIScale)", notes: "one factor for both axes" },
    PropDoc { css: "z-index", group: Group::Position, roblox: "ZIndex", notes: "" },
    PropDoc {
        css: "margin",
        group: Group::Position,
        roblox: "Position / AnchorPoint / Size",
        notes: "offsets the element from the edge it's placed by, and shrinks one stretched between opposite \
                edges; `auto` on both sides centres it, on one side pushes it to the other. Siblings don't move, \
                and flex and grid containers ignore it (with a warning): use `gap` or the container's padding",
    },
    PropDoc { css: "margin-top", group: Group::Position, roblox: "Position / Size", notes: "see `margin`" },
    PropDoc { css: "margin-right", group: Group::Position, roblox: "Position / Size", notes: "see `margin`" },
    PropDoc { css: "margin-bottom", group: Group::Position, roblox: "Position / Size", notes: "see `margin`" },
    PropDoc { css: "margin-left", group: Group::Position, roblox: "Position / Size", notes: "see `margin`" },
    PropDoc { css: "margin-inline", group: Group::Position, roblox: "Position / Size", notes: "left/right" },
    PropDoc { css: "margin-block", group: Group::Position, roblox: "Position / Size", notes: "top/bottom" },
    PropDoc {
        css: "position",
        group: Group::Position,
        roblox: "(implied)",
        notes: "GuiObjects are always positioned absolutely within their parent, so any value is accepted",
    },
    // box
    PropDoc { css: "border-radius", group: Group::Box, roblox: "UICorner (::UICorner)", notes: "" },
    PropDoc { css: "padding", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "" },
    PropDoc { css: "padding-top", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "" },
    PropDoc { css: "padding-right", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "" },
    PropDoc { css: "padding-bottom", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "" },
    PropDoc { css: "padding-left", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "" },
    PropDoc { css: "padding-inline", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "left/right" },
    PropDoc { css: "padding-block", group: Group::Box, roblox: "UIPadding (::UIPadding)", notes: "top/bottom" },
    PropDoc {
        css: "border",
        group: Group::Box,
        roblox: "UIStroke (::UIStroke)",
        notes: "solid strokes only; wins over outline. Also clears the legacy border (BorderSizePixel = 0); \
                `border: none` disables the stroke",
    },
    PropDoc { css: "border-width", group: Group::Box, roblox: "UIStroke (::UIStroke)", notes: "" },
    PropDoc {
        css: "border-style",
        group: Group::Box,
        roblox: "UIStroke (::UIStroke)",
        notes: "dashed/dotted/etc are still solid",
    },
    PropDoc { css: "border-color", group: Group::Box, roblox: "UIStroke (::UIStroke)", notes: "" },
    PropDoc {
        css: "outline",
        group: Group::Box,
        roblox: "UIStroke (::UIStroke)",
        notes: "loses to border in the same rule",
    },
    PropDoc { css: "outline-width", group: Group::Box, roblox: "UIStroke (::UIStroke)", notes: "" },
    PropDoc { css: "outline-style", group: Group::Box, roblox: "UIStroke (::UIStroke)", notes: "" },
    PropDoc { css: "outline-color", group: Group::Box, roblox: "UIStroke (::UIStroke)", notes: "" },
    // layout
    PropDoc {
        css: "display",
        group: Group::Layout,
        roblox: "Visible, UIListLayout/UIGridLayout (::UIListLayout/::UIGridLayout)",
        notes: "",
    },
    PropDoc { css: "flex-direction", group: Group::Layout, roblox: "UIListLayout.FillDirection", notes: "" },
    PropDoc { css: "justify-content", group: Group::Layout, roblox: "UIListLayout/UIGridLayout alignment", notes: "" },
    PropDoc {
        css: "align-items",
        group: Group::Layout,
        roblox: "UIListLayout alignment / ItemLineAlignment",
        notes: "defaults to `stretch` in a flex container, as in CSS. Roblox stretches items to the largest item \
                on the line rather than to the container, and stretches explicitly sized items too, so outlass \
                keeps a px `width`/`height` out of the stretch with a UISizeConstraint",
    },
    PropDoc { css: "gap", group: Group::Layout, roblox: "UIListLayout.Padding / UIGridLayout.CellPadding", notes: "" },
    PropDoc {
        css: "row-gap",
        group: Group::Layout,
        roblox: "UIListLayout.Padding / UIGridLayout.CellPadding",
        notes: "",
    },
    PropDoc {
        css: "column-gap",
        group: Group::Layout,
        roblox: "UIListLayout.Padding / UIGridLayout.CellPadding",
        notes: "",
    },
    PropDoc {
        css: "grid-template-columns",
        group: Group::Layout,
        roblox: "UIGridLayout.FillDirectionMaxCells / CellSize (::UIGridLayout)",
        notes: "tracks must all be the same size, as Roblox grid cells are uniform",
    },
    PropDoc {
        css: "grid-template-rows",
        group: Group::Layout,
        roblox: "UIGridLayout.FillDirectionMaxCells / CellSize (::UIGridLayout)",
        notes: "tracks must all be the same size, as Roblox grid cells are uniform",
    },
    PropDoc {
        css: "grid-auto-columns",
        group: Group::Layout,
        roblox: "UIGridLayout.CellSize (::UIGridLayout)",
        notes: "",
    },
    PropDoc {
        css: "grid-auto-rows",
        group: Group::Layout,
        roblox: "UIGridLayout.CellSize (::UIGridLayout)",
        notes: "",
    },
    PropDoc {
        css: "grid-auto-flow",
        group: Group::Layout,
        roblox: "UIGridLayout.FillDirection (::UIGridLayout)",
        notes: "`dense` packing has no equivalent",
    },
    PropDoc { css: "flex-wrap", group: Group::Layout, roblox: "UIListLayout.Wraps", notes: "" },
    PropDoc {
        css: "flex-grow",
        group: Group::Layout,
        roblox: "UIFlexItem (::UIFlexItem)",
        notes: "an item that grows with no width starts from 0 (like `flex: 1`) rather than its content, which \
                a percentage-sized descendant would inflate; this assumes a row, so set a width in a column",
    },
    PropDoc {
        css: "flex-shrink",
        group: Group::Layout,
        roblox: "UIFlexItem (::UIFlexItem)",
        notes: "the initial 1 is given to every child of a flex row that doesn't scroll along the line, not                 to columns, where Roblox would shrink content-sized items below their content",
    },
    PropDoc { css: "flex", group: Group::Layout, roblox: "UIFlexItem (::UIFlexItem)", notes: "basis is ignored" },
    PropDoc { css: "align-self", group: Group::Layout, roblox: "UIFlexItem (::UIFlexItem)", notes: "" },
    PropDoc { css: "order", group: Group::Layout, roblox: "LayoutOrder", notes: "" },
    // visibility
    PropDoc { css: "visibility", group: Group::Visibility, roblox: "Visible", notes: "" },
    PropDoc {
        css: "overflow",
        group: Group::Visibility,
        roblox: "ClipsDescendants / ScrollingEnabled / ScrollingDirection / AutomaticCanvasSize",
        notes: "`scroll`/`auto` size the canvas to the content, like CSS; one or two values (x y). Only a \
                ScrollingFrame actually scrolls",
    },
    PropDoc {
        css: "overflow-x",
        group: Group::Visibility,
        roblox: "ClipsDescendants / ScrollingEnabled / ScrollingDirection / AutomaticCanvasSize",
        notes: "only a ScrollingFrame actually scrolls",
    },
    PropDoc {
        css: "overflow-y",
        group: Group::Visibility,
        roblox: "ClipsDescendants / ScrollingEnabled / ScrollingDirection / AutomaticCanvasSize",
        notes: "only a ScrollingFrame actually scrolls",
    },
    PropDoc {
        css: "scrollbar-width",
        group: Group::Visibility,
        roblox: "ScrollBarThickness",
        notes: "`auto` is Roblox's 12px, `thin` 8px, `none` 0; a length is taken as the thickness",
    },
    PropDoc {
        css: "scrollbar-color",
        group: Group::Color,
        roblox: "ScrollBarImageColor3 / ScrollBarImageTransparency",
        notes: "the thumb colour; Roblox draws no track, so the track colour is ignored",
    },
    PropDoc {
        css: "scrollbar-gutter",
        group: Group::Visibility,
        roblox: "VerticalScrollBarInset",
        notes: "`auto` insets the canvas while the scrollbar shows, `stable` always",
    },
    PropDoc { css: "pointer-events", group: Group::Visibility, roblox: "Interactable", notes: "" },
    PropDoc {
        css: "appearance",
        group: Group::Visibility,
        roblox: "AutoButtonColor",
        notes: "`none` turns off a button's automatic hover/press colouring; `auto` turns it on",
    },
    // transition
    PropDoc { css: "transition", group: Group::Transition, roblox: "TweenInfo", notes: "" },
    PropDoc { css: "transition-property", group: Group::Transition, roblox: "TweenInfo", notes: "" },
    PropDoc { css: "transition-duration", group: Group::Transition, roblox: "TweenInfo", notes: "" },
    PropDoc { css: "transition-timing-function", group: Group::Transition, roblox: "TweenInfo", notes: "" },
    PropDoc { css: "transition-delay", group: Group::Transition, roblox: "TweenInfo", notes: "" },
];

fn strip_vendor_prefix(name: &str) -> &str {
    for prefix in ["-webkit-", "-moz-", "-ms-", "-o-"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
    }
    name
}

/// Looks up a CSS property by name. Vendor prefixes are ignored on both sides, so
/// `-webkit-text-stroke` finds the table entry however either one is spelled.
pub fn lookup(css: &str) -> Option<&'static PropDoc> {
    let stripped = strip_vendor_prefix(css);
    PROPERTIES.iter().find(|p| strip_vendor_prefix(p.css) == stripped)
}

/// A short hint for CSS properties that are commonly written but have no Roblox equivalent at
/// all (regardless of `--approx`).
fn unknown_hint(css: &str) -> Option<&'static str> {
    match strip_vendor_prefix(css) {
        "text-decoration" | "text-decoration-line" => Some("use RichText <u>/<s> tags"),
        "text-transform" => Some("transform the text itself, or wrap it in RichText <uc>/<sc> tags"),
        "text-shadow" | "box-shadow" => Some("use a UIStroke or a shadow ImageLabel"),
        // `letter-spacing`, `word-spacing` and `cursor` have nothing to point at: Roblox has no
        // property and no rich-text attribute for them. The bare warning already says so.
        "float" | "clear" => Some("use layout properties instead"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Translates all CSS declarations of one style rule.
pub fn translate(decls: &[Decl], opts: &ApproxOptions, diag: &mut Diagnostics) -> Translated {
    let mut out = Translated::default();
    warn_disabled_and_unknown(decls, opts, diag);
    let (decls, scaled_text) = desugar(decls, opts, diag);
    let decls = &decls[..];
    if opts.strict {
        warn_layout_differences(decls, opts, diag);
    }

    let opacity_factor = if opts.groups.contains(&Group::Opacity) {
        last(decls, "opacity").and_then(|d| match parse_opacity_value(&d.value) {
            Ok(v) => Some(v),
            Err(e) => {
                diag.warn(format!("`opacity`: {e} (ignored)"), d.span.as_ref());
                None
            }
        })
    } else {
        None
    };

    if opts.groups.contains(&Group::Color) || opts.groups.contains(&Group::Opacity) {
        translate_color_opacity(decls, opts, opacity_factor, diag, &mut out);
    }
    if opts.groups.contains(&Group::Text) {
        translate_text(decls, diag, &mut out);
        if let Some((lower, upper)) = scaled_text {
            scale_text(lower, upper, &mut out);
        }
    }
    if opts.groups.contains(&Group::Color) {
        translate_images(decls, diag, &mut out);
    }
    if opts.groups.contains(&Group::Size) {
        translate_size(decls, opts, diag, &mut out);
    }
    if opts.groups.contains(&Group::Position) {
        translate_position(decls, diag, &mut out);
    }
    if opts.groups.contains(&Group::Box) {
        translate_box(decls, opts, opacity_factor, diag, &mut out);
    }
    if opts.groups.contains(&Group::Layout) {
        translate_layout(decls, opts, diag, &mut out);
    }
    if opts.groups.contains(&Group::Visibility) {
        translate_visibility(decls, diag, &mut out);
    }
    translate_scrollbar(decls, opts, opacity_factor, diag, &mut out);
    if opts.groups.contains(&Group::Transition) {
        translate_transition_group(decls, diag, &mut out);
    }

    out
}

/// Rewrites CSS that the translators below understand in another form: `currentColor`, the
/// standalone `translate`/`rotate`/`scale` properties, and `min()`/`max()`/`clamp()` sizes. A
/// `clamp()` font size around a size Roblox can't express (`2vw`) becomes text scaled to fit its
/// box between the two bounds, returned as the second value.
fn desugar(decls: &[Decl], opts: &ApproxOptions, diag: &mut Diagnostics) -> (Vec<Decl>, Option<(f64, f64)>) {
    let mut out: Vec<Decl> = match last(decls, "color").map(|d| d.value.clone()) {
        Some(color) => {
            decls.iter().map(|d| Decl { value: replace_current_color(&d.value, &color), ..d.clone() }).collect()
        }
        None => decls.to_vec(),
    };
    let is_none = |v: &Value| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("none"));
    let px = |v: &Value| v.as_number().and_then(length_px);

    // CSS applies `translate`, then `rotate`, then `scale`, then `transform`.
    if opts.groups.contains(&Group::Position) {
        let mut functions = Vec::new();
        let mut span = None;
        for name in ["translate", "rotate", "scale"] {
            let Some(d) = last(&out, name).filter(|d| !is_none(&d.value)) else { continue };
            let mut args = space_items(&d.value);
            if name == "rotate" {
                // `rotate: z 45deg` names the axis first; Roblox only rotates in the plane.
                args = args.split_off(args.len() - 1);
            }
            functions.push(Value::Call { name: name.to_string(), args });
            span = span.or(d.span.clone());
        }
        if !functions.is_empty() {
            if let Some(t) = last(&out, "transform").filter(|t| !is_none(&t.value)) {
                functions.extend(space_items(&t.value));
            }
            out.push(Decl { name: "transform".into(), value: Value::list(functions, ListSep::Space), span });
        }
    }

    // `width: min(100%, 300px)` is a percentage capped at 300px: a size and a UISizeConstraint.
    if opts.groups.contains(&Group::Size) {
        for (axis, min_name, max_name) in [("width", "min-width", "max-width"), ("height", "min-height", "max-height")]
        {
            let Some(d) = last(&out, axis).cloned() else { continue };
            let Value::Call { name, args } = &d.value else { continue };
            if !matches!(name.as_str(), "min" | "max" | "clamp") {
                continue;
            }
            let fixed: Vec<f64> = args.iter().filter_map(px).collect();
            let relative: Vec<&Value> = args.iter().filter(|a| px(a).is_none()).collect();
            let (size, lower, upper) = match (name.as_str(), relative.as_slice()) {
                ("min", []) => (Value::num_unit(fixed.iter().copied().fold(f64::INFINITY, f64::min), "px"), None, None),
                ("max", []) => {
                    (Value::num_unit(fixed.iter().copied().fold(f64::NEG_INFINITY, f64::max), "px"), None, None)
                }
                ("clamp", []) if fixed.len() == 3 => {
                    (Value::num_unit(fixed[0].max(fixed[1].min(fixed[2])), "px"), None, None)
                }
                ("min", [r]) => ((*r).clone(), None, fixed.iter().copied().reduce(f64::min)),
                ("max", [r]) => ((*r).clone(), fixed.iter().copied().reduce(f64::max), None),
                ("clamp", [r]) if args.len() == 3 && px(&args[0]).is_some() && px(&args[2]).is_some() => {
                    ((*r).clone(), px(&args[0]), px(&args[2]))
                }
                _ => {
                    diag.warn(
                        format!(
                            "`{axis}: {}`: only one bound of a min()/max()/clamp() can be relative (ignored)",
                            d.value.to_css().unwrap_or_else(|_| d.value.inspect())
                        ),
                        d.span.as_ref(),
                    );
                    out.retain(|o| o.name != axis);
                    continue;
                }
            };
            let existing = |out: &[Decl], name: &str| last(out, name).and_then(|d| px(&d.value));
            let lower = lower.map(|l| existing(&out, min_name).map_or(l, |e| e.max(l)));
            let upper = upper.map(|u| existing(&out, max_name).map_or(u, |e| e.min(u)));
            out.push(Decl { name: axis.into(), value: size, span: d.span.clone() });
            if let Some(l) = lower {
                out.push(Decl { name: min_name.into(), value: Value::num_unit(l, "px"), span: d.span.clone() });
            }
            if let Some(u) = upper {
                out.push(Decl { name: max_name.into(), value: Value::num_unit(u, "px"), span: d.span.clone() });
            }
        }
    }

    let mut scaled = None;
    if opts.groups.contains(&Group::Text)
        && let Some(d) = last(&out, "font-size").cloned()
        && let Value::Call { name, args } = &d.value
        && name == "clamp"
        && let [lo, mid, hi] = args.as_slice()
        && let (Some(lo), Some(hi)) = (px(lo), px(hi))
    {
        let size = match px(mid) {
            Some(mid) => lo.max(mid.min(hi)),
            None => {
                scaled = Some((lo, hi));
                hi
            }
        };
        out.push(Decl { name: "font-size".into(), value: Value::num_unit(size, "px"), span: d.span.clone() });
    }
    (out, scaled)
}

/// `currentColor` anywhere in `v`, replaced by the rule's `color`.
fn replace_current_color(v: &Value, color: &Value) -> Value {
    match v {
        Value::Str { text, quoted: false } if text.eq_ignore_ascii_case("currentcolor") => color.clone(),
        Value::List { items, sep, bracketed } => Value::List {
            items: items.iter().map(|i| replace_current_color(i, color)).collect(),
            sep: *sep,
            bracketed: *bracketed,
        },
        Value::Call { name, args } => {
            Value::Call { name: name.clone(), args: args.iter().map(|a| replace_current_color(a, color)).collect() }
        }
        _ => v.clone(),
    }
}

/// Text that scales to fit its box, between two font sizes: TextScaled with a
/// UITextSizeConstraint. TextSize already holds the upper bound in Roblox units.
fn scale_text(lower: f64, upper: f64, out: &mut Translated) {
    let max = match out.props.iter().find(|(k, _)| k == "TextSize") {
        Some((_, luau::Value::Number(n))) => *n,
        _ => upper,
    };
    out.set_prop("TextScaled", luau::Value::Bool(true));
    out.set_pseudo_prop("UITextSizeConstraint", "MinTextSize", luau::Value::Number((lower * max / upper).round()));
    out.set_pseudo_prop("UITextSizeConstraint", "MaxTextSize", luau::Value::Number(max.round()));
}

/// How an ImageLabel/ImageButton draws its Image: `object-fit`, `background-size` and
/// `background-repeat` choose the ScaleType, `image-rendering` the ResampleMode.
fn translate_images(decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    let keyword = |d: &Decl| d.value.as_str().map(str::to_ascii_lowercase);
    let scale_type = |t: &str| luau::Value::enum_item("ScaleType", t);
    if let Some(d) = last(decls, "object-fit") {
        match keyword(d).as_deref() {
            Some("fill") => out.set_prop("ScaleType", scale_type("Stretch")),
            Some("contain" | "scale-down") => out.set_prop("ScaleType", scale_type("Fit")),
            Some("cover") => out.set_prop("ScaleType", scale_type("Crop")),
            _ => diag.warn("`object-fit` has no Roblox equivalent for this value (ignored)", d.span.as_ref()),
        }
    }
    let no_repeat = last(decls, "background-repeat").and_then(keyword).is_some_and(|k| k == "no-repeat");
    if let Some(d) = last(decls, "background-size") {
        match keyword(d).as_deref() {
            Some("cover") => out.set_prop("ScaleType", scale_type("Crop")),
            Some("contain") => out.set_prop("ScaleType", scale_type("Fit")),
            Some("auto") => {}
            _ => {
                let items = space_items(&d.value);
                let axis = |v: &Value| match v.as_number() {
                    Some(n) => length_component(n).ok(),
                    None => None,
                };
                let x = items.first().and_then(axis);
                let y = items.get(1).map_or(x, axis);
                match (x, y) {
                    (Some((1.0, 0.0)), Some((1.0, 0.0))) => out.set_prop("ScaleType", scale_type("Stretch")),
                    (Some(_), Some(_)) if no_repeat => {
                        diag.warn(
                            "`background-size` without repeating stretches the image to the element (Roblox can't \
                             draw it once at a size)",
                            d.span.as_ref(),
                        );
                        out.set_prop("ScaleType", scale_type("Stretch"));
                    }
                    (Some((xs, xo)), Some((ys, yo))) => {
                        out.set_prop("ScaleType", scale_type("Tile"));
                        out.set_prop("TileSize", luau::Value::udim2(xs, xo, ys, yo));
                    }
                    _ => diag.warn("`background-size`: expected cover, contain or a size (ignored)", d.span.as_ref()),
                }
            }
        }
    }
    if let Some(d) = last(decls, "image-rendering") {
        match keyword(d).as_deref() {
            Some("pixelated" | "crisp-edges") => {
                out.set_prop("ResampleMode", luau::Value::enum_item("ResamplerMode", "Pixelated"))
            }
            Some("auto" | "smooth" | "high-quality") => {
                out.set_prop("ResampleMode", luau::Value::enum_item("ResamplerMode", "Default"))
            }
            _ => diag.warn("`image-rendering` has no Roblox equivalent for this value (ignored)", d.span.as_ref()),
        }
    }
}

/// `--strict`: CSS that compiles, but that Roblox lays out differently from a browser. Each warning
/// names the CSS that makes the two agree.
fn warn_layout_differences(decls: &[Decl], opts: &ApproxOptions, diag: &mut Diagnostics) {
    let is = |d: &Decl, word: &str| d.value.as_str().is_some_and(|s| s.eq_ignore_ascii_case(word));

    if opts.groups.contains(&Group::Size) {
        for name in ["width", "height"] {
            if let Some(d) = last(decls, name)
                && is(d, "auto")
            {
                diag.error(
                    format!(
                        "strict: `{name}: auto` fills the parent in CSS but fits the content in Roblox; write \
                         `{name}: fit-content`, or a length or percentage"
                    ),
                    d.span.as_ref(),
                );
            }
        }
    }

    // A UISizeConstraint caps a ScrollingFrame's canvas along with its window, so the canvas never
    // outgrows it; and AutomaticSize grows anything but text past its MaxSize (text wraps at it),
    // while the parent's layout still places the next sibling at the cap. Both measured in Studio.
    if opts.groups.contains(&Group::Size) {
        let (stretch_x, stretch_y) = stretched_size(decls, opts);
        let (scroll_x, scroll_y) =
            if opts.groups.contains(&Group::Visibility) { scroll_axes(decls) } else { (false, false) };
        let styles_text = decls.iter().any(|d| {
            let n = d.name.as_str();
            n.starts_with("font") || n.starts_with("text-") || matches!(n, "color" | "line-height" | "white-space")
        });
        let grows_from_zero = opts.groups.contains(&Group::Layout) && flex_grows(decls);
        for (axis, max, stretched, scrolls) in [
            ("width", "max-width", stretch_x.is_some(), scroll_x),
            ("height", "max-height", stretch_y.is_some(), scroll_y),
        ] {
            let Some(d) = last(decls, max).filter(|d| !is(d, "none")) else { continue };
            let size = last(decls, axis);
            let automatic = !stretched
                && size.is_none_or(|s| matches!(axis_length(&s.value), Ok(AxisResult::Auto)))
                && !(axis == "width" && size.is_none() && grows_from_zero);
            if scrolls {
                diag.error(
                    format!(
                        "strict: a `{max}` on a scroll container caps its canvas too in Roblox, so the content never \
                         scrolls; put the `{max}` on a wrapper and give the scroll container `{axis}: 100%`"
                    ),
                    d.span.as_ref(),
                );
            } else if automatic && !styles_text {
                diag.error(
                    format!(
                        "strict: Roblox grows an element sized by its content past its `{max}` (only text wraps at \
                         it), overlapping whatever the layout places after the cap; give it a fixed `{axis}`, or make \
                         it a scroll container inside a capped wrapper"
                    ),
                    d.span.as_ref(),
                );
            }
        }
    }

    if opts.groups.contains(&Group::Text) {
        let m = text_metrics(decls);
        if m.size.is_some() && m.family_ratio.is_none() {
            let d = decls.iter().rev().find(|d| matches!(d.name.as_str(), "font-size" | "font"));
            diag.error(
                "strict: Roblox's TextSize is the height of a line and CSS's font-size the em, and outlass only \
                 knows how the two relate for built-in families, so this text won't match CSS; set a built-in \
                 `font-family` (here or in a weaker rule)",
                d.and_then(|d| d.span.as_ref()),
            );
        }
        if let Some(d) = last(decls, "vertical-align") {
            diag.error(
                "strict: `vertical-align` doesn't align a block's content in CSS; use `align-content`",
                d.span.as_ref(),
            );
        }
    }

    if opts.groups.contains(&Group::Size) && opts.groups.contains(&Group::Box) {
        let pads = decls.iter().any(|d| d.name.starts_with("padding"));
        let sized = ["width", "height"]
            .iter()
            .filter_map(|n| last(decls, n))
            .find(|d| matches!(axis_length(&d.value), Ok(AxisResult::Value((s, o))) if s != 0.0 || o != 0.0));
        let border_box = last(decls, "box-sizing").is_some_and(|d| is(d, "border-box"));
        if pads
            && !border_box
            && let Some(d) = sized
        {
            diag.error(
                format!(
                    "strict: Roblox puts padding inside the `{}`, as `box-sizing: border-box` does, where CSS \
                     adds it on top; set `box-sizing: border-box`",
                    d.name
                ),
                d.span.as_ref(),
            );
        }
    }

    if opts.groups.contains(&Group::Layout) && is_flex(decls) {
        let stretches = last(decls, "align-items").is_none_or(|d| is(d, "stretch") || is(d, "normal"));
        let column = last(decls, "flex-direction")
            .is_some_and(|d| d.value.as_str().is_some_and(|s| s.to_ascii_lowercase().starts_with("column")));
        let cross = if column { "width" } else { "height" };
        let cross_decl = last(decls, cross);
        let fits = cross_decl.is_some_and(|d| matches!(axis_length(&d.value), Ok(AxisResult::Auto)));
        // Roblox stretches items to the largest item on the line; CSS stretches them to the
        // container. The two agree only when the container is itself sized by its largest item: a
        // row with no height of its own, or a column that is `width: fit-content`.
        let differs = if column { !fits } else { cross_decl.is_some() && !fits };
        if stretches && differs {
            let at = cross_decl.or(last(decls, "align-items")).or(last(decls, "display"));
            diag.error(
                format!(
                    "strict: Roblox stretches flex items to the largest item on the line, not to the container's \
                     {cross} as CSS does; set `align-items` (and give the items that should fill it \
                     `{cross}: 100%`), or make the container `{cross}: fit-content`"
                ),
                at.and_then(|d| d.span.as_ref()),
            );
        }
    }

    // In CSS, `position: relative` on a container is what anchors its absolutely positioned
    // children. Roblox's list and grid layouts place every child, `Position` or not, so such a
    // child would join the flow. outlass positions every element anyway, so the declaration can go.
    if opts.groups.contains(&Group::Layout)
        && is_flex_or_grid(decls)
        && let Some(d) = last(decls, "position")
        && is(d, "relative")
    {
        diag.error(
            "strict: an absolutely positioned child of a flex or grid container takes part in Roblox's layout; \
             put the positioned children in a wrapper without `display: flex`/`grid`, and drop \
             `position: relative` (every Roblox element is positioned already)",
            d.span.as_ref(),
        );
    }
}

fn warn_disabled_and_unknown(decls: &[Decl], opts: &ApproxOptions, diag: &mut Diagnostics) {
    for d in decls {
        match lookup(&d.name) {
            Some(doc) => {
                if !opts.groups.contains(&doc.group) {
                    diag.warn(
                        format!(
                            "`{}` is a CSS property; pass `--approx={}` to translate it (ignored)",
                            d.name,
                            doc.group.name()
                        ),
                        d.span.as_ref(),
                    );
                }
            }
            None => {
                let msg = match unknown_hint(&d.name) {
                    Some(hint) => format!("`{}` has no Roblox equivalent ({hint}) (ignored)", d.name),
                    None => format!("`{}` has no Roblox equivalent (ignored)", d.name),
                };
                diag.warn(msg, d.span.as_ref());
            }
        }
    }
}

/// The last declaration with this (already-stripped) name, i.e. the one that wins under CSS
/// cascade semantics for same-named declarations.
fn last<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Decl> {
    decls.iter().rev().find(|d| strip_vendor_prefix(&d.name) == name)
}

// ---------------------------------------------------------------------------------------------
// Shared value helpers
// ---------------------------------------------------------------------------------------------

fn unit_of(n: &Number) -> String {
    n.simple_unit().unwrap_or("").to_ascii_lowercase()
}

/// Converts a length to a `(scale, offset)` pair suitable for `UDim`/`UDim2`.
fn length_component(n: &Number) -> Result<(f64, f64), String> {
    let unit = unit_of(n);
    match unit.as_str() {
        "%" => Ok((n.value / 100.0, 0.0)),
        "vw" | "vh" | "vmin" | "vmax" => Err("viewport units have no Roblox equivalent".to_string()),
        _ => length_px(n).map(|px| (0.0, px)).ok_or_else(|| format!("unit `{unit}` has no Roblox equivalent")),
    }
}

/// Converts a length to plain pixels (no percentage support).
fn length_px(n: &Number) -> Option<f64> {
    let unit = unit_of(n);
    match unit.as_str() {
        "" | "px" => Some(n.value),
        "em" | "rem" => Some(n.value * roblox::REM_PX),
        _ => n.value_in("px"),
    }
}

fn strip_outer_parens(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with('(') && s.ends_with(')') { &s[1..s.len() - 1] } else { s }
}

fn split_num_unit(tok: &str) -> Option<(f64, String)> {
    let bytes = tok.as_bytes();
    let mut i = 0;
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let (num, unit) = tok.split_at(i);
    num.parse::<f64>().ok().map(|v| (v, unit.to_string()))
}

/// Parses a simplified `calc()` expression text (e.g. `"50% + 10px"`) into `(scale, offset)`.
fn parse_calc_text(text: &str) -> Result<(f64, f64), String> {
    let s = strip_outer_parens(text);
    let mut scale = 0.0;
    let mut offset = 0.0;
    let mut sign = 1.0;
    for tok in s.split_whitespace() {
        match tok {
            "+" => sign = 1.0,
            "-" => sign = -1.0,
            _ => {
                let (val, unit) = split_num_unit(tok).ok_or_else(|| format!("can't parse calc() term `{tok}`"))?;
                match unit.as_str() {
                    "" => return Err(format!("calc() term `{tok}` has no unit")),
                    "%" => scale += sign * val / 100.0,
                    "px" => offset += sign * val,
                    "em" | "rem" => offset += sign * val * roblox::REM_PX,
                    other => match Number::with_unit(val, other).value_in("px") {
                        Some(px) => offset += sign * px,
                        None => return Err(format!("unit `{other}` in calc() has no Roblox equivalent")),
                    },
                }
                sign = 1.0;
            }
        }
    }
    Ok((scale, offset))
}

/// Converts a `Value` (a `Number` or a `calc()` call) into a `(scale, offset)` length pair.
fn length_value(v: &Value) -> Result<(f64, f64), String> {
    match v {
        Value::Number(n) => length_component(n),
        Value::Call { name, args } if name == "calc" => {
            let text = args.first().and_then(|a| a.as_str()).ok_or("calc() expects a single expression string")?;
            parse_calc_text(text)
        }
        _ => Err(format!("unsupported length `{}`", v.inspect())),
    }
}

/// A plain pixel length (no percentages) for properties like `UIStroke.Thickness`.
fn px_only(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => {
            let unit = unit_of(n);
            if unit == "%" {
                return Err("percentages aren't supported here".to_string());
            }
            length_px(n).ok_or_else(|| format!("unit `{unit}` has no Roblox equivalent"))
        }
        _ => Err(format!("expected a length, got `{}`", v.inspect())),
    }
}

/// Splits a CSS shorthand list (space-separated, or a single value) into its items.
fn space_items(v: &Value) -> Vec<Value> {
    match v {
        Value::List { items, sep: ListSep::Space, .. } if !items.is_empty() => items.clone(),
        other => vec![other.clone()],
    }
}

/// A 1-4 value shorthand like `padding`/`inset`, in CSS order (top, right, bottom, left).
fn four_sides(v: &Value) -> [Value; 4] {
    let items = space_items(v);
    match items.len() {
        1 => {
            let a = items[0].clone();
            [a.clone(), a.clone(), a.clone(), a]
        }
        2 => {
            let a = items[0].clone();
            let b = items[1].clone();
            [a.clone(), b.clone(), a, b]
        }
        3 => {
            let a = items[0].clone();
            let b = items[1].clone();
            let c = items[2].clone();
            [a, b.clone(), c, b]
        }
        _ => [items[0].clone(), items[1].clone(), items[2].clone(), items[3].clone()],
    }
}

/// A 1-2 value shorthand like `padding-inline`/`gap`: (first, second-or-first).
fn two_sides(v: &Value) -> (Value, Value) {
    let items = space_items(v);
    if items.len() >= 2 { (items[0].clone(), items[1].clone()) } else { (items[0].clone(), items[0].clone()) }
}

/// Resolves a color value: a token reference (skips derived transparency, alpha = `None`), a
/// `Value::Color`, or the `none` keyword (fully transparent black).
fn resolve_color(v: &Value) -> Result<(luau::Value, Option<f64>), String> {
    if let Some(token) = roblox::token_reference(v) {
        return Ok((luau::Value::Token(token), None));
    }
    match v {
        Value::Color(c) => Ok((roblox::color3(c), Some(c.a))),
        Value::Str { text, quoted: false } if text.eq_ignore_ascii_case("none") => {
            Ok((roblox::color3(&Color::rgba(0.0, 0.0, 0.0, 1.0)), Some(0.0)))
        }
        _ => Err(format!("expected a color, got `{}`", v.inspect())),
    }
}

/// `1 - alpha * opacity`: the Roblox transparency a colour's alpha and `opacity` add up to.
///
/// This is always emitted by the rule that sets either input, opaque colours included. Roblox has
/// one Transparency slot where CSS has a colour alpha and an `opacity`, so a rule that leaves the
/// slot alone leaves whatever a weaker rule put there — which is how `background-color: <opaque>`
/// used to be unable to undo a weaker `background-color: transparent`.
fn compute_transparency(alpha: f64, opacity_factor: Option<f64>) -> f64 {
    (1.0 - alpha * opacity_factor.unwrap_or(1.0)).clamp(0.0, 1.0)
}

fn parse_opacity_value(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) if n.has_unit("%") => Ok((n.value / 100.0).clamp(0.0, 1.0)),
        Value::Number(n) if n.is_unitless() => Ok(n.value.clamp(0.0, 1.0)),
        _ => Err("opacity must be a number or percentage".to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Color + opacity
// ---------------------------------------------------------------------------------------------

fn is_solid_bg_value(v: &Value) -> bool {
    matches!(v, Value::Color(_)) || is_clear(v)
}

/// `none`, `transparent`, or any fully transparent colour.
fn is_clear(v: &Value) -> bool {
    match v {
        Value::Color(c) => c.a <= 1e-9,
        Value::Str { text, quoted: false } => {
            text.eq_ignore_ascii_case("none") || text.eq_ignore_ascii_case("transparent")
        }
        _ => false,
    }
}

/// Any CSS gradient function. The radial and conic ones have no Roblox equivalent, but they are
/// still recognised here so that they warn rather than falling through to "expected a color".
fn is_gradient_call(v: &Value) -> bool {
    matches!(v, Value::Call { name, .. } if name.ends_with("-gradient"))
}

/// The comma-separated layers of a `background` / `background-image`, topmost first.
fn layers(v: &Value) -> Vec<Value> {
    match v {
        Value::List { items, sep: ListSep::Comma, .. } if !items.is_empty() => items.clone(),
        other => vec![other.clone()],
    }
}

/// The layer a UIGradient would draw: the topmost gradient of the stack.
fn top_gradient(v: &Value) -> Option<Value> {
    layers(v).into_iter().find(is_gradient_call)
}

/// The layer BackgroundColor3 would hold: the bottom-most flat colour of the stack.
fn bottom_solid(v: &Value) -> Option<Value> {
    layers(v).into_iter().rev().find(is_solid_bg_value)
}

/// The gradients a UIGradient can draw: `linear-gradient()` and, written out stop by stop,
/// `repeating-linear-gradient()`.
fn is_linear_gradient_call(v: &Value) -> bool {
    matches!(v, Value::Call { name, .. } if name == "linear-gradient" || name == "repeating-linear-gradient")
}

fn white() -> luau::Value {
    roblox::color3(&Color::rgba(255.0, 255.0, 255.0, 1.0))
}

fn translate_color_opacity(
    decls: &[Decl],
    opts: &ApproxOptions,
    opacity_factor: Option<f64>,
    diag: &mut Diagnostics,
    out: &mut Translated,
) {
    let color_on = opts.groups.contains(&Group::Color);
    let opacity_on = opts.groups.contains(&Group::Opacity);

    // Gradient text: `background: linear-gradient(...); background-clip: text; color: transparent`.
    // A UIGradient under a TextLabel tints its text, so the text is drawn white and tinted.
    let clip_text = color_on
        && last(decls, "background-clip")
            .is_some_and(|d| d.value.as_str().is_some_and(|s| s.eq_ignore_ascii_case("text")));

    // TextColor3 / TextTransparency
    let mut text_alpha = 1.0;
    let mut have_text = false;
    if color_on {
        // -webkit-text-fill-color overrides color, as in browsers.
        if let Some(d) = last(decls, "text-fill-color").or_else(|| last(decls, "color")) {
            if clip_text && is_clear(&d.value) {
                // Part of the gradient-text idiom; the gradient provides the colour.
            } else {
                match resolve_color(&d.value) {
                    Ok((expr, alpha)) => {
                        out.set_prop("TextColor3", expr);
                        have_text = true;
                        if let Some(a) = alpha {
                            text_alpha = a;
                        }
                    }
                    Err(e) => diag.warn(format!("`{}`: {e} (ignored)", d.name), d.span.as_ref()),
                }
            }
        }
        if clip_text {
            out.set_prop("TextColor3", white());
            have_text = true;
            text_alpha = 1.0;
        }
    }
    if have_text || opacity_factor.is_some() {
        out.set_prop("TextTransparency", luau::Value::Number(compute_transparency(text_alpha, opacity_factor)));
    }

    // BackgroundColor3 / BackgroundTransparency / UIGradient / Image
    let mut bg_alpha = 1.0;
    let mut have_bg = false;
    let mut gradient: Option<Gradient> = None;
    let mut gradient_span: Option<Span> = None;
    let mut mask: Option<Gradient> = None;
    // The colour the gradient is painted over, when it's a colour we can actually read.
    let mut under: Option<Color> = None;
    let mut under_is_token = false;
    if color_on {
        let bg_color_decl = last(decls, "background-color");
        let bg_decl = last(decls, "background");
        let bg_image_decl = last(decls, "background-image");
        // CSS paints background layers front to back, a gradient over the background colour.
        // Roblox has one BackgroundColor3 and one UIGradient, and the UIGradient *multiplies* the
        // colour under it instead of covering it, so the layers are flattened into the gradient's
        // own stops further down and the element is painted white to make the multiply a no-op.
        let gradient_layer =
            [bg_decl, bg_image_decl].into_iter().flatten().find_map(|d| top_gradient(&d.value).map(|v| (d, v)));
        let solid = bg_color_decl
            .map(|d| (d, d.value.clone()))
            .or_else(|| bg_decl.and_then(|d| bottom_solid(&d.value).map(|v| (d, v))));
        if let Some((d, value)) = &solid {
            if is_clear(value) {
                // `none` / `transparent`: nothing to draw, so no colour to set.
                have_bg = true;
                bg_alpha = 0.0;
            } else {
                match resolve_color(value) {
                    Ok((expr, alpha)) => {
                        out.set_prop("BackgroundColor3", expr);
                        have_bg = true;
                        if let Some(a) = alpha {
                            bg_alpha = a;
                        }
                        match value {
                            Value::Color(c) => under = Some(c.clone()),
                            _ => under_is_token = true,
                        }
                    }
                    Err(e) => diag.warn(format!("`{}`: {e} (ignored)", d.name), d.span.as_ref()),
                }
            }
        }

        if let Some((d, value)) = &gradient_layer {
            let Value::Call { name, .. } = value else { unreachable!() };
            if is_linear_gradient_call(value) {
                gradient = parse_linear_gradient(value, diag, d.span.as_ref());
                gradient_span = d.span.clone();
            } else {
                diag.warn(
                    format!("`{name}()` has no Roblox equivalent; a UIGradient is linear (ignored)"),
                    d.span.as_ref(),
                );
            }
        }

        if let Some(d) = bg_image_decl
            && let Some(Value::Call { name, args }) = layers(&d.value).first()
            && name == "url"
            && let Some(text) = args.first().and_then(|a| a.as_str())
        {
            out.set_prop("Image", luau::Value::String(text.to_string()));
        }

        if let Some(d) = last(decls, "mask-image").or_else(|| last(decls, "mask")) {
            if is_linear_gradient_call(&d.value) {
                mask = parse_linear_gradient(&d.value, diag, d.span.as_ref());
            } else if !is_clear(&d.value) {
                diag.warn(
                    format!(
                        "`{}`: only linear-gradient() and repeating-linear-gradient() are supported here \
                             (ignored)",
                        d.name
                    ),
                    d.span.as_ref(),
                );
            }
            if let (Some(g), Some(m)) = (&gradient, &mask)
                && (g.rotation - m.rotation).abs() > 1e-6
            {
                diag.warn(
                    "a UIGradient has a single rotation: the mask uses the background gradient's angle",
                    d.span.as_ref(),
                );
            }
        }
    }

    // A UIGradient multiplies BackgroundColor3 and multiplies its Transparency into
    // BackgroundTransparency (both measured in Studio), where CSS paints `background-image` over
    // `background-color` and an opaque gradient hides the colour completely. Flattening the colour
    // into the stops and painting the element white reproduces the CSS result: the multiply becomes
    // a no-op and every layer is already in the sequence.
    if let Some(g) = &mut gradient
        && !clip_text
    {
        if let Some(under) = &under {
            g.composite_over(under);
        } else if under_is_token && g.has_alpha() {
            diag.warn(
                "a translucent gradient can't be flattened onto a `var()` background colour, so the \
                 colour is dropped (give the gradient opaque stops, or the element a literal colour)",
                gradient_span.as_ref(),
            );
        }
        // A UIGradient colours everything its element draws, text included (it's how gradient text
        // works), and there's no way to keep it to the background. Text over a gradient background
        // takes on the gradient's colours, which makes it the colour of the background under it.
        if decls.iter().any(|d| {
            matches!(strip_vendor_prefix(&d.name), "color" | "text-fill-color" | "font" | "font-family" | "font-size")
        }) {
            diag.warn(
                "a gradient background also tints this element's text (a UIGradient colours everything its \
                 element draws), which hides the text in the gradient; put the text in a child element over the \
                 gradient one",
                gradient_span.as_ref(),
            );
        }
        out.set_prop("BackgroundColor3", white());
        have_bg = true;
        bg_alpha = 1.0;
    }
    if gradient.is_some() || mask.is_some() {
        if let Some(g) = &gradient {
            out.set_pseudo_prop("UIGradient", "Color", g.color_sequence());
        }
        if let Some(t) = transparency_sequence(gradient.as_ref(), mask.as_ref()) {
            out.set_pseudo_prop("UIGradient", "Transparency", t);
        }
        let px = |name: &str| {
            last(decls, name).and_then(|d| match axis_length(&d.value) {
                Ok(AxisResult::Value((s, o))) if s == 0.0 && o > 0.0 => Some(o),
                _ => None,
            })
        };
        let size = px("width").zip(px("height"));
        let rotation = match gradient.as_ref().or(mask.as_ref()) {
            Some(g) => {
                // Only worth a warning when a mildly non-square element would already look
                // noticeably different.
                if size.is_none() && g.shape_divergence() > 10.0 {
                    diag.warn(
                        format!(
                            "a `{}deg` gradient's direction depends on the element's shape: a UIGradient is rotated \
                             as if its element were square, so without a px `width` and `height` outlass assumes \
                             one; give it px sizes, or use a right angle or a `to right`-style direction",
                            crate::value::format_number(g.angle.unwrap_or_default())
                        ),
                        gradient_span.as_ref(),
                    );
                }
                g.rotation_for(size)
            }
            None => 0.0,
        };
        out.set_pseudo_prop("UIGradient", "Rotation", luau::Value::Number(rotation));
    }

    if clip_text {
        out.set_prop("BackgroundTransparency", luau::Value::Number(1.0));
    } else if have_bg {
        // Without a known background there's nothing for `opacity` to scale: CSS fades whatever
        // background the element has, and guessing an opaque one here would paint over a weaker
        // rule's `background-color: transparent`.
        out.set_prop("BackgroundTransparency", luau::Value::Number(compute_transparency(bg_alpha, opacity_factor)));
    }

    if opacity_on && let Some(of) = opacity_factor {
        let t = luau::Value::Number((1.0 - of).clamp(0.0, 1.0));
        // ImageTransparency: only opacity drives this (no "image color" concept in CSS).
        out.set_prop("ImageTransparency", t.clone());
        // On a CanvasGroup this fades the element and its children together, exactly like CSS opacity.
        out.set_prop("GroupTransparency", t);
    }
}

fn direction_to_angle(dir: &[String]) -> f64 {
    let set: Vec<&str> = dir.iter().map(|s| s.as_str()).collect();
    match set.as_slice() {
        ["top"] => 0.0,
        ["right"] => 90.0,
        ["bottom"] => 180.0,
        ["left"] => 270.0,
        ["top", "right"] | ["right", "top"] => 45.0,
        ["bottom", "right"] | ["right", "bottom"] => 135.0,
        ["bottom", "left"] | ["left", "bottom"] => 225.0,
        ["top", "left"] | ["left", "top"] => 315.0,
        _ => 180.0,
    }
}

/// A Roblox `ColorSequence`/`NumberSequence` holds at most 20 keypoints.
const MAX_KEYPOINTS: usize = 20;

/// A parsed `linear-gradient()`: stops at positions 0..=1 and the UIGradient rotation in degrees.
struct Gradient {
    stops: Vec<(f64, Color)>,
    rotation: f64,
    /// The CSS angle, when the gradient gave one (`135deg`) rather than a direction keyword. A CSS
    /// angle is a real angle on the element, but a UIGradient is rotated as if its element were
    /// square, so the rotation depends on the element's shape (see `rotation_for`).
    angle: Option<f64>,
}

impl Gradient {
    /// The UIGradient rotation for an element `size` px wide and tall. A UIGradient works in the
    /// element's normalised space, where a CSS angle A points along (W sin A, -H cos A); on a
    /// square that is just A - 90. Direction keywords (`to bottom right`) already run corner to
    /// corner in both, as do right angles, so only other CSS angles need the size.
    fn rotation_for(&self, size: Option<(f64, f64)>) -> f64 {
        match (self.angle, size) {
            (Some(a), Some((w, h))) if w > 0.0 && h > 0.0 => {
                let a = a.to_radians();
                let r = (-h * a.cos()).atan2(w * a.sin()).to_degrees();
                (r * 1e6).round() / 1e6
            }
            _ => self.rotation,
        }
    }

    /// How far (in degrees) the direction moves when the element is 2:1 or 1:2 instead of square,
    /// which is what outlass has to assume without a px size. Near a right angle it barely moves;
    /// at 45 degrees it moves 18.
    fn shape_divergence(&self) -> f64 {
        let square = self.rotation_for(Some((1.0, 1.0)));
        let diff = |r: f64| {
            let d = (r - square).rem_euclid(360.0);
            d.min(360.0 - d)
        };
        diff(self.rotation_for(Some((2.0, 1.0)))).max(diff(self.rotation_for(Some((1.0, 2.0)))))
    }
}

impl Gradient {
    fn color_sequence(&self) -> luau::Value {
        luau::Value::ColorSequence(
            self.stops.iter().map(|(p, c)| (*p, luau::Color3 { r: c.r, g: c.g, b: c.b })).collect(),
        )
    }

    fn has_alpha(&self) -> bool {
        self.stops.iter().any(|(_, c)| c.a < 1.0 - 1e-9)
    }

    /// Paints the gradient over a solid colour, the way CSS layers a background image on top of
    /// the background colour. An opaque stop swallows the colour; a translucent one blends with it.
    fn composite_over(&mut self, under: &Color) {
        for (_, c) in &mut self.stops {
            *c = over(c, under);
        }
    }

    /// Alpha at `position`, interpolated linearly between stops.
    fn alpha_at(&self, position: f64) -> f64 {
        color_at(&self.stops, position).a
    }
}

/// `UIGradient.Transparency` combining a background gradient's own alpha with a mask's alpha.
fn transparency_sequence(gradient: Option<&Gradient>, mask: Option<&Gradient>) -> Option<luau::Value> {
    let sources: Vec<&Gradient> = [gradient.filter(|g| g.has_alpha()), mask].into_iter().flatten().collect();
    if sources.is_empty() {
        return None;
    }
    let mut positions: Vec<f64> = sources.iter().flat_map(|g| g.stops.iter().map(|(p, _)| *p)).collect();
    positions.sort_by(|a, b| a.total_cmp(b));
    positions.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let alpha_at = |p: f64| -> f64 { sources.iter().map(|g| g.alpha_at(p)).product() };
    let mut keypoints: Vec<(f64, f64)> = Vec::new();
    for p in positions {
        // Two stops at one position are a hard stop: keep both sides of the step, which Roblox
        // allows as two keypoints with the same time.
        let stepped = sources.iter().any(|g| g.stops.iter().filter(|(q, _)| (q - p).abs() < 1e-9).count() > 1);
        let left = alpha_at(p - 1e-9);
        let right = alpha_at(p);
        if stepped && (left - right).abs() > 1e-9 && p > 1e-9 {
            keypoints.push((p, left));
        }
        keypoints.push((p, right));
    }
    if keypoints.len() > MAX_KEYPOINTS {
        keypoints.truncate(MAX_KEYPOINTS - 1);
        let held = keypoints[MAX_KEYPOINTS - 2].1;
        keypoints.push((1.0, held));
    }
    Some(luau::Value::NumberSequence(
        keypoints.iter().map(|(p, alpha)| (*p, (1.0 - alpha).clamp(0.0, 1.0), 0.0)).collect(),
    ))
}

/// The colour a stop list shows at `position`, clamped at both ends. Where two stops share a
/// position (a hard stop) this is the colour on its right, which is what the gradient draws from
/// there on.
fn color_at(stops: &[(f64, Color)], position: f64) -> Color {
    let last = &stops[stops.len() - 1];
    if position <= stops[0].0 {
        return stops[0].1.clone();
    }
    if position >= last.0 {
        return last.1.clone();
    }
    // The last segment that starts at or before `position`.
    let i = stops[..stops.len() - 1].iter().rposition(|(q, _)| *q <= position).unwrap_or(0);
    let (p0, c0) = &stops[i];
    let (p1, c1) = &stops[i + 1];
    if p1 - p0 <= 1e-9 {
        return c1.clone();
    }
    mix(c0, c1, (position - p0) / (p1 - p0))
}

/// Source-over compositing: `src` painted on top of `dst`, as CSS stacks background layers.
fn over(src: &Color, dst: &Color) -> Color {
    let alpha = src.a + dst.a * (1.0 - src.a);
    if alpha <= 1e-9 {
        return Color::rgba(0.0, 0.0, 0.0, 0.0);
    }
    let channel = |s: f64, d: f64| (s * src.a + d * dst.a * (1.0 - src.a)) / alpha;
    Color::rgba(channel(src.r, dst.r), channel(src.g, dst.g), channel(src.b, dst.b), alpha)
}

/// Linear interpolation in sRGB, like a CSS gradient's legacy `srgb` interpolation space.
fn mix(a: &Color, b: &Color, t: f64) -> Color {
    let lerp = |x: f64, y: f64| x + (y - x) * t;
    Color::rgba(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b), lerp(a.a, b.a))
}

/// One item of a gradient's stop list: a colour with zero, one or two positions, or a bare
/// position, which CSS calls a colour hint (where the fade around it is half done).
enum StopItem {
    Stop { color: Color, positions: Vec<f64> },
    Hint(f64),
}

/// A gradient stop position: a percentage, or a plain `0`. Lengths (`10px`) would need the
/// element's size, which a StyleRule doesn't know.
fn stop_position(n: &Number) -> Option<f64> {
    if n.has_unit("%") {
        Some(n.value / 100.0)
    } else if n.is_unitless() && n.value == 0.0 {
        Some(0.0)
    } else {
        None
    }
}

fn parse_stop_item(v: &Value, diag: &mut Diagnostics, span: Option<&Span>) -> Option<StopItem> {
    let mut position = |n: &Number| match stop_position(n) {
        Some(p) => Some(p),
        None => {
            diag.warn(format!("gradient stop position `{}` needs a percentage (ignored)", n.to_css()), span);
            None
        }
    };
    match v {
        Value::Color(c) => Some(StopItem::Stop { color: c.clone(), positions: Vec::new() }),
        Value::Number(n) => position(n).map(StopItem::Hint),
        Value::List { items: parts, sep: ListSep::Space, .. } => {
            let color = parts.iter().find_map(|p| if let Value::Color(c) = p { Some(c.clone()) } else { None })?;
            let mut positions: Vec<f64> = parts.iter().filter_map(|p| p.as_number()).filter_map(position).collect();
            positions.truncate(2);
            Some(StopItem::Stop { color, positions })
        }
        _ => None,
    }
}

/// Resolves a stop list onto the gradient line the way CSS does: an unpositioned first or last
/// stop sits at each end, a run of unpositioned stops is spread evenly between its neighbours, and
/// a position may never go backwards. Colour hints become an explicit stop holding the colour the
/// fade is halfway to.
fn resolve_stops(items: Vec<StopItem>) -> Vec<(f64, Color)> {
    // `<color> <pos> <pos>` is two stops of the same colour: the CSS hard-stop shorthand.
    let mut colors: Vec<Color> = Vec::new();
    let mut positions: Vec<Option<f64>> = Vec::new();
    // Hints, as (index of the stop they follow, position).
    let mut hints: Vec<(usize, f64)> = Vec::new();
    for item in items {
        match item {
            StopItem::Stop { color, positions: given } if given.is_empty() => {
                colors.push(color);
                positions.push(None);
            }
            StopItem::Stop { color, positions: given } => {
                for p in given {
                    colors.push(color.clone());
                    positions.push(Some(p));
                }
            }
            StopItem::Hint(p) => {
                if !colors.is_empty() {
                    hints.push((colors.len() - 1, p));
                }
            }
        }
    }
    if colors.is_empty() {
        return Vec::new();
    }
    if colors.len() == 1 {
        let only = colors[0].clone();
        return vec![(0.0, only.clone()), (1.0, only)];
    }
    let last = positions.len() - 1;
    positions[0].get_or_insert(0.0);
    positions[last].get_or_insert(1.0);
    // Positions never decrease.
    let mut floor = f64::NEG_INFINITY;
    for p in positions.iter_mut().flatten() {
        *p = p.max(floor);
        floor = *p;
    }
    // Spread each run of unpositioned stops evenly between the positioned stops around it.
    let mut i = 1;
    while i < positions.len() {
        if positions[i].is_some() {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while positions[end].is_none() {
            end += 1;
        }
        let before = positions[start - 1].unwrap();
        let after = positions[end].unwrap();
        let steps = (end - start + 1) as f64;
        for (k, slot) in positions[start..end].iter_mut().enumerate() {
            *slot = Some(before + (after - before) * (k as f64 + 1.0) / steps);
        }
        i = end;
    }
    let mut stops: Vec<(f64, Color)> = positions.into_iter().map(Option::unwrap).zip(colors).collect();
    // A hint says where the two colours around it are mixed half and half. One extra stop is not
    // the CSS easing curve, but it does put the midpoint where the author asked for it.
    for (after, pos) in hints.into_iter().rev() {
        if after + 1 >= stops.len() {
            continue;
        }
        let (p0, c0) = stops[after].clone();
        let (p1, c1) = stops[after + 1].clone();
        let pos = pos.clamp(p0, p1);
        if pos - p0 > 1e-9 && p1 - pos > 1e-9 {
            stops.insert(after + 1, (pos, mix(&c0, &c1, 0.5)));
        }
    }
    stops
}

/// Tiles a `repeating-linear-gradient()`'s pattern across the whole gradient line. The pattern is
/// the span between its first and last stop; CSS repeats it in both directions forever, so this
/// writes out by hand every copy that shows up in 0..=1.
fn repeat_stops(stops: &[(f64, Color)]) -> Vec<(f64, Color)> {
    let first = stops[0].0;
    let period = stops[stops.len() - 1].0 - first;
    let from = ((0.0 - first) / period).floor() as i64;
    let to = ((1.0 - first) / period).ceil() as i64;
    let mut out: Vec<(f64, Color)> = Vec::new();
    for k in from..=to {
        let offset = k as f64 * period;
        for (p, c) in stops {
            let p = p + offset;
            // Consecutive copies meet at one position. Both stops stay, since the step from the
            // last colour back to the first is the hard edge that makes the pattern repeat; only
            // an identical pair would be redundant.
            let same = out.last().is_some_and(|(lp, lc): &(f64, Color)| {
                (lp - p).abs() < 1e-9
                    && (lc.r - c.r).abs() < 1e-9
                    && (lc.g - c.g).abs() < 1e-9
                    && (lc.b - c.b).abs() < 1e-9
                    && (lc.a - c.a).abs() < 1e-9
            });
            if !same {
                out.push((p, c.clone()));
            }
        }
        if out.last().is_some_and(|(p, _)| *p >= 1.0) {
            break;
        }
    }
    out
}

/// Clips a stop list to the 0..=1 gradient line, which is the only part a UIGradient draws, and
/// which Roblox requires a sequence to span exactly.
fn clip_to_line(stops: Vec<(f64, Color)>) -> Vec<(f64, Color)> {
    let head = color_at(&stops, 0.0);
    let tail = color_at(&stops, 1.0);
    let mut out: Vec<(f64, Color)> = stops.into_iter().filter(|(p, _)| *p > 1e-9 && *p < 1.0 - 1e-9).collect();
    out.insert(0, (0.0, head));
    out.push((1.0, tail));
    out
}

fn parse_linear_gradient(v: &Value, diag: &mut Diagnostics, span: Option<&Span>) -> Option<Gradient> {
    let Value::Call { name, args } = v else { return None };
    let repeating = name.starts_with("repeating-");
    if args.is_empty() {
        diag.warn(format!("`{name}()` has no arguments (ignored)"), span);
        return None;
    }
    let mut angle_deg = 180.0; // default "to bottom"
    let mut css_angle: Option<f64> = None;
    let mut idx = 0;
    match &args[0] {
        Value::Number(n) => {
            angle_deg = n.value_in("deg").unwrap_or(n.value);
            css_angle = Some(angle_deg);
            idx = 1;
        }
        Value::List { items: parts, sep: ListSep::Space, .. }
            if parts.first().and_then(|p| p.as_str()) == Some("to") =>
        {
            let dir: Vec<String> =
                parts.iter().skip(1).filter_map(|p| p.as_str().map(|s| s.to_ascii_lowercase())).collect();
            angle_deg = direction_to_angle(&dir);
            idx = 1;
        }
        _ => {}
    }
    let mut items = Vec::new();
    for s in &args[idx..] {
        match parse_stop_item(s, diag, span) {
            Some(item) => items.push(item),
            None => diag.warn(format!("unsupported `{name}()` color stop (ignored)"), span),
        }
    }
    let mut stops = resolve_stops(items);
    if stops.is_empty() {
        diag.warn(format!("`{name}()` has no color stops (ignored)"), span);
        return None;
    }
    if repeating {
        if stops[stops.len() - 1].0 - stops[0].0 <= 1e-9 {
            diag.warn(
                format!("`{name}()` repeats a pattern of no width; give its stops percentage positions (drawn once)"),
                span,
            );
        } else {
            stops = repeat_stops(&stops);
        }
    }
    stops = clip_to_line(stops);
    if stops.len() > MAX_KEYPOINTS {
        let needed = stops.len();
        stops.truncate(MAX_KEYPOINTS - 1);
        let held = stops[MAX_KEYPOINTS - 2].1.clone();
        stops.push((1.0, held));
        diag.warn(
            format!(
                "`{name}()` needs {needed} keypoints but a Roblox sequence holds {MAX_KEYPOINTS}; the last color \
                 reached is held to the end"
            ),
            span,
        );
    }
    let mut rotation = angle_deg - 90.0;
    rotation = ((rotation + 180.0).rem_euclid(360.0)) - 180.0;
    if rotation <= -180.0 {
        rotation += 360.0;
    }
    Some(Gradient { stops, rotation, angle: css_angle })
}

// ---------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------

fn nearest_weight(n: f64) -> &'static str {
    const TABLE: &[(f64, &str)] = &[
        (100.0, "Thin"),
        (200.0, "ExtraLight"),
        (300.0, "Light"),
        (400.0, "Regular"),
        (500.0, "Medium"),
        (600.0, "SemiBold"),
        (700.0, "Bold"),
        (800.0, "ExtraBold"),
        (900.0, "Heavy"),
    ];
    TABLE.iter().min_by(|a, b| (a.0 - n).abs().partial_cmp(&(b.0 - n).abs()).unwrap()).unwrap().1
}

fn font_weight_enum(v: &Value) -> Result<&'static str, String> {
    match v {
        Value::Number(n) => Ok(nearest_weight(n.value)),
        Value::Str { text, .. } => match text.to_ascii_lowercase().as_str() {
            "normal" => Ok("Regular"),
            "bold" => Ok("Bold"),
            "bolder" => Ok("Bold"),
            "lighter" => Ok("Light"),
            other => Err(format!("unknown font-weight `{other}`")),
        },
        _ => Err("font-weight must be a number or keyword".to_string()),
    }
}

fn font_style_enum(v: &Value) -> Result<&'static str, String> {
    match v.as_str().map(|s| s.to_ascii_lowercase()) {
        Some(s) if s == "italic" || s == "oblique" => Ok("Italic"),
        Some(s) if s == "normal" => Ok("Normal"),
        Some(other) => Err(format!("unknown font-style `{other}`")),
        None => Err("font-style must be a keyword".to_string()),
    }
}

/// The family names of a `font-family` list, in order.
fn family_texts(v: &Value) -> Vec<String> {
    let items = match v {
        Value::List { items, sep: ListSep::Comma, .. } => items.clone(),
        other => vec![other.clone()],
    };
    items.iter().filter_map(family_text).collect()
}

fn family_text(v: &Value) -> Option<String> {
    match v {
        Value::Str { text, .. } => Some(text.clone()),
        Value::List { items, sep: ListSep::Space, .. } => {
            let words: Vec<&str> = items.iter().filter_map(|i| i.as_str()).collect();
            if words.is_empty() { None } else { Some(words.join(" ")) }
        }
        _ => None,
    }
}

/// The families under `rbxasset://fonts/families/`: every `Enum.Font` family plus the ones Roblox
/// ships without an enum item. Measured in Studio (2026-09-23) by preloading each one; a name
/// outside this list, like `Gotham` or `Inter`, fails to load.
const ROBLOX_FONT_FAMILIES: &[&str] = &[
    "AccanthisADFStd",
    "AmaticSC",
    "Arial",
    "Arimo",
    "Balthazar",
    "Bangers",
    "BuilderExtended",
    "BuilderMono",
    "BuilderSans",
    "ComicNeueAngular",
    "Creepster",
    "DenkOne",
    "Fondamento",
    "FredokaOne",
    "GothamSSm",
    "GrenzeGotisch",
    "Guru",
    "HighwayGothic",
    "Inconsolata",
    "IndieFlower",
    "JosefinSans",
    "Jura",
    "Kalam",
    "LegacyArial",
    "LegacyArimo",
    "LuckiestGuy",
    "Merriweather",
    "Michroma",
    "Montserrat",
    "NotoSansCJKFallback",
    "Nunito",
    "Oswald",
    "PatrickHand",
    "PermanentMarker",
    "PressStart2P",
    "Roboto",
    "RobotoCondensed",
    "RobotoMono",
    "RomanAntique",
    "Sarpanch",
    "SourceSansPro",
    "SpecialElite",
    "TitilliumWeb",
    "Ubuntu",
    "Zekton",
];

const FAMILIES_PREFIX: &str = "rbxasset://fonts/families/";

/// Each built-in family's line height over its em size, `(ascent + descent) / unitsPerEm` from the
/// regular face's hhea table. Roblox's TextSize is the height of that line, not the em that CSS's
/// `font-size` names (measured in Studio 2026-09: Source Sans Pro at TextSize 100 has an 80px em),
/// so a CSS font size becomes `TextSize = font-size x ratio`. Browsers use the same metrics for
/// `line-height: normal`, which therefore matches too. The Legacy families size text differently
/// and aren't listed, nor are those whose font files Studio doesn't ship (GothamSSm, Arial).
const FAMILY_LINE_RATIOS: &[(&str, f64)] = &[
    ("AccanthisADFStd", 1.168),
    ("AmaticSC", 1.261),
    ("Arimo", 1.1172),
    ("Balthazar", 1.018),
    ("Bangers", 1.064),
    ("BuilderExtended", 1.26),
    ("BuilderMono", 1.26),
    ("BuilderSans", 1.26),
    ("ComicNeueAngular", 1.209),
    ("Creepster", 1.1689),
    ("DenkOne", 1.25),
    ("Fondamento", 1.3838),
    ("FredokaOne", 1.21),
    ("GrenzeGotisch", 1.48),
    ("Guru", 1.1846),
    ("HighwayGothic", 1.107),
    ("Inconsolata", 1.049),
    ("IndieFlower", 1.459),
    ("JosefinSans", 1.0),
    ("Jura", 1.183),
    ("Kalam", 1.594),
    ("LuckiestGuy", 1.0),
    ("Merriweather", 1.257),
    ("Michroma", 1.4219),
    ("Montserrat", 1.219),
    ("Nunito", 1.364),
    ("Oswald", 1.482),
    ("PatrickHand", 1.354),
    ("PermanentMarker", 1.4268),
    ("PressStart2P", 1.0244),
    ("Roboto", 1.1719),
    ("RobotoCondensed", 1.1719),
    ("RobotoMono", 1.3188),
    ("RomanAntique", 1.049),
    ("Sarpanch", 1.4),
    ("SourceSansPro", 1.257),
    ("SpecialElite", 1.0),
    ("TitilliumWeb", 1.521),
    ("Ubuntu", 1.121),
    ("Zekton", 1.2),
];

/// The line-height-to-em ratio of a resolved font asset, when it's a built-in family outlass knows.
fn family_line_ratio(asset: &str) -> Option<f64> {
    let stem = asset.strip_prefix(FAMILIES_PREFIX)?.strip_suffix(".json")?;
    FAMILY_LINE_RATIOS.iter().find(|(f, _)| f.eq_ignore_ascii_case(stem)).map(|(_, r)| *r)
}

/// Roblox's largest TextSize.
const MAX_TEXT_SIZE: f64 = 100.0;

/// A family name as Roblox files it: `Source Sans Pro` -> `SourceSansPro`, matched ignoring case.
fn builtin_family(name: &str) -> Option<&'static str> {
    let key: String = name.chars().filter(|c| !c.is_whitespace()).collect();
    ROBLOX_FONT_FAMILIES.iter().copied().find(|f| f.eq_ignore_ascii_case(&key))
}

/// A built-in family close to `name`, for a "did you mean" hint: `Gotham` -> `GothamSSm`.
fn similar_family(name: &str) -> Option<&'static str> {
    let key = name.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    if key.len() < 3 {
        return None;
    }
    ROBLOX_FONT_FAMILIES
        .iter()
        .copied()
        .map(|f| {
            let lower = f.to_ascii_lowercase();
            let d = if lower.starts_with(&key) || key.starts_with(&lower) { 0 } else { edit_distance(&key, &lower) };
            (d, f)
        })
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, f)| f)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// The family asset for a `font-family` list. Like a browser, the first family that exists wins,
/// and the default font stands in when none does. Uploaded fonts (`rbxassetid://`) and other
/// asset paths are taken as given; a name that isn't a built-in Roblox family is warned about,
/// since Roblox would silently draw its fallback font instead.
fn resolve_families(families: &[String], diag: &mut Diagnostics, span: Option<&Span>) -> String {
    let mut missing: Vec<&str> = Vec::new();
    let mut chosen = None;
    for name in families {
        let found = if let Some(stem) = name.strip_prefix(FAMILIES_PREFIX).and_then(|r| r.strip_suffix(".json")) {
            builtin_family(stem).map(|_| name.clone())
        } else if name.starts_with("rbxasset://") || name.starts_with("rbxassetid://") {
            Some(name.clone())
        } else {
            match name.to_ascii_lowercase().as_str() {
                "sans-serif" | "system-ui" | "ui-sans-serif" => Some(DEFAULT_FONT.to_string()),
                "serif" | "ui-serif" => Some(format!("{FAMILIES_PREFIX}Merriweather.json")),
                "monospace" | "ui-monospace" => Some(format!("{FAMILIES_PREFIX}RobotoMono.json")),
                _ => builtin_family(name).map(|f| format!("{FAMILIES_PREFIX}{f}.json")),
            }
        };
        match found {
            Some(asset) => {
                chosen = Some((name.as_str(), asset));
                break;
            }
            None => missing.push(name),
        }
    }
    if !missing.is_empty() {
        let names = missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ");
        let (what, them) = if missing.len() == 1 {
            ("is not a built-in Roblox font", "it")
        } else {
            ("are not built-in Roblox fonts", "them")
        };
        let hint = missing
            .iter()
            .find_map(|m| {
                let bare = m.strip_prefix(FAMILIES_PREFIX).and_then(|r| r.strip_suffix(".json")).unwrap_or(m);
                similar_family(bare).map(|f| format!(" (did you mean `{f}`?)"))
            })
            .unwrap_or_default();
        let using = match &chosen {
            Some((name, _)) => format!("`{name}`"),
            None => "the default font".to_string(),
        };
        diag.warn(
            format!(
                "{names} {what}{hint}; upload {them} and use the `rbxassetid://` id, or pick a built-in family \
                 (using {using})"
            ),
            span,
        );
    }
    chosen.map_or_else(|| DEFAULT_FONT.to_string(), |(_, asset)| asset)
}

fn font_size_value(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => {
            let unit = unit_of(n);
            match unit.as_str() {
                "" | "px" => Ok(n.value),
                "em" | "rem" => Ok(n.value * roblox::REM_PX),
                "%" => Ok(roblox::REM_PX * n.value / 100.0),
                _ => n.value_in("px").ok_or_else(|| format!("unit `{unit}` has no Roblox equivalent")),
            }
        }
        _ => Err("font-size must be a number".to_string()),
    }
}

/// The parts of a `font` shorthand: weight, style, family asset, size in px, line-height ratio.
type FontShorthand = (Option<&'static str>, Option<&'static str>, Option<String>, Option<f64>, Option<f64>);

/// Parses the `font` shorthand: `[style] [weight] size[/line-height] family`.
fn parse_font_shorthand(v: &Value, diag: &mut Diagnostics, span: Option<&Span>) -> Result<FontShorthand, String> {
    // `bold 20px "Inter", sans-serif`: the fallback families come after the first comma.
    let (head, fallbacks): (Value, Vec<String>) = match v {
        Value::List { items, sep: ListSep::Comma, .. } if !items.is_empty() => {
            (items[0].clone(), items[1..].iter().filter_map(family_text).collect())
        }
        other => (other.clone(), Vec::new()),
    };
    let items = space_items(&head);
    let mut weight = None;
    let mut style = None;
    let mut size = None;
    let mut line_height = None;
    let mut family_words: Vec<String> = Vec::new();
    let mut past_size = false;
    for item in &items {
        if past_size {
            if let Some(s) = item.as_str() {
                family_words.push(s.to_string());
            }
            continue;
        }
        // The size term may be a `size/line-height` slash list.
        if let Value::List { items: parts, sep: ListSep::Slash, .. } = item
            && let (Some(sz), Some(lh)) = (parts.first(), parts.get(1))
        {
            size = Some(font_size_value(sz)?);
            line_height = match lh {
                Value::Number(n) if n.is_unitless() => Some(n.value),
                Value::Number(n) if n.has_unit("%") => Some(n.value / 100.0),
                _ => None,
            };
            past_size = true;
            continue;
        }
        if let Value::Number(n) = item {
            let is_weight_like = weight.is_none()
                && n.is_unitless()
                && (100.0..=900.0).contains(&n.value)
                && ((n.value / 100.0).fract()).abs() < 1e-9;
            if is_weight_like {
                weight = Some(nearest_weight(n.value));
            } else {
                size = Some(font_size_value(item)?);
                past_size = true;
            }
            continue;
        }
        if let Some(s) = item.as_str() {
            match s.to_ascii_lowercase().as_str() {
                "italic" | "oblique" => style = Some("Italic"),
                "normal" => {} // ambiguous between font-style/font-weight; both defaults are already "Normal"/"Regular"
                "bold" => weight = Some("Bold"),
                "bolder" => weight = Some("Bold"),
                "lighter" => weight = Some("Light"),
                _ => {
                    // Not a recognized style/weight keyword before the size: treat it as the
                    // (unusually early) start of the family list rather than erroring out.
                    family_words.push(s.to_string());
                    past_size = true;
                }
            }
        }
    }
    if size.is_none() {
        return Err("the `font` shorthand requires a size".to_string());
    }
    let family = (!family_words.is_empty()).then(|| {
        let families: Vec<String> = std::iter::once(family_words.join(" ")).chain(fallbacks).collect();
        resolve_families(&families, diag, span)
    });
    Ok((weight, style, family, size, line_height))
}

/// A rule's effective font size (px), line height and family, in cascade order.
struct TextMetrics {
    size: Option<f64>,
    /// As a multiple of the font size; `None` for `normal` (or unknown).
    line_height: Option<f64>,
    /// The family's line-height-to-em ratio (see FAMILY_LINE_RATIOS); `None` when it isn't known.
    family_ratio: Option<f64>,
}

impl TextMetrics {
    /// The TextSize that draws `font-size` at the size a browser does. Roblox floors TextSize to
    /// whole pixels, so it's rounded here, and it tops out at 100.
    fn text_size(&self) -> Option<f64> {
        Some((self.size? * self.family_ratio.unwrap_or(1.0)).round().min(MAX_TEXT_SIZE))
    }

    /// The height of one CSS line box, in px.
    fn line_px(&self) -> Option<f64> {
        Some(self.line_height? * self.size?)
    }
}

fn text_metrics(decls: &[Decl]) -> TextMetrics {
    enum LineHeight {
        Ratio(f64),
        /// Resolved against the final font size.
        Px(f64),
        Normal,
    }
    let mut size = None;
    let mut line_height = None;
    let mut family: Option<String> = None;
    // A weight or style without a family uses the default font (see translate_text).
    let mut any_font = false;
    for d in decls {
        match strip_vendor_prefix(&d.name) {
            "font-size" => size = font_size_value(&d.value).ok().or(size),
            "font-family" => {
                let families = family_texts(&d.value);
                if !families.is_empty() {
                    family = Some(resolve_families(&families, &mut Diagnostics::default(), None));
                }
            }
            "font-weight" | "font-style" => any_font = true,
            "font" => {
                if let Ok((_, _, fam, sz, lh)) = parse_font_shorthand(&d.value, &mut Diagnostics::default(), None) {
                    size = sz.or(size);
                    family = fam.or(family);
                    any_font = true;
                    // The shorthand resets an unstated line-height to `normal`.
                    line_height = Some(lh.map_or(LineHeight::Normal, LineHeight::Ratio));
                }
            }
            "line-height" => {
                line_height = Some(match &d.value {
                    Value::Number(n) if n.is_unitless() => LineHeight::Ratio(n.value),
                    Value::Number(n) if n.has_unit("%") => LineHeight::Ratio(n.value / 100.0),
                    Value::Number(n) => length_px(n).map_or(LineHeight::Normal, LineHeight::Px),
                    _ => LineHeight::Normal,
                })
            }
            _ => {}
        }
    }
    let ratio = match line_height {
        Some(LineHeight::Ratio(r)) => Some(r),
        Some(LineHeight::Px(px)) => size.filter(|s| *s > 0.0).map(|s| px / s),
        _ => None,
    };
    let family = family.or_else(|| any_font.then(|| DEFAULT_FONT.to_string()));
    TextMetrics { size, line_height: ratio, family_ratio: family.as_deref().and_then(family_line_ratio) }
}

/// CSS centres every line in a box `line-height` tall. Roblox makes the first line exactly
/// TextSize tall and applies LineHeight only to the lines after it, so a CSS text box is taller by
/// half the leading above and below: `(line-height x font-size - TextSize) / 2` each. Returns the
/// (top, bottom) padding for that, when both are known. Roblox rounds padding to whole pixels, so
/// the leading is split into whole pixels that add up to it (a -3px leading as -1.5 each would
/// lose a pixel).
fn half_leading(decls: &[Decl]) -> Option<(f64, f64)> {
    let m = text_metrics(decls);
    let leading = (m.line_px()? - m.text_size()?).round();
    let top = (leading / 2.0).floor();
    (leading != 0.0).then_some((top, leading - top))
}

fn translate_text(decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    let mut weight: Option<&'static str> = None;
    let mut style: Option<&'static str> = None;
    let mut family: Option<String> = None;
    let mut have_font = false;
    let mut font_size_px: Option<f64> = None;
    let mut stroke = StrokeState::default();
    let mut stroke_present = false;

    for d in decls {
        match strip_vendor_prefix(&d.name) {
            "font-size" => match font_size_value(&d.value) {
                Ok(px) => font_size_px = Some(px),
                Err(e) => diag.warn(format!("`font-size`: {e} (ignored)"), d.span.as_ref()),
            },
            "font-family" => {
                let families = family_texts(&d.value);
                if families.is_empty() {
                    diag.warn("`font-family`: unsupported value (ignored)", d.span.as_ref());
                } else {
                    family = Some(resolve_families(&families, diag, d.span.as_ref()));
                    have_font = true;
                }
            }
            "font-weight" => match font_weight_enum(&d.value) {
                Ok(w) => {
                    weight = Some(w);
                    have_font = true;
                }
                Err(e) => diag.warn(format!("`font-weight`: {e} (ignored)"), d.span.as_ref()),
            },
            "font-style" => match font_style_enum(&d.value) {
                Ok(s) => {
                    style = Some(s);
                    have_font = true;
                }
                Err(e) => diag.warn(format!("`font-style`: {e} (ignored)"), d.span.as_ref()),
            },
            "font" => match parse_font_shorthand(&d.value, diag, d.span.as_ref()) {
                // The size and line-height become TextSize and LineHeight after the loop.
                Ok((w, s, fam, sz, _)) => {
                    if let Some(w) = w {
                        weight = Some(w);
                    }
                    if let Some(s) = s {
                        style = Some(s);
                    }
                    if let Some(fam) = fam {
                        family = Some(fam);
                    }
                    if sz.is_some() {
                        font_size_px = sz;
                    }
                    have_font = true;
                }
                Err(e) => diag.warn(format!("`font`: {e} (ignored)"), d.span.as_ref()),
            },
            "text-align" => {
                if let Some(s) = d.value.as_str() {
                    let lower = s.to_ascii_lowercase();
                    if lower == "justify" {
                        diag.warn("`text-align: justify` has no Roblox equivalent; using Left", d.span.as_ref());
                    }
                    let e = match lower.as_str() {
                        "left" | "start" | "justify" => Some("Left"),
                        "center" => Some("Center"),
                        "right" | "end" => Some("Right"),
                        _ => None,
                    };
                    match e {
                        Some(e) => out.set_prop("TextXAlignment", luau::Value::enum_item("TextXAlignment", e)),
                        None => diag.warn(format!("unknown `text-align: {s}` (ignored)"), d.span.as_ref()),
                    }
                }
            }
            "vertical-align" => {
                if let Some(s) = d.value.as_str() {
                    let lower = s.to_ascii_lowercase();
                    let e = match lower.as_str() {
                        "top" | "text-top" => Some("Top"),
                        "middle" | "center" => Some("Center"),
                        "bottom" | "text-bottom" => Some("Bottom"),
                        _ => None,
                    };
                    match e {
                        Some(e) => out.set_prop("TextYAlignment", luau::Value::enum_item("TextYAlignment", e)),
                        None => diag.warn(format!("unknown `vertical-align: {s}` (ignored)"), d.span.as_ref()),
                    }
                }
            }
            // A block's `align-content` aligns its content vertically; a flex or grid container's
            // aligns its lines, which Roblox has no counterpart for.
            "align-content" => {
                if is_flex_or_grid(decls) {
                    diag.warn(
                        "`align-content` on a flex or grid container has no Roblox equivalent (ignored)",
                        d.span.as_ref(),
                    );
                } else if let Some(s) = d.value.as_str() {
                    let e = match s.to_ascii_lowercase().as_str() {
                        "start" | "flex-start" | "normal" => Some("Top"),
                        "center" => Some("Center"),
                        "end" | "flex-end" => Some("Bottom"),
                        _ => None,
                    };
                    match e {
                        Some(e) => out.set_prop("TextYAlignment", luau::Value::enum_item("TextYAlignment", e)),
                        None => diag.warn(format!("unsupported `align-content: {s}` (ignored)"), d.span.as_ref()),
                    }
                }
            }
            // Converted after the loop (see text_metrics); only the errors are reported here.
            "line-height" => match &d.value {
                Value::Number(n) if n.is_unitless() || n.has_unit("%") => {}
                Value::Number(n) => {
                    let px = length_px(n);
                    match (px, last(decls, "font-size").or(last(decls, "font")).and(font_size_px)) {
                        (Some(_), Some(fs)) if fs > 0.0 => {}
                        _ => diag.warn(
                            format!(
                                "`line-height: {}` needs a known font-size to convert to a ratio (ignored)",
                                n.to_css()
                            ),
                            d.span.as_ref(),
                        ),
                    }
                }
                _ => diag.warn("`line-height`: unsupported value (ignored)", d.span.as_ref()),
            },
            "white-space" => {
                if let Some(s) = d.value.as_str() {
                    let lower = s.to_ascii_lowercase();
                    match lower.as_str() {
                        "nowrap" | "pre" => out.set_prop("TextWrapped", luau::Value::Bool(false)),
                        "normal" | "pre-wrap" | "pre-line" | "break-spaces" => {
                            out.set_prop("TextWrapped", luau::Value::Bool(true))
                        }
                        _ => diag.warn(format!("unknown `white-space: {s}` (ignored)"), d.span.as_ref()),
                    }
                }
            }
            "text-wrap" => {
                if let Some(s) = d.value.as_str() {
                    match s.to_ascii_lowercase().as_str() {
                        "wrap" => out.set_prop("TextWrapped", luau::Value::Bool(true)),
                        "nowrap" => out.set_prop("TextWrapped", luau::Value::Bool(false)),
                        _ => diag.warn(format!("unknown `text-wrap: {s}` (ignored)"), d.span.as_ref()),
                    }
                }
            }
            // CSS Generated Content lets `content` replace an element's content; for a text object
            // that's its Text.
            "content" => match &d.value {
                Value::Str { text, quoted: true } => out.set_prop("Text", luau::Value::String(text.clone())),
                Value::Str { text, quoted: false } if text == "none" || text == "normal" => {}
                other => diag.warn(
                    format!("`content: {}`: only a string is supported (ignored)", other.inspect()),
                    d.span.as_ref(),
                ),
            },
            "text-overflow" => {
                if let Some(s) = d.value.as_str() {
                    match s.to_ascii_lowercase().as_str() {
                        "ellipsis" => out.set_prop("TextTruncate", luau::Value::enum_item("TextTruncate", "AtEnd")),
                        "clip" => out.set_prop("TextTruncate", luau::Value::enum_item("TextTruncate", "None")),
                        _ => diag.warn(format!("unknown `text-overflow: {s}` (ignored)"), d.span.as_ref()),
                    }
                }
            }
            // `-webkit-text-stroke` and its two longhands; like `border`, the shorthand resets
            // whatever it doesn't name.
            "text-stroke" => {
                let items = space_items(&d.value);
                stroke.width = items.iter().find(|i| matches!(i, Value::Number(_))).cloned();
                stroke.color = items.iter().find(|i| matches!(i, Value::Color(_))).cloned();
                stroke.span = d.span.clone();
                stroke_present = true;
            }
            "text-stroke-width" => {
                stroke.width = Some(d.value.clone());
                stroke.span = d.span.clone();
                stroke_present = true;
            }
            "text-stroke-color" => {
                stroke.color = Some(d.value.clone());
                stroke.span = d.span.clone();
                stroke_present = true;
            }
            _ => {}
        }
    }

    if stroke_present {
        text_stroke(&stroke, decls, diag, out);
    }

    // TextSize is a line's height and CSS's font-size the em (see FAMILY_LINE_RATIOS). LineHeight
    // then spaces each following line one CSS line box apart; the first line's half-leading is
    // padding (see half_leading).
    let metrics = text_metrics(decls);
    if let Some(size) = metrics.text_size() {
        if metrics.size.zip(metrics.family_ratio).is_some_and(|(s, r)| s * r > MAX_TEXT_SIZE + 0.5)
            && let Some(d) = decls.iter().rev().find(|d| d.name == "font-size" || d.name == "font")
        {
            diag.warn(
                format!(
                    "this font size needs a TextSize above Roblox's maximum of {MAX_TEXT_SIZE}; using {MAX_TEXT_SIZE}"
                ),
                d.span.as_ref(),
            );
        }
        out.set_prop("TextSize", luau::Value::Number(size));
    }
    let line_height = match (metrics.line_px(), metrics.text_size()) {
        (Some(px), Some(size)) if size > 0.0 => Some(px / size),
        // Without a font size, a ratio is all there is.
        _ => metrics.line_height,
    };
    if let Some(lh) = line_height {
        // Roblox clamps LineHeight to 3 (measured in Studio 2026-09).
        if lh > 3.0
            && let Some(d) = decls.iter().rev().find(|d| d.name == "line-height" || d.name == "font")
        {
            diag.warn(
                "this line-height spaces lines more than Roblox's LineHeight allows (3x the TextSize); using 3",
                d.span.as_ref(),
            );
        }
        out.set_prop("LineHeight", luau::Value::Number(lh.min(3.0)));
    }

    if have_font {
        let w = weight.unwrap_or("Regular");
        let s = style.unwrap_or("Normal");
        let fam = family.unwrap_or_else(|| DEFAULT_FONT.to_string());
        out.set_prop(
            "FontFace",
            luau::Value::Font { family: fam, weight: Some(w.to_string()), style: Some(s.to_string()) },
        );
    }
}

/// `-webkit-text-stroke` as a `::UIStroke` drawn around the glyphs. An omitted color is
/// `currentColor`, which for a text object is its own `color`.
fn text_stroke(st: &StrokeState, decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    let width = match &st.width {
        Some(w) => match px_only(w) {
            Ok(px) => px,
            Err(e) => {
                diag.warn(format!("`-webkit-text-stroke`: {e} (ignored)"), st.span.as_ref());
                return;
            }
        },
        None => {
            diag.warn("`-webkit-text-stroke` needs a width (ignored)", st.span.as_ref());
            return;
        }
    };
    // CSS draws nothing at width 0; an explicit Enabled = false also undoes a weaker rule's stroke.
    if width == 0.0 {
        out.set_pseudo_prop("UIStroke", "Enabled", luau::Value::Bool(false));
        return;
    }
    let current_color = || last(decls, "text-fill-color").or_else(|| last(decls, "color")).map(|d| d.value.clone());
    let Some(color) = st.color.clone().or_else(current_color) else {
        diag.warn(
            "`-webkit-text-stroke` has no color and the rule sets no `color` to take one from (ignored)",
            st.span.as_ref(),
        );
        return;
    };
    // Explicit, so a lower-priority `border: none` can't switch the outline off.
    out.set_pseudo_prop("UIStroke", "Enabled", luau::Value::Bool(true));
    out.set_pseudo_prop("UIStroke", "ApplyStrokeMode", luau::Value::enum_item("ApplyStrokeMode", "Contextual"));
    out.set_pseudo_prop("UIStroke", "Thickness", luau::Value::Number(width));
    match resolve_color(&color) {
        Ok((expr, alpha)) => {
            out.set_pseudo_prop("UIStroke", "Color", expr);
            if let Some(a) = alpha {
                out.set_pseudo_prop("UIStroke", "Transparency", luau::Value::Number(compute_transparency(a, None)));
            }
        }
        Err(e) => diag.warn(format!("`-webkit-text-stroke`: {e} (ignored)"), st.span.as_ref()),
    }
}

// ---------------------------------------------------------------------------------------------
// Size
// ---------------------------------------------------------------------------------------------

enum AxisResult {
    Value((f64, f64)),
    Auto,
}

fn axis_length(v: &Value) -> Result<AxisResult, String> {
    if let Some(s) = v.as_str() {
        let l = s.to_ascii_lowercase();
        if matches!(l.as_str(), "auto" | "fit-content" | "max-content" | "min-content") {
            return Ok(AxisResult::Auto);
        }
    }
    match v {
        Value::Number(n) => {
            let unit = unit_of(n);
            if matches!(unit.as_str(), "vw" | "vh" | "vmin" | "vmax") {
                return Err("viewport units have no Roblox equivalent".to_string());
            }
            length_component(n).map(AxisResult::Value)
        }
        Value::Call { name, .. } if name == "calc" => length_value(v).map(AxisResult::Value),
        _ => Err(format!("unsupported size value `{}`", v.inspect())),
    }
}

fn max_length_px(d: Option<&Decl>, diag: &mut Diagnostics) -> f64 {
    let Some(d) = d else { return f64::INFINITY };
    if let Some(s) = d.value.as_str()
        && s.eq_ignore_ascii_case("none")
    {
        return f64::INFINITY;
    }
    match &d.value {
        Value::Number(n) => {
            let unit = unit_of(n);
            match unit.as_str() {
                "" | "px" => n.value,
                "%" => {
                    diag.warn("percentage min/max size constraints aren't supported (ignored)", d.span.as_ref());
                    f64::INFINITY
                }
                _ => length_px(n).unwrap_or_else(|| {
                    diag.warn(format!("unit `{unit}` has no Roblox equivalent (ignored)"), d.span.as_ref());
                    f64::INFINITY
                }),
            }
        }
        _ => {
            diag.warn("expected a length (ignored)", d.span.as_ref());
            f64::INFINITY
        }
    }
}

fn min_length_px(d: Option<&Decl>, diag: &mut Diagnostics) -> f64 {
    let Some(d) = d else { return 0.0 };
    match &d.value {
        Value::Number(n) => {
            let unit = unit_of(n);
            match unit.as_str() {
                "" | "px" => n.value,
                "%" => {
                    diag.warn("percentage min/max size constraints aren't supported (ignored)", d.span.as_ref());
                    0.0
                }
                _ => length_px(n).unwrap_or_else(|| {
                    diag.warn(format!("unit `{unit}` has no Roblox equivalent (ignored)"), d.span.as_ref());
                    0.0
                }),
            }
        }
        _ => {
            diag.warn("expected a length (ignored)", d.span.as_ref());
            0.0
        }
    }
}

fn translate_size(decls: &[Decl], opts: &ApproxOptions, diag: &mut Diagnostics, out: &mut Translated) {
    let (stretch_x, stretch_y) = stretched_size(decls, opts);
    // A flex item that grows but has no width starts from its content in CSS and grows from there.
    // Sized by its content in Roblox, it would feed back through any percentage-sized descendant
    // (a `width: 100%` bar makes the item as wide as the line, which wraps every item onto its
    // own). So it starts from 0 instead, as CSS's `flex: 1` does, and takes its width from its
    // share of the line. That assumes a row, where growing items without a width belong; an item
    // that grows down a column should set its own width.
    let grow_basis = opts.groups.contains(&Group::Layout)
        && flex_grows(decls)
        && last(decls, "width").is_none()
        && stretch_x.is_none();
    let zero_width = Decl { name: "width".to_string(), value: Value::num_unit(0.0, "px"), span: None };
    let w = if grow_basis { Some(&zero_width) } else { last(decls, "width") };
    let h = last(decls, "height");
    // Axes sized in px, which a stretching flex line must leave alone (see below).
    let mut fixed_px: (Option<f64>, Option<f64>) = (None, None);
    if let Some(d) = last(decls, "box-sizing")
        && d.value.as_str().is_some_and(|s| s.eq_ignore_ascii_case("content-box"))
    {
        diag.warn(
            "`box-sizing: content-box` isn't supported: Roblox's Size always includes the padding (ignored)",
            d.span.as_ref(),
        );
    }
    let fits = |d: Option<&Decl>| d.is_some_and(|d| matches!(axis_length(&d.value), Ok(AxisResult::Auto)));
    let sized = |d: Option<&Decl>, stretch: Option<(f64, f64)>| stretch.is_some() || (d.is_some() && !fits(d));
    if (fits(w) || fits(h)) && !sized(w, stretch_x) && !sized(h, stretch_y) {
        // Only `fit-content`/`auto` axes, and no length for either axis anywhere in the cascade:
        // the element is sized by its content on both. AutomaticSize only ever grows an element
        // past its Size, so the Size is zeroed too, or a weaker rule's size would stay as a floor.
        out.set_prop("Size", luau::Value::udim2(0.0, 0.0, 0.0, 0.0));
        out.set_prop("AutomaticSize", luau::Value::enum_item("AutomaticSize", "XY"));
    } else if w.is_some() || h.is_some() || stretch_x.is_some() || stretch_y.is_some() {
        let x_given = w.is_some() || stretch_x.is_some();
        let y_given = h.is_some() || stretch_y.is_some();
        if let (Some(given), None) | (None, Some(given)) = (w, h)
            && x_given != y_given
            && given.span.is_some()
        {
            let missing = if given.name == "width" { "height" } else { "width" };
            diag.warn(
                format!(
                    "only `{}` is set, but Roblox's Size covers both axes: {missing} becomes automatic here and \
                     overrides any {missing} from other rules (set `{missing}` too, e.g. `{missing}: fit-content`)",
                    given.name
                ),
                given.span.as_ref(),
            );
        }
        let mut xs = 0.0;
        let mut xo = 0.0;
        let mut ys = 0.0;
        let mut yo = 0.0;
        let mut auto_x = false;
        let mut auto_y = false;
        match w {
            Some(d) => match axis_length(&d.value) {
                Ok(AxisResult::Value((s, o))) => {
                    xs = s;
                    xo = o;
                    fixed_px.0 = (s == 0.0 && o > 0.0).then_some(o);
                }
                Ok(AxisResult::Auto) => auto_x = true,
                Err(e) => diag.warn(format!("`width`: {e} (ignored)"), d.span.as_ref()),
            },
            None => match stretch_x {
                Some((s, o)) => {
                    xs = s;
                    xo = o;
                }
                None => auto_x = true,
            },
        }
        match h {
            Some(d) => match axis_length(&d.value) {
                Ok(AxisResult::Value((s, o))) => {
                    ys = s;
                    yo = o;
                    fixed_px.1 = (s == 0.0 && o > 0.0).then_some(o);
                }
                Ok(AxisResult::Auto) => auto_y = true,
                Err(e) => diag.warn(format!("`height`: {e} (ignored)"), d.span.as_ref()),
            },
            None => match stretch_y {
                Some((s, o)) => {
                    ys = s;
                    yo = o;
                }
                None => auto_y = true,
            },
        }
        out.set_prop("Size", luau::Value::udim2(xs, xo, ys, yo));
        // `None` matters as much as the rest: text elements are content-sized by default, and a CSS
        // length turns that off for the axis it fixes.
        let automatic = match (auto_x, auto_y) {
            (true, true) => "XY",
            (true, false) => "X",
            (false, true) => "Y",
            (false, false) => "None",
        };
        out.set_prop("AutomaticSize", luau::Value::enum_item("AutomaticSize", automatic));
    }

    // A stretching flex line (the CSS default) stretches only items whose cross size is `auto`,
    // but Roblox's ItemLineAlignment.Stretch stretches explicitly sized items too. A MaxSize at the
    // element's own size keeps it out of the stretch without changing anything else, unless the
    // item grows along the line, which the cap would stop.
    if !opts.groups.contains(&Group::Layout) || flex_grows(decls) {
        fixed_px = (None, None);
    }
    // A UISizeConstraint on a ScrollingFrame caps its AutomaticCanvasSize too (measured in Studio),
    // so a cap at the window's size leaves nothing to scroll. A scrolling axis is left to the
    // stretch instead; `math.huge` still lifts a cap from a weaker rule.
    let (scroll_x, scroll_y) =
        if opts.groups.contains(&Group::Visibility) { scroll_axes(decls) } else { (false, false) };
    if scroll_x && fixed_px.0.is_some() {
        fixed_px.0 = Some(f64::INFINITY);
    }
    if scroll_y && fixed_px.1.is_some() {
        fixed_px.1 = Some(f64::INFINITY);
    }
    let minw = last(decls, "min-width");
    let minh = last(decls, "min-height");
    let maxw = last(decls, "max-width");
    let maxh = last(decls, "max-height");
    if minw.is_some() || minh.is_some() || maxw.is_some() || maxh.is_some() || fixed_px != (None, None) {
        let minw_v = min_length_px(minw, diag);
        let minh_v = min_length_px(minh, diag);
        let maxw_v = max_length_px(maxw, diag).min(fixed_px.0.unwrap_or(f64::INFINITY)).max(minw_v);
        let maxh_v = max_length_px(maxh, diag).min(fixed_px.1.unwrap_or(f64::INFINITY)).max(minh_v);
        if minw.is_some() || minh.is_some() {
            out.set_pseudo_prop("UISizeConstraint", "MinSize", luau::Value::Vector2(minw_v, minh_v));
        }
        out.set_pseudo_prop("UISizeConstraint", "MaxSize", luau::Value::Vector2(maxw_v, maxh_v));
    }

    if let Some(d) = last(decls, "aspect-ratio") {
        if let Some(s) = d.value.as_str() {
            if !s.eq_ignore_ascii_case("auto") {
                diag.warn(format!("unknown `aspect-ratio: {s}` (ignored)"), d.span.as_ref());
            }
        } else {
            let ratio = match &d.value {
                Value::Number(n) => Some(n.value),
                Value::List { items, sep: ListSep::Slash, .. } if items.len() == 2 => {
                    match (items[0].as_number(), items[1].as_number()) {
                        (Some(a), Some(b)) if b.value != 0.0 => Some(a.value / b.value),
                        _ => None,
                    }
                }
                _ => None,
            };
            match ratio {
                Some(r) => out.set_pseudo_prop("UIAspectRatioConstraint", "AspectRatio", luau::Value::Number(r)),
                None => {
                    diag.warn(format!("unsupported `aspect-ratio: {}` (ignored)", d.value.inspect()), d.span.as_ref())
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------------------------------

/// The effective `top`, `right`, `bottom`, `left` values (from the longhands and `inset`, later wins).
struct Edges {
    top: Option<Value>,
    right: Option<Value>,
    bottom: Option<Value>,
    left: Option<Value>,
    span: Option<Span>,
}

fn edges(decls: &[Decl]) -> Edges {
    let mut e = Edges { top: None, right: None, bottom: None, left: None, span: None };
    for d in decls {
        match d.name.as_str() {
            "inset" => {
                let [top, right, bottom, left] = four_sides(&d.value);
                e.top = Some(top);
                e.right = Some(right);
                e.bottom = Some(bottom);
                e.left = Some(left);
            }
            "top" => e.top = Some(d.value.clone()),
            "right" => e.right = Some(d.value.clone()),
            "bottom" => e.bottom = Some(d.value.clone()),
            "left" => e.left = Some(d.value.clone()),
            _ => continue,
        }
        e.span = d.span.clone();
    }
    e
}

/// A `(scale, offset)` length pair, as Roblox's UDim stores one.
type Length = (f64, f64);

/// One side of a `margin`.
#[derive(Clone, Copy, PartialEq)]
enum Margin {
    Length(Length),
    Auto,
}

impl Margin {
    /// The length it takes up when it isn't absorbing free space (CSS resolves `auto` to 0 then).
    fn length(m: Option<Margin>) -> Length {
        match m {
            Some(Margin::Length(l)) => l,
            _ => (0.0, 0.0),
        }
    }

    fn is_auto(m: Option<Margin>) -> bool {
        m == Some(Margin::Auto)
    }
}

/// The effective margins (top, right, bottom, left), from the shorthand and longhands in source order.
fn margins(decls: &[Decl], diag: &mut Diagnostics) -> [Option<Margin>; 4] {
    let mut sides: [Option<Margin>; 4] = [None; 4];
    let mut side = |v: &Value, d: &Decl| -> Option<Margin> {
        if v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("auto")) {
            return Some(Margin::Auto);
        }
        match length_value(v) {
            Ok(l) => Some(Margin::Length(l)),
            Err(e) => {
                diag.warn(format!("`{}`: {e} (ignored)", d.name), d.span.as_ref());
                None
            }
        }
    };
    for d in decls {
        match d.name.as_str() {
            "margin" => {
                let [t, r, b, l] = four_sides(&d.value);
                sides = [side(&t, d), side(&r, d), side(&b, d), side(&l, d)];
            }
            "margin-top" => sides[0] = side(&d.value, d),
            "margin-right" => sides[1] = side(&d.value, d),
            "margin-bottom" => sides[2] = side(&d.value, d),
            "margin-left" => sides[3] = side(&d.value, d),
            "margin-inline" => {
                let (a, b) = two_sides(&d.value);
                sides[3] = side(&a, d);
                sides[1] = side(&b, d);
            }
            "margin-block" => {
                let (a, b) = two_sides(&d.value);
                sides[0] = side(&a, d);
                sides[2] = side(&b, d);
            }
            _ => {}
        }
    }
    sides
}

/// CSS absolute positioning: with both opposite edges set and no explicit size on that axis, the
/// element stretches between them. Returns the size for the stretched axes.
fn stretched_size(decls: &[Decl], opts: &ApproxOptions) -> (Option<Length>, Option<Length>) {
    if !opts.groups.contains(&Group::Position) {
        return (None, None);
    }
    let e = edges(decls);
    // Margins inset the stretched element further (translate_position reports their errors).
    let [mt, mr, mb, ml] = margins(decls, &mut Diagnostics::default());
    let between = |a: &Option<Value>, b: &Option<Value>, ma: Option<Margin>, mb: Option<Margin>| {
        let (a_scale, a_offset) = length_value(a.as_ref()?).ok()?;
        let (b_scale, b_offset) = length_value(b.as_ref()?).ok()?;
        let ((ma_scale, ma_offset), (mb_scale, mb_offset)) = (Margin::length(ma), Margin::length(mb));
        Some((1.0 - a_scale - b_scale - ma_scale - mb_scale, -a_offset - b_offset - ma_offset - mb_offset))
    };
    let x = if last(decls, "width").is_none() { between(&e.left, &e.right, ml, mr) } else { None };
    let y = if last(decls, "height").is_none() { between(&e.top, &e.bottom, mt, mb) } else { None };
    (x, y)
}

fn translate_position(decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    // With both `left` and `right` (or `top` and `bottom`), the element is placed by `left` (`top`)
    // and, without an explicit size, stretched between them by translate_size — like CSS.
    let Edges { top, right, bottom, left, span: last_span } = edges(decls);

    let mut xs = 0.0;
    let mut xo = 0.0;
    let mut anchor_x = 0.0;
    let mut ys = 0.0;
    let mut yo = 0.0;
    let mut anchor_y = 0.0;
    let mut have_pos = false;

    if let Some(v) = &left {
        match length_value(v) {
            Ok((s, o)) => {
                xs = s;
                xo = o;
                have_pos = true;
            }
            Err(e) => diag.warn(format!("`left`: {e} (ignored)"), last_span.as_ref()),
        }
    } else if let Some(v) = &right {
        match length_value(v) {
            Ok((s, o)) => {
                xs = 1.0 - s;
                xo = -o;
                anchor_x = 1.0;
                have_pos = true;
            }
            Err(e) => diag.warn(format!("`right`: {e} (ignored)"), last_span.as_ref()),
        }
    }
    if let Some(v) = &top {
        match length_value(v) {
            Ok((s, o)) => {
                ys = s;
                yo = o;
                have_pos = true;
            }
            Err(e) => diag.warn(format!("`top`: {e} (ignored)"), last_span.as_ref()),
        }
    } else if let Some(v) = &bottom {
        match length_value(v) {
            Ok((s, o)) => {
                ys = 1.0 - s;
                yo = -o;
                anchor_y = 1.0;
                have_pos = true;
            }
            Err(e) => diag.warn(format!("`bottom`: {e} (ignored)"), last_span.as_ref()),
        }
    }

    // Margins. GuiObjects are always placed absolutely, so a margin moves the element away from the
    // edge it's placed by, and `auto` margins share out the space between two placed edges (or the
    // parent's own edges, when neither is set) as CSS does. They never move siblings.
    let [mt, mr, mb, ml] = margins(decls, diag);
    if [mt, mr, mb, ml].iter().any(Option::is_some) {
        let horizontal =
            place_with_margins((left.as_ref(), right.as_ref()), (ml, mr), true, (&mut xs, &mut xo, &mut anchor_x));
        // Vertical `auto` margins are 0 in normal flow; they only centre between `top` and `bottom`.
        let vertical = place_with_margins(
            (top.as_ref(), bottom.as_ref()),
            (mt, mb),
            top.is_some() && bottom.is_some(),
            (&mut ys, &mut yo, &mut anchor_y),
        );
        have_pos |= horizontal || vertical;
        let placed = left.is_some() || right.is_some() || top.is_some() || bottom.is_some();
        if !placed && let Some(d) = decls.iter().rev().find(|d| d.name.starts_with("margin")) {
            diag.warn(
                format!(
                    "`{}` moves this element within its parent, but siblings don't make room for it, and a flex or \
                     grid container ignores it (use `gap` or the container's padding there)",
                    d.name
                ),
                d.span.as_ref(),
            );
        }
    }

    let mut rotation: Option<f64> = None;
    let mut scale_out: Option<(f64, Option<f64>)> = None;
    if let Some(d) = last(decls, "transform") {
        if let Some(s) = d.value.as_str()
            && s.eq_ignore_ascii_case("none")
        {
            rotation = Some(0.0);
        }
        let items = space_items(&d.value);
        for it in &items {
            let Value::Call { name, args } = it else { continue };
            match name.as_str() {
                "translate" => {
                    if let Some(px) = args.first() {
                        apply_translate_component(px, &mut xo, &mut anchor_x, &mut have_pos, diag, d.span.as_ref());
                    }
                    if let Some(py) = args.get(1) {
                        apply_translate_component(py, &mut yo, &mut anchor_y, &mut have_pos, diag, d.span.as_ref());
                    }
                }
                "translateX" => {
                    if let Some(px) = args.first() {
                        apply_translate_component(px, &mut xo, &mut anchor_x, &mut have_pos, diag, d.span.as_ref());
                    }
                }
                "translateY" => {
                    if let Some(py) = args.first() {
                        apply_translate_component(py, &mut yo, &mut anchor_y, &mut have_pos, diag, d.span.as_ref());
                    }
                }
                "rotate" => {
                    if let Some(n) = args.first().and_then(|a| a.as_number()) {
                        match n.value_in("deg") {
                            Some(v) => rotation = Some(v),
                            None => diag
                                .warn(format!("`rotate({})`: unsupported unit (ignored)", n.to_css()), d.span.as_ref()),
                        }
                    }
                }
                "scale" => {
                    let s = args.first().and_then(|a| a.as_number()).map(|n| n.value);
                    let sy = args.get(1).and_then(|a| a.as_number()).map(|n| n.value);
                    if let Some(s) = s {
                        scale_out = Some((s, sy));
                    }
                }
                other => {
                    diag.warn(format!("`transform: {other}(...)` has no Roblox equivalent (ignored)"), d.span.as_ref())
                }
            }
        }
    }

    if have_pos {
        out.set_prop("Position", luau::Value::udim2(xs, xo, ys, yo));
    }
    // A percentage translate states the anchor explicitly, even when it's 0% (e.g. a top-left utility).
    let percent_translate = last(decls, "transform").is_some_and(|d| {
        space_items(&d.value).iter().any(|t| match t {
            Value::Call { name, args } if name.starts_with("translate") => {
                args.iter().any(|a| a.as_number().is_some_and(|n| n.has_unit("%")))
            }
            _ => false,
        })
    });
    if anchor_x != 0.0 || anchor_y != 0.0 || percent_translate {
        out.set_prop("AnchorPoint", luau::Value::Vector2(anchor_x, anchor_y));
    }
    if let Some(r) = rotation {
        out.set_prop("Rotation", luau::Value::Number(r));
    }
    if let Some((s, sy)) = scale_out {
        if let Some(sy) = sy
            && (sy - s).abs() > 1e-9
        {
            diag.warn("non-uniform `scale()` is approximated using the X value", None);
        }
        out.set_pseudo_prop("UIScale", "Scale", luau::Value::Number(s));
    }

    if let Some(d) = last(decls, "z-index") {
        match &d.value {
            Value::Str { text, .. } if text.eq_ignore_ascii_case("auto") => {}
            Value::Number(n) => out.set_prop("ZIndex", luau::Value::Number(n.value.round())),
            _ => diag.warn("`z-index`: unsupported value (ignored)", d.span.as_ref()),
        }
    }
}

/// Applies one axis's margins to a position already computed from its edges (`start`, `end`; left
/// and right, or top and bottom). `auto_spreads` says whether `auto` margins absorb free space on
/// this axis. Returns whether the position changed.
fn place_with_margins(
    (start, end): (Option<&Value>, Option<&Value>),
    (m_start, m_end): (Option<Margin>, Option<Margin>),
    auto_spreads: bool,
    (scale, offset, anchor): (&mut f64, &mut f64, &mut f64),
) -> bool {
    let edge = |v: Option<&Value>| v.and_then(|v| length_value(v).ok());
    // `auto` margins only share out space when the element isn't pinned to just one edge.
    let free = auto_spreads && (start.is_some() == end.is_some());
    let (s_start, o_start) = edge(start).unwrap_or((0.0, 0.0));
    let (s_end, o_end) = edge(end).unwrap_or((0.0, 0.0));
    let (ms_start, mo_start) = Margin::length(m_start);
    let (ms_end, mo_end) = Margin::length(m_end);
    match (free && Margin::is_auto(m_start), free && Margin::is_auto(m_end)) {
        // Centred in the space between the two edges.
        (true, true) => {
            *scale = (s_start + 1.0 - s_end) / 2.0;
            *offset = (o_start - o_end) / 2.0;
            *anchor = 0.5;
        }
        // Pushed against the far edge.
        (true, false) => {
            *scale = 1.0 - s_end - ms_end;
            *offset = -o_end - mo_end;
            *anchor = 1.0;
        }
        _ if start.is_none() && end.is_some() => {
            *scale -= ms_end;
            *offset -= mo_end;
        }
        _ => {
            if ms_start == 0.0 && mo_start == 0.0 {
                return false;
            }
            *scale += ms_start;
            *offset += mo_start;
        }
    }
    true
}

fn apply_translate_component(
    v: &Value,
    offset: &mut f64,
    anchor: &mut f64,
    have_pos: &mut bool,
    diag: &mut Diagnostics,
    span: Option<&Span>,
) {
    match v {
        Value::Number(n) if n.has_unit("%") => *anchor += -n.value / 100.0,
        Value::Number(n) => match length_px(n) {
            Some(px) => {
                *offset += px;
                *have_pos = true;
            }
            None => {
                diag.warn(format!("`translate()`: unit `{}` has no Roblox equivalent (ignored)", n.unit_string()), span)
            }
        },
        _ => diag.warn("`translate()`: unsupported value (ignored)", span),
    }
}

// ---------------------------------------------------------------------------------------------
// Box
// ---------------------------------------------------------------------------------------------

#[derive(Default, Clone)]
struct StrokeState {
    width: Option<Value>,
    style: Option<Value>,
    color: Option<Value>,
    span: Option<Span>,
}

fn parse_border_shorthand(v: &Value) -> (Option<Value>, Option<Value>, Option<Value>) {
    let items = space_items(v);
    let mut width = None;
    let mut style = None;
    let mut color = None;
    for it in items {
        match &it {
            Value::Number(_) => width = Some(it),
            // A `var()` token or a colour function (`rgba(...)` kept as a call) is the colour too.
            Value::Color(_) | Value::Call { .. } => color = Some(it),
            Value::Str { .. } => style = Some(it),
            _ => {}
        }
    }
    (width, style, color)
}

/// How far a CSS border pushes the content in, in px. CSS draws a border inside the element's
/// box (with `box-sizing: border-box`), taking room from the content; a UIStroke in Border mode
/// is drawn outside the box and takes none. An outline takes no room in either.
fn border_inset(decls: &[Decl]) -> f64 {
    let mut width: Option<Value> = None;
    let mut style: Option<Value> = None;
    let mut present = false;
    for d in decls {
        match d.name.as_str() {
            "border" => {
                let (w, s, _) = parse_border_shorthand(&d.value);
                // The shorthand resets what it leaves out, and `border: none` is a bare style.
                width = w;
                style = s;
                present = true;
            }
            "border-width" => {
                width = Some(d.value.clone());
                present = true;
            }
            "border-style" => {
                style = Some(d.value.clone());
                present = true;
            }
            _ => {}
        }
    }
    let none = style.as_ref().and_then(|s| s.as_str()).is_some_and(|s| {
        let s = s.to_ascii_lowercase();
        s == "none" || s == "hidden"
    });
    if !present || none {
        return 0.0;
    }
    // A UIStroke is 1px when the border gives no width (see translate_box).
    width.as_ref().map_or(Some(1.0), |w| px_only(w).ok()).unwrap_or(0.0)
}

fn translate_box(
    decls: &[Decl],
    opts: &ApproxOptions,
    opacity_factor: Option<f64>,
    diag: &mut Diagnostics,
    out: &mut Translated,
) {
    // border-radius
    if let Some(d) = last(decls, "border-radius") {
        let items = space_items(&d.value);
        if items.len() > 1 && !items.iter().all(|i| i.sass_eq(&items[0])) {
            diag.warn(
                format!("`border-radius: {}` has different values per corner; using the first", d.value.inspect()),
                d.span.as_ref(),
            );
        }
        match length_value(&items[0]) {
            Ok((s, o)) => out.set_pseudo_prop("UICorner", "CornerRadius", luau::Value::udim(s, o)),
            Err(e) => diag.warn(format!("`border-radius`: {e} (ignored)"), d.span.as_ref()),
        }
    }

    // padding (shorthand + longhands, in source order)
    let mut sides: [Option<(Value, Option<Span>)>; 4] = [None, None, None, None];
    for d in decls {
        match d.name.as_str() {
            "padding" => {
                let s = four_sides(&d.value);
                sides = [
                    Some((s[0].clone(), d.span.clone())),
                    Some((s[1].clone(), d.span.clone())),
                    Some((s[2].clone(), d.span.clone())),
                    Some((s[3].clone(), d.span.clone())),
                ];
            }
            "padding-top" => sides[0] = Some((d.value.clone(), d.span.clone())),
            "padding-right" => sides[1] = Some((d.value.clone(), d.span.clone())),
            "padding-bottom" => sides[2] = Some((d.value.clone(), d.span.clone())),
            "padding-left" => sides[3] = Some((d.value.clone(), d.span.clone())),
            "padding-inline" => {
                let (a, b) = two_sides(&d.value);
                sides[3] = Some((a, d.span.clone()));
                sides[1] = Some((b, d.span.clone()));
            }
            "padding-block" => {
                let (a, b) = two_sides(&d.value);
                sides[0] = Some((a, d.span.clone()));
                sides[2] = Some((b, d.span.clone()));
            }
            _ => {}
        }
    }
    const PADDING_NAMES: [&str; 4] = ["PaddingTop", "PaddingRight", "PaddingBottom", "PaddingLeft"];
    // The half-leading CSS puts above and below the text (see half_leading) goes on top of the padding,
    // and so does the border, which CSS draws inside the box and Roblox's UIStroke outside it.
    let leading = if opts.groups.contains(&Group::Text) { half_leading(decls) } else { None };
    let inset = border_inset(decls);
    for (i, side) in sides.into_iter().enumerate() {
        let extra = inset
            + match (i, leading) {
                (0, Some((top, _))) => top,
                (2, Some((_, bottom))) => bottom,
                _ => 0.0,
            };
        match side {
            Some((v, span)) => match length_value(&v) {
                Ok((s, o)) => out.set_pseudo_prop("UIPadding", PADDING_NAMES[i], luau::Value::udim(s, o + extra)),
                Err(e) => diag.warn(format!("`padding`: {e} (ignored)"), span.as_ref()),
            },
            None if extra != 0.0 => out.set_pseudo_prop("UIPadding", PADDING_NAMES[i], luau::Value::udim(0.0, extra)),
            None => {}
        }
    }

    // border / outline
    let mut border = StrokeState::default();
    let mut border_present = false;
    let mut outline = StrokeState::default();
    let mut outline_present = false;
    for d in decls {
        match d.name.as_str() {
            "border" => {
                let (w, s, c) = parse_border_shorthand(&d.value);
                if w.is_some() {
                    border.width = w;
                }
                if s.is_some() {
                    border.style = s;
                }
                if c.is_some() {
                    border.color = c;
                }
                border_present = true;
                border.span = d.span.clone();
            }
            "border-width" => {
                border.width = Some(d.value.clone());
                border_present = true;
                border.span = d.span.clone();
            }
            "border-style" => {
                border.style = Some(d.value.clone());
                border_present = true;
                border.span = d.span.clone();
            }
            "border-color" => {
                border.color = Some(d.value.clone());
                border_present = true;
                border.span = d.span.clone();
            }
            "outline" => {
                let (w, s, c) = parse_border_shorthand(&d.value);
                if w.is_some() {
                    outline.width = w;
                }
                if s.is_some() {
                    outline.style = s;
                }
                if c.is_some() {
                    outline.color = c;
                }
                outline_present = true;
                outline.span = d.span.clone();
            }
            "outline-width" => {
                outline.width = Some(d.value.clone());
                outline_present = true;
                outline.span = d.span.clone();
            }
            "outline-style" => {
                outline.style = Some(d.value.clone());
                outline_present = true;
                outline.span = d.span.clone();
            }
            "outline-color" => {
                outline.color = Some(d.value.clone());
                outline_present = true;
                outline.span = d.span.clone();
            }
            _ => {}
        }
    }
    if border_present && outline_present {
        diag.warn("`border` and `outline` are both set; using `border`", border.span.as_ref());
    }
    let chosen = if border_present {
        Some(&border)
    } else if outline_present {
        Some(&outline)
    } else {
        None
    };
    if let Some(st) = chosen {
        let style_is_none = st
            .style
            .as_ref()
            .and_then(|s| s.as_str())
            .map(|s| matches!(s.to_ascii_lowercase().as_str(), "none" | "hidden"))
            .unwrap_or(false);
        let width_is_zero = st.width.as_ref().and_then(|v| v.as_number()).map(|n| n.value == 0.0).unwrap_or(false);
        // CSS borders replace Roblox's legacy border entirely.
        out.set_prop("BorderSizePixel", luau::Value::Number(0.0));
        if style_is_none || width_is_zero {
            out.set_pseudo_prop("UIStroke", "Enabled", luau::Value::Bool(false));
        } else {
            // Explicit, so a lower-priority `border: none` can't switch this stroke off.
            out.set_pseudo_prop("UIStroke", "Enabled", luau::Value::Bool(true));
            out.set_pseudo_prop("UIStroke", "ApplyStrokeMode", luau::Value::enum_item("ApplyStrokeMode", "Border"));
            if let Some(w) = &st.width {
                match px_only(w) {
                    Ok(px) => out.set_pseudo_prop("UIStroke", "Thickness", luau::Value::Number(px)),
                    Err(e) => diag.warn(format!("`border`: {e} (ignored)"), st.span.as_ref()),
                }
            } else {
                out.set_pseudo_prop("UIStroke", "Thickness", luau::Value::Number(1.0));
            }
            let mut base_alpha = 1.0;
            if let Some(c) = &st.color {
                match resolve_color(c) {
                    Ok((expr, alpha)) => {
                        out.set_pseudo_prop("UIStroke", "Color", expr);
                        if let Some(a) = alpha {
                            base_alpha = a;
                        }
                    }
                    Err(e) => diag.warn(format!("`border-color`: {e} (ignored)"), st.span.as_ref()),
                }
            }
            out.set_pseudo_prop(
                "UIStroke",
                "Transparency",
                luau::Value::Number(compute_transparency(base_alpha, opacity_factor)),
            );
            if let Some(s) = &st.style
                && let Some(name) = s.as_str()
            {
                let lower = name.to_ascii_lowercase();
                if !matches!(lower.as_str(), "solid" | "none" | "hidden") {
                    diag.warn(format!("`border-style: {name}` is approximated as a solid stroke"), st.span.as_ref());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

fn apply_justify(s: &str, column: bool, grid: bool, out: &mut Translated, diag: &mut Diagnostics, span: Option<&Span>) {
    let lower = s.to_ascii_lowercase();
    let class = if grid { "UIGridLayout" } else { "UIListLayout" };
    if let Some(flex_enum) = match lower.as_str() {
        "space-between" => Some("SpaceBetween"),
        "space-around" => Some("SpaceAround"),
        "space-evenly" => Some("SpaceEvenly"),
        _ => None,
    } {
        let prop = if column { "VerticalFlex" } else { "HorizontalFlex" };
        out.set_pseudo_prop(class, prop, luau::Value::enum_item("UIFlexAlignment", flex_enum));
        return;
    }
    let side = match lower.as_str() {
        "start" | "flex-start" | "left" | "normal" => 0,
        "center" => 1,
        "end" | "flex-end" | "right" => 2,
        _ => {
            diag.warn(format!("unknown `justify-content: {s}` (ignored)"), span);
            return;
        }
    };
    if column {
        let v = ["Top", "Center", "Bottom"][side];
        out.set_pseudo_prop(class, "VerticalAlignment", luau::Value::enum_item("VerticalAlignment", v));
    } else {
        let v = ["Left", "Center", "Right"][side];
        out.set_pseudo_prop(class, "HorizontalAlignment", luau::Value::enum_item("HorizontalAlignment", v));
    }
}

fn apply_align_items(s: &str, column: bool, out: &mut Translated, diag: &mut Diagnostics, span: Option<&Span>) {
    let lower = s.to_ascii_lowercase();
    if lower == "stretch" || lower == "normal" {
        out.set_pseudo_prop(
            "UIListLayout",
            "ItemLineAlignment",
            luau::Value::enum_item("ItemLineAlignment", "Stretch"),
        );
        return;
    }
    // Any other value undoes the stretch a `display: flex` rule gives by default.
    out.set_pseudo_prop("UIListLayout", "ItemLineAlignment", luau::Value::enum_item("ItemLineAlignment", "Automatic"));
    // Roblox has no baseline alignment. Bottom edges line up with the baselines whenever the items
    // share a font size, which is the usual case; taller text then sits a descender too low. In a
    // column the cross axis is horizontal, where CSS itself falls back to `start`.
    if lower == "baseline" || lower == "first baseline" || lower == "last baseline" {
        let side = if column { "Left" } else { "Bottom" };
        diag.warn(format!("`align-items: {lower}` is approximated as `{side}`"), span);
        let prop = if column { "HorizontalAlignment" } else { "VerticalAlignment" };
        out.set_pseudo_prop("UIListLayout", prop, luau::Value::enum_item(prop, side));
        return;
    }
    let side = match lower.as_str() {
        "start" | "flex-start" => 0,
        "center" => 1,
        "end" | "flex-end" => 2,
        _ => {
            diag.warn(format!("unknown `align-items: {s}` (ignored)"), span);
            return;
        }
    };
    if column {
        let v = ["Left", "Center", "Right"][side];
        out.set_pseudo_prop("UIListLayout", "HorizontalAlignment", luau::Value::enum_item("HorizontalAlignment", v));
    } else {
        let v = ["Top", "Center", "Bottom"][side];
        out.set_pseudo_prop("UIListLayout", "VerticalAlignment", luau::Value::enum_item("VerticalAlignment", v));
    }
}

/// One CSS grid track size. Roblox cells are uniform, so a track list only translates when every
/// track is the same.
#[derive(Clone, Copy, PartialEq)]
enum Track {
    /// A fixed length, as a `(scale, offset)` pair.
    Fixed(f64, f64),
    /// A `<n>fr` share of the container.
    Fraction,
    /// `auto` / `min-content` / `max-content`: content-sized, so only approximable as a share.
    Auto,
}

fn track_size(v: &Value) -> Result<Track, String> {
    if let Some(s) = v.as_str() {
        return match s.to_ascii_lowercase().as_str() {
            "auto" | "min-content" | "max-content" => Ok(Track::Auto),
            other => Err(format!("track size `{other}` has no Roblox equivalent")),
        };
    }
    if let Value::Number(n) = v
        && unit_of(n) == "fr"
    {
        return Ok(Track::Fraction);
    }
    if let Value::Call { name, .. } = v {
        return Err(format!("`{name}()` track sizes have no Roblox equivalent"));
    }
    length_value(v).map(|(s, o)| Track::Fixed(s, o))
}

/// Flattens a track list (`repeat(3, 68px)`, `68px 68px 68px`, …) into a cell count and the one
/// size all of its tracks share.
fn grid_tracks(v: &Value) -> Result<(usize, Track), String> {
    let mut tracks: Vec<Track> = Vec::new();
    for item in space_items(v) {
        match &item {
            Value::Call { name, args } if name.eq_ignore_ascii_case("repeat") => {
                let [count, track] = args.as_slice() else {
                    return Err("`repeat()` takes a count and a track size".to_string());
                };
                let Some(n) = count.as_number().map(|n| n.value) else {
                    let text = count.as_str().unwrap_or_default().to_string();
                    return Err(format!(
                        "`repeat({text}, …)` has no Roblox equivalent (the cell count must be a number)"
                    ));
                };
                if n < 1.0 || n.fract() != 0.0 {
                    return Err(format!("`repeat()` needs a whole cell count, got `{}`", count.inspect()));
                }
                let size = track_size(track)?;
                tracks.extend(std::iter::repeat_n(size, n as usize));
            }
            other => tracks.push(track_size(other)?),
        }
    }
    let first = *tracks.first().ok_or("expected a track list")?;
    if tracks.iter().any(|t| *t != first) {
        return Err("Roblox grid cells are all the same size, so every track must be too".to_string());
    }
    Ok((tracks.len(), first))
}

/// The cell extent along one axis: fixed tracks keep their length, shares split the container
/// minus the gaps that sit between the cells (`CellPadding` is added on top of `CellSize`).
fn cell_extent(track: Track, count: usize, gap: (f64, f64)) -> (f64, f64) {
    match track {
        Track::Fixed(s, o) => (s, o),
        Track::Fraction | Track::Auto => {
            let n = count as f64;
            (1.0 / n - gap.0 * (n - 1.0) / n, -gap.1 * (n - 1.0) / n)
        }
    }
}

/// `grid-template-*` / `grid-auto-*` → `UIGridLayout.FillDirectionMaxCells` and `CellSize`.
fn translate_grid(
    decls: &[Decl],
    opts: &ApproxOptions,
    column_flow: bool,
    gaps: ((f64, f64), (f64, f64)),
    diag: &mut Diagnostics,
    out: &mut Translated,
) {
    let (column_gap, row_gap) = gaps;
    let mut axis = |names: [&str; 2]| -> Option<(usize, Track, Option<&Span>)> {
        let d = names.iter().find_map(|n| last(decls, n))?;
        match grid_tracks(&d.value) {
            Ok((count, track)) => Some((count, track, d.span.as_ref())),
            Err(e) => {
                diag.warn(format!("`{}`: {e} (ignored)", d.name), d.span.as_ref());
                None
            }
        }
    };
    let columns = axis(["grid-template-columns", "grid-auto-columns"]);
    let rows = axis(["grid-template-rows", "grid-auto-rows"]);
    let span = columns.and_then(|c| c.2).or_else(|| rows.and_then(|r| r.2));

    // The cells per line are counted along the fill direction: columns for a row flow, rows for a
    // column flow.
    if let Some((count, _, _)) = if column_flow { rows } else { columns } {
        out.set_pseudo_prop("UIGridLayout", "FillDirectionMaxCells", luau::Value::Number(count as f64));
    }
    if columns.is_none() && rows.is_none() {
        return;
    }
    for (track, axis_name) in [(columns, "grid-auto-columns"), (rows, "grid-auto-rows")] {
        if track.is_none() {
            let message =
                format!("grid cells need a size on both axes; add `{axis_name}` (the Roblox default of 100px is used)");
            if opts.strict {
                diag.error(format!("strict: {message}"), span);
            } else {
                diag.warn(message, span);
            }
        }
        if let Some((_, Track::Auto, s)) = track {
            diag.warn("content-sized grid tracks are approximated as equal shares of the container", s);
        }
    }
    let x = columns.map_or((0.0, 100.0), |(count, track, _)| cell_extent(track, count, column_gap));
    let y = rows.map_or((0.0, 100.0), |(count, track, _)| cell_extent(track, count, row_gap));
    out.set_pseudo_prop("UIGridLayout", "CellSize", luau::Value::udim2(x.0, x.1, y.0, y.1));
}

/// Whether the rule makes its element a flex container.
fn is_flex(decls: &[Decl]) -> bool {
    last(decls, "display").and_then(|d| d.value.as_str()).is_some_and(|s| {
        let s = s.to_ascii_lowercase();
        s == "flex" || s == "inline-flex"
    })
}

/// Whether the element is a flex row whose items should shrink to fit it, as CSS's initial
/// `flex-shrink: 1` makes them. A column is left out: Roblox shrinks a content-sized item below its
/// content, where CSS stops at it, and every column that overflows would crush its items. So is a
/// row that scrolls along the line, whose items the same shrink would squeeze into the window.
pub fn shrinks_items(decls: &[Decl]) -> bool {
    let row = last(decls, "flex-direction")
        .and_then(|d| d.value.as_str())
        .is_none_or(|s| s.to_ascii_lowercase().starts_with("row"));
    is_flex(decls) && row && !scroll_axes(decls).0
}

/// Whether the rule makes its element a flex or grid container.
pub fn is_flex_or_grid(decls: &[Decl]) -> bool {
    is_flex(decls)
        || last(decls, "display").and_then(|d| d.value.as_str()).is_some_and(|s| {
            let s = s.to_ascii_lowercase();
            s == "grid" || s == "inline-grid"
        })
}

/// Whether the rule lets its element grow along a flex line (past its `width`/`height`).
fn flex_grows(decls: &[Decl]) -> bool {
    decls.iter().any(|d| match d.name.as_str() {
        "flex-grow" => d.value.as_number().is_some_and(|n| n.value > 0.0),
        "flex" => match d.value.as_str() {
            Some(s) => s.eq_ignore_ascii_case("auto"),
            None => space_items(&d.value).first().and_then(|v| v.as_number()).is_some_and(|n| n.value > 0.0),
        },
        _ => false,
    })
}

fn translate_layout(decls: &[Decl], opts: &ApproxOptions, diag: &mut Diagnostics, out: &mut Translated) {
    const GRID_PROPERTIES: [&str; 5] =
        ["grid-template-columns", "grid-template-rows", "grid-auto-columns", "grid-auto-rows", "grid-auto-flow"];
    // Grid tracks imply a grid even when `display: grid` is set by another rule.
    let mut is_grid = GRID_PROPERTIES.iter().any(|n| last(decls, n).is_some());
    if let Some(d) = last(decls, "display")
        && let Some(s) = d.value.as_str()
    {
        match s.to_ascii_lowercase().as_str() {
            "flex" | "inline-flex" => {
                out.set_pseudo_prop(
                    "UIListLayout",
                    "FillDirection",
                    luau::Value::enum_item("FillDirection", "Horizontal"),
                );
                out.set_pseudo_prop("UIListLayout", "SortOrder", luau::Value::enum_item("SortOrder", "LayoutOrder"));
                out.set_prop("Visible", luau::Value::Bool(true));
            }
            "grid" | "inline-grid" => {
                is_grid = true;
                out.set_pseudo_prop("UIGridLayout", "SortOrder", luau::Value::enum_item("SortOrder", "LayoutOrder"));
                out.set_prop("Visible", luau::Value::Bool(true));
            }
            "none" => out.set_prop("Visible", luau::Value::Bool(false)),
            _ => out.set_prop("Visible", luau::Value::Bool(true)),
        }
    }

    let mut direction_column = false;
    if let Some(d) = last(decls, "flex-direction")
        && let Some(s) = d.value.as_str()
    {
        let lower = s.to_ascii_lowercase();
        let dir = match lower.as_str() {
            "row" => "Horizontal",
            "row-reverse" => {
                diag.warn("`flex-direction: row-reverse` reversal has no Roblox equivalent", d.span.as_ref());
                "Horizontal"
            }
            "column" => "Vertical",
            "column-reverse" => {
                diag.warn("`flex-direction: column-reverse` reversal has no Roblox equivalent", d.span.as_ref());
                "Vertical"
            }
            other => {
                diag.warn(format!("unknown `flex-direction: {other}` (ignored)"), d.span.as_ref());
                "Horizontal"
            }
        };
        direction_column = dir == "Vertical";
        out.set_pseudo_prop("UIListLayout", "FillDirection", luau::Value::enum_item("FillDirection", dir));
    }

    let mut grid_column_flow = false;
    if let Some(d) = last(decls, "grid-auto-flow") {
        for item in space_items(&d.value) {
            match item.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("row") => grid_column_flow = false,
                Some("column") => grid_column_flow = true,
                Some("dense") => diag.warn("`grid-auto-flow: dense` packing has no Roblox equivalent", d.span.as_ref()),
                _ => diag.warn(format!("unknown `grid-auto-flow: {}` (ignored)", d.value.inspect()), d.span.as_ref()),
            }
        }
        let dir = if grid_column_flow { "Vertical" } else { "Horizontal" };
        out.set_pseudo_prop("UIGridLayout", "FillDirection", luau::Value::enum_item("FillDirection", dir));
    }
    let column_flow = if is_grid { grid_column_flow } else { direction_column };

    if let Some(d) = last(decls, "justify-content")
        && let Some(s) = d.value.as_str()
    {
        apply_justify(s, column_flow, is_grid, out, diag, d.span.as_ref());
    }
    match last(decls, "align-items") {
        Some(d) => {
            if let Some(s) = d.value.as_str() {
                apply_align_items(s, direction_column, out, diag, d.span.as_ref());
            }
        }
        // CSS flex items stretch across the line unless told otherwise. translate_size keeps
        // explicitly sized items out of it, as CSS does.
        None if is_flex(decls) => out.set_pseudo_prop(
            "UIListLayout",
            "ItemLineAlignment",
            luau::Value::enum_item("ItemLineAlignment", "Stretch"),
        ),
        None => {}
    }

    let mut rg: Option<Value> = None;
    let mut cg: Option<Value> = None;
    if let Some(d) = last(decls, "gap") {
        let items = space_items(&d.value);
        if items.len() >= 2 {
            rg = Some(items[0].clone());
            cg = Some(items[1].clone());
        } else {
            rg = Some(items[0].clone());
            cg = Some(items[0].clone());
        }
    }
    if let Some(d) = last(decls, "row-gap") {
        rg = Some(d.value.clone());
    }
    if let Some(d) = last(decls, "column-gap") {
        cg = Some(d.value.clone());
    }
    let gap_span = ["gap", "row-gap", "column-gap"].iter().find_map(|n| last(decls, n)).and_then(|d| d.span.as_ref());
    let list_gap = if direction_column { &rg } else { &cg };
    if let (Some(v), false) = (list_gap, is_grid) {
        match length_value(v) {
            Ok((s, o)) => out.set_pseudo_prop("UIListLayout", "Padding", luau::Value::udim(s, o)),
            Err(e) => diag.warn(format!("`gap`: {e} (ignored)"), gap_span),
        }
    }
    let cell_padding = |v: &Option<Value>| match v {
        Some(v) => length_value(v).unwrap_or((0.0, 0.0)),
        None => (0.0, 0.0),
    };
    let (column_gap, row_gap) = (cell_padding(&cg), cell_padding(&rg));
    if is_grid
        && let (Some(c), Some(r)) = (&cg, &rg)
        && let (Ok((cs, co)), Ok((rs, ro))) = (length_value(c), length_value(r))
    {
        out.set_pseudo_prop("UIGridLayout", "CellPadding", luau::Value::udim2(cs, co, rs, ro));
    }
    translate_grid(decls, opts, grid_column_flow, (column_gap, row_gap), diag, out);

    if let Some(d) = last(decls, "flex-wrap")
        && let Some(s) = d.value.as_str()
    {
        match s.to_ascii_lowercase().as_str() {
            "wrap" => out.set_pseudo_prop("UIListLayout", "Wraps", luau::Value::Bool(true)),
            "nowrap" => out.set_pseudo_prop("UIListLayout", "Wraps", luau::Value::Bool(false)),
            "wrap-reverse" => {
                diag.warn("`flex-wrap: wrap-reverse` approximated as `wrap`", d.span.as_ref());
                out.set_pseudo_prop("UIListLayout", "Wraps", luau::Value::Bool(true));
            }
            other => diag.warn(format!("unknown `flex-wrap: {other}` (ignored)"), d.span.as_ref()),
        }
    }

    let mut grow: Option<f64> = None;
    let mut shrink: Option<f64> = None;
    let mut flex_mode: Option<&'static str> = None;
    for d in decls {
        match d.name.as_str() {
            "flex-grow" => {
                if let Some(n) = d.value.as_number() {
                    grow = Some(n.value);
                    flex_mode = Some("Custom");
                }
            }
            "flex-shrink" => {
                if let Some(n) = d.value.as_number() {
                    shrink = Some(n.value);
                    flex_mode = Some("Custom");
                }
            }
            "flex" => {
                if let Some(s) = d.value.as_str() {
                    match s.to_ascii_lowercase().as_str() {
                        "none" => flex_mode = Some("None"),
                        "auto" => flex_mode = Some("Fill"),
                        other => diag.warn(format!("unknown `flex: {other}` (ignored)"), d.span.as_ref()),
                    }
                } else {
                    let items = space_items(&d.value);
                    if let Some(n) = items.first().and_then(|v| v.as_number()) {
                        grow = Some(n.value);
                        flex_mode = Some("Custom");
                    }
                    if let Some(n) = items.get(1).and_then(|v| v.as_number()) {
                        shrink = Some(n.value);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(mode) = flex_mode {
        out.set_pseudo_prop("UIFlexItem", "FlexMode", luau::Value::enum_item("UIFlexMode", mode));
        if mode == "Custom" {
            if let Some(g) = grow {
                out.set_pseudo_prop("UIFlexItem", "GrowRatio", luau::Value::Number(g));
            }
            if let Some(s) = shrink {
                out.set_pseudo_prop("UIFlexItem", "ShrinkRatio", luau::Value::Number(s));
            }
        }
    }

    if let Some(d) = last(decls, "align-self")
        && let Some(s) = d.value.as_str()
    {
        let e = match s.to_ascii_lowercase().as_str() {
            "auto" => Some("Automatic"),
            "flex-start" | "start" => Some("Start"),
            "center" => Some("Center"),
            "flex-end" | "end" => Some("End"),
            "stretch" => Some("Stretch"),
            other => {
                diag.warn(format!("unknown `align-self: {other}` (ignored)"), d.span.as_ref());
                None
            }
        };
        if let Some(e) = e {
            out.set_pseudo_prop("UIFlexItem", "ItemLineAlignment", luau::Value::enum_item("ItemLineAlignment", e));
        }
    }

    if let Some(d) = last(decls, "order")
        && let Some(n) = d.value.as_number()
    {
        out.set_prop("LayoutOrder", luau::Value::Number(n.value.round()));
    }
}

// ---------------------------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Overflow {
    Visible,
    Clip,
    Scroll,
}

fn parse_overflow(v: &Value, d: &Decl, diag: &mut Diagnostics) -> Option<Overflow> {
    let s = v.as_str()?;
    match s.to_ascii_lowercase().as_str() {
        "visible" => Some(Overflow::Visible),
        "hidden" | "clip" => Some(Overflow::Clip),
        "scroll" | "auto" | "overlay" => Some(Overflow::Scroll),
        other => {
            diag.warn(format!("unknown `overflow: {other}` (ignored)"), d.span.as_ref());
            None
        }
    }
}

/// `overflow`, `overflow-x` and `overflow-y`, cascaded per axis in declaration order.
///
/// A CSS scroll container's scrollable area is always exactly its content, so a scrolling axis also
/// gets AutomaticCanvasSize. CanvasSize is a floor under the automatic size (measured in Studio), and
/// its `{0, 0}, {2, 0}` default would keep a canvas twice the frame's height, so it's zeroed.
fn translate_overflow(decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    let (x, y) = overflow_axes(decls, diag);
    if x.is_none() && y.is_none() {
        return;
    }
    let clips = [x, y].iter().any(|a| matches!(a, Some(Overflow::Clip | Overflow::Scroll)));
    out.set_prop("ClipsDescendants", luau::Value::Bool(clips));
    let axes = match (x == Some(Overflow::Scroll), y == Some(Overflow::Scroll)) {
        (true, true) => "XY",
        (true, false) => "X",
        (false, true) => "Y",
        (false, false) => return,
    };
    out.set_prop("ScrollingEnabled", luau::Value::Bool(true));
    out.set_prop("ScrollingDirection", luau::Value::enum_item("ScrollingDirection", axes));
    out.set_prop("AutomaticCanvasSize", luau::Value::enum_item("AutomaticSize", axes));
    out.set_prop("CanvasSize", luau::Value::udim2(0.0, 0.0, 0.0, 0.0));
}

/// Whether the element scrolls along each axis: `(x, y)`.
pub fn scroll_axes(decls: &[Decl]) -> (bool, bool) {
    let (x, y) = overflow_axes(decls, &mut Diagnostics::default());
    (x == Some(Overflow::Scroll), y == Some(Overflow::Scroll))
}

/// The effective overflow of each axis: `(x, y)`.
fn overflow_axes(decls: &[Decl], diag: &mut Diagnostics) -> (Option<Overflow>, Option<Overflow>) {
    let (mut x, mut y) = (None, None);
    for d in decls {
        match strip_vendor_prefix(&d.name) {
            "overflow" => {
                let (vx, vy) = match &d.value {
                    Value::List { items, sep: ListSep::Space, .. } if items.len() == 2 => (&items[0], &items[1]),
                    v => (v, v),
                };
                if let Some(vx) = parse_overflow(vx, d, diag) {
                    x = Some(vx);
                }
                if let Some(vy) = parse_overflow(vy, d, diag) {
                    y = Some(vy);
                }
            }
            "overflow-x" => x = parse_overflow(&d.value, d, diag).or(x),
            "overflow-y" => y = parse_overflow(&d.value, d, diag).or(y),
            _ => {}
        }
    }
    (x, y)
}

/// Roblox's own scrollbar thickness, which `scrollbar-width: auto` restores.
const SCROLLBAR_AUTO_PX: f64 = 12.0;
/// Firefox's `thin` scrollbar.
const SCROLLBAR_THIN_PX: f64 = 8.0;

fn translate_scrollbar(
    decls: &[Decl],
    opts: &ApproxOptions,
    opacity_factor: Option<f64>,
    diag: &mut Diagnostics,
    out: &mut Translated,
) {
    if opts.groups.contains(&Group::Visibility) {
        if let Some(d) = last(decls, "scrollbar-width") {
            let px = match d.value.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("auto") => Ok(SCROLLBAR_AUTO_PX),
                Some("thin") => Ok(SCROLLBAR_THIN_PX),
                Some("none") => Ok(0.0),
                Some(other) => Err(format!("unknown value `{other}`")),
                // Not CSS, but the obvious meaning of a length.
                None => px_only(&d.value),
            };
            match px {
                Ok(px) => out.set_prop("ScrollBarThickness", luau::Value::Number(px)),
                Err(e) => diag.warn(format!("`scrollbar-width`: {e} (ignored)"), d.span.as_ref()),
            }
        }
        if let Some(d) = last(decls, "scrollbar-gutter") {
            let text = d.value.to_css().unwrap_or_default().to_ascii_lowercase();
            let words: Vec<&str> = text.split_whitespace().collect();
            match words.as_slice() {
                ["auto"] => {
                    out.set_prop("VerticalScrollBarInset", luau::Value::enum_item("ScrollBarInset", "ScrollBar"))
                }
                ["stable"] => {
                    out.set_prop("VerticalScrollBarInset", luau::Value::enum_item("ScrollBarInset", "Always"))
                }
                ["stable", "both-edges"] | ["both-edges", "stable"] => {
                    out.set_prop("VerticalScrollBarInset", luau::Value::enum_item("ScrollBarInset", "Always"));
                    diag.warn(
                        "`scrollbar-gutter: stable both-edges`: Roblox only reserves the scrollbar's own edge",
                        d.span.as_ref(),
                    );
                }
                _ => diag.warn(format!("unknown `scrollbar-gutter: {text}` (ignored)"), d.span.as_ref()),
            }
        }
    }

    // `scrollbar-color: <thumb> <track>`. Roblox draws only the thumb.
    if (opts.groups.contains(&Group::Color) || opts.groups.contains(&Group::Opacity))
        && let Some(d) = last(decls, "scrollbar-color")
    {
        let thumb = match &d.value {
            Value::List { items, sep: ListSep::Space, .. } if items.len() == 2 => Some(&items[0]),
            v if v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("auto")) => None,
            v => Some(v),
        };
        let (color, alpha) = match thumb {
            // `auto`: back to Roblox's own white thumb.
            None => (Ok((white(), Some(1.0))), 1.0),
            Some(v) => {
                let r = resolve_color(v);
                let a = r.as_ref().ok().and_then(|(_, a)| *a).unwrap_or(1.0);
                (r, a)
            }
        };
        match color {
            Ok((expr, _)) => {
                if opts.groups.contains(&Group::Color) {
                    out.set_prop("ScrollBarImageColor3", expr);
                }
                out.set_prop(
                    "ScrollBarImageTransparency",
                    luau::Value::Number(compute_transparency(alpha, opacity_factor)),
                );
            }
            Err(e) => diag.warn(format!("`scrollbar-color`: {e} (ignored)"), d.span.as_ref()),
        }
    }
}

fn translate_visibility(decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    if let Some(d) = last(decls, "visibility")
        && let Some(s) = d.value.as_str()
    {
        match s.to_ascii_lowercase().as_str() {
            "hidden" | "collapse" => out.set_prop("Visible", luau::Value::Bool(false)),
            "visible" => out.set_prop("Visible", luau::Value::Bool(true)),
            other => diag.warn(format!("unknown `visibility: {other}` (ignored)"), d.span.as_ref()),
        }
    }
    translate_overflow(decls, diag, out);
    if let Some(d) = last(decls, "pointer-events")
        && let Some(s) = d.value.as_str()
    {
        match s.to_ascii_lowercase().as_str() {
            "none" => out.set_prop("Interactable", luau::Value::Bool(false)),
            "auto" => out.set_prop("Interactable", luau::Value::Bool(true)),
            other => diag.warn(format!("unknown `pointer-events: {other}` (ignored)"), d.span.as_ref()),
        }
    }
    // `appearance: none` drops a control's built-in styling; for Roblox buttons that's the automatic
    // hover/press darkening.
    if let Some(d) = last(decls, "appearance")
        && let Some(s) = d.value.as_str()
    {
        match s.to_ascii_lowercase().as_str() {
            "none" => out.set_prop("AutoButtonColor", luau::Value::Bool(false)),
            "auto" | "button" => out.set_prop("AutoButtonColor", luau::Value::Bool(true)),
            other => diag.warn(format!("unknown `appearance: {other}` (ignored)"), d.span.as_ref()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Transition
// ---------------------------------------------------------------------------------------------

/// Roblox properties on the element itself that a transitioned CSS property animates.
pub(crate) fn css_property_targets(name: &str) -> Vec<&'static str> {
    match name {
        "background-color" | "background" => vec!["BackgroundColor3", "BackgroundTransparency"],
        "color" => vec!["TextColor3", "TextTransparency"],
        "opacity" => vec![
            "GroupTransparency",
            "BackgroundTransparency",
            "TextTransparency",
            "ImageTransparency",
            "ScrollBarImageTransparency",
        ],
        "scrollbar-color" => vec!["ScrollBarImageColor3", "ScrollBarImageTransparency"],
        "scrollbar-width" => vec!["ScrollBarThickness"],
        // Stretched elements get their size from the edges too.
        "left" | "top" | "right" | "bottom" | "inset" => vec!["Position", "Size"],
        "width" | "height" => vec!["Size"],
        "transform" => vec!["Rotation", "Position", "AnchorPoint"],
        "translate" => vec!["Position", "AnchorPoint"],
        "rotate" => vec!["Rotation"],
        "font-size" => vec!["TextSize"],
        "z-index" => vec!["ZIndex"],
        _ => Vec::new(),
    }
}

/// Pseudo-instance properties (class, property) that a transitioned CSS property animates.
pub(crate) fn css_pseudo_targets(name: &str) -> Vec<(&'static str, &'static str)> {
    const PADDING: [(&str, &str); 4] = [
        ("UIPadding", "PaddingTop"),
        ("UIPadding", "PaddingRight"),
        ("UIPadding", "PaddingBottom"),
        ("UIPadding", "PaddingLeft"),
    ];
    match name {
        "transform" | "scale" => vec![("UIScale", "Scale")],
        "opacity" => vec![("UIStroke", "Transparency")],
        "border" | "outline" => {
            vec![("UIStroke", "Color"), ("UIStroke", "Thickness"), ("UIStroke", "Transparency")]
        }
        "border-color" | "outline-color" => vec![("UIStroke", "Color"), ("UIStroke", "Transparency")],
        "border-width" | "outline-width" => vec![("UIStroke", "Thickness")],
        "border-radius" => vec![("UICorner", "CornerRadius")],
        "padding" | "padding-inline" | "padding-block" => PADDING.to_vec(),
        "padding-top" => vec![PADDING[0]],
        "padding-right" => vec![PADDING[1]],
        "padding-bottom" => vec![PADDING[2]],
        "padding-left" => vec![PADDING[3]],
        "gap" | "row-gap" | "column-gap" => vec![("UIListLayout", "Padding"), ("UIGridLayout", "CellPadding")],
        "min-width" | "min-height" => vec![("UISizeConstraint", "MinSize")],
        "max-width" | "max-height" => vec![("UISizeConstraint", "MaxSize")],
        _ => Vec::new(),
    }
}

/// Picks the closest Roblox easing for `cubic-bezier(x1, y1, x2, y2)`.
///
/// Control points outside 0..1 on the y axis mean overshoot (`Back`): above 1 overshoots the end
/// (Out), below 0 anticipates the start (In). Otherwise the direction comes from which control point
/// bends away from the diagonal — the first below it accelerates (In), the second above it
/// decelerates (Out) — and the total bend picks how strong a curve to use.
fn cubic_bezier_easing(x1: f64, y1: f64, x2: f64, y2: f64) -> (&'static str, &'static str) {
    let overshoot = y1 > 1.0 + 1e-9 || y2 > 1.0 + 1e-9;
    let anticipate = y1 < -1e-9 || y2 < -1e-9;
    match (overshoot, anticipate) {
        (true, true) => return ("Back", "InOut"),
        (true, false) => return ("Back", "Out"),
        (false, true) => return ("Back", "In"),
        _ => {}
    }
    let accelerate = (x1 - y1).max(0.0) + (x2 - y2).max(0.0);
    let decelerate = (y1 - x1).max(0.0) + (y2 - x2).max(0.0);
    if accelerate + decelerate < 0.1 {
        return ("Linear", "InOut");
    }
    let (direction, bend) = if accelerate.min(decelerate) > 0.3 * accelerate.max(decelerate) {
        ("InOut", accelerate + decelerate)
    } else if decelerate > accelerate {
        ("Out", decelerate)
    } else {
        ("In", accelerate)
    };
    let style = match bend {
        b if b < 0.5 => "Sine",
        b if b < 1.0 => "Quad",
        b if b < 1.3 => "Cubic",
        b if b < 1.65 => "Quint",
        _ => "Exponential",
    };
    (style, direction)
}

/// A timing function as a token for `timing_to_style_direction`: keywords, Roblox easing names, or
/// `cubic-bezier()` (encoded as `Style:Direction`).
fn timing_token(v: &Value, diag: &mut Diagnostics, span: Option<&Span>) -> Option<String> {
    match v {
        Value::Str { text, .. } => Some(text.to_ascii_lowercase()),
        Value::Call { name, args } if name == "cubic-bezier" => {
            let n: Vec<f64> = args.iter().filter_map(|a| a.as_number().map(|n| n.value)).collect();
            if let [x1, y1, x2, y2] = n[..] {
                let (style, direction) = cubic_bezier_easing(x1, y1, x2, y2);
                Some(format!("{style}:{direction}"))
            } else {
                diag.warn("`cubic-bezier()` needs four numbers; using Quad Out", span);
                Some("quad".to_string())
            }
        }
        Value::Call { name, .. } if name == "steps" => {
            diag.warn("`steps()` timing has no Roblox equivalent; using Linear", span);
            Some("linear".to_string())
        }
        _ => None,
    }
}

fn easing_style(l: &str) -> Option<&'static str> {
    const NAMES: &[(&str, &str)] = &[
        ("linear", "Linear"),
        ("sine", "Sine"),
        ("back", "Back"),
        ("quad", "Quad"),
        ("quart", "Quart"),
        ("quint", "Quint"),
        ("bounce", "Bounce"),
        ("elastic", "Elastic"),
        ("exponential", "Exponential"),
        ("circular", "Circular"),
        ("cubic", "Cubic"),
    ];
    NAMES.iter().find(|(n, _)| *n == l).map(|(_, v)| *v)
}

fn easing_direction(l: &str) -> Option<&'static str> {
    match l {
        "in" => Some("In"),
        "out" => Some("Out"),
        "inout" => Some("InOut"),
        _ => None,
    }
}

/// Maps a CSS/Roblox timing token to `(EasingStyle, EasingDirection)`.
fn timing_to_style_direction(token: &str) -> Option<(&'static str, &'static str)> {
    // Pre-classified `cubic-bezier()` curves arrive as `Style:Direction`.
    if let Some((style, direction)) = token.split_once(':') {
        let style = easing_style(&style.to_ascii_lowercase())?;
        let direction = easing_direction(&direction.to_ascii_lowercase())?;
        return Some((style, direction));
    }
    let l = token.to_ascii_lowercase();
    match l.as_str() {
        "linear" => Some(("Linear", "InOut")),
        "ease" => Some(("Quad", "Out")),
        "ease-in" => Some(("Quad", "In")),
        "ease-out" => Some(("Quad", "Out")),
        "ease-in-out" => Some(("Quad", "InOut")),
        _ => easing_style(&l).map(|s| (s, "Out")),
    }
}

fn seconds_of(n: &Number, diag: &mut Diagnostics) -> f64 {
    let unit = unit_of(n);
    match unit.as_str() {
        "s" | "" => n.value,
        "ms" => n.value / 1000.0,
        _ => {
            diag.warn(format!("unsupported duration unit in `{}`", n.to_css()), None);
            0.0
        }
    }
}

fn build_tween_info(dur: f64, style: &str, direction: &str, delay: f64) -> luau::TweenInfo {
    luau::TweenInfo::new(dur, style, direction, delay.max(0.0))
}

fn emit_transition_entry(prop: &str, dur: f64, timing: &str, delay: f64, diag: &mut Diagnostics, out: &mut Translated) {
    if dur <= 0.0 {
        diag.warn(format!("`transition: {prop}` has a 0s duration (ignored)"), None);
        return;
    }
    let (style, direction) = timing_to_style_direction(timing).unwrap_or_else(|| {
        diag.warn(format!("`transition` timing function `{timing}` has no Roblox equivalent; using Quad Out"), None);
        ("Quad", "Out")
    });
    let expr = build_tween_info(dur, style, direction, delay);

    if prop == "all" || prop == "*" {
        out.set_transition("*", expr);
        return;
    }
    if prop.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        out.set_transition(prop, expr);
        return;
    }
    let targets = css_property_targets(prop);
    let pseudo_targets = css_pseudo_targets(prop);
    if targets.is_empty() && pseudo_targets.is_empty() {
        diag.warn(format!("`{prop}` has no known Roblox property to transition (ignored)"), None);
        return;
    }
    for t in targets {
        out.set_transition(t, expr.clone());
    }
    for (class, property) in pseudo_targets {
        out.set_pseudo_transition(class, property, expr.clone());
    }
}

fn comma_list_strings(d: Option<&Decl>) -> Vec<String> {
    let Some(d) = d else { return Vec::new() };
    match &d.value {
        Value::List { items, sep: ListSep::Comma, .. } => {
            items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        }
        other => other.as_str().map(|s| vec![s.to_string()]).unwrap_or_default(),
    }
}

fn comma_list_seconds(d: Option<&Decl>, diag: &mut Diagnostics) -> Vec<f64> {
    let Some(d) = d else { return Vec::new() };
    match &d.value {
        Value::List { items, sep: ListSep::Comma, .. } => {
            items.iter().filter_map(|v| v.as_number()).map(|n| seconds_of(n, diag)).collect()
        }
        Value::Number(n) => vec![seconds_of(n, diag)],
        _ => Vec::new(),
    }
}

fn translate_transition_group(decls: &[Decl], diag: &mut Diagnostics, out: &mut Translated) {
    let has_longhand = ["transition-property", "transition-duration", "transition-timing-function", "transition-delay"]
        .iter()
        .any(|n| last(decls, n).is_some());

    if has_longhand {
        let props = comma_list_strings(last(decls, "transition-property"));
        let durs = comma_list_seconds(last(decls, "transition-duration"), diag);
        let timings: Vec<String> = match last(decls, "transition-timing-function") {
            Some(d) => {
                let items = match &d.value {
                    Value::List { items, sep: ListSep::Comma, .. } => items.clone(),
                    other => vec![other.clone()],
                };
                items.iter().filter_map(|v| timing_token(v, diag, d.span.as_ref())).collect()
            }
            None => Vec::new(),
        };
        let delays = comma_list_seconds(last(decls, "transition-delay"), diag);
        if props.is_empty() {
            diag.warn("`transition-property` is required to build a transition (ignored)", None);
            return;
        }
        for (i, prop) in props.iter().enumerate() {
            let dur = if durs.is_empty() { 0.0 } else { durs[i % durs.len()] };
            let timing = if timings.is_empty() { "ease".to_string() } else { timings[i % timings.len()].clone() };
            let delay = if delays.is_empty() { 0.0 } else { delays[i % delays.len()] };
            emit_transition_entry(prop, dur, &timing, delay, diag, out);
        }
        return;
    }

    if let Some(d) = last(decls, "transition") {
        if let Some(s) = d.value.as_str()
            && s.eq_ignore_ascii_case("none")
        {
            return;
        }
        let entries: Vec<Value> = match &d.value {
            Value::List { items, sep: ListSep::Comma, .. } => items.clone(),
            other => vec![other.clone()],
        };
        for e in entries {
            let items = space_items(&e);
            let mut prop: Option<String> = None;
            let mut durs: Vec<f64> = Vec::new();
            let mut timing: Option<String> = None;
            for it in &items {
                match it {
                    Value::Number(n) => durs.push(seconds_of(n, diag)),
                    Value::Str { text, .. } => {
                        let lower = text.to_ascii_lowercase();
                        if lower == "all" {
                            prop = Some("all".to_string());
                        } else if timing_to_style_direction(&lower).is_some() {
                            timing = Some(lower);
                        } else if prop.is_none() {
                            prop = Some(text.clone());
                        }
                    }
                    Value::Call { .. } => timing = timing_token(it, diag, d.span.as_ref()),
                    _ => {}
                }
            }
            let Some(prop) = prop else {
                diag.warn("transition entry has no property (ignored)", d.span.as_ref());
                continue;
            };
            let dur = durs.first().copied().unwrap_or(0.0);
            let delay = durs.get(1).copied().unwrap_or(0.0);
            let timing_tok = timing.unwrap_or_else(|| "ease".to_string());
            emit_transition_entry(&prop, dur, &timing_tok, delay, diag, out);
        }
    }
}

/// `Transition: BackgroundColor3 0.2s Quad Out, BackgroundTransparency`
/// (Roblox property names, used WITHOUT `--approx`).
pub fn roblox_transitions(value: &Value) -> Result<Vec<(String, luau::TweenInfo)>, String> {
    let entries: Vec<Value> = match value {
        Value::List { items, sep: ListSep::Comma, .. } => items.clone(),
        other => vec![other.clone()],
    };
    let mut out = Vec::new();
    for e in entries {
        let items = space_items(&e);
        let mut prop: Option<String> = None;
        let mut durations: Vec<f64> = Vec::new();
        let mut style = "Quad";
        let mut direction = "Out";
        for it in &items {
            match it {
                Value::Number(n) => {
                    let unit = unit_of(n);
                    let secs = match unit.as_str() {
                        "s" | "" => n.value,
                        "ms" => n.value / 1000.0,
                        _ => return Err(format!("unsupported duration unit in `{}`", n.to_css())),
                    };
                    durations.push(secs);
                }
                Value::Str { text, .. } => {
                    let lower = text.to_ascii_lowercase();
                    if lower == "all" || lower == "*" {
                        prop = Some("*".to_string());
                    } else if let Some(s) = easing_style(&lower) {
                        style = s;
                    } else if let Some(d) = easing_direction(&lower) {
                        direction = d;
                    } else if prop.is_none() {
                        prop = Some(text.clone());
                    } else {
                        return Err(format!("unexpected token `{text}` in transition"));
                    }
                }
                other => return Err(format!("unsupported token `{}` in transition", other.inspect())),
            }
        }
        let prop = prop.ok_or("transition entry has no property")?;
        let dur = durations.first().copied().unwrap_or(1.0);
        let delay = durations.get(1).copied();
        let expr = build_tween_info(dur, style, direction, delay.unwrap_or(0.0));
        out.push((prop, expr));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_for(groups: &[Group]) -> ApproxOptions {
        ApproxOptions { groups: Group::expand(groups), strict: false }
    }

    fn all_opts() -> ApproxOptions {
        opts_for(&[Group::All])
    }

    fn decl(name: &str, value: Value) -> Decl {
        Decl { name: name.to_string(), value, span: None }
    }

    fn prop(t: &Translated, name: &str) -> Option<String> {
        t.props.iter().find(|(n, _)| n == name).map(|(_, v)| luau::render(v))
    }

    fn pseudo(t: &Translated, class: &str, name: &str) -> Option<String> {
        t.pseudo
            .iter()
            .find(|(n, _)| n == class)
            .and_then(|(_, props)| props.iter().find(|(n, _)| n == name))
            .map(|(_, v)| luau::render(v))
    }

    #[test]
    fn group_help_is_generated_and_nonempty() {
        use clap::ValueEnum;
        for g in Group::value_variants() {
            let pv = g.to_possible_value().expect("possible value");
            assert!(!pv.get_help().unwrap().to_string().is_empty(), "{} has empty help", g.name());
        }
        // Sanity check a couple of specific ones.
        let opacity_help = Group::Opacity.to_possible_value().unwrap().get_help().unwrap().to_string();
        assert_eq!(opacity_help, "opacity");
        let box_help = Group::Box.to_possible_value().unwrap().get_help().unwrap().to_string();
        assert!(box_help.starts_with("border-radius, padding"));
        assert_eq!(Group::All.to_possible_value().unwrap().get_help().unwrap().to_string(), "every group");
    }

    #[test]
    fn expand_all_dedups() {
        let g = Group::expand(&[Group::All, Group::Color]);
        assert_eq!(g.len(), CONCRETE_GROUPS.len());
    }

    #[test]
    fn opacity_alone_leaves_an_unknown_background_alone() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("opacity", Value::num(0.25))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "BackgroundTransparency").as_deref(), None);
        assert_eq!(prop(&t, "TextTransparency").as_deref(), Some("0.75"));
        assert_eq!(prop(&t, "ImageTransparency").as_deref(), Some("0.75"));
    }

    #[test]
    fn background_color_alpha_combines_with_opacity() {
        let mut diag = Diagnostics::default();
        let decls = vec![
            decl("background-color", Value::Color(Color::rgba(255.0, 0.0, 0.0, 0.5))),
            decl("opacity", Value::num(0.5)),
        ];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "BackgroundColor3").as_deref(), Some("Color3.fromRGB(255, 0, 0)"));
        assert_eq!(prop(&t, "BackgroundTransparency").as_deref(), Some("0.75"));
    }

    #[test]
    fn width_calc_sets_size_and_automatic_size() {
        let mut diag = Diagnostics::default();
        let calc = Value::Call { name: "calc".to_string(), args: vec![Value::str("50% + 10px")] };
        let decls = vec![decl("width", calc)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "Size").as_deref(), Some("UDim2.new(0.5, 10, 0, 0)"));
        assert_eq!(prop(&t, "AutomaticSize").as_deref(), Some("Enum.AutomaticSize.Y"));
    }

    #[test]
    fn border_radius_percent() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("border-radius", Value::num_unit(50.0, "%"))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UICorner", "CornerRadius").as_deref(), Some("UDim.new(0.5, 0)"));
    }

    #[test]
    fn padding_shorthand_two_values() {
        let mut diag = Diagnostics::default();
        let value = Value::list(vec![Value::num_unit(4.0, "px"), Value::num_unit(8.0, "px")], ListSep::Space);
        let decls = vec![decl("padding", value)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIPadding", "PaddingTop").as_deref(), Some("UDim.new(0, 4)"));
        assert_eq!(pseudo(&t, "UIPadding", "PaddingRight").as_deref(), Some("UDim.new(0, 8)"));
        assert_eq!(pseudo(&t, "UIPadding", "PaddingBottom").as_deref(), Some("UDim.new(0, 4)"));
        assert_eq!(pseudo(&t, "UIPadding", "PaddingLeft").as_deref(), Some("UDim.new(0, 8)"));
    }

    #[test]
    fn border_shorthand() {
        let mut diag = Diagnostics::default();
        let value = Value::list(
            vec![Value::num_unit(2.0, "px"), Value::str("solid"), Value::Color(Color::rgba(0.0, 0.0, 0.0, 1.0))],
            ListSep::Space,
        );
        let decls = vec![decl("border", value)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIStroke", "ApplyStrokeMode").as_deref(), Some("Enum.ApplyStrokeMode.Border"));
        assert_eq!(pseudo(&t, "UIStroke", "Thickness").as_deref(), Some("2"));
        assert_eq!(pseudo(&t, "UIStroke", "Color").as_deref(), Some("Color3.fromRGB(0, 0, 0)"));
    }

    #[test]
    fn transform_translate_and_rotate() {
        let mut diag = Diagnostics::default();
        let translate_call = Value::Call {
            name: "translate".to_string(),
            args: vec![Value::num_unit(-50.0, "%"), Value::num_unit(-50.0, "%")],
        };
        let rotate_call = Value::Call { name: "rotate".to_string(), args: vec![Value::num_unit(45.0, "deg")] };
        let value = Value::list(vec![translate_call, rotate_call], ListSep::Space);
        let decls = vec![decl("transform", value)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "AnchorPoint").as_deref(), Some("Vector2.new(0.5, 0.5)"));
        assert_eq!(prop(&t, "Rotation").as_deref(), Some("45"));
    }

    #[test]
    fn flex_column_gap_and_align_items() {
        let mut diag = Diagnostics::default();
        let decls = vec![
            decl("display", Value::str("flex")),
            decl("flex-direction", Value::str("column")),
            decl("gap", Value::num_unit(8.0, "px")),
            decl("align-items", Value::str("center")),
        ];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIListLayout", "FillDirection").as_deref(), Some("Enum.FillDirection.Vertical"));
        assert_eq!(pseudo(&t, "UIListLayout", "Padding").as_deref(), Some("UDim.new(0, 8)"));
        assert_eq!(
            pseudo(&t, "UIListLayout", "HorizontalAlignment").as_deref(),
            Some("Enum.HorizontalAlignment.Center")
        );
        assert_eq!(prop(&t, "Visible").as_deref(), Some("true"));
    }

    #[test]
    fn align_items_baseline_is_approximated_as_bottom() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("display", Value::str("flex")), decl("align-items", Value::str("baseline"))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIListLayout", "VerticalAlignment").as_deref(), Some("Enum.VerticalAlignment.Bottom"));
        assert!(diag.items.iter().any(|m| m.message.contains("approximated as `Bottom`")), "{:?}", diag.items);
    }

    #[test]
    fn fixed_grid_tracks_set_the_cell_count_and_size() {
        let mut diag = Diagnostics::default();
        let repeat =
            Value::Call { name: "repeat".to_string(), args: vec![Value::num(3.0), Value::num_unit(68.0, "px")] };
        let decls = vec![
            decl("display", Value::str("grid")),
            decl("grid-template-columns", repeat),
            decl("grid-auto-rows", Value::num_unit(68.0, "px")),
            decl("gap", Value::num_unit(6.0, "px")),
        ];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIGridLayout", "FillDirectionMaxCells").as_deref(), Some("3"));
        assert_eq!(pseudo(&t, "UIGridLayout", "CellSize").as_deref(), Some("UDim2.new(0, 68, 0, 68)"));
        assert_eq!(pseudo(&t, "UIGridLayout", "CellPadding").as_deref(), Some("UDim2.new(0, 6, 0, 6)"));
        assert!(diag.items.is_empty(), "{:?}", diag.items);
    }

    #[test]
    fn fractional_grid_tracks_split_the_container_minus_the_gaps() {
        let mut diag = Diagnostics::default();
        let repeat =
            Value::Call { name: "repeat".to_string(), args: vec![Value::num(4.0), Value::num_unit(1.0, "fr")] };
        let decls = vec![
            decl("display", Value::str("grid")),
            decl("grid-template-columns", repeat),
            decl("grid-auto-rows", Value::num_unit(40.0, "px")),
            decl("column-gap", Value::num_unit(8.0, "px")),
        ];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIGridLayout", "FillDirectionMaxCells").as_deref(), Some("4"));
        // Three 8px gaps sit between four cells, so each gives up 6px of its quarter.
        assert_eq!(pseudo(&t, "UIGridLayout", "CellSize").as_deref(), Some("UDim2.new(0.25, -6, 0, 40)"));
    }

    #[test]
    fn a_column_flow_grid_counts_its_rows() {
        let mut diag = Diagnostics::default();
        let repeat =
            Value::Call { name: "repeat".to_string(), args: vec![Value::num(2.0), Value::num_unit(30.0, "px")] };
        let decls = vec![
            decl("display", Value::str("grid")),
            decl("grid-auto-flow", Value::str("column")),
            decl("grid-template-rows", repeat),
            decl("grid-auto-columns", Value::num_unit(90.0, "px")),
        ];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIGridLayout", "FillDirection").as_deref(), Some("Enum.FillDirection.Vertical"));
        assert_eq!(pseudo(&t, "UIGridLayout", "FillDirectionMaxCells").as_deref(), Some("2"));
        assert_eq!(pseudo(&t, "UIGridLayout", "CellSize").as_deref(), Some("UDim2.new(0, 90, 0, 30)"));
    }

    #[test]
    fn uneven_grid_tracks_are_rejected() {
        let mut diag = Diagnostics::default();
        let tracks = Value::list(vec![Value::num_unit(1.0, "fr"), Value::num_unit(80.0, "px")], ListSep::Space);
        let decls = vec![decl("display", Value::str("grid")), decl("grid-template-columns", tracks)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIGridLayout", "CellSize").as_deref(), None);
        assert_eq!(pseudo(&t, "UIGridLayout", "FillDirectionMaxCells").as_deref(), None);
        assert!(diag.items.iter().any(|m| m.message.contains("same size")), "{:?}", diag.items);
    }

    #[test]
    fn grid_columns_without_rows_warn_about_the_cell_height() {
        let mut diag = Diagnostics::default();
        let repeat =
            Value::Call { name: "repeat".to_string(), args: vec![Value::num(2.0), Value::num_unit(50.0, "px")] };
        let decls = vec![decl("display", Value::str("grid")), decl("grid-template-columns", repeat)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIGridLayout", "CellSize").as_deref(), Some("UDim2.new(0, 50, 0, 100)"));
        assert!(diag.items.iter().any(|m| m.message.contains("grid-auto-rows")), "{:?}", diag.items);
    }

    #[test]
    fn linear_gradient_90deg() {
        let mut diag = Diagnostics::default();
        let gradient = Value::Call {
            name: "linear-gradient".to_string(),
            args: vec![
                Value::num_unit(90.0, "deg"),
                Value::Color(Color::rgba(255.0, 0.0, 0.0, 1.0)),
                Value::Color(Color::rgba(0.0, 0.0, 255.0, 1.0)),
            ],
        };
        let decls = vec![decl("background", gradient)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UIGradient", "Rotation").as_deref(), Some("0"));
        assert!(pseudo(&t, "UIGradient", "Color").as_deref().unwrap().starts_with("ColorSequence.new"));
        assert_eq!(prop(&t, "BackgroundColor3").as_deref(), Some("Color3.fromRGB(255, 255, 255)"));
    }

    #[test]
    fn transition_shorthand_two_targets() {
        let mut diag = Diagnostics::default();
        let value = Value::list(
            vec![Value::str("background-color"), Value::num_unit(200.0, "ms"), Value::str("ease-in-out")],
            ListSep::Space,
        );
        let decls = vec![decl("transition", value)];
        let t = translate(&decls, &all_opts(), &mut diag);
        let bg = t.transitions.iter().find(|(n, _)| n == "BackgroundColor3").map(|(_, v)| luau::render_tween(v));
        let bgt = t.transitions.iter().find(|(n, _)| n == "BackgroundTransparency").map(|(_, v)| luau::render_tween(v));
        assert_eq!(bg.as_deref(), Some("TweenInfo.new(0.2, Enum.EasingStyle.Quad, Enum.EasingDirection.InOut)"));
        assert_eq!(bgt.as_deref(), Some("TweenInfo.new(0.2, Enum.EasingStyle.Quad, Enum.EasingDirection.InOut)"));
    }

    #[test]
    fn disabled_group_emits_warning_and_no_output() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("opacity", Value::num(0.5))];
        let opts = opts_for(&[Group::Color]); // opacity group not enabled
        let t = translate(&decls, &opts, &mut diag);
        assert!(t.props.is_empty());
        assert_eq!(diag.warning_count(), 1);
        assert!(diag.items[0].message.contains("pass `--approx=opacity`"));
    }

    #[test]
    fn unknown_property_warns_with_hint() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("float", Value::str("left"))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert!(t.props.is_empty());
        assert_eq!(diag.warning_count(), 1);
        assert!(diag.items[0].message.contains("float"));
        assert!(diag.items[0].message.contains("use layout properties instead"));
    }

    #[test]
    fn roblox_transitions_lass_example() {
        let value =
            Value::list(vec![Value::str("BackgroundColor3"), Value::str("BackgroundTransparency")], ListSep::Comma);
        let entries = roblox_transitions(&value).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "BackgroundColor3");
        assert_eq!(
            luau::render_tween(&entries[0].1),
            "TweenInfo.new(1, Enum.EasingStyle.Quad, Enum.EasingDirection.Out)"
        );
        assert_eq!(entries[1].0, "BackgroundTransparency");
        assert_eq!(
            luau::render_tween(&entries[1].1),
            "TweenInfo.new(1, Enum.EasingStyle.Quad, Enum.EasingDirection.Out)"
        );
    }

    #[test]
    fn font_shorthand_produces_single_fontface() {
        let mut diag = Diagnostics::default();
        let value = Value::list(
            vec![Value::str("bold"), Value::num_unit(16.0, "px"), Value::quoted("GothamSSm")],
            ListSep::Space,
        );
        let decls = vec![decl("font", value)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "TextSize").as_deref(), Some("16"));
        assert_eq!(
            prop(&t, "FontFace").as_deref(),
            Some("Font.new(\"rbxasset://fonts/families/GothamSSm.json\", Enum.FontWeight.Bold, Enum.FontStyle.Normal)")
        );
        assert!(diag.items.is_empty());
    }

    #[test]
    fn visibility_and_overflow() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("visibility", Value::str("hidden")), decl("overflow", Value::str("scroll"))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "Visible").as_deref(), Some("false"));
        assert_eq!(prop(&t, "ClipsDescendants").as_deref(), Some("true"));
        assert_eq!(prop(&t, "ScrollingEnabled").as_deref(), Some("true"));
        assert_eq!(prop(&t, "ScrollingDirection").as_deref(), Some("Enum.ScrollingDirection.XY"));
        assert_eq!(prop(&t, "AutomaticCanvasSize").as_deref(), Some("Enum.AutomaticSize.XY"));
        assert_eq!(prop(&t, "CanvasSize").as_deref(), Some("UDim2.new(0, 0, 0, 0)"));
    }

    #[test]
    fn scrollbar_properties() {
        let mut diag = Diagnostics::default();
        let colors = Value::List {
            items: vec![Value::Color(Color::rgba(255.0, 0.0, 0.0, 0.5)), Value::Color(Color::rgba(0.0, 0.0, 0.0, 1.0))],
            sep: ListSep::Space,
            bracketed: false,
        };
        let decls = vec![
            decl("scrollbar-width", Value::str("thin")),
            decl("scrollbar-color", colors),
            decl("scrollbar-gutter", Value::str("stable")),
            decl("opacity", Value::num(0.5)),
        ];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "ScrollBarThickness").as_deref(), Some("8"));
        assert_eq!(prop(&t, "ScrollBarImageColor3").as_deref(), Some("Color3.fromRGB(255, 0, 0)"));
        assert_eq!(prop(&t, "ScrollBarImageTransparency").as_deref(), Some("0.75"));
        assert_eq!(prop(&t, "VerticalScrollBarInset").as_deref(), Some("Enum.ScrollBarInset.Always"));
        assert!(diag.items.is_empty(), "{:?}", diag.items.iter().map(|d| &d.message).collect::<Vec<_>>());

        let t = translate(&[decl("scrollbar-width", Value::str("none"))], &all_opts(), &mut diag);
        assert_eq!(prop(&t, "ScrollBarThickness").as_deref(), Some("0"));
        let t = translate(&[decl("scrollbar-width", Value::num_unit(4.0, "px"))], &all_opts(), &mut diag);
        assert_eq!(prop(&t, "ScrollBarThickness").as_deref(), Some("4"));
        let t = translate(&[decl("scrollbar-color", Value::str("auto"))], &all_opts(), &mut diag);
        assert_eq!(prop(&t, "ScrollBarImageColor3").as_deref(), Some("Color3.fromRGB(255, 255, 255)"));
        assert_eq!(prop(&t, "ScrollBarImageTransparency").as_deref(), Some("0"));
        assert!(diag.items.is_empty());
    }

    #[test]
    fn overflow_axes_cascade_in_order() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("overflow", Value::str("auto")), decl("overflow-x", Value::str("hidden"))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "ScrollingDirection").as_deref(), Some("Enum.ScrollingDirection.Y"));
        assert_eq!(prop(&t, "AutomaticCanvasSize").as_deref(), Some("Enum.AutomaticSize.Y"));

        let two = Value::List {
            items: vec![Value::str("scroll"), Value::str("hidden")],
            sep: ListSep::Space,
            bracketed: false,
        };
        let t = translate(&[decl("overflow", two)], &all_opts(), &mut diag);
        assert_eq!(prop(&t, "AutomaticCanvasSize").as_deref(), Some("Enum.AutomaticSize.X"));

        let t = translate(&[decl("overflow", Value::str("hidden"))], &all_opts(), &mut diag);
        assert_eq!(prop(&t, "ClipsDescendants").as_deref(), Some("true"));
        assert_eq!(prop(&t, "AutomaticCanvasSize").as_deref(), None);
        assert!(diag.items.is_empty());
    }

    #[test]
    fn min_max_size_constraint() {
        let mut diag = Diagnostics::default();
        let decls = vec![decl("min-width", Value::num_unit(10.0, "px")), decl("max-width", Value::str("none"))];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(pseudo(&t, "UISizeConstraint", "MinSize").as_deref(), Some("Vector2.new(10, 0)"));
        assert_eq!(pseudo(&t, "UISizeConstraint", "MaxSize").as_deref(), Some("Vector2.new(math.huge, math.huge)"));
    }

    #[test]
    fn token_reference_is_opaque() {
        // A StyleSheet attribute holds a Color3, which has no alpha channel, so a token-coloured
        // background is opaque — and says so, to undo a weaker rule's transparency.
        let mut diag = Diagnostics::default();
        let token = Value::Call { name: "var".to_string(), args: vec![Value::str("--Accent")] };
        let decls = vec![decl("background-color", token)];
        let t = translate(&decls, &all_opts(), &mut diag);
        assert_eq!(prop(&t, "BackgroundColor3").as_deref(), Some("\"$Accent\""));
        assert_eq!(prop(&t, "BackgroundTransparency").as_deref(), Some("0"));
    }
}
