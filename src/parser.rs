//! Recursive-descent SCSS parser.

use std::rc::Rc;

use crate::ast::*;
use crate::diag::{Error, Result, Span};
use crate::value::{Color, ListSep, Number, Value};

pub fn parse(source: &str, file: Rc<str>) -> Result<Vec<Stmt>> {
    let mut parser = Parser::new(source, file);
    parser.statements(true)
}

/// Parses a standalone expression (used for `--define` values on the command line).
pub fn parse_expression(source: &str, file: Rc<str>) -> Result<Expr> {
    let mut parser = Parser::new(source, file);
    parser.skip_trivia();
    let expr = parser.expression_list()?;
    parser.skip_trivia();
    if !parser.eof() {
        return parser.err("Expected end of expression.");
    }
    Ok(expr)
}

struct Parser {
    src: Vec<char>,
    pos: usize,
    file: Rc<str>,
    line_starts: Vec<usize>,
    /// Identifiers that end an expression in the current context (`in`, `to`, `through`).
    stop_words: &'static [&'static str],
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || !c.is_ascii()
}

/// Normalizes a Sass member name: `_` and `-` are interchangeable.
pub fn normalize(name: &str) -> String {
    name.replace('_', "-")
}

impl Parser {
    fn new(source: &str, file: Rc<str>) -> Self {
        // Windows editors often save UTF-8 with a byte-order mark.
        let source = source.strip_prefix('\u{FEFF}').unwrap_or(source);
        let src: Vec<char> = source.chars().collect();
        let mut line_starts = vec![0];
        for (i, c) in src.iter().enumerate() {
            if *c == '\n' {
                line_starts.push(i + 1);
            }
        }
        Parser { src, pos: 0, file, line_starts, stop_words: &[] }
    }

    // ----- low level -----

    fn span_at(&self, pos: usize) -> Span {
        let line = match self.line_starts.binary_search(&pos) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        Span::new(self.file.clone(), line as u32 + 1, (pos - self.line_starts[line]) as u32 + 1)
    }

    fn span(&self) -> Span {
        self.span_at(self.pos)
    }

