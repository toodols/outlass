//! Indented syntax (.sass / .lass) → SCSS conversion.
//!
//! Converts whitespace-indented stylesheets into equivalent SCSS text that can then be fed
//! to the regular SCSS parser. The output always has exactly the same number of lines as the
//! input, and every construct stays on the line it came from, so that SCSS parse errors point
//! at the correct original line.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Sass,
    Lass,
}

/// One "logical" grouping of physical lines.
enum Group {
    /// Blank line, `//` comment-only line, or a `/* ... */` block comment spanning
    /// `start..=end`. Not part of the block/statement structure.
    Skip,
    /// A statement/selector/etc, possibly spanning multiple physical lines (unbalanced
    /// brackets, or a selector list with trailing commas). `lines` holds the physical line
    /// indices (0-based) that belong to it, in order.
    Content(Vec<usize>),
}

/// Converts indented source to SCSS.
pub fn to_scss(source: &str, flavor: Flavor) -> Result<String, (u32, String)> {
    let source = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    if source.trim().is_empty() {
        return Ok(source.to_string());
    }

    let had_trailing_newline = source.ends_with('\n');
    let lines: Vec<&str> = source.lines().collect();
    let n = lines.len();

    let is_blank = |s: &str| s.trim().is_empty();

    // Pass 1: indentation (mixed tab/space validation + counts).
    let mut indent_style: Option<char> = None;
    let mut indents: Vec<Option<usize>> = vec![None; n];
    for (i, line) in lines.iter().enumerate() {
        if is_blank(line) {
            continue;
        }
        indents[i] = Some(scan_indent(line, &mut indent_style, i as u32 + 1)?);
    }

    // Pass 2: group physical lines into Skip / Content groups.
    let mut groups: Vec<Group> = Vec::new();
    let mut block_comments: Vec<(usize, usize, bool)> = Vec::new();
    let mut i = 0usize;
    while i < n {
        if is_blank(lines[i]) {
            groups.push(Group::Skip);
            i += 1;
            continue;
        }
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("//") {
            groups.push(Group::Skip);
            i += 1;
            continue;
        }
        if trimmed.starts_with("/*") {
            let start = i;
            let start_indent = indents[i].unwrap();
            let mut last_qual = i;
            let mut k = i + 1;
            while k < n {
                if is_blank(lines[k]) {
                    k += 1;
                    continue;
                }
                let ind = indents[k].unwrap();
                if ind > start_indent {
                    last_qual = k;
                    k += 1;
                } else {
                    break;
                }
            }
            let end = last_qual;
            let has_close = (start..=end).any(|idx| lines[idx].contains("*/"));
            groups.push(Group::Skip);
            block_comments.push((start, end, has_close));
            i = end + 1;
            continue;
        }

        // Content group: merge continuation lines (unbalanced brackets / trailing commas).
        let (_, delta0, comma0) = scan_line(lines[i]);
        let mut depth = delta0;
        let mut trailing_comma = depth == 0 && comma0;
        let mut cur = i;
        let mut group_lines = vec![i];
        loop {
            if depth > 0 {
                cur += 1;
                if cur >= n {
                    return Err((n as u32, "unbalanced ( or [ in indented source".to_string()));
                }
                group_lines.push(cur);
                if is_blank(lines[cur]) {
                    continue;
                }
                let (_, delta, comma) = scan_line(lines[cur]);
                depth += delta;
                trailing_comma = depth == 0 && comma;
                continue;
            }
            if trailing_comma {
                if cur + 1 >= n || is_blank(lines[cur + 1]) {
                    break;
                }
                cur += 1;
                group_lines.push(cur);
                let (_, delta, comma) = scan_line(lines[cur]);
                depth += delta;
                trailing_comma = depth == 0 && comma;
                continue;
            }
            break;
        }
        groups.push(Group::Content(group_lines));
        i = cur + 1;
    }

    // If there's no actual code, leave the input untouched.
    if !groups.iter().any(|g| matches!(g, Group::Content(_))) {
        return Ok(source.to_string());
    }

    let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    // Patch unterminated block comments (`start..=end`, `has_close`).
    for (start, end, has_close) in block_comments {
        if !has_close {
            out[end].push_str(" */");
        }
        let _ = start;
    }

    // Pass 3: structure — block vs statement, dedent closing, transforms.
    let mut levels: Vec<usize> = vec![0];
    let mut last_stmt_line: Option<usize> = None;
    // Byte length of the trailing `// comment` suffix (including its separating space) tacked
    // onto each finalized statement line, so later `}` insertions land before it, not after.
    let mut trailing_comment_len: Vec<usize> = vec![0; n];

    for gi in 0..groups.len() {
        let group_lines = match &groups[gi] {
            Group::Content(gl) => gl,
            Group::Skip => continue,
        };
        let first = group_lines[0];
        let last = *group_lines.last().unwrap();
        let ind = indents[first].unwrap();

        if ind > *levels.last().unwrap() {
            levels.push(ind);
        } else if ind == *levels.last().unwrap() {
            // sibling at the same level
        } else {
            while *levels.last().unwrap() > ind {
                levels.pop();
                if let Some(l) = last_stmt_line {
                    insert_close(&mut out, &trailing_comment_len, l);
                }
            }
            if *levels.last().unwrap() != ind {
                return Err((
                    first as u32 + 1,
                    "inconsistent indentation: dedent does not match an enclosing level".to_string(),
                ));
            }
        }

        let mut is_block = false;
        for later in groups.iter().skip(gi + 1) {
            if let Group::Content(next_lines) = later {
                let next_ind = indents[next_lines[0]].unwrap();
                is_block = next_ind > ind;
                break;
            }
        }

        let (cs_first, _, _) = scan_line(lines[first]);
        let raw_first = match cs_first {
            Some(p) => &lines[first][..p],
            None => lines[first],
        };
        let first_comment = cs_first.map(|p| &lines[first][p..]);
        let mut first_content = raw_first.to_string();
        if flavor == Flavor::Sass {
            first_content = apply_sass_shorthand(&first_content);
            first_content = apply_import_quote(&first_content);
        }
        if flavor == Flavor::Lass && is_block && ind > 0 {
            first_content = attach_to_parent(&first_content);
        }
        if flavor == Flavor::Lass && !is_block {
            first_content = raw_luau_value(&first_content);
        }

        if first == last {
            let mut content = first_content.trim_end().to_string();
            if flavor == Flavor::Lass && is_block && content.ends_with(':') {
                content.pop();
                let trimmed_len = content.trim_end().len();
                content.truncate(trimmed_len);
            }
            if is_block {
                content.push_str(" {");
            } else if flavor == Flavor::Lass && is_empty_rule(&content) {
                // lass allowed a selector with no body.
                content.push_str(" {}");
            } else {
                content.push(';');
            }
            if let Some(c) = first_comment {
                content.push(' ');
                content.push_str(c);
                trailing_comment_len[first] = c.len() + 1;
            }
            out[first] = content;
        } else {
            let mut content = first_content.trim_end().to_string();
            if let Some(c) = first_comment {
                content.push(' ');
                content.push_str(c);
            }
            out[first] = content;

            let (cs_last, _, _) = scan_line(lines[last]);
            let raw_last = match cs_last {
                Some(p) => &lines[last][..p],
                None => lines[last],
            };
            let last_comment = cs_last.map(|p| &lines[last][p..]);
            let mut content_last = raw_last.trim_end().to_string();
            if flavor == Flavor::Lass && is_block && content_last.ends_with(':') {
                content_last.pop();
                let trimmed_len = content_last.trim_end().len();
                content_last.truncate(trimmed_len);
            }
            if is_block {
                content_last.push_str(" {");
            } else {
                content_last.push(';');
            }
            if let Some(c) = last_comment {
                content_last.push(' ');
                content_last.push_str(c);
                trailing_comment_len[last] = c.len() + 1;
            }
            out[last] = content_last;
        }

        last_stmt_line = Some(last);
    }

    if let Some(l) = last_stmt_line {
        while levels.len() > 1 {
            levels.pop();
            insert_close(&mut out, &trailing_comment_len, l);
        }
    }

    let mut result = out.join("\n");
    if had_trailing_newline {
        result.push('\n');
    }
    Ok(result)
}

