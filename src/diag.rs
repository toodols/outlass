//! Source locations, errors and warnings.

use std::fmt;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub file: Rc<str>,
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn new(file: Rc<str>, line: u32, col: u32) -> Self {
        Span { file, line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

/// A fatal compile error.
#[derive(Clone, Debug)]
pub struct Error {
    pub message: String,
    pub span: Option<Span>,
    /// Include/function call trace, innermost first.
    pub trace: Vec<String>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error { message: message.into(), span: None, trace: Vec::new() }
    }

    pub fn at(message: impl Into<String>, span: &Span) -> Self {
        Error { message: message.into(), span: Some(span.clone()), trace: Vec::new() }
    }

    /// Attaches a span if the error does not have one yet.
    pub fn or_at(mut self, span: &Span) -> Self {
        if self.span.is_none() {
            self.span = Some(span.clone());
        }
        self
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error::new(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Error::new(message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.span {
            Some(span) => write!(f, "{span}: error: {}", self.message)?,
            None => write!(f, "error: {}", self.message)?,
        }
        for frame in &self.trace {
            write!(f, "\n    at {frame}")?;
        }
        Ok(())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Warning,
    Debug,
    /// A `--strict` parity violation: the output would lay out differently from a browser. The
    /// build fails, but compilation carries on so every violation is reported.
    Error,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
    pub span: Option<Span>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.level {
            Level::Warning => "warning",
            Level::Debug => "debug",
            Level::Error => "error",
        };
        match &self.span {
            Some(span) => write!(f, "{span}: {label}: {}", self.message),
            None => write!(f, "{label}: {}", self.message),
        }
    }
}

/// Collects non-fatal diagnostics during compilation.
#[derive(Default, Debug)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn warn(&mut self, message: impl Into<String>, span: Option<&Span>) {
        let message = message.into();
        let span = span.cloned();
        // The same warning from the same place (e.g. inside a mixin used many times) is noise.
        if self.items.iter().any(|d| d.level == Level::Warning && d.message == message && d.span == span) {
            return;
        }
        self.items.push(Diagnostic { level: Level::Warning, message, span });
    }

    pub fn error(&mut self, message: impl Into<String>, span: Option<&Span>) {
        let message = message.into();
        let span = span.cloned();
        if self.items.iter().any(|d| d.level == Level::Error && d.message == message && d.span == span) {
            return;
        }
        self.items.push(Diagnostic { level: Level::Error, message, span });
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.level == Level::Error).count()
    }

    pub fn debug(&mut self, message: impl Into<String>, span: Option<&Span>) {
        self.items.push(Diagnostic { level: Level::Debug, message: message.into(), span: span.cloned() });
    }

    pub fn warning_count(&self) -> usize {
        self.items.iter().filter(|d| d.level == Level::Warning).count()
    }
}
