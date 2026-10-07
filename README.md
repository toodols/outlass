# outlass

A better version of [lass](https://github.com/toodols/lass). It compiles SCSS into a Roblox `StyleSheet`, and
tries to be as faithful to CSS as possible, so an agent can write the CSS it already knows and get a mirror
result in Roblox in as few shots as possible. Reasoning is that agents are better at web design than roblox ui.

```sh
cargo install --path .
outlass ui.scss --approx
```

## Options

| Option | What it does |
| --- | --- |
| `-o, --output FILE\|-` | Output file, `-` for stdout. Default is the input path with `.luau` |
| `-d, --out-dir DIR` | Write outputs into DIR instead of next to each input |
| `--merge` | Compile all inputs into one StyleSheet (needs `-o`) |
| `-a, --approx[=GROUPS]` | Approximates CSS properties in Roblox ones. Default = all groups, or pick from `color,opacity,text,size,position,box,layout,visibility,transition`. Without it, CSS properties are ignored with a warning |
| `-I, --load-path DIR` | Extra directory for `@use`/`@forward`/`@import` |
| `-D, --define NAME=VALUE` | Set a global variable, overriding `!default` |
| `--emit luau\|json\|css` | `json` writes the compiled StyleSheet (rules and typed values) the Luau is generated from. `css` prints the evaluated SCSS before Roblox translation, for debugging |
| `--tags FILE` | JSON mapping each tag (or `#Name`) to the GuiObject classes it's used on, so rules drop and warn about properties those classes don't have |
| `--allow-raw-luau` | Let `luau("...")` insert raw Luau. Off by default, so the output can only build a StyleSheet |
| `-w, --watch` | Recompile when an input or anything it imports changes |
| `-q, --quiet` | Hide warnings and `@debug` |
| `--deny-warnings` | Warnings fail the build |
| `--strict` | Fail on CSS that compiles but lays out differently in Roblox than in a browser |

`outlass properties [NAME]` shows how each CSS property is translated. `outlass functions [FILTER]` lists
the built-in Sass functions.

## Changes of note

Roblox and CSS disagree in a lot of small ways. These are the fixes you wouldn't guess:

- **Rule order.** In CSS, `.button:hover` beats `.button` automatically. Roblox has no such rule; it
  only looks at a number called `Priority`. So outlass works out which rule CSS would pick and gives it
  the higher number.
- **Half a property.** In CSS you can set `top` in one rule and `left` in another. Roblox only has
  `Position`, which holds both. So if a rule only sets `left`, outlass copies `top` into it from the
  other rules that apply.
- **Starting styles.** A new Roblox Frame is 0x0 with a solid background. A new HTML div fits its
  content and has no background. So outlass makes every element start like a div. Text also starts at
  the top left instead of the middle, and buttons don't darken on hover unless you say so.
- **Font size.** Roblox's `TextSize` is bigger than CSS's `font-size` for the same text. So outlass
  scales it up. `font-size: 13px` in Roboto Mono becomes `TextSize = 17`.
- **Line height.** Roblox puts extra line spacing only between lines. CSS puts it above and below
  too. So outlass adds padding above and below the text to make up for it.
- **Borders.** A CSS border pushes the content inward. A Roblox `UIStroke` is drawn outside and
  doesn't. So outlass adds the border's width to the padding.
- `repeating-linear-gradient` is written out stop by stop (max 20 keypoints).
- **See-through backgrounds.** CSS has a color's alpha and `opacity` as separate things. Roblox has one
  `BackgroundTransparency`. So outlass multiplies them together and writes it every time, even when the
  result is fully solid, so a later rule can undo an earlier transparent one.
- **Variables.** `var(--x)` stays live in Roblox, so you can change it at runtime, but only for solid
  colors. Anything else (sizes, fonts, gradients) gets its value copied in when compiling.
- **Flex stretch.** Roblox stretches flex items to match the biggest item, and ignores their set
  size. CSS stretches to the container and respects a set size. So items with a set size get a
  constraint that stops them stretching.
- **Flex shrink.** In CSS, items in a row shrink to fit by default. In Roblox they overflow. So outlass
  gives every item in a row a `UIFlexItem` that lets it shrink.
- **`fit-content`.** Roblox can only grow an element past its `Size`, never shrink it. So
  `fit-content` sets `Size` to 0 and lets it grow to fit.

Some things can't be fixed by the stylesheet alone. For example, `width: 100%` inside a parent that
fits its content is 0 in Roblox but works in CSS. `--strict` turns these into errors and tells you what
to write instead.

## Behavior that could be annoying
**A fade-in from code that ends invisible.** Roblox ignores a property set from Luau when the value is
the default, and the stylesheet's value shows instead. Tweening `BackgroundTransparency` to `0` therefore
ends at the starting style's `1`. Tween to `0.001`, or use a selector with transitions. See
[A fade from code ends invisible](REFERENCE.md#a-fade-from-code-ends-invisible).

See [REFERENCE.md](REFERENCE.md) for the full details.