/// Inserts a `}` at the end of `out[l]`'s statement content, i.e. before any trailing
/// `// comment` suffix that was appended to it (tracked in `trailing_comment_len`), so the
/// brace stays live code rather than landing inside the comment.
fn insert_close(out: &mut [String], trailing_comment_len: &[usize], l: usize) {
    let clen = trailing_comment_len[l];
    let at = out[l].len() - clen;
    out[l].insert(at, '}');
}

fn scan_indent(line: &str, style: &mut Option<char>, line_no: u32) -> Result<usize, (u32, String)> {
    let mut count = 0usize;
    let mut local: Option<char> = None;
    for c in line.chars() {
        match c {
            ' ' | '\t' => {
                if let Some(l) = local {
                    if l != c {
                        return Err((line_no, "mixed tabs and spaces in indentation".to_string()));
                    }
                } else {
                    local = Some(c);
                }
                count += 1;
            }
            _ => break,
        }
    }
    if let Some(l) = local {
        match style {
            None => *style = Some(l),
            Some(s) => {
                if *s != l {
                    return Err((line_no, "inconsistent indentation: file mixes tabs and spaces".to_string()));
                }
            }
        }
    }
    Ok(count)
}

/// Scans a single line for: where a real (non-string, non-url) `//` comment starts, the net
/// change in bracket depth contributed by `(`/`[`/`)`/`]` outside strings/url(...), and whether
/// the content (excluding any trailing comment) ends with a trailing comma.
fn scan_line(line: &str) -> (Option<usize>, i32, bool) {
    let cs: Vec<(usize, char)> = line.char_indices().collect();
    let count = cs.len();
    let mut idx = 0usize;
    let mut in_string: Option<char> = None;
    let mut in_url = false;
    let mut depth_delta = 0i32;
    let mut comment_start: Option<usize> = None;

    while idx < count {
        let (byte_idx, c) = cs[idx];
        if let Some(qc) = in_string {
            if c == qc {
                in_string = None;
            }
            idx += 1;
            continue;
        }
        if in_url {
            if c == ')' {
                in_url = false;
            }
            idx += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            in_string = Some(c);
            idx += 1;
            continue;
        }
        if c == '/' && idx + 1 < count && cs[idx + 1].1 == '/' {
            comment_start = Some(byte_idx);
            break;
        }
        if (c == 'u' || c == 'U') && line[byte_idx..].starts_with("url(") {
            in_url = true;
            idx += 4;
            continue;
        }
        match c {
            '(' | '[' => depth_delta += 1,
            ')' | ']' => depth_delta -= 1,
            _ => {}
        }
        idx += 1;
    }

    let content_end = comment_start.unwrap_or(line.len());
    let trimmed = line[..content_end].trim_end();
    let ends_comma = trimmed.ends_with(',');
    (comment_start, depth_delta, ends_comma)
}

