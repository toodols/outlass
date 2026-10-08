//! Compiles SCSS (and indented Sass / lass) into Luau code that builds a Roblox StyleSheet.
//!
//! The `outlass` binary is the command-line front end; the web playground drives the same
//! compiler through [`eval::Options::fs`] with files held in memory.

pub mod approx;
pub mod ast;
pub mod builtins;
pub mod codegen;
pub mod diag;
pub mod eval;
pub mod fs;
pub mod indented;
pub mod luau;
pub mod parser;
pub mod query;
pub mod rbxmx;
pub mod roblox;
pub mod selector;
pub mod value;
