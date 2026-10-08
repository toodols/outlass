//! Evaluates the SCSS syntax tree into flat style rules.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::ast::*;
use crate::builtins;
use crate::diag::{Diagnostics, Error, Result, Span};
use crate::fs::{FileSystem, OsFs};
use crate::indented;
use crate::parser::{self, normalize};
use crate::query::{self, Query};
use crate::selector::{self, SelectorList, Simple};
use crate::value::{ListSep, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    Scss,
    Sass,
}

impl Syntax {
    pub fn from_path(path: &Path) -> Syntax {
        match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("sass") => Syntax::Sass,
            _ => Syntax::Scss,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Extra directories searched by `@use`/`@forward`/`@import`.
    pub load_paths: Vec<PathBuf>,
    /// Global variables set before compilation (`--define`).
    pub defines: Vec<(String, Value)>,
    /// Where stylesheets are read from.
    pub fs: Rc<dyn FileSystem>,
}

impl Default for Options {
    fn default() -> Self {
        Options { load_paths: Vec::new(), defines: Vec::new(), fs: Rc::new(OsFs) }
    }
}

#[derive(Clone, Debug)]
pub struct OutDecl {
    pub name: String,
    pub value: Value,
    pub important: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub name: String,
    pub value: Value,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct OutRule {
    pub selector: SelectorList,
    /// The module this rule's CSS was generated in, for `@extend` scoping (see `Evaluator::finish`).
    pub owner: usize,
    pub decls: Vec<OutDecl>,
    /// Custom properties (`--Name: value`) → StyleRule attributes (design tokens).
    pub tokens: Vec<Token>,
    /// The `@layer` the rule is in, outermost first; empty when it isn't in one.
    pub layer: Vec<String>,
    /// The theme the rule is in: `@media (prefers-color-scheme: dark)` → `dark`.
    pub theme: Option<String>,
    /// Index of the enclosing query container rule, if any.
    pub parent: Option<usize>,
    /// True for Roblox `@Query` container rules.
    pub query: bool,
    /// For query containers: the alternatives (any may match), already combined with enclosing queries.
    pub queries: Vec<QueryRef>,
    pub span: Span,
}

/// A query that rules inside `@media` / `@container` / `@Name { }` depend on.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryRef {
    /// Parsed conditions (built-in or custom StyleQuery).
    Known(Query),
    /// `@Name { }` for a query outlass doesn't know: a StyleQuery defined elsewhere by the author.
    Named(String),
}

impl QueryRef {
    pub fn name(&self) -> String {
        match self {
            QueryRef::Known(q) => q.name(),
            QueryRef::Named(n) => n.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub rules: Vec<OutRule>,
    /// Stylesheet-level tokens (from `:root` or top-level custom properties).
    pub tokens: Vec<Token>,
    /// Every `@layer` path, in the order the layers were first named.
    pub layers: Vec<Vec<String>>,
    /// `@font-face` family names and the font assets they stand for.
    pub font_faces: Vec<(String, String)>,
    /// Themes, in the order they first appear, with the tokens each one sets.
    pub themes: Vec<(String, Vec<Token>)>,
    /// Every file read during compilation (for `--watch`).
    pub files: Vec<PathBuf>,
}

// ----- environments -----

type EnvRef = Rc<RefCell<Env>>;

/// Evaluated call arguments: the positional values, then the named ones.
type EvaluatedArgs = (Vec<Value>, Vec<(String, Value)>);

#[derive(Default)]
struct Env {
    vars: HashMap<String, Value>,
    mixins: HashMap<String, Rc<Callable>>,
    functions: HashMap<String, Rc<Callable>>,
    parent: Option<EnvRef>,
    /// Control-flow scopes: assignments to existing outer variables go through them.
    transparent: bool,
    /// Present on module roots only.
    module: Option<Rc<ModuleInfo>>,
}

#[derive(Clone)]
enum Namespace {
    Builtin(&'static str),
    Module(EnvRef),
}

#[derive(Default)]
struct ModuleInfo {
    uses: RefCell<HashMap<String, Namespace>>,
    /// `@use ... as *` and `@import`-less globals.
    stars: RefCell<Vec<EnvRef>>,
    /// `@forward` targets with optional prefix.
    forwards: RefCell<Vec<(EnvRef, Option<String>)>>,
    /// `@use ... with (...)` configuration applied to `!default` variables.
    config: RefCell<HashMap<String, Value>>,
}

struct Callable {
    name: String,
    params: Params,
    body: Rc<Vec<Stmt>>,
    env: EnvRef,
}

struct ContentClosure {
    block: Rc<ContentBlock>,
    env: EnvRef,
    outer: Option<Rc<ContentClosure>>,
}

enum Flow {
    Normal,
    Return(Value),
}

fn new_env(parent: Option<EnvRef>, transparent: bool) -> EnvRef {
    Rc::new(RefCell::new(Env { parent, transparent, ..Default::default() }))
}

fn module_root(env: &EnvRef) -> EnvRef {
    let mut cur = env.clone();
    loop {
        let parent = cur.borrow().parent.clone();
        match parent {
            Some(p) => cur = p,
            None => return cur,
        }
    }
}

const BUILTIN_MODULES: &[&str] = &["math", "color", "list", "map", "string", "meta", "selector"];

struct Extend {
    target: Simple,
    extender: SelectorList,
    optional: bool,
    /// The module the `@extend` ran in; it reaches that module and its dependencies, nothing else.
    owner: usize,
    span: Span,
}

pub struct Evaluator<'a> {
    opts: &'a Options,
    diag: &'a mut Diagnostics,
    sheet: Sheet,
    parsed: HashMap<PathBuf, Rc<Vec<Stmt>>>,
    modules: HashMap<PathBuf, EnvRef>,
    loading: Vec<PathBuf>,
    extends: Vec<Extend>,
    /// Every module root, in load order; a module is named by its index here.
    module_roots: Vec<EnvRef>,
    /// The module whose CSS is being generated (`@import` and `@include` don't change it).
    current_module: usize,
    /// Current (resolved) selector context.
    selector: Option<SelectorList>,
    /// Rule receiving declarations.
    current_rule: Option<usize>,
    /// Enclosing query container.
    container: Option<usize>,
    /// The `@layer` being evaluated, outermost first.
    layer: Vec<String>,
    /// The theme being evaluated (`@media (prefers-color-scheme: dark)`).
    theme: Option<String>,
    content: Option<Rc<ContentClosure>>,
    /// Prefix for nested properties (`font: { family: x }` → `font-family`).
    decl_prefix: Option<String>,
    in_function: bool,
    depth: usize,
    /// Directory of the file being evaluated (for relative imports).
    dirs: Vec<PathBuf>,
}

pub fn compile_file(path: &Path, opts: &Options, diag: &mut Diagnostics) -> Result<Sheet> {
    let source = opts.fs.read_to_string(path).map_err(|e| Error::new(format!("can't read {}: {e}", path.display())))?;
    let syntax = Syntax::from_path(path);
    compile_source(&source, path, syntax, opts, diag)
}

pub fn compile_source(
    source: &str,
    path: &Path,
    syntax: Syntax,
    opts: &Options,
    diag: &mut Diagnostics,
) -> Result<Sheet> {
    let mut ev = Evaluator::new(opts, diag, path.parent().map(Path::to_path_buf).unwrap_or_default());
    let canonical = opts.fs.canonicalize(path);
    ev.sheet.files.push(canonical.clone());
    let stmts = Rc::new(parse_source(source, path, syntax)?);
    let root = new_module_root();
    ev.module_roots.push(root.clone());
    for (name, value) in &opts.defines {
        root.borrow_mut().vars.insert(normalize(name), value.clone());
    }
    ev.loading.push(canonical);
    ev.exec_block(&stmts, &root)?;
    ev.finish()
}

/// Evaluates a standalone SassScript expression (used for `--define` values).
pub fn evaluate_expression(source: &str) -> Result<Value> {
    let expr = parser::parse_expression(source, "<define>".into())?;
    let opts = Options::default();
    let mut diag = Diagnostics::default();
    let mut ev = Evaluator::new(&opts, &mut diag, PathBuf::new());
    ev.eval(&expr, &new_module_root())
}

fn new_module_root() -> EnvRef {
    let root = new_env(None, false);
    root.borrow_mut().module = Some(Rc::new(ModuleInfo::default()));
    root
}

impl Sheet {
    /// Appends another sheet's rules and tokens (for `--merge`).
    pub fn append(&mut self, mut other: Sheet) {
        let offset = self.rules.len();
        for rule in &mut other.rules {
            rule.parent = rule.parent.map(|p| p + offset);
        }
        self.rules.append(&mut other.rules);
        self.tokens.append(&mut other.tokens);
        self.font_faces.append(&mut other.font_faces);
        for (name, mut tokens) in other.themes {
            match self.themes.iter_mut().find(|(n, _)| *n == name) {
                Some((_, existing)) => existing.append(&mut tokens),
                None => self.themes.push((name, tokens)),
            }
        }
        for layer in other.layers {
            if !self.layers.contains(&layer) {
                self.layers.push(layer);
            }
        }
        self.files.append(&mut other.files);
    }
}

fn parse_source(source: &str, path: &Path, syntax: Syntax) -> Result<Vec<Stmt>> {
    let file: Rc<str> = path.display().to_string().into();
    let scss;
    let text = match syntax {
        Syntax::Scss => source,
        Syntax::Sass => {
            scss =
                indented::to_scss(source).map_err(|(line, msg)| Error::at(msg, &Span::new(file.clone(), line, 1)))?;
            &scss
        }
    };
    parser::parse(text, file)
}

impl<'a> Evaluator<'a> {
    fn new(opts: &'a Options, diag: &'a mut Diagnostics, dir: PathBuf) -> Self {
        Evaluator {
            opts,
            diag,
            sheet: Sheet::default(),
            parsed: HashMap::new(),
            modules: HashMap::new(),
            loading: Vec::new(),
            extends: Vec::new(),
            module_roots: Vec::new(),
            current_module: 0,
            selector: None,
            current_rule: None,
            container: None,
            layer: Vec::new(),
            theme: None,
            content: None,
            decl_prefix: None,
            in_function: false,
            depth: 0,
            dirs: vec![dir],
        }
    }

    // ----- statements -----

    fn exec_block(&mut self, stmts: &[Stmt], env: &EnvRef) -> Result<Flow> {
        for stmt in stmts {
            match self.exec(stmt, env).map_err(|e| e.or_at(&stmt.span))? {
                Flow::Normal => {}
                ret => return Ok(ret),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, stmt: &Stmt, env: &EnvRef) -> Result<Flow> {
        let span = &stmt.span;
        match &stmt.kind {
            StmtKind::VarDecl { ns, name, value, default, global } => {
                self.assign(ns.as_deref(), name, value, *default, *global, env, span)?;
            }
            StmtKind::Decl { name, value, children, important } => {
                self.declaration(name, value.as_ref(), children, *important, env, span)?;
            }
            StmtKind::Rule { selector, body } => {
                if self.in_function {
                    return Err(Error::at("Style rules aren't allowed in functions.", span));
                }
                let text = self.interp(selector, env)?;
                let parsed =
                    selector::parse(&text).map_err(|e| Error::at(format!("{e} in selector \"{text}\""), span))?;
                let resolved = parsed.resolve(self.selector.as_ref()).map_err(|e| Error::at(e, span))?;
                self.style_rule(resolved, body, env, span)?;
            }
            StmtKind::Mixin { name, params, body } | StmtKind::Function { name, params, body } => {
                let callable = Rc::new(Callable {
                    name: name.clone(),
                    params: params.clone(),
                    body: Rc::new(body.clone()),
                    env: env.clone(),
                });
                let mut e = env.borrow_mut();
                if matches!(stmt.kind, StmtKind::Mixin { .. }) {
                    e.mixins.insert(name.clone(), callable);
                } else {
                    e.functions.insert(name.clone(), callable);
                }
            }
            StmtKind::Include { ns, name, args, content } => {
                let mixin = self.find_mixin(ns.as_deref(), name, env).ok_or_else(|| {
                    Error::at(format!("Undefined mixin \"{}\".", qualified(ns.as_deref(), name)), span)
                })?;
                let (positional, named) = self.eval_args(args, env)?;
                let closure = content.as_ref().map(|block| {
                    Rc::new(ContentClosure {
                        block: Rc::new(block.clone()),
                        env: env.clone(),
                        outer: self.content.clone(),
                    })
                });
                let call_env = new_env(Some(mixin.env.clone()), false);
                self.bind(&mixin.params, positional, named, &call_env, &mixin.name, span)?;
                let saved = std::mem::replace(&mut self.content, closure);
                self.enter(span, &mixin.name)?;
                let result = self.exec_block(&mixin.body, &call_env);
                self.depth -= 1;
                self.content = saved;
                result.map_err(|e| trace(e, format!("@include {} ({span})", mixin.name)))?;
            }
            StmtKind::Content { args } => {
                if let Some(closure) = self.content.clone() {
                    let (positional, named) = self.eval_args(args, env)?;
                    let content_env = new_env(Some(closure.env.clone()), false);
                    self.bind(&closure.block.params, positional, named, &content_env, "@content", span)?;
                    let saved = std::mem::replace(&mut self.content, closure.outer.clone());
                    let result = self.exec_block(&closure.block.body, &content_env);
                    self.content = saved;
                    result?;
                }
            }
            StmtKind::Return(expr) => {
                if !self.in_function {
                    return Err(Error::at("@return may only be used within a function.", span));
                }
                return Ok(Flow::Return(self.eval(expr, env)?));
            }
            StmtKind::If { clauses, else_body } => {
                for (cond, body) in clauses {
                    if self.eval(cond, env)?.is_truthy() {
                        return self.exec_block(body, &new_env(Some(env.clone()), true));
                    }
                }
                if let Some(body) = else_body {
                    return self.exec_block(body, &new_env(Some(env.clone()), true));
                }
            }
            StmtKind::Each { vars, list, body } => {
                let list = self.eval(list, env)?;
                for item in list.as_list() {
                    let scope = new_env(Some(env.clone()), true);
                    if vars.len() == 1 {
                        scope.borrow_mut().vars.insert(vars[0].clone(), item);
                    } else {
                        let parts = item.as_list();
                        for (i, var) in vars.iter().enumerate() {
                            scope.borrow_mut().vars.insert(var.clone(), parts.get(i).cloned().unwrap_or(Value::Null));
                        }
                    }
                    if let Flow::Return(v) = self.exec_block(body, &scope)? {
                        return Ok(Flow::Return(v));
                    }
                }
            }
            StmtKind::For { var, from, to, inclusive, body } => {
                let from_v = self.eval(from, env)?;
                let to_v = self.eval(to, env)?;
                let (Value::Number(a), Value::Number(b)) = (&from_v, &to_v) else {
                    return Err(Error::at("@for bounds must be numbers.", span));
                };
                let start = a.value.round() as i64;
                let end = b.value_in(a.simple_unit().unwrap_or("")).unwrap_or(b.value).round() as i64;
                let step = if end >= start { 1 } else { -1 };
                let stop = if *inclusive { end + step } else { end };
                let mut i = start;
                while i != stop {
                    let scope = new_env(Some(env.clone()), true);
                    let mut n = a.clone();
                    n.value = i as f64;
                    n.slash = None;
                    scope.borrow_mut().vars.insert(var.clone(), Value::Number(n));
                    if let Flow::Return(v) = self.exec_block(body, &scope)? {
                        return Ok(Flow::Return(v));
                    }
                    i += step;
                }
            }
            StmtKind::While { cond, body } => {
                let mut guard = 0u32;
                while self.eval(cond, env)?.is_truthy() {
                    guard += 1;
                    if guard > 100_000 {
                        return Err(Error::at(
                            "@while loop ran more than 100000 times; is the condition ever false?",
                            span,
                        ));
                    }
                    if let Flow::Return(v) = self.exec_block(body, &new_env(Some(env.clone()), true))? {
                        return Ok(Flow::Return(v));
                    }
                }
            }
            StmtKind::Extend { selector: sel, optional } => {
                let Some(extender) = self.selector.clone() else {
                    return Err(Error::at("@extend may only be used within style rules.", span));
                };
                let text = self.interp(sel, env)?;
                for target_text in text.split(',') {
                    let parsed = selector::parse(target_text.trim()).map_err(|e| Error::at(e, span))?;
                    let target = match parsed.0.as_slice() {
                        [complex] => match complex.as_slice() {
                            [selector::Part::Compound(c)] if c.len() == 1 => c[0].clone(),
                            _ => {
                                return Err(Error::at(
                                    format!(
                                        "Can't extend complex or compound selector \"{}\"; extend a single class or placeholder.",
                                        target_text.trim()
                                    ),
                                    span,
                                ));
                            }
                        },
                        _ => unreachable!(),
                    };
                    self.extends.push(Extend {
                        target,
                        extender: extender.clone(),
                        optional: *optional,
                        owner: self.current_module,
                        span: span.clone(),
                    });
                }
            }
            StmtKind::Use { url, namespace, with } => self.use_rule(url, namespace.as_deref(), with, env, span)?,
            StmtKind::Forward { url, prefix, with } => {
                let config = self.eval_config(with, env)?;
                let module = self.load_module(url, config, span)?;
                let root = module_root(env);
                let info = root.borrow().module.clone().unwrap();
                info.forwards.borrow_mut().push((module, prefix.clone()));
            }
            StmtKind::Import { urls } => {
                for url in urls {
                    let lower = url.to_ascii_lowercase();
                    if lower.ends_with(".css")
                        || lower.starts_with("http:")
                        || lower.starts_with("https:")
                        || lower.starts_with("//")
                        || lower.starts_with("url(")
                        || url.contains(' ')
                    {
                        self.diag.warn(
                            format!("plain CSS @import \"{url}\" can't be compiled into a StyleSheet (ignored)"),
                            Some(span),
                        );
                        continue;
                    }
                    let path = self
                        .resolve(url)
                        .ok_or_else(|| Error::at(format!("Can't find stylesheet to import: \"{url}\"."), span))?;
                    if self.loading.contains(&path) {
                        return Err(Error::at(format!("This file is already being loaded: {}", path.display()), span));
                    }
                    let stmts = self.parse_file(&path, span)?;
                    self.loading.push(path.clone());
                    self.dirs.push(path.parent().map(Path::to_path_buf).unwrap_or_default());
                    let result = self.exec_block(&stmts, env);
                    self.dirs.pop();
                    self.loading.pop();
                    result?;
                }
            }
            StmtKind::Debug(expr) => {
                let v = self.eval(expr, env)?;
                let text = match &v {
                    Value::Str { text, .. } => text.clone(),
                    other => other.inspect(),
                };
                self.diag.debug(text, Some(span));
            }
            StmtKind::Warn(expr) => {
                let v = self.eval(expr, env)?;
                let text = v.to_interp().unwrap_or_else(|_| v.inspect());
                self.diag.warn(text, Some(span));
            }
            StmtKind::Error(expr) => {
                let v = self.eval(expr, env)?;
                let text = v.to_interp().unwrap_or_else(|_| v.inspect());
                return Err(Error::at(text, span));
            }
            StmtKind::AtRoot { selector: sel, body } => {
                let saved_sel = self.selector.clone();
                let saved_rule = self.current_rule;
                match sel {
                    Some(sel) => {
                        let text = self.interp(sel, env)?;
                        let parsed = selector::parse(&text).map_err(|e| Error::at(e, span))?;
                        let resolved = if parsed.contains_parent() {
                            parsed.resolve(self.selector.as_ref()).map_err(|e| Error::at(e, span))?
                        } else {
                            parsed
                        };
                        self.selector = None;
                        self.current_rule = None;
                        let r = self.style_rule(resolved, body, env, span);
                        self.selector = saved_sel;
                        self.current_rule = saved_rule;
                        r?;
                    }
                    None => {
                        self.selector = None;
                        self.current_rule = None;
                        let r = self.exec_block(body, &new_env(Some(env.clone()), false));
                        self.selector = saved_sel;
                        self.current_rule = saved_rule;
                        r?;
                    }
                }
            }
            StmtKind::AtRule { name, params, body } => self.at_rule(name, params, body.as_deref(), env, span)?,
        }
        Ok(Flow::Normal)
    }

    fn enter(&mut self, span: &Span, name: &str) -> Result<()> {
        self.depth += 1;
        if self.depth > 200 {
            self.depth -= 1;
            return Err(Error::at(format!("Stack overflow: \"{name}\" recursed too deeply."), span));
        }
        Ok(())
    }

    fn style_rule(&mut self, selector: SelectorList, body: &[Stmt], env: &EnvRef, span: &Span) -> Result<()> {
        let idx = self.sheet.rules.len();
        self.sheet.rules.push(OutRule {
            selector: selector.clone(),
            owner: self.current_module,
            decls: Vec::new(),
            tokens: Vec::new(),
            layer: self.layer.clone(),
            theme: self.theme.clone(),
            parent: self.container,
            query: false,
            queries: Vec::new(),
            span: span.clone(),
        });
        let saved_sel = self.selector.replace(selector);
        let saved_rule = self.current_rule.replace(idx);
        let saved_prefix = self.decl_prefix.take();
        let result = self.exec_block(body, &new_env(Some(env.clone()), false));
        self.selector = saved_sel;
        self.current_rule = saved_rule;
        self.decl_prefix = saved_prefix;
        result.map(|_| ())
    }

    fn at_rule(&mut self, name: &str, params: &Interp, body: Option<&[Stmt]>, env: &EnvRef, span: &Span) -> Result<()> {
        let params_text = self.interp(params, env)?;
        let parsed = if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            // `@PreferredInputTouch { }`: a built-in query, or a StyleQuery the author defined.
            Ok(vec![query::builtin_by_name(name).map_or_else(|| QueryRef::Named(name.to_string()), QueryRef::Known)])
        } else if name == "media"
            && let Some(theme) = color_scheme(&params_text)
        {
            let Some(body) = body else { return Ok(()) };
            let saved = self.theme.replace(theme);
            let result = self.exec_block(body, &new_env(Some(env.clone()), false));
            self.theme = saved;
            return result.map(|_| ());
        } else if name == "layer" {
            return self.layer_rule(&params_text, body, env, span);
        } else if name == "font-face" {
            return self.font_face(body, env, span);
        } else if name == "media" || name == "container" {
            let parsed =
                if name == "media" { query::parse_media(&params_text) } else { query::parse_container(&params_text) };
            parsed.map(|alts| alts.into_iter().map(QueryRef::Known).collect())
        } else {
            if name != "charset" {
                self.diag.warn(format!("@{name} has no Roblox equivalent (ignored)"), Some(span));
            }
            return Ok(());
        };
        let alternatives = match parsed {
            Ok(alts) => alts,
            Err(e) => {
                self.diag.warn(format!("@{name} {params_text}: {e}; its contents are ignored"), Some(span));
                return Ok(());
            }
        };
        let Some(body) = body else { return Ok(()) };
        if alternatives.is_empty() {
            return Ok(()); // e.g. `@media print`: never applies in a game
        }
        if self.in_function {
            return Err(Error::at("Query rules aren't allowed in functions.", span));
        }
        let alternatives = self.combine_with_enclosing_query(alternatives, span);
        let display = alternatives.iter().map(QueryRef::name).collect::<Vec<_>>().join(", ");
        let container = self.sheet.rules.len();
        self.sheet.rules.push(OutRule {
            selector: SelectorList(vec![vec![selector::Part::Compound(vec![Simple::Query(display)])]]),
            owner: self.current_module,
            decls: Vec::new(),
            tokens: Vec::new(),
            layer: Vec::new(),
            theme: None,
            parent: self.container,
            query: true,
            queries: alternatives,
            span: span.clone(),
        });
        let saved_container = self.container.replace(container);
        let saved_rule = self.current_rule;
        // Declarations directly inside the query apply to the enclosing selector (like @media bubbling).
        if let Some(sel) = self.selector.clone() {
            self.current_rule = Some(self.sheet.rules.len());
            self.sheet.rules.push(OutRule {
                selector: sel,
                owner: self.current_module,
                decls: Vec::new(),
                tokens: Vec::new(),
                layer: self.layer.clone(),
                theme: self.theme.clone(),
                parent: Some(container),
                query: false,
                queries: Vec::new(),
                span: span.clone(),
            });
        } else {
            self.current_rule = None;
        }
        let result = self.exec_block(body, &new_env(Some(env.clone()), false));
        self.container = saved_container;
        self.current_rule = saved_rule;
        result.map(|_| ())
    }

    /// `@layer a, b.c;` fixes the order of layers; `@layer a { ... }` (or an anonymous
    /// `@layer { ... }`) puts the rules inside in a layer, nested in the enclosing one.
    fn layer_rule(&mut self, params: &str, body: Option<&[Stmt]>, env: &EnvRef, span: &Span) -> Result<()> {
        let names: Vec<&str> = params.split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
        let Some(body) = body else {
            for name in names {
                self.declare_layer(name, span)?;
            }
            return Ok(());
        };
        let path = match names.as_slice() {
            [name] => self.declare_layer(name, span)?,
            // An anonymous layer can't be named again, so it gets a name no other layer can have.
            [] => {
                let mut path = self.layer.clone();
                path.push(format!("<anonymous {span}>"));
                self.sheet.layers.push(path.clone());
                path
            }
            _ => return Err(Error::at("@layer with a block takes a single layer name", span)),
        };
        let saved_layer = std::mem::replace(&mut self.layer, path);
        let saved_rule = self.current_rule;
        // Declarations directly inside apply to the enclosing selector, in the layer.
        if let Some(sel) = self.selector.clone() {
            self.current_rule = Some(self.sheet.rules.len());
            self.sheet.rules.push(OutRule {
                selector: sel,
                owner: self.current_module,
                decls: Vec::new(),
                tokens: Vec::new(),
                layer: self.layer.clone(),
                theme: self.theme.clone(),
                parent: self.container,
                query: false,
                queries: Vec::new(),
                span: span.clone(),
            });
        }
        let result = self.exec_block(body, &new_env(Some(env.clone()), false));
        self.layer = saved_layer;
        self.current_rule = saved_rule;
        result.map(|_| ())
    }

    /// `@font-face { font-family: Brand; src: url("rbxassetid://123"); }` names a font asset, so
    /// `font-family: Brand` uses it.
    fn font_face(&mut self, body: Option<&[Stmt]>, env: &EnvRef, span: &Span) -> Result<()> {
        let Some(body) = body else { return Ok(()) };
        let idx = self.sheet.rules.len();
        self.sheet.rules.push(OutRule {
            selector: SelectorList::default(),
            owner: self.current_module,
            decls: Vec::new(),
            tokens: Vec::new(),
            layer: Vec::new(),
            theme: None,
            parent: None,
            query: false,
            queries: Vec::new(),
            span: span.clone(),
        });
        let saved_rule = self.current_rule.replace(idx);
        let result = self.exec_block(body, &new_env(Some(env.clone()), false));
        self.current_rule = saved_rule;
        let face = self.sheet.rules.remove(idx);
        result?;
        let value = |name: &str| face.decls.iter().rev().find(|d| d.name == name).map(|d| d.value.clone());
        let family = value("font-family").and_then(|v| v.as_str().map(str::to_string));
        // The first source that's a url() or a string.
        let src = value("src").and_then(|v| {
            let items = match v {
                Value::List { items, .. } => items,
                other => vec![other],
            };
            items.into_iter().find_map(|item| match item {
                Value::Call { name, args } if name == "url" => {
                    args.first().and_then(|a| a.as_str().map(str::to_string))
                }
                Value::Str { text, quoted: true } => Some(text),
                _ => None,
            })
        });
        match (family, src) {
            (Some(family), Some(src)) => self.sheet.font_faces.push((family, src)),
            _ => self.diag.warn("@font-face needs a font-family and a url() src (ignored)", Some(span)),
        }
        Ok(())
    }

    /// Registers `name` (`a` or `a.b`) inside the current layer, with any parents it implies, and
    /// returns its full path.
    fn declare_layer(&mut self, name: &str, span: &Span) -> Result<Vec<String>> {
        let mut path = self.layer.clone();
        for part in name.split('.') {
            if part.is_empty() || !part.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                return Err(Error::at(format!("`{name}` isn't a valid layer name"), span));
            }
            path.push(part.to_string());
            if !self.sheet.layers.contains(&path) {
                self.sheet.layers.push(path.clone());
            }
        }
        Ok(path)
    }

    /// A query nested in another applies only when both do: combine them into one StyleQuery.
    fn combine_with_enclosing_query(&mut self, inner: Vec<QueryRef>, span: &Span) -> Vec<QueryRef> {
        let Some(outer) = self.container.map(|c| self.sheet.rules[c].queries.clone()) else { return inner };
        let mut combined = Vec::new();
        for o in &outer {
            for i in &inner {
                match (o, i) {
                    (QueryRef::Known(o), QueryRef::Known(i)) if o.and(i).is_some() => {
                        combined.push(QueryRef::Known(o.and(i).unwrap()));
                    }
                    _ => {
                        self.diag.warn(
                            format!(
                                "@{} inside @{} can't be combined into one Roblox StyleQuery (different containers, \
                                 or a hand-made query); only the inner query applies",
                                i.name(),
                                o.name()
                            ),
                            Some(span),
                        );
                        combined.push(i.clone());
                    }
                }
            }
        }
        combined.dedup();
        combined
    }

    fn declaration(
        &mut self,
        name: &Interp,
        value: Option<&Expr>,
        children: &[Stmt],
        important: bool,
        env: &EnvRef,
        span: &Span,
    ) -> Result<()> {
        let mut name = self.interp(name, env)?;
        if let Some(prefix) = &self.decl_prefix {
            name = format!("{prefix}-{name}");
        }
        if let Some(value) = value {
            let v = self.eval(value, env)?;
            let is_empty = v.is_null() || matches!(&v, Value::List { items, bracketed: false, .. } if items.is_empty());
            if !is_empty {
                if let Some(token) = name.strip_prefix("--") {
                    let token = Token { name: token.to_string(), value: v, span: span.clone() };
                    match self.current_rule {
                        Some(idx) => self.sheet.rules[idx].tokens.push(token),
                        None if self.in_function => {
                            return Err(Error::at("Declarations aren't allowed in functions.", span));
                        }
                        None => self.sheet.tokens.push(token),
                    }
                } else {
                    let Some(idx) = self.current_rule else {
                        return Err(Error::at("Declarations may only be used within style rules.", span));
                    };
                    self.sheet.rules[idx].decls.push(OutDecl {
                        name: name.clone(),
                        value: v,
                        important,
                        span: span.clone(),
                    });
                }
            }
        }
        if !children.is_empty() {
            let saved = self.decl_prefix.replace(name);
            let r = self.exec_block(children, env);
            self.decl_prefix = saved;
            r?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn assign(
        &mut self,
        ns: Option<&str>,
        name: &str,
        value: &Expr,
        default: bool,
        global: bool,
        env: &EnvRef,
        span: &Span,
    ) -> Result<()> {
        if let Some(ns) = ns {
            let module = match self.namespace(ns, env) {
                Some(Namespace::Module(m)) => m,
                _ => return Err(Error::at(format!("There is no module with the namespace \"{ns}\"."), span)),
            };
            if !module.borrow().vars.contains_key(name) {
                return Err(Error::at(format!("Undefined variable \"{ns}.${name}\"."), span));
            }
            let v = self.eval(value, env)?;
            module.borrow_mut().vars.insert(name.to_string(), v);
            return Ok(());
        }
        let root = module_root(env);
        let at_root = global || Rc::ptr_eq(&root, env);
        if default {
            // Module configuration (`@use ... with`) wins over `!default` values.
            if at_root {
                let config = root.borrow().module.as_ref().and_then(|m| m.config.borrow().get(name).cloned());
                if let Some(v) = config {
                    root.borrow_mut().vars.insert(name.to_string(), v);
                    return Ok(());
                }
            }
            let existing = if global { root.borrow().vars.get(name).cloned() } else { self.lookup_var(name, env) };
            if existing.is_some_and(|v| !v.is_null()) {
                return Ok(());
            }
        }
        let v = self.eval(value, env)?;
        if global {
            root.borrow_mut().vars.insert(name.to_string(), v);
            return Ok(());
        }
        // Assign to an existing variable reachable through transparent (control-flow) scopes.
        let mut cur = Some(env.clone());
        while let Some(e) = cur {
            if e.borrow().vars.contains_key(name) {
                e.borrow_mut().vars.insert(name.to_string(), v);
                return Ok(());
            }
            if !e.borrow().transparent {
                break;
            }
            cur = e.borrow().parent.clone();
        }
        env.borrow_mut().vars.insert(name.to_string(), v);
        Ok(())
    }

    // ----- modules -----

    fn use_rule(
        &mut self,
        url: &str,
        namespace: Option<&str>,
        with: &[(String, Expr)],
        env: &EnvRef,
        span: &Span,
    ) -> Result<()> {
        let root = module_root(env);
        let info = root.borrow().module.clone().unwrap();
        if let Some(module) = url.strip_prefix("sass:") {
            let Some(builtin) = BUILTIN_MODULES.iter().find(|m| **m == module) else {
                return Err(Error::at(format!("Unknown built-in module \"{url}\"."), span));
            };
            let ns = namespace.unwrap_or(builtin);
            if ns != "*" {
                info.uses.borrow_mut().insert(ns.to_string(), Namespace::Builtin(builtin));
            }
            return Ok(());
        }
        let config = self.eval_config(with, env)?;
        let module = self.load_module(url, config, span)?;
        match namespace {
            Some("*") => info.stars.borrow_mut().push(module),
            Some(ns) => {
                info.uses.borrow_mut().insert(ns.to_string(), Namespace::Module(module));
            }
            None => {
                let stem = url.rsplit('/').next().unwrap_or(url);
                let stem = stem.split('.').next().unwrap_or(stem).trim_start_matches('_');
                info.uses.borrow_mut().insert(normalize(stem), Namespace::Module(module));
            }
        }
        Ok(())
    }

    fn eval_config(&mut self, with: &[(String, Expr)], env: &EnvRef) -> Result<HashMap<String, Value>> {
        let mut config = HashMap::new();
        for (name, expr) in with {
            config.insert(name.clone(), self.eval(expr, env)?);
        }
        Ok(config)
    }

    fn load_module(&mut self, url: &str, config: HashMap<String, Value>, span: &Span) -> Result<EnvRef> {
        let path =
            self.resolve(url).ok_or_else(|| Error::at(format!("Can't find stylesheet to import: \"{url}\"."), span))?;
        if let Some(existing) = self.modules.get(&path) {
            if !config.is_empty() {
                return Err(Error::at(
                    format!("{} was already loaded, so it can't be configured using \"with\".", path.display()),
                    span,
                ));
            }
            return Ok(existing.clone());
        }
        if self.loading.contains(&path) {
            return Err(Error::at(format!("Module loop: {} is already being loaded.", path.display()), span));
        }
        let stmts = self.parse_file(&path, span)?;
        let root = new_env(None, false);
        let info = ModuleInfo { config: RefCell::new(config), ..Default::default() };
        root.borrow_mut().module = Some(Rc::new(info));
        self.module_roots.push(root.clone());
        let module_id = self.module_roots.len() - 1;
        self.loading.push(path.clone());
        self.dirs.push(path.parent().map(Path::to_path_buf).unwrap_or_default());
        // Modules are evaluated at the top level, independent of where @use appears.
        let saved = (self.selector.take(), self.current_rule.take(), self.container.take(), self.content.take());
        let saved_module = std::mem::replace(&mut self.current_module, module_id);
        let result = self.exec_block(&stmts, &root);
        self.current_module = saved_module;
        (self.selector, self.current_rule, self.container, self.content) = saved;
        self.dirs.pop();
        self.loading.pop();
        result?;
        self.modules.insert(path, root.clone());
        Ok(root)
    }

    fn parse_file(&mut self, path: &Path, span: &Span) -> Result<Rc<Vec<Stmt>>> {
        if let Some(stmts) = self.parsed.get(path) {
            return Ok(stmts.clone());
        }
        let source = self
            .opts
            .fs
            .read_to_string(path)
            .map_err(|e| Error::at(format!("can't read {}: {e}", path.display()), span))?;
        let syntax = Syntax::from_path(path);
        let stmts = Rc::new(parse_source(&source, path, syntax)?);
        self.sheet.files.push(path.to_path_buf());
        self.parsed.insert(path.to_path_buf(), stmts.clone());
        Ok(stmts)
    }

    fn resolve(&self, url: &str) -> Option<PathBuf> {
        let current = self.dirs.last().cloned().unwrap_or_default();
        let bases = std::iter::once(current).chain(self.opts.load_paths.iter().cloned());
        for base in bases {
            let joined = base.join(url);
            let dir = joined.parent().map(Path::to_path_buf).unwrap_or_default();
            let Some(file) = joined.file_name().and_then(|f| f.to_str()) else { continue };
            let mut candidates = Vec::new();
            let has_ext = ["scss", "sass", "css"].iter().any(|e| file.to_ascii_lowercase().ends_with(&format!(".{e}")));
            if has_ext {
                candidates.push(dir.join(file));
                candidates.push(dir.join(format!("_{file}")));
            } else {
                for ext in ["scss", "sass", "css"] {
                    candidates.push(dir.join(format!("{file}.{ext}")));
                    candidates.push(dir.join(format!("_{file}.{ext}")));
                }
                for ext in ["scss", "sass", "css"] {
                    candidates.push(joined.join(format!("_index.{ext}")));
                    candidates.push(joined.join(format!("index.{ext}")));
                }
            }
            if let Some(found) = candidates.into_iter().find(|c| self.opts.fs.is_file(c)) {
                return Some(self.opts.fs.canonicalize(&found));
            }
        }
        None
    }

    fn namespace(&self, ns: &str, env: &EnvRef) -> Option<Namespace> {
        let root = module_root(env);
        let info = root.borrow().module.clone()?;
        if let Some(found) = info.uses.borrow().get(ns) {
            return Some(found.clone());
        }
        // Built-in modules are always available under their own names.
        BUILTIN_MODULES.iter().find(|m| **m == ns).map(|m| Namespace::Builtin(m))
    }

    // ----- lookups -----

    fn lookup_var(&self, name: &str, env: &EnvRef) -> Option<Value> {
        let mut cur = Some(env.clone());
        let mut last = env.clone();
        while let Some(e) = cur {
            if let Some(v) = e.borrow().vars.get(name) {
                return Some(v.clone());
            }
            last = e.clone();
            cur = e.borrow().parent.clone();
        }
        let info = last.borrow().module.clone()?;
        let stars = info.stars.borrow().clone();
        stars.iter().find_map(|m| module_member(m, name, |e, n| e.vars.get(n).cloned()))
    }

    fn find_callable(&self, ns: Option<&str>, name: &str, env: &EnvRef, mixin: bool) -> Option<Rc<Callable>> {
        let get = |e: &Env, n: &str| if mixin { e.mixins.get(n).cloned() } else { e.functions.get(n).cloned() };
        if let Some(ns) = ns {
            return match self.namespace(ns, env)? {
                Namespace::Module(m) => module_member(&m, name, get),
                Namespace::Builtin(_) => None,
            };
        }
        let mut cur = Some(env.clone());
        let mut last = env.clone();
        while let Some(e) = cur {
            if let Some(c) = get(&e.borrow(), name) {
                return Some(c);
            }
            last = e.clone();
            cur = e.borrow().parent.clone();
        }
        let info = last.borrow().module.clone()?;
        let stars = info.stars.borrow().clone();
        stars.iter().find_map(|m| module_member(m, name, get))
    }

    fn find_mixin(&self, ns: Option<&str>, name: &str, env: &EnvRef) -> Option<Rc<Callable>> {
        self.find_callable(ns, name, env, true)
    }

    // ----- expressions -----

    fn interp(&mut self, parts: &Interp, env: &EnvRef) -> Result<String> {
        let mut s = String::new();
        for part in parts {
            match part {
                InterpPart::Text(t) => s.push_str(t),
                InterpPart::Expr(e) => {
                    let v = self.eval(e, env)?;
                    s.push_str(&v.to_interp().map_err(Error::new)?);
                }
            }
        }
        Ok(s)
    }

    fn eval(&mut self, expr: &Expr, env: &EnvRef) -> Result<Value> {
        Ok(match expr {
            Expr::Value(v) => v.clone(),
            Expr::Str { parts, quoted } => {
                let text = self.interp(parts, env)?;
                Value::Str { text, quoted: *quoted }
            }
            Expr::Var { ns: None, name, span } => {
                self.lookup_var(name, env).ok_or_else(|| Error::at(format!("Undefined variable \"${name}\"."), span))?
            }
            Expr::Var { ns: Some(ns), name, span } => match self.namespace(ns, env) {
                Some(Namespace::Module(m)) => module_member(&m, name, |e, n| e.vars.get(n).cloned())
                    .ok_or_else(|| Error::at(format!("Undefined variable \"{ns}.${name}\"."), span))?,
                Some(Namespace::Builtin(module)) => builtin_variable(module, name)
                    .ok_or_else(|| Error::at(format!("Undefined variable \"{ns}.${name}\"."), span))?,
                None => return Err(Error::at(format!("There is no module with the namespace \"{ns}\"."), span)),
            },
            Expr::List { items, sep, bracketed } => {
                let items = items.iter().map(|i| self.eval(i, env)).collect::<Result<Vec<_>>>()?;
                Value::List { items, sep: *sep, bracketed: *bracketed }
            }
            Expr::Map(pairs) => {
                let mut out: Vec<(Value, Value)> = Vec::new();
                for (k, v) in pairs {
                    let key = self.eval(k, env)?;
                    if out.iter().any(|(existing, _)| existing.sass_eq(&key)) {
                        return Err(Error::new(format!("Duplicate key {} in map.", key.inspect())));
                    }
                    let value = self.eval(v, env)?;
                    out.push((key, value));
                }
                Value::Map(out)
            }
            // Parentheses make a `/` divide: `(20px/2)` is `10px`.
            Expr::Paren(inner) => match self.eval(inner, env)? {
                Value::Number(mut n) => {
                    n.slash = None;
                    Value::Number(n)
                }
                v => v,
            },
            Expr::Parent => match &self.selector {
                Some(sel) => Value::str(sel.to_css()),
                None => Value::Null,
            },
            Expr::Unary { op, expr, span } => {
                let v = self.eval(expr, env)?;
                match (op, v) {
                    (UnaryOp::Not, v) => Value::Bool(!v.is_truthy()),
                    (UnaryOp::Neg, Value::Number(mut n)) => {
                        n.value = -n.value;
                        n.slash = None;
                        Value::Number(n)
                    }
                    (UnaryOp::Plus, Value::Number(n)) => Value::Number(n),
                    (UnaryOp::Neg, v) => Value::str(format!("-{}", v.to_css().map_err(|e| Error::at(e, span))?)),
                    (UnaryOp::Plus, v) => Value::str(format!("+{}", v.to_css().map_err(|e| Error::at(e, span))?)),
                }
            }
            Expr::Binary { op, lhs, rhs, span } => {
                let l = self.eval(lhs, env)?;
                match op {
                    BinOp::And if !l.is_truthy() => return Ok(l),
                    BinOp::And => return self.eval(rhs, env),
                    BinOp::Or if l.is_truthy() => return Ok(l),
                    BinOp::Or => return self.eval(rhs, env),
                    _ => {}
                }
                let r = self.eval(rhs, env)?;
                let value = binary(*op, &l, &r).map_err(|e| Error::at(e, span))?;
                // `20px/1.5` between literal numbers stays a slash, as in Sass (`font`, `background`).
                match (value, &l, &r) {
                    (Value::Number(mut n), Value::Number(a), Value::Number(b))
                        if *op == BinOp::Div && allows_slash(lhs) && allows_slash(rhs) =>
                    {
                        n.slash = Some(Box::new((a.clone(), b.clone())));
                        Value::Number(n)
                    }
                    (value, ..) => value,
                }
            }
            Expr::Call { name, args, span } => self.call(name, args, env, span)?,
        })
    }

    fn eval_args(&mut self, args: &ArgList, env: &EnvRef) -> Result<EvaluatedArgs> {
        let mut positional = args.positional.iter().map(|a| self.eval(a, env)).collect::<Result<Vec<_>>>()?;
        let mut named = Vec::new();
        for (name, expr) in &args.named {
            named.push((name.clone(), self.eval(expr, env)?));
        }
        if let Some(rest) = &args.rest {
            match self.eval(rest, env)? {
                Value::Map(pairs) => {
                    for (k, v) in pairs {
                        let key = k.as_str().map(|s| normalize(s.trim_start_matches('$'))).unwrap_or_default();
                        named.push((key, v));
                    }
                }
                Value::List { items, .. } => positional.extend(items),
                other => positional.push(other),
            }
        }
        if let Some(kw) = &args.kw_rest
            && let Value::Map(pairs) = self.eval(kw, env)?
        {
            for (k, v) in pairs {
                let key = k.as_str().map(|s| normalize(s.trim_start_matches('$'))).unwrap_or_default();
                named.push((key, v));
            }
        }
        Ok((positional, named))
    }

    fn bind(
        &mut self,
        params: &Params,
        mut positional: Vec<Value>,
        mut named: Vec<(String, Value)>,
        env: &EnvRef,
        callee: &str,
        span: &Span,
    ) -> Result<()> {
        if positional.len() > params.params.len() && params.rest.is_none() {
            return Err(Error::at(
                format!(
                    "Only {} argument(s) allowed for {callee}, but {} were passed.",
                    params.params.len(),
                    positional.len()
                ),
                span,
            ));
        }
        let extra: Vec<Value> =
            if positional.len() > params.params.len() { positional.split_off(params.params.len()) } else { Vec::new() };
        let mut positional = positional.into_iter();
        for param in &params.params {
            let value = if let Some(v) = positional.next() {
                v
            } else if let Some(i) = named.iter().position(|(n, _)| *n == param.name) {
                named.remove(i).1
            } else if let Some(default) = &param.default {
                self.eval(default, env)?
            } else {
                return Err(Error::at(format!("Missing argument ${} for {callee}.", param.name), span));
            };
            env.borrow_mut().vars.insert(param.name.clone(), value);
        }
        if let Some(rest) = &params.rest {
            env.borrow_mut().vars.insert(rest.clone(), Value::list(extra, ListSep::Comma));
            named.clear();
        }
        if let Some((name, _)) = named.first() {
            return Err(Error::at(format!("No argument named ${name} for {callee}."), span));
        }
        Ok(())
    }

    fn call(&mut self, name: &str, args: &ArgList, env: &EnvRef, span: &Span) -> Result<Value> {
        let (ns, local) = match name.split_once('.') {
            Some((ns, rest)) if !rest.contains('.') => (Some(ns), rest),
            _ => (None, name),
        };
        let local_norm = normalize(local);

        // Namespaced: Sass module or user module; otherwise a Luau call like `Color3.fromRGB`.
        if let Some(ns) = ns {
            match self.namespace(ns, env) {
                Some(Namespace::Module(m)) => {
                    let f = module_member(&m, &local_norm, |e, n| e.functions.get(n).cloned())
                        .ok_or_else(|| Error::at(format!("Undefined function \"{name}\"."), span))?;
                    return self.call_function(&f, args, env, span);
                }
                Some(Namespace::Builtin(module)) => {
                    if let Some(v) = self.meta_function(Some(module), &local_norm, args, env, span)? {
                        return Ok(v);
                    }
                    let (positional, named) = self.eval_args(args, env)?;
                    return match builtins::call(Some(module), &local_norm, builtins::Args::new(positional, named)) {
                        Some(r) => r.map_err(|e| Error::at(format!("{name}(): {e}"), span)),
                        None => Err(Error::at(format!("Undefined function \"{name}\"."), span)),
                    };
                }
                None => {}
            }
        } else {
            match local_norm.as_str() {
                "if" => {
                    let (cond, a, b) = match (args.positional.as_slice(), args.named.as_slice()) {
                        ([c, a, b], []) => (c, a, b),
                        _ => {
                            return Err(Error::at(
                                "if() takes exactly three arguments: if($condition, $if-true, $if-false)",
                                span,
                            ));
                        }
                    };
                    return if self.eval(cond, env)?.is_truthy() { self.eval(a, env) } else { self.eval(b, env) };
                }
                "calc" => {
                    let [inner] = args.positional.as_slice() else {
                        return Err(Error::at("calc() takes exactly one argument.", span));
                    };
                    return self.calc(inner, env);
                }
                "clamp" if args.named.is_empty() && args.positional.len() == 3 && args.rest.is_none() => {
                    let (vals, _) = self.eval_args(args, env)?;
                    if let [Value::Number(lo), Value::Number(v), Value::Number(hi)] = vals.as_slice()
                        && lo.is_comparable_to(v)
                        && v.is_comparable_to(hi)
                        && lo.is_comparable_to(hi)
                    {
                        let pick = if v.cmp_value(lo).is_some_and(|o| o.is_lt()) {
                            lo
                        } else if v.cmp_value(hi).is_some_and(|o| o.is_gt()) {
                            hi
                        } else {
                            v
                        };
                        return Ok(Value::Number(pick.clone()));
                    }
                    return Ok(Value::Call { name: "clamp".into(), args: vals });
                }
                "var" | "env" | "attr" | "url" | "expression" | "luau" | "lua" | "token" => {
                    let (positional, _) = self.eval_args(args, env)?;
                    return Ok(Value::Call { name: local_norm, args: positional });
                }
                _ => {}
            }
            if let Some(f) = self.find_callable(None, &local_norm, env, false) {
                return self.call_function(&f, args, env, span);
            }
            if let Some(v) = self.meta_function(None, &local_norm, args, env, span)? {
                return Ok(v);
            }
            if builtins::exists(None, &local_norm) {
                let (positional, named) = self.eval_args(args, env)?;
                return builtins::call(None, &local_norm, builtins::Args::new(positional, named))
                    .unwrap()
                    .map_err(|e| Error::at(format!("{name}(): {e}"), span));
            }
        }

        // Plain CSS function or Luau expression: keep the call with evaluated arguments.
        let (positional, named) = self.eval_args(args, env)?;
        if !named.is_empty() {
            return Err(Error::at(
                format!("Plain function {name}() doesn't take named arguments (is it a typo for a Sass function?)."),
                span,
            ));
        }
        Ok(Value::Call { name: name.to_string(), args: positional })
    }

    fn call_function(&mut self, f: &Rc<Callable>, args: &ArgList, env: &EnvRef, span: &Span) -> Result<Value> {
        let (positional, named) = self.eval_args(args, env)?;
        self.invoke(f, positional, named, span)
    }

    fn invoke(
        &mut self,
        f: &Rc<Callable>,
        positional: Vec<Value>,
        named: Vec<(String, Value)>,
        span: &Span,
    ) -> Result<Value> {
        let call_env = new_env(Some(f.env.clone()), false);
        self.bind(&f.params, positional, named, &call_env, &format!("{}()", f.name), span)?;
        self.enter(span, &f.name)?;
        let saved = std::mem::replace(&mut self.in_function, true);
        let result = self.exec_block(&f.body, &call_env);
        self.in_function = saved;
        self.depth -= 1;
        match result.map_err(|e| trace(e, format!("{}() ({span})", f.name)))? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Err(Error::at(format!("Function {}() finished without @return.", f.name), span)),
        }
    }

    /// Introspection functions that need the evaluator.
    fn meta_function(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &ArgList,
        env: &EnvRef,
        span: &Span,
    ) -> Result<Option<Value>> {
        if !matches!(module, None | Some("meta")) {
            return Ok(None);
        }
        let known = [
            "variable-exists",
            "global-variable-exists",
            "function-exists",
            "mixin-exists",
            "content-exists",
            "get-function",
            "call",
            "keywords",
            "module-variables",
            "module-functions",
        ];
        if !known.contains(&name) {
            return Ok(None);
        }
        let (mut positional, named) = self.eval_args(args, env)?;
        let arg = |i: usize, key: &str| -> Option<Value> {
            if let Some(p) = named.iter().position(|(n, _)| n == key) {
                return Some(named[p].1.clone());
            }
            (i < positional.len()).then(|| positional[i].clone())
        };
        let text = |v: Option<Value>| -> Result<String> {
            match v {
                Some(Value::Str { text, .. }) => Ok(normalize(&text)),
                Some(other) => Err(Error::at(format!("{} is not a string.", other.inspect()), span)),
                None => Err(Error::at("Missing argument $name.", span)),
            }
        };
        Ok(Some(match name {
            "variable-exists" => Value::Bool(self.lookup_var(&text(arg(0, "name"))?, env).is_some()),
            "global-variable-exists" => {
                let n = text(arg(0, "name"))?;
                let root = module_root(env);
                let exists = root.borrow().vars.contains_key(&n);
                Value::Bool(exists)
            }
            "function-exists" => {
                let n = text(arg(0, "name"))?;
                Value::Bool(self.find_callable(None, &n, env, false).is_some() || builtins::exists(None, &n))
            }
            "mixin-exists" => Value::Bool(self.find_mixin(None, &text(arg(0, "name"))?, env).is_some()),
            "content-exists" => Value::Bool(self.content.is_some()),
            "get-function" => {
                let n = text(arg(0, "name"))?;
                if self.find_callable(None, &n, env, false).is_none() && !builtins::exists(None, &n) {
                    return Err(Error::at(format!("Function not found: {n}"), span));
                }
                Value::Function(n)
            }
            "call" => {
                if positional.is_empty() {
                    return Err(Error::at("Missing argument $function.", span));
                }
                let target = positional.remove(0);
                let fname = match target {
                    Value::Function(n) => n,
                    Value::Str { text, .. } => normalize(&text),
                    other => return Err(Error::at(format!("{} is not a function reference.", other.inspect()), span)),
                };
                if let Some(f) = self.find_callable(None, &fname, env, false) {
                    return Ok(Some(self.invoke(&f, positional, named, span)?));
                }
                match builtins::call(None, &fname, builtins::Args::new(positional, named)) {
                    Some(r) => r.map_err(|e| Error::at(e, span))?,
                    None => return Err(Error::at(format!("Undefined function \"{fname}\"."), span)),
                }
            }
            "keywords" => Value::Map(Vec::new()),
            "module-variables" | "module-functions" => {
                let ns = text(arg(0, "module"))?;
                let Some(Namespace::Module(m)) = self.namespace(&ns, env) else {
                    return Err(Error::at(format!("There is no module with namespace \"{ns}\"."), span));
                };
                let e = m.borrow();
                let mut pairs: Vec<(Value, Value)> = if name == "module-variables" {
                    e.vars.iter().map(|(k, v)| (Value::quoted(k.clone()), v.clone())).collect()
                } else {
                    e.functions.keys().map(|k| (Value::quoted(k.clone()), Value::Function(k.clone()))).collect()
                };
                pairs.sort_by_key(|a| a.0.inspect());
                Value::Map(pairs)
            }
            _ => unreachable!(),
        }))
    }

    /// Evaluates `calc()`: folds compatible arithmetic, keeps the rest as text.
    fn calc(&mut self, expr: &Expr, env: &EnvRef) -> Result<Value> {
        let v = self.calc_inner(expr, env)?;
        Ok(match v {
            Value::Number(_) => v,
            Value::Str { text, .. } => {
                let inner =
                    text.strip_prefix('(').and_then(|t| t.strip_suffix(')')).map(str::to_string).unwrap_or(text);
                Value::Call { name: "calc".into(), args: vec![Value::str(inner)] }
            }
            other => Value::Call { name: "calc".into(), args: vec![other] },
        })
    }

    fn calc_inner(&mut self, expr: &Expr, env: &EnvRef) -> Result<Value> {
        match expr {
            Expr::Paren(inner) => self.calc_inner(inner, env),
            Expr::Binary { op: op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div), lhs, rhs, span } => {
                let l = self.calc_inner(lhs, env)?;
                let r = self.calc_inner(rhs, env)?;
                if let (Value::Number(a), Value::Number(b)) = (&l, &r) {
                    let folded = match op {
                        BinOp::Add => a.add(b).ok(),
                        BinOp::Sub => a.sub(b).ok(),
                        BinOp::Mul => Some(a.mul(b)),
                        _ => Some(a.div(b)),
                    };
                    if let Some(n) = folded {
                        return Ok(Value::Number(n));
                    }
                }
                let sym = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    _ => "/",
                };
                let l = l.to_css().map_err(|e| Error::at(e, span))?;
                let r = r.to_css().map_err(|e| Error::at(e, span))?;
                Ok(Value::str(format!("({l} {sym} {r})")))
            }
            other => {
                let v = self.eval(other, env)?;
                Ok(match v {
                    Value::Call { name, args } if name == "calc" && args.len() == 1 => match &args[0] {
                        Value::Str { text, .. } => Value::str(format!("({text})")),
                        other => other.clone(),
                    },
                    v => v,
                })
            }
        }
    }

    // ----- finishing -----

    /// Which modules each module can extend into: itself, plus everything it loads with `@use` or
    /// `@forward`, transitively. `@extend` never reaches a stylesheet that loads *it*, which is how
    /// dart-sass scopes extension, so an extend in one leaf sheet can't rewrite another's selectors.
    /// (`@import`ed files have no module of their own, so their rules and extends belong to the
    /// importer and keep the old global behavior.)
    fn module_reachability(&self) -> Vec<Vec<bool>> {
        let mut roots = self.module_roots.clone();
        let mut deps: Vec<Vec<usize>> = Vec::new();
        let mut i = 0;
        while i < roots.len() {
            let mut direct: Vec<usize> = Vec::new();
            let info = roots[i].borrow().module.clone();
            if let Some(info) = info {
                let mut loaded: Vec<EnvRef> = info
                    .uses
                    .borrow()
                    .values()
                    .filter_map(|ns| match ns {
                        Namespace::Module(m) => Some(m.clone()),
                        Namespace::Builtin(_) => None,
                    })
                    .collect();
                loaded.extend(info.stars.borrow().iter().cloned());
                loaded.extend(info.forwards.borrow().iter().map(|(m, _)| m.clone()));
                for module in loaded {
                    let id = roots.iter().position(|r| Rc::ptr_eq(r, &module)).unwrap_or_else(|| {
                        roots.push(module);
                        roots.len() - 1
                    });
                    if !direct.contains(&id) {
                        direct.push(id);
                    }
                }
            }
            deps.push(direct);
            i += 1;
        }
        let n = roots.len();
        let mut reach = vec![vec![false; n]; n];
        for (from, row) in reach.iter_mut().enumerate() {
            row[from] = true;
            let mut stack = vec![from];
            while let Some(m) = stack.pop() {
                for &d in &deps[m] {
                    if !row[d] {
                        row[d] = true;
                        stack.push(d);
                    }
                }
            }
        }
        reach
    }

    fn finish(mut self) -> Result<Sheet> {
        // @extend, iterated so extenders that are themselves extended are handled.
        let reach = self.module_reachability();
        let visible = |ext: &Extend, rule: &OutRule| {
            reach.get(ext.owner).and_then(|r| r.get(rule.owner)).copied().unwrap_or(false)
        };
        let mut matched = vec![false; self.extends.len()];
        for _ in 0..8 {
            let mut changed = false;
            for (i, ext) in self.extends.iter().enumerate() {
                for rule in self.sheet.rules.iter_mut().filter(|r| !r.query && visible(ext, r)) {
                    if let Some(extended) = selector::extend(&rule.selector, &ext.target, &ext.extender) {
                        rule.selector = extended;
                        changed = true;
                        matched[i] = true;
                    } else if rule.selector.0.iter().any(|c| {
                        c.iter().any(|p| matches!(p, selector::Part::Compound(cmp) if cmp.contains(&ext.target)))
                    }) {
                        matched[i] = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (ext, found) in self.extends.iter().zip(&matched) {
            if !found && !ext.optional {
                let target = SelectorList(vec![vec![selector::Part::Compound(vec![ext.target.clone()])]]).to_css();
                return Err(Error::at(
                    format!(
                        "The target selector \"{target}\" of @extend was not found. Use \
                         \"@extend {target} !optional\" to allow that, and note that an @extend only reaches its \
                         own stylesheet and the ones it loads with @use/@forward."
                    ),
                    &ext.span,
                ));
            }
        }

        // Placeholders never reach the output; `:root` rules become stylesheet tokens, and
        // `[data-theme="dark"]` rules (or `:root` in `@media (prefers-color-scheme: dark)`) a theme's.
        let mut root_tokens = Vec::new();
        for rule in &mut self.sheet.rules {
            if rule.query {
                continue;
            }
            rule.selector = rule.selector.without_placeholders();
            if let Some(theme) = rule.theme.clone().or_else(|| rule.selector.theme_name()) {
                let is_root = rule.selector.is_root() || rule.selector.theme_name().is_some();
                if is_root && rule.parent.is_none() {
                    match self.sheet.themes.iter_mut().find(|(n, _)| *n == theme) {
                        Some((_, tokens)) => tokens.append(&mut rule.tokens),
                        None => self.sheet.themes.push((theme, std::mem::take(&mut rule.tokens))),
                    }
                    for decl in rule.decls.drain(..) {
                        self.diag.warn(
                            format!(
                                "property \"{}\" in a theme is ignored (a theme only sets --custom-properties)",
                                decl.name
                            ),
                            Some(&decl.span),
                        );
                    }
                } else {
                    self.diag.warn(
                        format!(
                            "`{}` depends on the theme `{theme}`, but only `:root` custom properties can (ignored)",
                            rule.selector.to_css()
                        ),
                        Some(&rule.span),
                    );
                    rule.decls.clear();
                    rule.tokens.clear();
                }
                rule.selector = SelectorList::default();
                continue;
            }
            if rule.selector.is_root() && rule.parent.is_none() {
                root_tokens.append(&mut rule.tokens);
                for decl in rule.decls.drain(..) {
                    self.diag.warn(
                        format!(
                            "property \"{}\" in :root is ignored (only --custom-properties become stylesheet tokens)",
                            decl.name
                        ),
                        Some(&decl.span),
                    );
                }
                rule.selector = SelectorList::default();
            }
        }
        self.sheet.tokens.extend(root_tokens);
        Ok(self.sheet)
    }
}

/// `(prefers-color-scheme: dark)` (optionally after `screen and`): the theme it selects.
fn color_scheme(params: &str) -> Option<String> {
    let lower = params.trim().to_ascii_lowercase();
    let condition = ["only screen and ", "screen and ", "all and "]
        .iter()
        .find_map(|p| lower.strip_prefix(p))
        .unwrap_or(&lower)
        .trim();
    let inner = condition.strip_prefix('(')?.strip_suffix(')')?;
    let (feature, value) = inner.split_once(':')?;
    (feature.trim() == "prefers-color-scheme").then(|| value.trim().to_string()).filter(|v| !v.is_empty())
}

fn qualified(ns: Option<&str>, name: &str) -> String {
    match ns {
        Some(ns) => format!("{ns}.{name}"),
        None => name.to_string(),
    }
}

fn trace(mut e: Error, frame: String) -> Error {
    e.trace.push(frame);
    e
}

/// Looks a member up in a module, following `@forward`s.
fn module_member<T>(module: &EnvRef, name: &str, get: impl Fn(&Env, &str) -> Option<T> + Copy) -> Option<T> {
    let m = module.borrow();
    if let Some(v) = get(&m, name) {
        return Some(v);
    }
    let info = m.module.clone()?;
    let forwards = info.forwards.borrow().clone();
    forwards.iter().find_map(|(target, prefix)| match prefix {
        Some(p) => name
            .strip_prefix(&format!("{p}-"))
            .or_else(|| name.strip_prefix(p.as_str()))
            .and_then(|n| module_member(target, n, get)),
        None => module_member(target, name, get),
    })
}

fn builtin_variable(module: &str, name: &str) -> Option<Value> {
    match (module, name) {
        ("math", "pi") => Some(Value::num(std::f64::consts::PI)),
        ("math", "e") => Some(Value::num(std::f64::consts::E)),
        ("math", "epsilon") => Some(Value::num(f64::EPSILON)),
        ("math", "max-safe-integer") => Some(Value::num(9007199254740991.0)),
        ("math", "min-safe-integer") => Some(Value::num(-9007199254740991.0)),
        ("math", "max-number") => Some(Value::num(f64::MAX)),
        ("math", "min-number") => Some(Value::num(f64::MIN_POSITIVE)),
        _ => None,
    }
}

fn binary(op: BinOp, l: &Value, r: &Value) -> Result<Value, String> {
    use BinOp::*;
    Ok(match op {
        Eq => Value::Bool(l.sass_eq(r)),
        Ne => Value::Bool(!l.sass_eq(r)),
        Lt | Le | Gt | Ge => {
            let (Value::Number(a), Value::Number(b)) = (l, r) else {
                return Err(format!("Undefined operation \"{} {} {}\".", l.inspect(), op_symbol(op), r.inspect()));
            };
            let ord = a
                .cmp_value(b)
                .ok_or_else(|| format!("Incompatible units {} and {}.", a.unit_string(), b.unit_string()))?;
            Value::Bool(match op {
                Lt => ord.is_lt(),
                Le => ord.is_le(),
                Gt => ord.is_gt(),
                _ => ord.is_ge(),
            })
        }
        Add => match (l, r) {
            (Value::Number(a), Value::Number(b)) => Value::Number(a.add(b)?),
            (Value::Str { text, quoted }, other) => {
                Value::Str { text: format!("{text}{}", other.to_interp()?), quoted: *quoted }
            }
            (other, Value::Str { text, quoted }) => {
                if matches!(other, Value::Color(_) | Value::Number(_)) && *quoted {
                    Value::Str { text: format!("{}{text}", other.to_css()?), quoted: true }
                } else {
                    Value::str(format!("{}{text}", other.to_css()?))
                }
            }
            (Value::Color(_), _) | (_, Value::Color(_)) => {
                return Err(format!("Undefined operation \"{} + {}\".", l.inspect(), r.inspect()));
            }
            _ => Value::str(format!("{}{}", l.to_css()?, r.to_css()?)),
        },
        Sub => match (l, r) {
            (Value::Number(a), Value::Number(b)) => Value::Number(a.sub(b)?),
            (Value::Color(_), _) | (_, Value::Color(_)) => {
                return Err(format!("Undefined operation \"{} - {}\".", l.inspect(), r.inspect()));
            }
            _ => Value::str(format!("{}-{}", l.to_css()?, r.to_css()?)),
        },
        Mul => match (l, r) {
            (Value::Number(a), Value::Number(b)) => Value::Number(a.mul(b)),
            _ => return Err(format!("Undefined operation \"{} * {}\".", l.inspect(), r.inspect())),
        },
        Div => match (l, r) {
            (Value::Number(a), Value::Number(b)) => Value::Number(a.div(b)),
            (Value::Color(_), _) | (_, Value::Color(_)) => {
                return Err(format!("Undefined operation \"{} / {}\".", l.inspect(), r.inspect()));
            }
            _ => Value::str(format!("{}/{}", l.to_css()?, r.to_css()?)),
        },
        Mod => match (l, r) {
            (Value::Number(a), Value::Number(b)) => Value::Number(a.rem(b)?),
            _ => return Err(format!("Undefined operation \"{} % {}\".", l.inspect(), r.inspect())),
        },
        And | Or => unreachable!(),
    })
}

/// Whether a `/` with this operand keeps its slash: a literal number, or another such `/`.
fn allows_slash(e: &Expr) -> bool {
    match e {
        Expr::Value(Value::Number(_)) => true,
        Expr::Binary { op: BinOp::Div, lhs, rhs, .. } => allows_slash(lhs) && allows_slash(rhs),
        _ => false,
    }
}

fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn compile(src: &str) -> (Sheet, Diagnostics) {
        let mut diag = Diagnostics::default();
        let sheet = compile_source(src, Path::new("test.scss"), Syntax::Scss, &Options::default(), &mut diag)
            .unwrap_or_else(|e| panic!("{e}"));
        (sheet, diag)
    }

    #[test]
    fn a_slash_between_literal_numbers_stays_a_slash() {
        let (sheet, _) = compile(
            "$x: 10px/2; .a { a: 20px/1.5; b: (10px/2); c: $x; d: $x * 2; e: percentage(1/3); f: 1/2/3; g: -(1/2); \
             h: 1/2 + 1; $w: 4px; i: $w/2; }",
        );
        assert_eq!(
            css(&sheet),
            ".a { a: 20px/1.5; b: 5px; c: 10px/2; d: 10px; e: 33.3333333333%; f: 1/2/3; g: -0.5; h: 1.5; i: 2px; }\n"
        );
    }

    #[test]
    fn imports_resolve_through_the_options_filesystem() {
        let mut fs = crate::fs::MemoryFs::new();
        fs.insert(
            "ui/main.scss",
            "@use \"theme\";
@import \"../shared/base\";
Frame { BackgroundColor3: theme.$bg; }",
        );
        fs.insert("ui/_theme.scss", "$bg: #102030;");
        fs.insert("shared/_base.scss", "TextLabel { TextSize: 14; }");
        let opts = Options { fs: Rc::new(fs), ..Options::default() };
        let mut diag = Diagnostics::default();
        let sheet = compile_file(Path::new("ui/main.scss"), &opts, &mut diag).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            css(&sheet),
            "TextLabel { TextSize: 14; }
Frame { BackgroundColor3: #102030; }
"
        );
        assert_eq!(sheet.files.len(), 3);
    }

    fn css(sheet: &Sheet) -> String {
        let mut out = String::new();
        for rule in &sheet.rules {
            if rule.decls.is_empty() || rule.query {
                continue;
            }
            out.push_str(&rule.selector.to_css());
            out.push_str(" {");
            for d in &rule.decls {
                out.push_str(&format!(" {}: {};", d.name, d.value.to_css().unwrap()));
            }
            out.push_str(" }\n");
        }
        out
    }

    #[test]
    fn variables_and_nesting() {
        let (sheet, _) = compile("$w: 10px; .a { width: $w * 2; &:hover { width: $w; } .b { c: d } }");
        assert_eq!(css(&sheet), ".a { width: 20px; }\n.a:hover { width: 10px; }\n.a .b { c: d; }\n");
    }

    #[test]
    fn mixins_functions_content() {
        let (sheet, _) = compile(
            "@function double($x) { @return $x * 2; }
             @mixin hover($c: red) { &:hover { color: $c; @content; } }
             .btn { @include hover(blue) { size: double(3px); } }",
        );
        assert_eq!(css(&sheet), ".btn:hover { color: #0000ff; size: 6px; }\n");
    }

    #[test]
    fn control_flow() {
        let (sheet, _) = compile(
            "$sizes: (sm: 4px, lg: 8px);
             @each $name, $size in $sizes { .p-#{$name} { padding: $size; } }
             @for $i from 1 through 2 { .z#{$i} { z: $i } }
             $i: 0; @while $i < 2 { $i: $i + 1; }
             .w { v: $i; }",
        );
        assert_eq!(
            css(&sheet),
            ".p-sm { padding: 4px; }\n.p-lg { padding: 8px; }\n.z1 { z: 1; }\n.z2 { z: 2; }\n.w { v: 2; }\n"
        );
    }

    #[test]
    fn extend_and_placeholders() {
        let (sheet, _) = compile("%base { a: b } .x { @extend %base; c: d }");
        assert_eq!(css(&sheet), ".x { a: b; }\n.x { c: d; }\n");
    }

    #[test]
    fn scoping() {
        let (sheet, _) = compile("$x: 1; .a { $x: 2; v: $x } .b { v: $x } @if true { $x: 3 } .c { v: $x }");
        assert_eq!(css(&sheet), ".a { v: 2; }\n.b { v: 1; }\n.c { v: 3; }\n");
    }

    #[test]
    fn layers_and_tokens() {
        let (sheet, _) = compile(
            ":root { --Accent: #ff0000; } @layer base, theme.dark; .u { x: 0 } @layer theme { .a { x: 1; &:hover { y: 2 } } @layer dark { .b { z: 1 } } } .c { @layer base { w: 1 } }",
        );
        assert_eq!(sheet.tokens.len(), 1);
        let layer = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(sheet.layers, vec![layer(&["base"]), layer(&["theme"]), layer(&["theme", "dark"])]);
        let layers: Vec<_> = sheet.rules.iter().filter(|r| !r.decls.is_empty()).map(|r| r.layer.clone()).collect();
        assert_eq!(
            layers,
            vec![layer(&[]), layer(&["theme"]), layer(&["theme"]), layer(&["theme", "dark"]), layer(&["base"])]
        );
    }

    #[test]
    fn calc_and_builtins() {
        let (sheet, _) = compile(
            "@use 'sass:math'; .a { w: calc(50% + 10px); h: calc(2px * 3); d: math.div(10px, 4); l: lighten(#000, 50%); }",
        );
        assert_eq!(css(&sheet), ".a { w: calc(50% + 10px); h: 6px; d: 2.5px; l: #808080; }\n");
    }

    #[test]
    fn queries() {
        let (sheet, _) = compile(
            ".a { x: 1; @PreferredInputTouch { x: 2 } } @media (prefers-reduced-motion: reduce) { .b { y: 1 } }",
        );
        let queries: Vec<_> = sheet.rules.iter().filter(|r| r.query).map(|r| r.selector.to_css()).collect();
        assert_eq!(queries, vec!["@PreferredInputTouch", "@ReducedMotionEnabledTrue"]);
        assert_eq!(sheet.rules[2].parent, Some(1));
    }
}