/// A childless lass line that can only be a selector: it starts like one, or has no `:` at all
/// (lass declarations are `Name: value`; `@` rules and `$` variables are statements).
fn is_empty_rule(content: &str) -> bool {
    let t = content.trim();
    t.starts_with(['.', '#', '&', '>', '[', '*'])
        || (!t.contains(':') && !t.starts_with(['@', '$', '/']) && !t.is_empty())
}

/// lass copied declaration values into the output verbatim, so they could hold any Luau, including
/// table constructors (`ColorSequence.new({ ... })`) that SassScript can't parse. Such values are
/// wrapped in `luau('...')`, which outlass emits verbatim.
fn raw_luau_value(content: &str) -> String {
    let Some((name, value)) = content.split_once(':') else { return content.to_string() };
    let is_property =
        !name.trim().is_empty() && name.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !is_property || !value.contains(['{', '}']) {
        return content.to_string();
    }
    let escaped = value.trim().replace('\\', "\\\\").replace('\'', "\\'");
    format!("{name}: luau('{escaped}')")
}

/// lass compiled nesting into nested Roblox StyleRules, whose selectors refer to the parent's element
/// unless they start with a combinator: a nested `:Hover`, `::UICorner` or `.active` meant the parent
/// itself. In SCSS that is `&:Hover`, so bare nested selectors get an explicit `&` (`> .x` is unchanged).
fn attach_to_parent(content: &str) -> String {
    let indent_len = content.len() - content.trim_start().len();
    let (indent, selectors) = content.split_at(indent_len);
    let parts: Vec<String> = selectors
        .split(',')
        .map(|part| {
            let trimmed = part.trim_start();
            let lead = &part[..part.len() - trimmed.len()];
            if trimmed.starts_with(['.', '#', ':']) { format!("{lead}&{trimmed}") } else { part.to_string() }
        })
        .collect();
    format!("{indent}{}", parts.join(","))
}

/// Sass indented shorthands: `=name` → `@mixin name`, `+name` → `@include name` (only when `+`
/// is immediately followed by an identifier character, so `+ .sibling` selectors are untouched).
fn apply_sass_shorthand(content: &str) -> String {
    let trimmed_start = content.trim_start();
    let leading_len = content.len() - trimmed_start.len();
    let prefix = &content[..leading_len];
    if let Some(name) = trimmed_start.strip_prefix('=') {
        return format!("{prefix}@mixin {name}");
    }
    if let Some(rest) = trimmed_start.strip_prefix('+')
        && rest.chars().next().map(|c| c.is_alphanumeric() || c == '_' || c == '-').unwrap_or(false)
    {
        return format!("{prefix}@include {rest}");
    }
    content.to_string()
}

