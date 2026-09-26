//! Syntax tree produced by the SCSS parser.

use crate::diag::Span;
use crate::value::{ListSep, Value};

#[derive(Clone, Debug)]
pub enum InterpPart {
    Text(String),
    Expr(Expr),
}

/// Text with `#{...}` interpolations.
pub type Interp = Vec<InterpPart>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Plus,
    Not,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Value(Value),
    /// An identifier or string containing interpolation (or a plain unquoted identifier).
    Str {
        parts: Interp,
        quoted: bool,
    },
    Var {
        ns: Option<String>,
        name: String,
        span: Span,
    },
    List {
        items: Vec<Expr>,
        sep: ListSep,
        bracketed: bool,
    },
    Map(Vec<(Expr, Expr)>),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    /// `name(args)`; `name` may contain dots (`math.div`, `Color3.fromRGB`).
    Call {
        name: String,
        args: ArgList,
        span: Span,
    },
    Paren(Box<Expr>),
    /// `&`
    Parent,
}

#[derive(Clone, Debug, Default)]
pub struct ArgList {
    pub positional: Vec<Expr>,
    pub named: Vec<(String, Expr)>,
    /// `$args...`
    pub rest: Option<Box<Expr>>,
    /// Second `...` argument (keyword rest).
    pub kw_rest: Option<Box<Expr>>,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, Default)]
pub struct Params {
    pub params: Vec<Param>,
    pub rest: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ContentBlock {
    pub params: Params,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    VarDecl {
        ns: Option<String>,
        name: String,
        value: Expr,
        default: bool,
        global: bool,
    },
    /// A property declaration. `value` is None for pure nested-property blocks (`font: { ... }`).
    Decl {
        name: Interp,
        value: Option<Expr>,
        children: Vec<Stmt>,
        important: bool,
    },
    Rule {
        selector: Interp,
        body: Vec<Stmt>,
    },
    Mixin {
        name: String,
        params: Params,
        body: Vec<Stmt>,
    },
    Function {
        name: String,
        params: Params,
        body: Vec<Stmt>,
    },
    Include {
        ns: Option<String>,
        name: String,
        args: ArgList,
        content: Option<ContentBlock>,
    },
    Content {
        args: ArgList,
    },
    Return(Expr),
    If {
        clauses: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    Each {
        vars: Vec<String>,
        list: Expr,
        body: Vec<Stmt>,
    },
    For {
        var: String,
        from: Expr,
        to: Expr,
        inclusive: bool,
        body: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Extend {
        selector: Interp,
        optional: bool,
    },
    /// `@use`; namespace `Some("*")` means `as *`.
    Use {
        url: String,
        namespace: Option<String>,
        with: Vec<(String, Expr)>,
    },
    Forward {
        url: String,
        prefix: Option<String>,
        with: Vec<(String, Expr)>,
    },
    Import {
        urls: Vec<String>,
    },
    Debug(Expr),
    Warn(Expr),
    Error(Expr),
    /// outlass extension: `@priority <n>;`
    Priority(Expr),
    AtRoot {
        selector: Option<Interp>,
        body: Vec<Stmt>,
    },
    /// Any other at-rule (`@media`, `@supports`, Roblox `@QueryName`, ...).
    AtRule {
        name: String,
        params: Interp,
        body: Option<Vec<Stmt>>,
    },
}
