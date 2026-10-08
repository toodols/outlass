# outlass reference

Compiles **SCSS** (plus indented **.sass**) into a Luau module that builds and returns a Roblox
[`StyleSheet`](https://create.roblox.com/docs/reference/engine/classes/StyleSheet). It's the successor to
[lass](https://github.com/toodols/lass): a real SCSS front end with modules, mixins, functions, and control
flow, plus optional translation of CSS properties Roblox doesn't have.

```sh
cargo install --path .
outlass ui.scss                       # writes ui.luau
outlass src/*.scss -d out --approx    # every file, translating CSS properties
outlass --help                        # full reference; `outlass properties`, `outlass functions`
```

## Example

```scss
@use "theme" with ($accent: #10b981);

:root { --Surface: #1f2937; }          // StyleSheet attribute (design token)

.card {
  BackgroundColor3: var(--Surface);    // Roblox property, references the token → "$Surface"
  border-radius: theme.$radius;        // CSS property → ::UICorner (with --approx)
  padding: 8px 16px;                   // → ::UIPadding
  opacity: 0.9;                        // → Text/Image/GroupTransparency = 0.1

  &:hover { BackgroundColor3: lighten(#1f2937, 10%); }  // outranks .card by the CSS cascade
  > .title { font: bold 20px "Montserrat"; }
}
```

The output is a plain Luau module:

```luau
local sheet = Instance.new("StyleSheet")
sheet.Name = "ui"
sheet:SetAttribute("Surface", Color3.fromRGB(31, 41, 55))

local function rule(parent, selector, priority, properties) ... end

rule(sheet, ".card", 1, {
	["BackgroundColor3"] = "$Surface",
	...
})
rule(sheet, ".card::UICorner", 1, { ["CornerRadius"] = UDim.new(0, 6) })
rule(sheet, ".card:Hover", 2, { ["BackgroundColor3"] = Color3.fromRGB(49, 65, 88) })
...
-- the user-agent defaults, in a StyleSheet the sheet derives from (sheet.UserAgent)
local userAgent = Instance.new("StyleSheet")
rule(userAgent, "TextLabel, TextButton, TextBox", 1, { ["RichText"] = true, ... })
...
return sheet
```

Put it in a ModuleScript and apply it with a `StyleLink` under your `ScreenGui`.
`examples/showcase.scss` is a larger tour. Run `outlass examples/showcase.scss --approx -o -` to see its output.

## How SCSS maps to Roblox

| SCSS | Roblox |
| --- | --- |
| `Frame`, `.Tag`, `#Name`, `::UICorner` | same selector syntax |
| `*` | `GuiObject`, which matches every element (a class selector matches by `IsA`); next to a type, tag or name it's dropped (`*.a` → `.a`) |
| `a b` (descendant) | `a >> b` |
| `:hover` / `:active` / `:disabled` | `:Hover` / `:Press` / `:NonInteractable` |
| `:is(...)`, `:where(...)` | expanded into a selector list (`.a:is(.b, .c)` → `.a.b, .a.c`); `:where` adds no specificity |
| `::placeholder { color }` | `PlaceholderColor3` on the TextBox |
| nesting, `&`, `&-suffix`, `@extend` | flattened into full selectors, like Sass does |
| `PascalCase: value` | Roblox property, set to the value below |
| `--Name: value` | `SetAttribute` on the rule (or on the sheet in `:root`/top level); raw text, so use `#{}` |
| `@font-face { font-family: Brand; src: url("rbxassetid://…") }` | `font-family: Brand` uses that asset |
| `var(--Name)`, `token(Name)` | token reference `"$Name"`; in a CSS property, a `:root` token that isn't a color is compiled in (see below) |
| the CSS cascade, `!important`, `@layer` | `StyleRule.Priority` (see [The cascade](#the-cascade)) |
| `Transition: BackgroundColor3 0.2s Quad Out` | `SetPropertyTransitions` (`*`/`all` → `SetDefaultPropertyTransition`) |
| `@media`, `@container`, `@PreferredInputTouch { ... }` | `@Name` query selectors (see [Queries](#queries)) |

**Values**: `10px` → `10`, `50%` → `0.5`, `500ms` → `0.5`, `90deg`/`0.25turn` → degrees, `1rem` → `16`,
colors → `Color3.fromRGB`, `"text"` → string, `true`/`false`, `math.huge`, and
`Enum.Font.Gotham`-style Enum items. These Roblox constructors are understood, with their arguments converted
the same way: `UDim.new`, `UDim2.new`/`fromScale`/`fromOffset`, `Vector2.new`, `Rect.new`, `NumberRange.new`,
`Color3.new`/`fromRGB`/`fromHex`/`fromHSV`, `ColorSequence.new`, `NumberSequence.new` (keypoint lists in
`[...]`), and `Font.new`/`fromName`/`fromId`/`fromEnum` (also `Font.new "rbxasset://..."`).

Anything else (another call, or a bare word that isn't an Enum item) is ignored with a warning, so a
stylesheet can't put code into the generated module: it can only build a StyleSheet. `luau("...")` inserts
raw Luau, but only with `--allow-raw-luau`; use it only for stylesheets you trust. JSON (`--emit json`) has
no way to hold raw Luau, so a stylesheet that uses `luau()` can't be written as JSON (or as a model file).

`--emit json` writes the compiled StyleSheet as JSON instead: the rules, their attributes and transitions,
and every value typed (`{"type": "UDim2", "x": {"scale": 0, "offset": 4}, ...}`, `{"type": "Enum", "enum":
"Font", "item": "Gotham"}`, `{"type": "token", "name": "Accent"}`; infinity is `"inf"`). The Luau is
generated from exactly this, by one module (`src/luau.rs`) that escapes every string and checks every name.

`--emit rbxmx` writes the same StyleSheet as a Roblox model file instead, which Studio and Rojo load
directly: instances whose properties are data, with no code to run, so it can't hold `luau("...")`.
Its Enum items are saved as numbers, which outlass knows for the enums GUI objects use (`src/enums.txt`,
from Roblox's API dump). Roblox doesn't save a default transition (`transition: all`) in a model, so
that one warns and is left out. Rojo builds and live-syncs StyleRule properties, but its live sync
doesn't update transitions.

## Themes

`[data-theme="dark"]` (also `:root[data-theme=dark]` or `html[data-theme=dark]`) and
`@media (prefers-color-scheme: dark) { :root { ... } }` set a theme's tokens:

```scss
:root { --bg: #ffffff; --radius: 8px; }
[data-theme="dark"] { --bg: #111827; --radius: 4px; }
```

Each theme becomes a StyleSheet in a `Themes` folder under the sheet, holding every themed token (a
theme starts from the `:root` values and overrides some), and the sheet derives from one of them
through a StyleDerive named `Theme`. The `default` theme is in use at first; switch with
`sheet.Theme.StyleSheet = sheet.Themes.dark`. A theme only sets custom properties, and only `:root`
can depend on one.

A theme can only change what Roblox looks up at run time (see below): a color, or a length that
`border-radius`, `padding` or `gap` uses whole. A themed token compiled in anywhere else warns that it
keeps the default theme's value.

## The cascade

Roblox resolves conflicting StyleRules only by `Priority`, so outlass works out which rule CSS would apply and
encodes that as the priority. Every rule is ranked, and its rank (1, 2, 3, ...) becomes its `StyleRule.Priority`:

1. `!important` declarations beat normal ones. A rule that has both is split into a normal StyleRule and
   an important one.
2. `@layer` works as in CSS. Layers rank in the order they're first named (`@layer reset, base, theme;`
   fixes it up front), a later layer wins regardless of specificity, and styles outside any layer beat
   every layer. Layers nest (`@layer theme { @layer dark { ... } }`, or `theme.dark`), and a layer's own
   rules beat its sublayers. For `!important` declarations the layer order is reversed, as in CSS.
3. The more specific selector wins: ids, then classes/states/attributes, then types.
4. The later rule wins.

So `.button:hover` beats `.button`, which beats `TextButton`, without any manual priorities. The numbers
themselves are internal: adding a rule renumbers the rest, so rely on the order, not on a value.

Like a browser, outlass also has a tiny user-agent stylesheet that sits below every author rule. For
`TextLabel`, `TextButton` and `TextBox` it turns `RichText` on, since CSS text is always markup. With
`--approx=size` it also sets `AutomaticSize` to `XY` on every GuiObject class, because a fresh Roblox
element is 0×0 where a CSS box with no size given takes one from its content. Any rule that gives an
element a `width` and a `height` sets `AutomaticSize` back to `None`, so authored sizes still win. With
`--approx=color`, every GuiObject class starts transparent with no border (`BackgroundTransparency = 1`,
`BorderSizePixel = 0`), as a CSS box does; a rule with a background makes it opaque again. Buttons also
get `AutoButtonColor = false`, since a browser button only changes on hover or press when a `:hover` or
`:active` rule says so. With
`--approx=text`, `TextLabel` and `TextBox` text starts at the top left (`text-align: start`), where Roblox
centers it; a `TextButton` stays centered, as a browser centers a button's content. To opt out by hand,
set the property on any rule, e.g. `RichText: false` or `AutomaticSize: Enum.AutomaticSize.None`.

A `var()` in a CSS property is a `"$Name"` reference that Roblox resolves at run time, so a theme can
change it, wherever the token is the whole value of a Roblox property: an opaque color, or a length in
`border-radius`, `padding` or `gap` (`CornerRadius = "$radius"`). Such a length token is a `UDim`
attribute, because Roblox reads a number attribute in a UDim property once and never sees it change.
Everything else has to be known when outlass compiles it: a font family into a `FontFace`, a length
into a `Size`, padding plus a border's width, a gradient into a `UIGradient`, a translucent color (a
Color3 attribute has no alpha). Those get the token's value compiled in.

Tokens cascade as in CSS: a rule uses the value its own elements get, from `:root`, a weaker rule for
the same elements, or the rule itself. A `"$Name"` reference only sees the sheet's tokens and its own
rule's, so when a rule redefines a token (`.a.b { --gap: 10px }`), the declarations that use it
(`.a { padding: var(--gap) }`) are repeated in that rule, with the token set on each of its rules. The
attribute is always set, as text when it has no attribute type (a gradient).

### A fade from code ends invisible

A fade-in tweened from Luau, such as `TweenService:Create(frame, info, { BackgroundTransparency = 0 })`,
plays and then the background disappears as the tween ends. In the Properties window
`BackgroundTransparency` still reads `0`.

This happens because Roblox treats a property set to its class default as not set at all, so whatever
the stylesheet says wins. `0` is the default for `BackgroundTransparency`, and with `--approx=color` the
user-agent rule says `1`. Every other value you set from code wins over the stylesheet as usual. outlass
doesn't change anything here: it emits one fixed `BackgroundTransparency = 1` rule, and Roblox swaps in
that value at run time when your code sets the default. To see what's actually drawn, use
`frame:GetStyled("BackgroundTransparency")`, not the property.

```lua
-- broken: ends at 0, which Roblox reads as unset, so the user-agent's 1 shows
TweenService:Create(frame, info, { BackgroundTransparency = 0 }):Play()

-- works: any value other than the default counts as set
TweenService:Create(frame, info, { BackgroundTransparency = 0.001 }):Play()
```

The cleaner fix is to put the visible state in the stylesheet and switch to it with a tag, so the
transition comes from the stylesheet. A rule with a background always writes `BackgroundTransparency = 0`
explicitly (see the transparency cascade), so this fade ends where it should:

```scss
.toast { background-color: #222; opacity: 0; transition: opacity 0.3s; }
.toast.shown { opacity: 1; }
```

```lua
frame:AddTag("shown")
```

The same applies to any property a rule sets, not only transparency:

- **User-agent defaults.** Setting `BorderSizePixel = 1`, `AutomaticSize = None`, `RichText = false`,
  `TextWrapped = false`, centered `TextXAlignment`/`TextYAlignment` or `AutoButtonColor = true` from code
  is ignored too, because each is Roblox's default. Set these on a rule instead.
- **Your own rules.** If `.badge { opacity: 0.5 }` gives a label `TextTransparency = 0.5`, a tween of
  `TextTransparency` to `0` stops at `0.5`. `ImageTransparency`, `ScrollBarImageTransparency` and a
  `::UIStroke`'s `Transparency` (all defaulting to `0`) behave the same way whenever a rule sets them.
  The user-agent stylesheet sets none of these, so a fade to `0` from code only breaks when one of
  your rules sets that property on the element.

### Sizing an element from its parent

Roblox resolves a percentage size against the parent's own size, so `width: 100%` only works under a
parent whose width is definite — one with a `width` of its own, or that is itself filling a definite
parent. Under a parent that sizes itself from its content, a percentage resolves to nothing and the
element collapses to zero, where CSS would treat the percentage as `auto`. Three consequences worth
knowing:

- Give every element on a fill chain a `width`: a 320px panel, a `width: 100%` row, a `width: 100%` cell.
- An element with no size hugs its content, so a row of them is only as wide as the text inside.
- `AutomaticSize` is a floor, not a replacement: an element keeps the size its rules give it and only
  grows when its content is bigger. Padding counts toward that content.

Flex items stretch across the line by default, but that's not a substitute either. Roblox stretches items
to the widest item on the line rather than to the container, so it evens up siblings but never fills a
wider parent. `flex-grow` does fill, and the width it produces is definite, so percentages under it
resolve normally.

A grid wraps at whatever width its parent offers, so a container narrower than
`columns x cell + gaps` quietly wraps a column early.

A container holding `position: absolute` children is the one place to turn the default off, because
Roblox grows a container around absolutely positioned children where CSS does not. Give those an explicit
`width` and `height` (`width: 100%; height: 100%` for a full-screen layer).

## Queries

`@media` and `@container` compile to Roblox
[style queries](https://create.roblox.com/docs/ui/styling/css-comparisons#queries). Every rule inside gets the
query's `@Name` as a selector prefix (`@ViewportDisplaySizeSmall .title`), and still takes part in the cascade
like any other rule.

- **Built-in queries.** Media features that Roblox documents an equivalent for use it directly, with no
  StyleQuery needed:

  | CSS | Roblox |
  | --- | --- |
  | `(max-width: 600px)`, `(min-width: 601px) and (max-width: 1200px)`, `(min-width: 1201px)` | `@ViewportDisplaySizeSmall` / `Medium` / `Large` |
  | `(pointer: fine)`, `(pointer: coarse)`, `(any-pointer: coarse)` | `@PreferredInputKeyboardAndMouse` / `Touch` / `Gamepad` |
  | `(prefers-reduced-motion: reduce)` / `no-preference` | `@ReducedMotionEnabledTrue` / `False` |

- **Other `@media` queries** (any `width`/`height`/`aspect-ratio`/`orientation`, range syntax like
  `(width >= 900px)`, combinations with `and`) become a custom query: `ScreenGui::StyleQuery #MediaMinWidth900
  { MinSize = Vector2.new(900, 0) }`, measured against the ScreenGui, which is the viewport.
- **`@container [name] (...)`** puts a `::StyleQuery` on every element that declares itself a container
  with `container-type`, `container-name` or `container`. Its `MinSize`/`MaxSize`/`AspectRatioRange` are checked
  against that element's `AbsoluteSize`. A named query only uses containers with that name.
- **Nesting and lists.** Nested queries of the same kind combine into one StyleQuery, since all of a query's
  conditions must hold. A comma-separated list emits the rules once per alternative. `print` never applies,
  and `not` isn't supported.
- **`@Name { ... }`** with a PascalCase name uses that query as-is, for built-ins or a StyleQuery you create
  yourself.

## CSS approximations (`--approx`)

Lowercase CSS properties have no Roblox counterpart. With `--approx` (every group) or
`--approx=opacity,box,...`, they are translated into the nearest Roblox equivalents. Without it, they are
ignored with a warning that names the flag to enable. Explicit Roblox properties in the same rule win.

| Group | Properties | Becomes |
| --- | --- | --- |
| `color` | `color`, `background[-color]`, `background-image`, `background-clip`, `background-size`, `background-repeat`, `background-blend-mode`, `border-image*`, `object-fit`, `image-rendering`, `mask-image`, `scrollbar-color` | `TextColor3`, `BackgroundColor3` + transparency from alpha, `::UIGradient` for `linear-gradient()` and `repeating-linear-gradient()` (Color from the background, Transparency from a mask), gradient text via `background-clip: text`, `Image` for `url()`, `ScaleType` (`cover` → Crop, `contain` → Fit, a `background-size` → Tile + `TileSize`), `ResampleMode` (`pixelated`), `ScrollBarImageColor3` (the thumb; Roblox draws no track) |
| `opacity` | `opacity` | `1 - x` into `GroupTransparency` (a CanvasGroup fades with its children, like CSS), `TextTransparency`, `ImageTransparency`, and `BackgroundTransparency` when a background is known (combined with color alpha) |
| `text` | `font*`, `text-align`, `vertical-align`, `align-content`, `line-height`, `white-space`, `text-overflow`, `text-shadow`, `-webkit-text-stroke[-width|-color]`, `content` | `FontFace` (the first family in the list that Roblox has; unknown names warn, `rbxassetid://` uploads pass through), `TextSize`, `TextX/YAlignment`, `LineHeight` (+ half-leading `::UIPadding`), `TextWrapped`, `TextTruncate`, `::UIStroke`, `Text` |
| `size` | `width`, `height`, `min-*`, `max-*`, `aspect-ratio`, `box-sizing` | `Size`/`AutomaticSize`, `::UISizeConstraint`, `::UIAspectRatioConstraint` |
| `position` | `left/top/right/bottom/inset`, `margin*`, `transform`, `translate`, `rotate`, `scale`, `z-index` | `Position`, `AnchorPoint`, `Rotation`, `::UIScale`, `ZIndex` |
| `box` | `border-radius`, `padding*`, `border*`, `outline*` | `::UICorner`, `::UIPadding`, `::UIStroke` |
| `layout` | `display`, `flex-*`, `justify-content`, `align-*`, `gap`, `grid-template-*`, `grid-auto-*`, `order` | `::UIListLayout`, `::UIGridLayout` (`FillDirectionMaxCells`, `CellSize`, `CellPadding`), `::UIFlexItem`, `LayoutOrder`, `Visible` |
| `visibility` | `visibility`, `overflow*`, `scrollbar-width`, `scrollbar-gutter`, `pointer-events`, `appearance` | `Visible`, `ClipsDescendants`, `ScrollingEnabled`/`ScrollingDirection`, `AutomaticCanvasSize` + `CanvasSize` (a scrolling axis scrolls exactly its content, like CSS), `ScrollBarThickness`, `VerticalScrollBarInset`, `Interactable`, `AutoButtonColor` |
| `transition` | `transition*` | property transitions (`TweenInfo`), including on pseudo-instances (`transform` animates `::UIScale`, `border-color` the `::UIStroke`, ...). `cubic-bezier()` maps to the nearest Roblox easing (`cubic-bezier(0.34, 1.56, 0.64, 1)` → `Back Out`) |

It follows CSS semantics where Roblox allows:
- `position: absolute` with `left` and `right` (or `inset: 0`) and no width stretches the element.
- `translate(-50%, -50%)` centers it.
- `transform: scale()` grows an element from its `AnchorPoint`, which is the top left unless something
  positions it from another edge; CSS grows it from the center. `rotate()` turns it around its center
  in both. Roblox has no `transform-origin`.
- `margin` offsets an element from the edge it's placed by, and insets one stretched between two
  edges, so `inset: 0; margin: 8px` leaves 8px all around. `margin: 0 auto` centers an element with a
  width, and `margin-left: auto` pushes it right. Margins never move siblings, and a flex or grid
  container ignores them, so outside absolute positioning outlass warns: use `gap`, or padding on the
  container.
- Flex items stretch across the line by default, as in CSS. Roblox's `Stretch` also stretches items
  that have an explicit size, which CSS doesn't, so an element with a px `width` or `height` gets a
  `::UISizeConstraint` at that size to keep it out of the stretch (not when it has `flex-grow`, which the
  cap would stop, or scrolls along that axis: a cap on a ScrollingFrame caps its canvas too, so it
  would never scroll). A size set from Luau is not protected this way.
- Items in a flex row shrink to fit it, as CSS's initial `flex-shrink: 1` makes them: the row gives
  every child a `::UIFlexItem` in Shrink mode (`.row > GuiObject::UIFlexItem`), below every author rule,
  so an item's own `flex`, `flex-grow` or `flex-shrink` still wins. Roblox splits the overflow by size
  as CSS does. Columns and rows that scroll along the line are left out: Roblox would shrink an item
  sized by its content below its content, where CSS stops, so they'd crush their items.
- Text is the size a browser draws it. CSS's `font-size` is the em, but Roblox's `TextSize` is the
  height of a whole line (ascent plus descent), so outlass multiplies by the family's line-to-em ratio,
  read from the font files Studio ships: `font-size: 13px` in Roboto Mono is `TextSize = 17`. That
  also makes `line-height: normal` match, since browsers use the same metrics. It needs a built-in
  `font-family` in the cascade; an uploaded or unknown font keeps `TextSize = font-size`. Roblox floors
  `TextSize` to whole pixels and caps it at 100, and glyph advances are rounded to whole pixels at small
  sizes, so widths can still differ by a pixel or two.
- `line-height` spaces lines as CSS does. Roblox makes the first line exactly `TextSize` tall and applies
  `LineHeight` only between lines, so outlass pads the element by half the leading,
  `(line-height x font-size - TextSize) / 2`, above and below (in whole pixels). A 20px font at
  `line-height: 1.5` is 30px tall per line in both. Text inside the element inherits the `LineHeight`
  but not the padding, so set `line-height` on the text element itself for exact spacing. Roblox caps
  `LineHeight` at 3.
- `fit-content` sizes an element from its content even when a weaker rule gives it a size: the Size is
  zeroed, since `AutomaticSize` only grows an element past its Size.
- `align-content: center` centers a block's text vertically, as it does in CSS. `vertical-align` does
  the same in outlass but nothing to a block in a browser.
- Padding is always inside the size (`box-sizing: border-box`), since that's how `UIPadding` works.
- A border takes room inside the box, as in CSS: its width is added to the padding, since a
  `UIStroke` is drawn outside the element and takes none. An outline takes no room in either.
- `border: none` also clears Roblox's legacy `BorderSizePixel`.
- A `background-image` gradient is painted **over** `background-color`, as in CSS, so an opaque
  gradient hides the color entirely. A UIGradient can't do that — it multiplies `BackgroundColor3`
  and multiplies its `Transparency` into `BackgroundTransparency` — so outlass flattens the color
  into the gradient's own stops and paints the element white, which turns the multiply into a no-op.
  Translucent stops blend with the color instead of replacing it, exactly as a browser composites
  them. A `var()` color can't be flattened, so a translucent gradient over one warns.
- `repeating-linear-gradient()` has no Roblox counterpart, so its pattern is written out stop by stop
  (`#222 0 10%, #444 10% 20%` becomes five copies, each ending in a hard stop). Stop positions have to
  be percentages, since a StyleRule doesn't know the element's size, and a Roblox sequence holds 20
  keypoints, which is what caps how often a pattern can repeat. `radial-gradient()` and
  `conic-gradient()` stay unsupported: a UIGradient is linear.
- `grid-template-columns: repeat(3, 68px)` wraps after three cells, and `repeat(3, 1fr)` splits the
  container three ways. A Roblox grid has one cell size for every cell, so the tracks have to match each
  other, and the cell needs a height as well: `grid-auto-rows`.

Roblox folds `left`/`top` into one `Position` and `width`/`height` into one `Size`, but CSS cascades each
separately. When a rule sets only part of such a group, outlass fills in the rest from the weaker rules that
match the same elements. So in `.bar { top: 36px; &.left { left: 34% } }`, `.bar.left` still gets
`top: 36px`. The same applies to fonts, flex settings, colors and opacity, and transitions.

Transparency works the same way, with one consequence worth knowing. Roblox has a single
`BackgroundTransparency` where CSS has a color alpha and an `opacity`, so any rule that sets either
input emits the transparency they add up to — including `BackgroundTransparency = 0` for an opaque
color. That's what lets `.panel.solid { background-color: #123 }` undo a weaker
`.panel { background-color: transparent }` instead of staying invisible. The same holds for `color` and
`TextTransparency`, and for a border's `::UIStroke`.

Text properties are inherited, as in CSS: `color`, `font*`, `line-height`, `text-align` and
`white-space` on `.card` also style every `TextLabel`, `TextButton` and `TextBox` inside it (a
`.card >> TextLabel` rule). Inherited values lose to any rule that styles the text element itself.
CSS takes the nearest ancestor's value, which a selector can't express, so between two ancestors' rules
the stronger one wins. Set the interface's type on its root, as a page does on its body
(`ScreenGui { font: 14px "Source Sans Pro" }`): a rule that sets `font-size` or `font-weight` without a
family is then sized and weighted in that family.

`min()`, `max()` and `clamp()` work on sizes when at most one bound is relative: `width: min(100%, 300px)`
is a 100% width with a 300px `::UISizeConstraint`. `font-size: clamp(12px, 2vw, 20px)` becomes
`TextScaled` text between 12 and 20 (a `::UITextSizeConstraint`), since Roblox has no viewport units.
`currentColor` is the rule's `color`.

Pictures: `background-image: url(...)` is the `Image` (and `none` clears it), `object-fit` and
`background-size` its `ScaleType`, and `background-blend-mode: multiply` over a `background-color`
tints it (`ImageColor3`). `border-image: url("rbxassetid://123#96x96") 32 fill` is a 9-slice
(`ScaleType.Slice` with its `SliceCenter`); Roblox measures the slices in the picture's pixels, so the
url ends with the picture's size, which a browser ignores and outlass leaves out of the id. Roblox
always draws the middle, so write `fill`. `border-image-width` scales the slices (`SliceScale`).
The same `#WxH` ending sizes a background picture: CSS tiles it at its own size unless
`background-size` says otherwise, and an `auto` side keeps its proportions (`32px auto` of a 64x32
picture is a 32x16 tile). Without it, or for a percentage next to `auto`, the picture is stretched
to the element, with a warning. Roblox has one `Image`, so a border image hides a background picture.
`contain: size` stops an element sizing itself by its content, for one whose size code sets.

`opacity` scales those transparencies rather than replacing them: `.a:hover { opacity: 0.5 }` fades
the background and border `.a` gives it, and leaves a transparent background transparent. A rule that
sets `opacity` without any known background leaves `BackgroundTransparency` alone.

### Element classes (`--tags`)

A selector like `.avatar` doesn't say what kind of element it styles, so outlass emits every property a
declaration produces: `opacity` sets `GroupTransparency`, `TextTransparency` and `ImageTransparency`
at once, and `background-image: url(...)` sets `Image` even on a tag only ever put on Frames. `--tags`
names the classes each CollectionService tag (or `#Name`) is used on:

```json
{ "avatar": ["ImageLabel", "ImageButton"], "card": "Frame", "#Search": ["TextBox"] }
```

A rule then keeps only the properties its elements' classes have, and warns about a Roblox property, or a
CSS declaration, that would do nothing (`` `background-image` sets Image, which Frame doesn't have ``).
Inherited text properties are exempt: `.card { color: white }` still colors the text inside the card.
A type selector narrows the classes the same way, with or without the file (`Frame.card`), and an
element with several tags can only be the classes they share.

Rojo builds a `.json` file into a ModuleScript that returns its contents, so the game can load the same
table and complain about any tagged element whose class the stylesheet doesn't expect:

```lua
local CollectionService = game:GetService("CollectionService")
local ReplicatedStorage = game:GetService("ReplicatedStorage")

-- tags.json, built by Rojo into a ModuleScript
local tags = require(ReplicatedStorage.Styles.tags)

local function check(instance: Instance, tag: string, classes: { string })
	if not table.find(classes, instance.ClassName) then
		warn(
			`{instance:GetFullName()} is a {instance.ClassName}, but the stylesheet expects "{tag}" on `
				.. table.concat(classes, " or ")
		)
	end
end

for tag, classes in tags do
	-- `#Name` keys select by name, not by a CollectionService tag.
	if string.sub(tag, 1, 1) == "#" then
		continue
	end
	if type(classes) == "string" then
		classes = { classes }
	end
	for _, instance in CollectionService:GetTagged(tag) do
		check(instance, tag, classes)
	end
	CollectionService:GetInstanceAddedSignal(tag):Connect(function(instance)
		check(instance, tag, classes)
	end)
end
```

### `--strict`

Some CSS compiles fine but lays out differently in Roblox than in a browser, because Roblox has no
equivalent behavior. `--strict` makes each case that can be seen from the stylesheet an error: every one
is reported, then the build fails and nothing is written. Each error names the CSS that makes the two
agree:

- `width: auto` fills the parent in CSS but fits the content in Roblox: write `fit-content`, a length or
  a percentage.
- Text with a `font-size` but no built-in `font-family` can't be sized like CSS: set a built-in family.
- Padding with a `width` or `height` under CSS's default `content-box`, where Roblox puts the padding
  inside the size: set `box-sizing: border-box`.
- `vertical-align` doesn't align a block's content in CSS: use `align-content`.
- A flex container that stretches its items (the CSS default) on an axis it doesn't size by its content:
  Roblox stretches items to the largest item on the line, CSS to the container. A column that isn't
  `width: fit-content` and a row with a `height` both count. Set `align-items`, and give the items that
  should fill the container `100%`.
- `position: relative` on a flex or grid container, which in CSS anchors absolutely positioned children:
  Roblox's layouts place every child, so those would join the flow. Move them into a wrapper without a
  layout, and drop the `position: relative`, which does nothing in Roblox.
- A percentage size whose selector names a parent sized by its content (`.card > .bar { width: 100% }`
  when `.card` has no `width`): Roblox resolves it to 0, or inflates the parent. Give the parent a size.
- A `max-width`/`max-height` on a scroll container along an axis that scrolls: the cap caps the canvas
  too, so nothing scrolls. Put the cap on a wrapper and give the scroll container `100%`.
- A `max-width`/`max-height` on an axis sized by its content: `AutomaticSize` grows the element past it,
  while the layout places the next sibling at the cap. Text is the exception (it wraps at the cap), so
  rules that style text aren't reported. Give the element a fixed size.
- An element with no `width` whose selector names a block parent (`.card > .caption` when `.card` isn't
  a flex or grid container): it fills the parent in CSS and fits its content in Roblox. Give it a width.
- A grid without a row (or column) size: Roblox cells are a fixed size, 100px unless given one.
- `fr` tracks in a grid sized by its content: CSS sizes the tracks from their contents, but Roblox cells
  are a fraction of the grid's size. Give the grid a `width` (or `height` for `fr` rows).

Regular warnings stay warnings; add `--deny-warnings` to fail on those too. Some differences depend on the
element tree, which a stylesheet doesn't fully know: a percentage size under a parent that no selector
names, for instance. Those can only be found by rendering.

`outlass properties` prints the exact mapping and caveats for every property. It's generated from the same
table the compiler uses. A few approximations are unavoidably lossy. For example, if nothing tells outlass
the other axis, a rule that only sets `height` makes the width automatic, and outlass warns about this.

## Supported Sass

Variables (`!default`, `!global`, scoping), nesting, `&`, nested properties, `#{}` interpolation,
`@mixin`/`@include` (defaults, named and rest arguments, `@content`, `using`), `@function`/`@return`,
`@if`/`@else`, `@each` (with destructuring), `@for`, `@while`, `@extend` and `%placeholders`,
`@use`/`@forward` (namespaces, `as`, `with`, `sass:` modules), `@import`, `@at-root`, `@debug`/`@warn`/`@error`,
maps, lists, unit arithmetic, `calc()`/`clamp()`/`min()`/`max()` folding, and 80+ built-in
functions across the `math`, `color`, `string`, `list`, `map` and `meta` modules, plus their legacy global
names (`outlass functions`).

Known differences from dart-sass: `/` always divides (there's no slash-separated shorthand).
`@extend` handles single simple selectors (`.a`, `%p`), not complex selector weaving. `@forward`'s
`show`/`hide` are accepted but not enforced. Plain CSS at-rules (`@font-face`, `@keyframes`, unsupported
`@media` queries) have no Roblox meaning and are dropped with a warning.

Where dart-sass draws a line, so does outlass:
- **`@extend` scope.** An `@extend` reaches the stylesheet it's written in and every stylesheet that
  one loads with `@use`/`@forward`, transitively — never a stylesheet that loads *it*. A target that
  nothing reachable defines is an error, not a warning; `!optional` allows it. `@import`ed files have
  no module of their own, so their rules and their `@extend`s belong to the importer.
- **Custom properties hold raw text.** `--Name: value` is never evaluated as SassScript, exactly as in
  CSS: `--Gap: $gap` is the four characters `$gap`, and `--Gap: #{$gap}` is what substitutes the
  variable. outlass then reads that text back as a CSS value, which is what turns `--Surface: #1f2937`
  into a `Color3` and `--Elevation: 2` into a number. Text it can't read back stays a string.

`darken()` and `lighten()` (and the other legacy global color functions) still work, but Dart Sass
deprecates them for removal in 3.0; `color.scale()` / `color.adjust()` are the forward-compatible
spellings.

## CLI

```
outlass [OPTIONS] <INPUT>...        compile (globs ok; `-` = SCSS on stdin; `_partials` skipped by globs)
outlass properties [PROPERTY] [-g GROUP]
outlass functions [FILTER]
```

Common options: `-o FILE|-`, `-d DIR`, `--merge`, `--approx[=GROUPS]`, `-I DIR`, `-D name=value`,
`--emit luau|json|rbxmx|css`, `--tags FILE`, `--allow-raw-luau`, `--no-user-agent-styles`, `--watch`, `-q`, `--strict`, `--deny-warnings`. See `outlass --help` for everything, with examples.