/// `@import foo, bar` (unquoted, unquoted) → `@import "foo", "bar"`. Quoted/url() imports are
/// left alone.
fn apply_import_quote(content: &str) -> String {
    let trimmed_start = content.trim_start();
    let leading_len = content.len() - trimmed_start.len();
    if let Some(rest) = trimmed_start.strip_prefix("@import")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        let prefix = &content[..leading_len];
        let quoted = rest
            .split(',')
            .map(|part| {
                let t = part.trim();
                if t.is_empty() || t.starts_with('"') || t.starts_with('\'') || t.starts_with("url(") {
                    part.to_string()
                } else {
                    let lead_ws_len = part.len() - part.trim_start().len();
                    format!("{}\"{}\"", &part[..lead_ws_len], t)
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        return format!("{prefix}@import{quoted}");
    }
    content.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: &str, flavor: Flavor) -> String {
        let out = to_scss(input, flavor).expect("should convert");
        assert_eq!(
            out.lines().count(),
            input.lines().count(),
            "line count mismatch\n--- input ---\n{input}\n--- output ---\n{out}"
        );
        out
    }

    fn assert_balanced(s: &str) {
        let mut depth = 0i32;
        for c in s.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0, "unbalanced braces (went negative): {s}");
        }
        assert_eq!(depth, 0, "unbalanced braces: {s}");
    }

    #[test]
    fn empty_input_unchanged() {
        assert_eq!(to_scss("", Flavor::Sass).unwrap(), "");
        let only_comments = "// hi\n// there\n";
        assert_eq!(to_scss(only_comments, Flavor::Sass).unwrap(), only_comments);
    }

    #[test]
    fn simple_statement() {
        let out = check("a\n  color: red\n", Flavor::Sass);
        assert_eq!(out, "a {\n  color: red;}\n");
    }

    #[test]
    fn nested_blocks_multiple_dedents() {
        let input = "a\n  b\n    c\n      d: 1\n  e: 2\nf: 3\n";
        let out = check(input, Flavor::Sass);
        assert_balanced(&out);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "a {");
        assert_eq!(lines[1], "  b {");
        assert_eq!(lines[2], "    c {");
        assert_eq!(lines[3], "      d: 1;}}");
        assert_eq!(lines[4], "  e: 2;}");
        assert_eq!(lines[5], "f: 3;");
    }

    #[test]
    fn comment_inside_string_is_not_a_comment() {
        let input = "a\n  src: \"rbxasset://fonts/x.json\"\n";
        let out = check(input, Flavor::Sass);
        assert!(out.contains("\"rbxasset://fonts/x.json\";"));
        assert!(!out.contains("// fonts"));
    }

    #[test]
    fn comment_inside_url_is_not_a_comment() {
        let input = "a\n  background: url(http://example.com//path)\n";
        let out = check(input, Flavor::Sass);
        assert!(out.contains("url(http://example.com//path);"));
    }

    #[test]
    fn trailing_comment_placed_before_punctuation() {
        let input = "a\n  color: red // hi\n";
        let out = check(input, Flavor::Sass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[1], "  color: red;} // hi");
    }

    #[test]
    fn multi_line_selector_list() {
        let input = "a,\nb\n  color: red\n";
        let out = check(input, Flavor::Sass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "a,");
        assert_eq!(lines[1], "b {");
        assert_eq!(lines[2], "  color: red;}");
        assert_balanced(&out);
    }

    #[test]
    fn multi_line_parenthesized_map() {
        let input = "$map: (\n  a: 1,\n  b: 2\n)\nc\n  color: red\n";
        let out = check(input, Flavor::Sass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "$map: (");
        assert_eq!(lines[1], "  a: 1,");
        assert_eq!(lines[2], "  b: 2");
        assert_eq!(lines[3], ");");
        assert_eq!(lines[4], "c {");
        assert_eq!(lines[5], "  color: red;}");
        assert_balanced(&out);
    }

    #[test]
    fn mixin_and_include_shorthand() {
        let input = "=transition\n  a: 1\n+transition\n";
        let out = check(input, Flavor::Sass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "@mixin transition {");
        assert_eq!(lines[1], "  a: 1;}");
        assert_eq!(lines[2], "@include transition;");
    }

    #[test]
    fn plus_not_followed_by_ident_is_untouched() {
        let input = "a\n  + .sibling\n    color: red\n";
        let out = check(input, Flavor::Sass);
        assert!(out.contains("+ .sibling {"));
    }

    #[test]
    fn import_quoting() {
        let input = "@import foo, bar\n";
        let out = check(input, Flavor::Sass);
        assert_eq!(out, "@import \"foo\", \"bar\";\n");
    }

    #[test]
    fn import_quoted_and_url_left_alone() {
        let input = "@import \"foo\", url(bar.css)\n";
        let out = check(input, Flavor::Sass);
        assert_eq!(out, "@import \"foo\", url(bar.css);\n");
    }

    #[test]
    fn if_else() {
        let input = "@if $x == 1\n  a: 1\n@else\n  a: 2\n";
        let out = check(input, Flavor::Sass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "@if $x == 1 {");
        assert_eq!(lines[1], "  a: 1;}");
        assert_eq!(lines[2], "@else {");
        assert_eq!(lines[3], "  a: 2;}");
        assert_balanced(&out);
    }

    #[test]
    fn block_comment() {
        let input = "/* header\n   still comment\na\n  color: red\n";
        let out = check(input, Flavor::Sass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "/* header");
        assert_eq!(lines[1], "   still comment */");
        assert_eq!(lines[2], "a {");
        assert_eq!(lines[3], "  color: red;}");
    }

    #[test]
    fn mixed_indentation_error() {
        let input = "a\n \tb\n";
        let err = to_scss(input, Flavor::Sass).unwrap_err();
        assert_eq!(err.0, 2);
    }

    #[test]
    fn inconsistent_style_across_lines_error() {
        let input = "a\n  b: 1\n\tc: 2\n";
        let err = to_scss(input, Flavor::Sass).unwrap_err();
        assert_eq!(err.0, 3);
    }

    #[test]
    fn bad_dedent_error() {
        let input = "a\n    b\n        c: 1\n  d: 2\n";
        let err = to_scss(input, Flavor::Sass).unwrap_err();
        assert_eq!(err.0, 4);
    }

    #[test]
    fn lass_priority_and_trailing_colon() {
        let input = "@priority(10)\nSelector:\n\tKey: value\n";
        let out = check(input, Flavor::Lass);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "@priority(10);");
        assert_eq!(lines[1], "Selector {");
        assert_eq!(lines[2], "\tKey: value;}");
    }

    #[test]
    fn lass_no_shorthand() {
        let input = "=notamixin\n\ta: 1\n";
        let out = check(input, Flavor::Lass);
        // '=' is left untouched in lass flavor.
        assert!(out.lines().next().unwrap().starts_with("=notamixin"));
    }

    #[test]
    fn lass_full_example() {
        let input = "@mixin transition\r\n\tTransition: BackgroundColor3, BackgroundTransparency\r\n\r\n@priority(10)\r\nTextButton, TextLabel\r\n\tTextXAlignment: Enum.TextXAlignment.Left\r\n\tAutomaticSize: Enum.AutomaticSize.X\r\n\tRichText: true\r\n\tTextColor3: Color3.fromRGB(255, 255, 255)\r\n\tTextSize: 20\r\n\tFontFace: Font.new \"rbxasset://fonts/families/SourceSansPro.json\"\r\n\tBorderSizePixel: 0\r\n\t@include transition\r\n\r\nTextButton::UICorner, .solid::UICorner\r\n\tCornerRadius: UDim.new(0, 4)\r\n\r\nFrame\r\n\tBorderSizePixel: 0\r\n\tBackgroundTransparency: 1\r\n\r\n.solid\r\n\tBackgroundColor3: Color3.fromRGB(0, 0, 0)\r\n\tBackgroundTransparency: 0.2\r\n\t\r\n:root\r\n\t--Red: Color3.fromRGB(1, 0, 0)\r\n\r\nTextLabel, TextButton\r\n\t--Blue: Color3.fromRGB(0, 0, 1)\r\n\t& > .title\r\n\t\tFontFace: Font.new(\"rbxasset://fonts/families/SourceSansPro.json\", Enum.FontWeight.Bold, Enum.FontStyle.Normal)\r\n\r\nImageLabel\r\n\t// Priority only works on the bottom most selector\r\n\t// Pretty sure this is intended behavior\r\n\t@priority(1)\r\n\t&:Hover\r\n\t\tImageTransparency: 0.5\r\n\r\n.a, .b\r\n\t.h, .j\r\n\t\tE: 1\r\n";
        let out = check(input, Flavor::Lass);
        assert_balanced(&out);
        assert!(out.contains("TextButton, TextLabel {"));
        assert!(out.contains("\tTextXAlignment: Enum.TextXAlignment.Left;"));
        assert!(out.contains("@priority(10);"));
        // The ImageLabel block (opened, then &:Hover nested inside) must fully close.
        assert!(out.contains("ImageTransparency: 0.5;}}"));
        assert!(out.contains("ImageLabel {"));
    }
}