    fn err<T>(&self, message: impl Into<String>) -> Result<T> {
        Err(Error::at(message, &self.span()))
    }

    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.src.get(self.pos + offset).copied()
    }

    fn peek_is(&self, c: char) -> bool {
        self.peek() == Some(c)
    }

    fn starts_with(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.peek_at(i) == Some(c))
    }

    /// Like `starts_with` but ASCII case-insensitive and requires a word boundary after.
    fn starts_with_word(&self, word: &str) -> bool {
        word.chars().enumerate().all(|(i, c)| self.peek_at(i).is_some_and(|p| p.eq_ignore_ascii_case(&c)))
            && !self.peek_at(word.chars().count()).is_some_and(is_name_char)
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek_is(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_str(&mut self, s: &str) -> bool {
        if self.starts_with(s) {
            self.pos += s.chars().count();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.starts_with_word(word) {
            self.pos += word.chars().count();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<()> {
        if self.eat(c) {
            Ok(())
        } else {
            match self.peek() {
                Some(found) => self.err(format!("Expected \"{c}\", found \"{found}\".")),
                None => self.err(format!("Expected \"{c}\", found end of file.")),
            }
        }
    }

    /// Skips whitespace and comments. Returns whether anything was skipped.
    fn skip_trivia(&mut self) -> bool {
        let start = self.pos;
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.pos += 1,
                Some('/') if self.peek_at(1) == Some('/') => {
                    while !matches!(self.peek(), None | Some('\n')) {
                        self.pos += 1;
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.pos += 2;
                    while !self.eof() && !self.starts_with("*/") {
                        self.pos += 1;
                    }
                    self.pos = (self.pos + 2).min(self.src.len());
                }
                _ => break,
            }
        }
        self.pos > start
    }

    fn ident_starts_here(&self) -> bool {
        match self.peek() {
            Some('-') => match self.peek_at(1) {
                Some(c) if is_name_start(c) || c == '-' || c == '\\' => true,
                Some('#') => self.peek_at(2) == Some('{'),
                _ => false,
            },
            Some('#') => self.peek_at(1) == Some('{'),
            Some('\\') => true,
            Some(c) => is_name_start(c),
            None => false,
        }
    }

    /// A plain identifier without interpolation.
    fn plain_ident(&mut self) -> Result<String> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == '-' {
                // A trailing `-` belongs to the identifier only if a name char follows.
                if s.is_empty() || self.peek_at(1).is_some_and(is_name_char) {
                    s.push(c);
                    self.pos += 1;
                    continue;
                }
                break;
            } else if is_name_char(c) {
                s.push(c);
                self.pos += 1;
            } else if c == '\\' {
                self.pos += 1;
                s.push(self.escape()?);
            } else {
                break;
            }
        }
        if s.is_empty() || s == "-" {
            return self.err("Expected identifier.");
        }
        Ok(s)
    }

    fn escape(&mut self) -> Result<char> {
        let mut hex = String::new();
        while hex.len() < 6 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            hex.push(self.advance().unwrap());
        }
        if hex.is_empty() {
            return self.advance().ok_or_else(|| Error::at("Expected escape sequence.", &self.span()));
        }
        if self.peek().is_some_and(|c| c == ' ') {
            self.pos += 1;
        }
        Ok(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap_or('\u{FFFD}'))
    }

    /// An identifier that may contain `#{}` interpolation.
    fn interp_ident(&mut self) -> Result<Interp> {
        let mut parts: Interp = Vec::new();
        let mut text = String::new();
        loop {
            if self.starts_with("#{") {
                if !text.is_empty() {
                    parts.push(InterpPart::Text(std::mem::take(&mut text)));
                }
                parts.push(InterpPart::Expr(self.interpolation()?));
                continue;
            }
            match self.peek() {
                Some('-') if text.is_empty() && parts.is_empty() => {
                    text.push('-');
                    self.pos += 1;
                }
                Some('-') => {
                    if self.peek_at(1).is_some_and(is_name_char) || self.peek_at(1) == Some('#') {
                        text.push('-');
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                Some('\\') => {
                    self.pos += 1;
                    text.push(self.escape()?);
                }
                Some(c) if is_name_char(c) => {
                    text.push(c);
                    self.pos += 1;
                }
                _ => break,
            }
        }
        if !text.is_empty() {
            parts.push(InterpPart::Text(text));
        }
        if parts.is_empty() {
            return self.err("Expected identifier.");
        }
        Ok(parts)
    }

    fn interpolation(&mut self) -> Result<Expr> {
        self.pos += 2; // #{
        self.skip_trivia();
        let expr = self.expression_list()?;
        self.skip_trivia();
        self.expect('}')?;
        Ok(expr)
    }

    /// Reads raw text (selectors, at-rule params) until one of `stops` at nesting depth 0.
    /// Comments are removed, interpolation is parsed, whitespace runs collapse to one space.
    fn raw_until(&mut self, stops: &[char]) -> Result<Interp> {
        let mut parts: Interp = Vec::new();
        let mut text = String::new();
        let mut depth = 0i32;
        while let Some(c) = self.peek() {
            if depth == 0 && stops.contains(&c) {
                break;
            }
            match c {
                '#' if self.peek_at(1) == Some('{') => {
                    if !text.is_empty() {
                        parts.push(InterpPart::Text(std::mem::take(&mut text)));
                    }
                    parts.push(InterpPart::Expr(self.interpolation()?));
                }
                '/' if self.peek_at(1) == Some('*') || (self.peek_at(1) == Some('/') && !text.ends_with(':')) => {
                    self.skip_trivia();
                    if !text.ends_with(' ') {
                        text.push(' ');
                    }
                }
                '"' | '\'' => {
                    // Copy quoted strings verbatim (with interpolation).
                    self.pos += 1;
                    text.push(c);
                    loop {
                        match self.peek() {
                            None => return self.err("Unterminated string."),
                            Some(q) if q == c => {
                                self.pos += 1;
                                text.push(q);
                                break;
                            }
                            Some('\\') => {
                                text.push('\\');
                                self.pos += 1;
                                if let Some(n) = self.advance() {
                                    text.push(n);
                                }
                            }
                            Some('#') if self.peek_at(1) == Some('{') => {
                                parts.push(InterpPart::Text(std::mem::take(&mut text)));
                                parts.push(InterpPart::Expr(self.interpolation()?));
                            }
                            Some(o) => {
                                text.push(o);
                                self.pos += 1;
                            }
                        }
                    }
                }
                c if c.is_whitespace() => {
                    self.pos += 1;
                    if !text.ends_with(' ') {
                        text.push(' ');
                    }
                }
                _ => {
                    match c {
                        '(' | '[' => depth += 1,
                        ')' | ']' => depth -= 1,
                        _ => {}
                    }
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        if !text.is_empty() {
            parts.push(InterpPart::Text(text));
        }
        // Trim surrounding whitespace.
        if let Some(InterpPart::Text(t)) = parts.first_mut() {
            *t = t.trim_start().to_string();
        }
        if let Some(InterpPart::Text(t)) = parts.last_mut() {
            *t = t.trim_end().to_string();
        }
        parts.retain(|p| !matches!(p, InterpPart::Text(t) if t.is_empty()));
        Ok(parts)
    }

    // ----- statements -----

    fn statements(&mut self, top_level: bool) -> Result<Vec<Stmt>> {
        let mut stmts = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None if top_level => break,
                None => return self.err("Expected \"}\"."),
                Some('}') if top_level => return self.err("Unexpected \"}\"."),
                Some('}') => break,
                Some(';') => {
                    self.pos += 1;
                }
                Some(_) => stmts.push(self.statement()?),
            }
        }
        Ok(stmts)
    }

    fn block(&mut self) -> Result<Vec<Stmt>> {
        self.skip_trivia();
        self.expect('{')?;
        let body = self.statements(false)?;
        self.expect('}')?;
        Ok(body)
    }

    fn expect_statement_end(&mut self) -> Result<()> {
        self.skip_trivia();
        match self.peek() {
            Some(';') => {
                self.pos += 1;
                Ok(())
            }
            None | Some('}') => Ok(()),
            Some(c) => self.err(format!("Expected \";\", found \"{c}\".")),
        }
    }

    fn statement(&mut self) -> Result<Stmt> {
        let span = self.span();
        let kind = match self.peek() {
            Some('@') => self.at_rule()?,
            Some('$') => self.var_decl(None)?,
            _ if self.starts_with("--") => self.declaration(true)?,
            _ => {
                // `ns.$var: value` assigns a module variable.
                let save = self.pos;
                if self.ident_starts_here()
                    && let Ok(ns) = self.plain_ident()
                    && self.peek_is('.')
                    && self.peek_at(1) == Some('$')
                {
                    self.pos += 1;
                    return Ok(Stmt { kind: self.var_decl(Some(ns))?, span });
                }
                self.pos = save;
                if self.looks_like_rule() { self.style_rule()? } else { self.declaration(false)? }
            }
        };
        Ok(Stmt { kind, span })
    }

    /// Decides whether the upcoming text is a style rule (`selector {`) or a declaration.
    fn looks_like_rule(&self) -> bool {
        let mut i = self.pos;
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        while let Some(&c) = self.src.get(i) {
            if let Some(q) = quote {
                if c == '\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            match c {
                '"' | '\'' => quote = Some(c),
                '#' if self.src.get(i + 1) == Some(&'{') => {
                    // Skip the interpolation.
                    let mut d = 0;
                    while let Some(&c) = self.src.get(i) {
                        if c == '{' {
                            d += 1;
                        } else if c == '}' {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        i += 1;
                    }
                }
                '/' if self.src.get(i + 1) == Some(&'*') => {
                    while i < self.src.len() && !(self.src[i] == '*' && self.src.get(i + 1) == Some(&'/')) {
                        i += 1;
                    }
                    i += 1;
                }
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                ';' | '}' if depth <= 0 => return false,
                '{' if depth <= 0 => return !self.is_nested_property_start(),
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// `name: {` or `name: value {` (nested properties) — an identifier, a colon, then whitespace or `{`.
    fn is_nested_property_start(&self) -> bool {
        let mut i = self.pos;
        while let Some(&c) = self.src.get(i) {
            if is_name_char(c) {
                i += 1;
            } else if c == '#' && self.src.get(i + 1) == Some(&'{') {
                while i < self.src.len() && self.src[i] != '}' {
                    i += 1;
                }
                i += 1;
            } else {
                break;
            }
        }
        i > self.pos
            && self.src.get(i) == Some(&':')
            && self.src.get(i + 1).is_some_and(|c| c.is_whitespace() || *c == '{')
    }

    fn style_rule(&mut self) -> Result<StmtKind> {
        let selector = self.raw_until(&['{', ';', '}'])?;
        if !self.peek_is('{') {
            return self.err("Expected \"{\" after selector (or \":\" for a declaration).");
        }
        let body = self.block()?;
        Ok(StmtKind::Rule { selector, body })
    }

    fn declaration(&mut self, custom: bool) -> Result<StmtKind> {
        let name = self.interp_ident()?;
        self.skip_trivia();
        if !self.eat(':') {
            return self.err("Expected \":\" (property declaration) or \"{\" (style rule).");
        }
        if custom {
            // A custom property's value is raw text, in CSS and in dart-sass alike: only `#{}` is
            // substituted, so a bare `$var` stays the four characters `$var`. Codegen reads the
            // text back as a CSS value when it turns the token into a StyleSheet attribute.
            self.skip_trivia();
            let mut raw = self.raw_until(&[';', '}'])?;
            let important = take_important(&mut raw);
            self.expect_statement_end()?;
            let value = Expr::Str { parts: raw, quoted: false };
            return Ok(StmtKind::Decl { name, value: Some(value), children: Vec::new(), important });
        }
        self.skip_trivia();
        if self.peek_is('{') {
            let children = self.block()?;
            return Ok(StmtKind::Decl { name, value: None, children, important: false });
        }
        let value = self.expression_list()?;
        let important = self.important()?;
        self.skip_trivia();
        let children = if self.peek_is('{') {
            self.block()?
        } else {
            self.expect_statement_end()?;
            Vec::new()
        };
        Ok(StmtKind::Decl { name, value: Some(value), children, important })
    }

    fn important(&mut self) -> Result<bool> {
        let save = self.pos;
        self.skip_trivia();
        if self.eat('!') {
            self.skip_trivia();
            if self.eat_word("important") {
                return Ok(true);
            }
            return self.err("Expected \"important\".");
        }
        self.pos = save;
        Ok(false)
    }

    fn var_decl(&mut self, ns: Option<String>) -> Result<StmtKind> {
        self.expect('$')?;
        let name = normalize(&self.plain_ident()?);
        self.skip_trivia();
        self.expect(':')?;
        self.skip_trivia();
        let value = self.expression_list()?;
        let mut default = false;
        let mut global = false;
        loop {
            self.skip_trivia();
            if !self.eat('!') {
                break;
            }
            let flag = self.plain_ident()?;
            match flag.as_str() {
                "default" => default = true,
                "global" => global = true,
                other => return self.err(format!("Invalid flag \"!{other}\"; expected !default or !global.")),
            }
        }
        self.expect_statement_end()?;
        Ok(StmtKind::VarDecl { ns, name, value, default, global })
    }

    fn at_rule(&mut self) -> Result<StmtKind> {
        self.expect('@')?;
        let name = self.plain_ident()?;
        self.skip_trivia();
        let kind = match name.as_str() {
            "use" => {
                let url = self.string_literal()?;
                self.skip_trivia();
                let mut namespace = None;
                if self.eat_word("as") {
                    self.skip_trivia();
                    namespace = Some(if self.eat('*') { "*".to_string() } else { normalize(&self.plain_ident()?) });
                    self.skip_trivia();
                }
                let with = self.with_clause()?;
                self.expect_statement_end()?;
                StmtKind::Use { url, namespace, with }
            }
            "forward" => {
                let url = self.string_literal()?;
                self.skip_trivia();
                let mut prefix = None;
                if self.eat_word("as") {
                    self.skip_trivia();
                    let p = self.plain_ident()?;
                    self.expect('*')?;
                    prefix = Some(normalize(&p));
                    self.skip_trivia();
                }
                if self.eat_word("show") || self.eat_word("hide") {
                    // Visibility filters are accepted but not enforced.
                    self.raw_until(&[';', '}'])?;
                }
                self.skip_trivia();
                let with = self.with_clause()?;
                self.expect_statement_end()?;
                StmtKind::Forward { url, prefix, with }
            }
            "import" => {
                let mut urls = Vec::new();
                loop {
                    self.skip_trivia();
                    if self.starts_with_word("url") {
                        let raw = self.raw_until(&[',', ';', '}'])?;
                        urls.push(interp_text(&raw));
                    } else {
                        urls.push(self.string_literal()?);
                    }
                    self.skip_trivia();
                    // Media queries after a plain CSS import are kept in the URL text.
                    if !self.peek_is(',') && !matches!(self.peek(), None | Some(';') | Some('}')) {
                        let rest = self.raw_until(&[';', '}'])?;
                        if let Some(last) = urls.last_mut() {
                            last.push(' ');
                            last.push_str(&interp_text(&rest));
                        }
                    }
                    if !self.eat(',') {
                        break;
                    }
                }
                self.expect_statement_end()?;
                StmtKind::Import { urls }
            }
            "mixin" => {
                let name = normalize(&self.plain_ident()?);
                self.skip_trivia();
                let params = if self.peek_is('(') { self.params()? } else { Params::default() };
                let body = self.block()?;
                StmtKind::Mixin { name, params, body }
            }
            "function" => {
                let name = normalize(&self.plain_ident()?);
                self.skip_trivia();
                let params = self.params()?;
                let body = self.block()?;
                StmtKind::Function { name, params, body }
            }
            "include" => {
                let mut name = normalize(&self.plain_ident()?);
                let mut ns = None;
                if self.peek_is('.') {
                    self.pos += 1;
                    ns = Some(name);
                    name = normalize(&self.plain_ident()?);
                }
                self.skip_trivia();
                let args = if self.peek_is('(') { self.args()? } else { ArgList::default() };
                self.skip_trivia();
                let mut content_params = None;
                if self.eat_word("using") {
                    self.skip_trivia();
                    content_params = Some(self.params()?);
                    self.skip_trivia();
                }
                let content = if self.peek_is('{') {
                    Some(ContentBlock { params: content_params.unwrap_or_default(), body: self.block()? })
                } else {
                    self.expect_statement_end()?;
                    None
                };
                StmtKind::Include { ns, name, args, content }
            }
            "content" => {
                let args = if self.peek_is('(') { self.args()? } else { ArgList::default() };
                self.expect_statement_end()?;
                StmtKind::Content { args }
            }
            "return" => {
                let value = self.expression_list()?;
                self.expect_statement_end()?;
                StmtKind::Return(value)
            }
            "if" => self.if_rule()?,
            "else" | "elseif" => return self.err("@else must come after @if."),
            "each" => {
                let mut vars = Vec::new();
                loop {
                    self.skip_trivia();
                    self.expect('$')?;
                    vars.push(normalize(&self.plain_ident()?));
                    self.skip_trivia();
                    if !self.eat(',') {
                        break;
                    }
                }
                if !self.eat_word("in") {
                    return self.err("Expected \"in\".");
                }
                self.skip_trivia();
                let list = self.with_stop_words(&["in"], |p| p.expression_list())?;
                let body = self.block()?;
                StmtKind::Each { vars, list, body }
            }
            "for" => {
                self.expect('$')?;
                let var = normalize(&self.plain_ident()?);
                self.skip_trivia();
                if !self.eat_word("from") {
                    return self.err("Expected \"from\".");
                }
                self.skip_trivia();
                let from = self.with_stop_words(&["to", "through"], |p| p.space_list())?;
                self.skip_trivia();
                let inclusive = if self.eat_word("through") {
                    true
                } else if self.eat_word("to") {
                    false
                } else {
                    return self.err("Expected \"to\" or \"through\".");
                };
                self.skip_trivia();
                let to = self.space_list()?;
                let body = self.block()?;
                StmtKind::For { var, from, to, inclusive, body }
            }
            "while" => {
                let cond = self.expression_list()?;
                let body = self.block()?;
                StmtKind::While { cond, body }
            }
            "extend" => {
                let mut selector = self.raw_until(&[';', '}'])?;
                let mut optional = false;
                if let Some(InterpPart::Text(t)) = selector.last_mut()
                    && let Some(stripped) = t.trim_end().strip_suffix("!optional")
                {
                    *t = stripped.trim_end().to_string();
                    optional = true;
                }
                self.expect_statement_end()?;
                StmtKind::Extend { selector, optional }
            }
            "debug" | "warn" | "error" => {
                let value = self.expression_list()?;
                self.expect_statement_end()?;
                match name.as_str() {
                    "debug" => StmtKind::Debug(value),
                    "warn" => StmtKind::Warn(value),
                    _ => StmtKind::Error(value),
                }
            }
            "at-root" => {
                let selector = if self.peek_is('{') {
                    None
                } else if self.peek_is('(') {
                    // `(with: ...)` / `(without: ...)` queries only matter for @media; ignore them.
                    self.raw_until(&['{'])?;
                    None
                } else {
                    Some(self.raw_until(&['{'])?)
                };
                let body = self.block()?;
                StmtKind::AtRoot { selector, body }
            }
            _ => {
                let params = self.raw_until(&['{', ';', '}'])?;
                let body = if self.peek_is('{') {
                    Some(self.block()?)
                } else {
                    self.expect_statement_end()?;
                    None
                };
                StmtKind::AtRule { name, params, body }
            }
        };
        Ok(kind)
    }

    fn with_stop_words<T>(
        &mut self,
        words: &'static [&'static str],
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let old = std::mem::replace(&mut self.stop_words, words);
        let result = f(self);
        self.stop_words = old;
        result
    }

    fn with_clause(&mut self) -> Result<Vec<(String, Expr)>> {
        let mut with = Vec::new();
        if !self.eat_word("with") {
            return Ok(with);
        }
        self.skip_trivia();
        self.expect('(')?;
        loop {
            self.skip_trivia();
            if self.eat(')') {
                break;
            }
            self.expect('$')?;
            let name = normalize(&self.plain_ident()?);
            self.skip_trivia();
            self.expect(':')?;
            self.skip_trivia();
            let value = self.space_list()?;
            self.skip_trivia();
            if self.eat_word("!default") {
                self.skip_trivia();
            }
            with.push((name, value));
            self.skip_trivia();
            if !self.eat(',') {
                self.skip_trivia();
                self.expect(')')?;
                break;
            }
        }
        Ok(with)
    }

    fn if_rule(&mut self) -> Result<StmtKind> {
        let cond = self.expression_list()?;
        let body = self.block()?;
        let mut clauses = vec![(cond, body)];
        let mut else_body = None;
        loop {
            let save = self.pos;
            self.skip_trivia();
            let is_elseif = self.eat_str("@elseif");
            if !is_elseif && !self.eat_word("@else") {
                self.pos = save;
                break;
            }
            self.skip_trivia();
            if is_elseif || self.eat_word("if") {
                self.skip_trivia();
                let cond = self.expression_list()?;
                let body = self.block()?;
                clauses.push((cond, body));
            } else {
                else_body = Some(self.block()?);
                break;
            }
        }
        Ok(StmtKind::If { clauses, else_body })
    }

    fn string_literal(&mut self) -> Result<String> {
        match self.peek() {
            Some(q @ ('"' | '\'')) => {
                self.pos += 1;
                let mut s = String::new();
                loop {
                    match self.advance() {
                        None => return self.err("Unterminated string."),
                        Some(c) if c == q => break,
                        Some('\\') => s.push(self.escape()?),
                        Some(c) => s.push(c),
                    }
                }
                Ok(s)
            }
            _ => self.err("Expected string."),
        }
    }

    fn params(&mut self) -> Result<Params> {
        self.expect('(')?;
        let mut params = Params::default();
        loop {
            self.skip_trivia();
            if self.eat(')') {
                break;
            }
            self.expect('$')?;
            let name = normalize(&self.plain_ident()?);
            self.skip_trivia();
            if self.eat_str("...") {
                params.rest = Some(name);
                self.skip_trivia();
                self.eat(',');
                self.skip_trivia();
                self.expect(')')?;
                break;
            }
            let default = if self.eat(':') {
                self.skip_trivia();
                Some(self.space_list()?)
            } else {
                None
            };
            params.params.push(Param { name, default });
            self.skip_trivia();
            if !self.eat(',') {
                self.expect(')')?;
                break;
            }
        }
        Ok(params)
    }

    fn args(&mut self) -> Result<ArgList> {
        self.expect('(')?;
        let mut args = ArgList::default();
        loop {
            self.skip_trivia();
            if self.eat(')') {
                break;
            }
            // Named argument?
            if self.peek_is('$') {
                let save = self.pos;
                self.pos += 1;
                if let Ok(name) = self.plain_ident() {
                    self.skip_trivia();
                    if self.peek_is(':') && self.peek_at(1) != Some(':') {
                        self.pos += 1;
                        self.skip_trivia();
                        let value = self.space_list()?;
                        args.named.push((normalize(&name), value));
                        self.skip_trivia();
                        if !self.eat(',') {
                            self.expect(')')?;
                            break;
                        }
                        continue;
                    }
                }
                self.pos = save;
            }
            let value = self.space_list()?;
            self.skip_trivia();
            if self.eat_str("...") {
                if args.rest.is_none() {
                    args.rest = Some(Box::new(value));
                } else {
                    args.kw_rest = Some(Box::new(value));
                }
            } else {
                args.positional.push(value);
            }
            self.skip_trivia();
            if !self.eat(',') {
                self.expect(')')?;
                break;
            }
        }
        Ok(args)
    }

    // ----- expressions -----

    fn at_expression_end(&self) -> bool {
        matches!(self.peek(), None | Some(';' | '}' | '{' | ')' | ']' | '!' | ',' | ':'))
    }

    /// Comma-separated list (lowest precedence).
    fn expression_list(&mut self) -> Result<Expr> {
        let first = self.space_list()?;
        let save = self.pos;
        self.skip_trivia();
        if !self.peek_is(',') {
            self.pos = save;
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(',') {
            self.skip_trivia();
            if self.at_expression_end() {
                break; // trailing comma
            }
            items.push(self.space_list()?);
            let save = self.pos;
            self.skip_trivia();
            if !self.peek_is(',') {
                self.pos = save;
            }
        }
        Ok(Expr::List { items, sep: ListSep::Comma, bracketed: false })
    }

    fn at_stop_word(&self) -> bool {
        self.stop_words.iter().any(|w| self.starts_with_word(w))
    }

    fn can_start_expression(&self) -> bool {
        if self.at_stop_word() {
            return false;
        }
        match self.peek() {
            Some(c) if c.is_ascii_alphanumeric() => true,
            Some('$' | '"' | '\'' | '(' | '[' | '&' | '_' | '\\') => true,
            Some('#') => true,
            Some('-' | '+') => true,
            Some('.') => self.peek_at(1).is_some_and(|c| c.is_ascii_digit()),
            Some(c) => !c.is_ascii(),
            None => false,
        }
    }

    /// Space-separated list.
    fn space_list(&mut self) -> Result<Expr> {
        let first = self.or_expr()?;
        let mut items = vec![first];
        loop {
            let save = self.pos;
            self.skip_trivia();
            if self.at_expression_end() || !self.can_start_expression() {
                self.pos = save;
                break;
            }
            items.push(self.or_expr()?);
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Expr::List { items, sep: ListSep::Space, bracketed: false }
        })
    }

    /// Peeks past whitespace for a keyword operator; consumes it (and trailing trivia) if found.
    fn eat_keyword_op(&mut self, word: &str) -> bool {
        let save = self.pos;
        self.skip_trivia();
        if self.eat_word(word) {
            self.skip_trivia();
            true
        } else {
            self.pos = save;
            false
        }
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.and_expr()?;
        loop {
            let span = self.span();
            if !self.eat_keyword_op("or") {
                return Ok(lhs);
            }
            let rhs = self.and_expr()?;
            lhs = Expr::Binary { op: BinOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.eq_expr()?;
        loop {
            let span = self.span();
            if !self.eat_keyword_op("and") {
                return Ok(lhs);
            }
            let rhs = self.eq_expr()?;
            lhs = Expr::Binary { op: BinOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
    }

    fn symbol_op(&mut self, ops: &[(&str, BinOp)]) -> Option<BinOp> {
        let save = self.pos;
        self.skip_trivia();
        for (text, op) in ops {
            if self.starts_with(text) {
                // Don't treat `=` of `==` as `<=`, etc.: ops are ordered longest first by callers.
                self.pos += text.len();
                self.skip_trivia();
                return Some(*op);
            }
        }
        self.pos = save;
        None
    }

    fn eq_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.rel_expr()?;
        loop {
            let span = self.span();
            let Some(op) = self.symbol_op(&[("==", BinOp::Eq), ("!=", BinOp::Ne)]) else { return Ok(lhs) };
            let rhs = self.rel_expr()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
    }

    fn rel_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.add_expr()?;
        loop {
            let span = self.span();
            let Some(op) = self.symbol_op(&[("<=", BinOp::Le), (">=", BinOp::Ge), ("<", BinOp::Lt), (">", BinOp::Gt)])
            else {
                return Ok(lhs);
            };
            let rhs = self.add_expr()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
    }

    fn add_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            let save = self.pos;
            let ws_before = self.skip_trivia();
            let span = self.span();
            let op = match self.peek() {
                Some('+') => BinOp::Add,
                Some('-') => BinOp::Sub,
                _ => {
                    self.pos = save;
                    return Ok(lhs);
                }
            };
            let next = self.peek_at(1);
            let ws_after = next.is_some_and(|c| c.is_whitespace());
            // `a -b` is a list of `a` and `-b`; `a - b` and `a-b` are subtraction.
            // `a -$b` / `a -(b)` are lists as well, matching Sass.
            if ws_before && !ws_after {
                self.pos = save;
                return Ok(lhs);
            }
            // `-` directly followed by a name char would have been lexed into an identifier.
            if op == BinOp::Sub && !ws_before && next.is_some_and(is_name_start) {
                self.pos = save;
                return Ok(lhs);
            }
            self.pos += 1;
            self.skip_trivia();
            let rhs = self.mul_expr()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
    }

    fn mul_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let save = self.pos;
            self.skip_trivia();
            let span = self.span();
            let op = match self.peek() {
                Some('*') => BinOp::Mul,
                Some('/') if !matches!(self.peek_at(1), Some('/' | '*')) => BinOp::Div,
                Some('%') => BinOp::Mod,
                _ => {
                    self.pos = save;
                    return Ok(lhs);
                }
            };
            self.pos += 1;
            self.skip_trivia();
            let rhs = self.unary()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        let span = self.span();
        match self.peek() {
            Some('-') => {
                let next = self.peek_at(1);
                if next.is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    // Negative number literal.
                    self.pos += 1;
                    let n = self.number()?;
                    return Ok(match n {
                        Expr::Value(Value::Number(mut n)) => {
                            n.value = -n.value;
                            Expr::Value(Value::Number(n))
                        }
                        other => Expr::Unary { op: UnaryOp::Neg, expr: Box::new(other), span },
                    });
                }
                if self.ident_starts_here() {
                    return self.primary();
                }
                self.pos += 1;
                self.skip_trivia();
                let expr = self.unary()?;
                Ok(Expr::Unary { op: UnaryOp::Neg, expr: Box::new(expr), span })
            }
            Some('+') => {
                self.pos += 1;
                self.skip_trivia();
                let expr = self.unary()?;
                Ok(Expr::Unary { op: UnaryOp::Plus, expr: Box::new(expr), span })
            }
            _ if self.starts_with_word("not") => {
                self.pos += 3;
                self.skip_trivia();
                let expr = self.unary()?;
                Ok(Expr::Unary { op: UnaryOp::Not, expr: Box::new(expr), span })
            }
            _ => self.primary(),
        }
    }

    fn number(&mut self) -> Result<Expr> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.peek_is('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            s.push('.');
            self.pos += 1;
            while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                s.push(c);
                self.pos += 1;
            }
        }
        // Exponent (not to be confused with the `em`/`ex` units).
        if matches!(self.peek(), Some('e' | 'E')) {
            let sign = matches!(self.peek_at(1), Some('+' | '-'));
            let digit_at = if sign { 2 } else { 1 };
            if self.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                s.push('e');
                if sign {
                    s.push(self.peek_at(1).unwrap());
                }
                self.pos += digit_at;
                while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                    s.push(c);
                    self.pos += 1;
                }
            }
        }
        let value: f64 = s.parse().map_err(|_| Error::at("Expected number.", &self.span()))?;
        let unit = if self.eat('%') {
            "%".to_string()
        } else if self.peek().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
            let mut unit = String::new();
            while let Some(c) = self.peek() {
                if c.is_ascii_alphabetic()
                    || c == '_'
                    || (c == '-' && self.peek_at(1).is_some_and(|n| n.is_ascii_alphabetic()))
                {
                    unit.push(c);
                    self.pos += 1;
                } else {
                    break;
                }
            }
            unit
        } else {
            String::new()
        };
        Ok(Expr::Value(Value::Number(Number::with_unit(value, &unit))))
    }

    fn primary(&mut self) -> Result<Expr> {
        let span = self.span();
        match self.peek() {
            None => self.err("Expected expression."),
            Some('(') => self.paren(),
            Some('[') => {
                self.pos += 1;
                self.skip_trivia();
                if self.eat(']') {
                    return Ok(Expr::List { items: Vec::new(), sep: ListSep::Undecided, bracketed: true });
                }
                let inner = self.expression_list()?;
                self.skip_trivia();
                self.expect(']')?;
                Ok(match inner {
                    Expr::List { items, sep, bracketed: false } => Expr::List { items, sep, bracketed: true },
                    other => Expr::List { items: vec![other], sep: ListSep::Undecided, bracketed: true },
                })
            }
            Some('"' | '\'') => self.quoted_string(),
            Some('$') => {
                self.pos += 1;
                let name = normalize(&self.plain_ident()?);
                Ok(Expr::Var { ns: None, name, span })
            }
            Some('&') => {
                self.pos += 1;
                Ok(Expr::Parent)
            }
            Some('#') if self.peek_at(1) != Some('{') => {
                self.pos += 1;
                let mut text = String::new();
                while let Some(c) = self.peek().filter(|c| is_name_char(*c)) {
                    text.push(c);
                    self.pos += 1;
                }
                if matches!(text.len(), 3 | 4 | 6 | 8)
                    && let Some(color) = Color::from_hex(&text)
                {
                    return Ok(Expr::Value(Value::Color(color)));
                }
                Ok(Expr::Value(Value::str(format!("#{text}"))))
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            // A `*` can't start an arithmetic expression, so it's the literal (`Transition: * 0.5s`).
            Some('*') => {
                self.pos += 1;
                Ok(Expr::Value(Value::str("*")))
            }
            _ if self.ident_starts_here() => self.identifier_expr(),
            Some(c) => self.err(format!("Expected expression, found \"{c}\".")),
        }
    }

    fn paren(&mut self) -> Result<Expr> {
        self.expect('(')?;
        self.skip_trivia();
        if self.eat(')') {
            return Ok(Expr::List { items: Vec::new(), sep: ListSep::Undecided, bracketed: false });
        }
        let first = self.space_list()?;
        self.skip_trivia();
        if self.eat(':') {
            // Map literal.
            self.skip_trivia();
            let value = self.space_list()?;
            let mut pairs = vec![(first, value)];
            loop {
                self.skip_trivia();
                if !self.eat(',') {
                    break;
                }
                self.skip_trivia();
                if self.peek_is(')') {
                    break;
                }
                let key = self.space_list()?;
                self.skip_trivia();
                self.expect(':')?;
                self.skip_trivia();
                let value = self.space_list()?;
                pairs.push((key, value));
            }
            self.skip_trivia();
            self.expect(')')?;
            return Ok(Expr::Map(pairs));
        }
        if self.peek_is(',') {
            let mut items = vec![first];
            while self.eat(',') {
                self.skip_trivia();
                if self.peek_is(')') {
                    break;
                }
                items.push(self.space_list()?);
                self.skip_trivia();
            }
            self.expect(')')?;
            return Ok(Expr::List { items, sep: ListSep::Comma, bracketed: false });
        }
        self.expect(')')?;
        Ok(Expr::Paren(Box::new(first)))
    }

    fn quoted_string(&mut self) -> Result<Expr> {
        let quote = self.advance().unwrap();
        let mut parts: Interp = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return self.err("Unterminated string."),
                Some(c) if c == quote => {
                    self.pos += 1;
                    break;
                }
                Some('\\') => {
                    self.pos += 1;
                    if self.eat('\n') {
                        continue; // line continuation
                    }
                    text.push(self.escape()?);
                }
                Some('#') if self.peek_at(1) == Some('{') => {
                    if !text.is_empty() {
                        parts.push(InterpPart::Text(std::mem::take(&mut text)));
                    }
                    parts.push(InterpPart::Expr(self.interpolation()?));
                }
                Some(c) => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        if parts.is_empty() {
            return Ok(Expr::Value(Value::quoted(text)));
        }
        if !text.is_empty() {
            parts.push(InterpPart::Text(text));
        }
        Ok(Expr::Str { parts, quoted: true })
    }

    fn identifier_expr(&mut self) -> Result<Expr> {
        let span = self.span();
        let mut parts = self.interp_ident()?;
        let plain = match parts.as_slice() {
            [InterpPart::Text(t)] => Some(t.clone()),
            _ => None,
        };
        let Some(mut name) = plain else {
            // Interpolated identifier; may still be a function call like `#{$fn}(x)` — keep it text.
            if self.peek_is('(') {
                let args = self.raw_parens()?;
                parts.extend(args);
            }
            return Ok(Expr::Str { parts, quoted: false });
        };

        // Dotted paths: `math.div`, `Enum.Font.Gotham`, `Color3.fromRGB`, and namespaced `ns.$var`.
        while self.peek_is('.') {
            match self.peek_at(1) {
                Some('$') => {
                    self.pos += 2;
                    let var = normalize(&self.plain_ident()?);
                    return Ok(Expr::Var { ns: Some(normalize(&name)), name: var, span });
                }
                Some(c) if is_name_start(c) => {
                    self.pos += 1;
                    name.push('.');
                    name.push_str(&self.plain_ident()?);
                }
                _ => break,
            }
        }

        if self.peek_is('(') {
            let lower = name.to_ascii_lowercase();
            if (lower == "url" || lower == "expression")
                && let Some(raw) = self.raw_url()?
            {
                return Ok(Expr::Call {
                    name: lower,
                    args: ArgList { positional: vec![Expr::Str { parts: raw, quoted: false }], ..Default::default() },
                    span,
                });
            }
            let args = self.args()?;
            return Ok(Expr::Call { name, args, span });
        }

        match name.as_str() {
            "true" => return Ok(Expr::Value(Value::Bool(true))),
            "false" => return Ok(Expr::Value(Value::Bool(false))),
            "null" => return Ok(Expr::Value(Value::Null)),
            _ => {}
        }
        if !name.contains('.')
            && let Some(color) = Color::from_name(&name)
        {
            return Ok(Expr::Value(Value::Color(color)));
        }
        Ok(Expr::Value(Value::str(name)))
    }

    /// For `url(`: if the argument is unquoted, reads it raw (with interpolation) up to `)`.
    fn raw_url(&mut self) -> Result<Option<Interp>> {
        let save = self.pos;
        self.pos += 1; // (
        self.skip_ws_only();
        if matches!(self.peek(), Some('"' | '\'')) {
            self.pos = save;
            return Ok(None);
        }
        let mut parts: Interp = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek() {
                None => return self.err("Expected \")\"."),
                Some(')') => {
                    self.pos += 1;
                    break;
                }
                Some('#') if self.peek_at(1) == Some('{') => {
                    if !text.is_empty() {
                        parts.push(InterpPart::Text(std::mem::take(&mut text)));
                    }
                    parts.push(InterpPart::Expr(self.interpolation()?));
                }
                Some('\\') => {
                    self.pos += 1;
                    text.push(self.escape()?);
                }
                // Whitespace only before the `)`.
                Some(c) if c.is_whitespace() => {
                    self.skip_ws_only();
                    if self.peek() != Some(')') {
                        self.pos = save;
                        return Ok(None);
                    }
                }
                // What an unquoted URL can hold. Anything else (`url($picture)`, `url(nth($list, 1))`) makes `url()` an
                // ordinary function call, as in Sass.
                Some(c) if matches!(c, '!' | '#' | '%' | '&' | '*'..='~') || !c.is_ascii() => {
                    text.push(c);
                    self.pos += 1;
                }
                Some(_) => {
                    self.pos = save;
                    return Ok(None);
                }
            }
        }
        let trimmed = text.trim_end().to_string();
        if !trimmed.is_empty() {
            parts.push(InterpPart::Text(trimmed));
        }
        Ok(Some(parts))
    }

    fn skip_ws_only(&mut self) {
        while self.peek().is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// Raw `( ... )` text including the parentheses, used after interpolated function names.
    fn raw_parens(&mut self) -> Result<Interp> {
        self.expect('(')?;
        let mut inner = self.raw_until(&[')'])?;
        self.expect(')')?;
        inner.insert(0, InterpPart::Text("(".into()));
        inner.push(InterpPart::Text(")".into()));
        Ok(inner)
    }
}

/// Concatenates the literal text of an interpolation (used where interpolation isn't allowed).
/// Splits a trailing `!important` off raw (unparsed) declaration text, as in `--x: 1 !important`.
fn take_important(raw: &mut Interp) -> bool {
    let Some(InterpPart::Text(text)) = raw.last_mut() else { return false };
    let trimmed = text.trim_end();
    let Some(head) = trimmed.strip_suffix("important").or_else(|| trimmed.strip_suffix("IMPORTANT")) else {
        return false;
    };
    let Some(head) = head.trim_end().strip_suffix('!') else { return false };
    *text = head.trim_end().to_string();
    if text.is_empty() {
        raw.pop();
    }
    true
}

fn interp_text(parts: &Interp) -> String {
    parts
        .iter()
        .map(|p| match p {
            InterpPart::Text(t) => t.as_str(),
            InterpPart::Expr(_) => "",
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Vec<Stmt> {
        parse(src, "test.scss".into()).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn distinguishes_rules_and_declarations() {
        let stmts = parse_ok("a:hover { color: red; font: { family: x; } b { c: d } }");
        let StmtKind::Rule { body, .. } = &stmts[0].kind else { panic!() };
        assert!(matches!(body[0].kind, StmtKind::Decl { .. }));
        assert!(matches!(&body[1].kind, StmtKind::Decl { value: None, children, .. } if children.len() == 1));
        assert!(matches!(body[2].kind, StmtKind::Rule { .. }));
    }

    #[test]
    fn parses_luau_values() {
        let stmts = parse_ok(
            r#"TextLabel { FontFace: Font.new "rbxasset://fonts/families/SourceSansPro.json"; C: Color3.fromRGB(1, 2, 3); X: Enum.Font.Gotham }"#,
        );
        let StmtKind::Rule { body, .. } = &stmts[0].kind else { panic!() };
        assert!(
            matches!(&body[0].kind, StmtKind::Decl { value: Some(Expr::List { items, .. }), .. } if items.len() == 2)
        );
        assert!(
            matches!(&body[1].kind, StmtKind::Decl { value: Some(Expr::Call { name, .. }), .. } if name == "Color3.fromRGB")
        );
        assert!(
            matches!(&body[2].kind, StmtKind::Decl { value: Some(Expr::Value(Value::Str { text, .. })), .. } if text == "Enum.Font.Gotham")
        );
    }

    #[test]
    fn minus_disambiguation() {
        let e = parse_expression("1 -2", "t".into()).unwrap();
        assert!(matches!(e, Expr::List { .. }));
        let e = parse_expression("1 - 2", "t".into()).unwrap();
        assert!(matches!(e, Expr::Binary { op: BinOp::Sub, .. }));
        let e = parse_expression("10px-5px", "t".into()).unwrap();
        assert!(matches!(e, Expr::Binary { op: BinOp::Sub, .. }));
        let e = parse_expression("a-b", "t".into()).unwrap();
        assert!(matches!(e, Expr::Value(Value::Str { .. })));
    }

    #[test]
    fn control_flow_and_mixins() {
        parse_ok(
            r#"
            @use "sass:math" as m;
            $map: (a: 1, b: (c: 2));
            @mixin m($a, $b: 2, $rest...) { x: $a; @content(1); }
            @function f($x) { @if $x > 1 { @return $x; } @else if $x == 0 { @return 1 } @else { @return -$x; } }
            @each $k, $v in $map { .#{$k} { v: $v } }
            @for $i from 1 through 3 { .w-#{$i} { width: $i * 10px } }
            .a { @include m(1, $b: 3) using ($x) { y: $x } @extend %p !optional; }
            "#,
        );
    }

    #[test]
    fn url_and_comments() {
        let stmts = parse_ok("a { b: url(http://x.com/a.png); // c\n d: 1 /* e */ }");
        let StmtKind::Rule { body, .. } = &stmts[0].kind else { panic!() };
        assert_eq!(body.len(), 2);
    }
}
