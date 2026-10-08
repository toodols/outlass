//! The outlass compiler for the browser. Files come from the playground's editor tabs and are
//! read through an in-memory filesystem, so `@use`/`@forward`/`@import` work between tabs.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use js_sys::{Array, Object, Reflect};
use outlass::approx::{ApproxOptions, Group};
use outlass::codegen::{self, CodegenOptions};
use outlass::diag::{Diagnostic, Diagnostics, Level};
use outlass::eval::{self, Options};
use outlass::fs::MemoryFs;
use outlass::roblox::ValueOptions;
use wasm_bindgen::prelude::*;

/// Compiles `entry` from `files` (an object of path → source).
///
/// `emit` is `luau`, `json`, `rbxmx` or `css`; `approx` lists approximation groups (`all` for
/// every group); `user_agent` keeps the user-agent stylesheet. Returns `{ ok, output, diagnostics: [{ level, message, file, line, col }] }`.
#[wasm_bindgen]
pub fn compile(
    files: &Object,
    entry: &str,
    emit: &str,
    approx: Vec<String>,
    strict: bool,
    user_agent: bool,
) -> Result<Object, JsValue> {
    let mut fs = MemoryFs::new();
    for pair in Object::entries(files).iter() {
        let pair = Array::from(&pair);
        let (Some(path), Some(source)) = (pair.get(0).as_string(), pair.get(1).as_string()) else {
            return Err(JsValue::from_str("files must map paths to source strings"));
        };
        fs.insert(path, source);
    }
    let mut groups = Vec::new();
    for name in &approx {
        let group = std::iter::once(Group::All)
            .chain(Group::concrete().iter().copied())
            .find(|g| g.name() == name)
            .ok_or_else(|| JsValue::from_str(&format!("unknown approximation group `{name}`")))?;
        groups.push(group);
    }

    let opts = Options { fs: Rc::new(fs), ..Options::default() };
    let mut diag = Diagnostics::default();
    let entry_path = Path::new(entry);
    let sheet = match eval::compile_file(entry_path, &opts, &mut diag) {
        Ok(sheet) => sheet,
        Err(e) => {
            // A fatal error is reported like any other diagnostic, after the ones before it.
            let mut message = e.message;
            for frame in &e.trace {
                message.push_str(&format!("\n    at {frame}"));
            }
            diag.items.push(Diagnostic { level: Level::Error, message, span: e.span });
            return result(false, "", &diag);
        }
    };

    let sheet_name = entry_path.file_stem().and_then(|s| s.to_str()).unwrap_or("StyleSheet").to_string();
    let codegen = |header: Option<String>| CodegenOptions {
        values: ValueOptions { allow_raw_luau: false },
        approx: ApproxOptions {
            groups: Group::expand(&groups),
            strict,
            tokens: HashMap::new(),
            inherited_family: None,
            user_agent: false,
        },
        sheet_name: sheet_name.clone(),
        header,
        tags: HashMap::new(),
        user_agent_styles: user_agent,
    };
    let output = match emit {
        "luau" => codegen::emit_luau(&sheet, &codegen(Some(entry.to_string())), &mut diag),
        "json" => codegen::emit_json(&sheet, &codegen(None), &mut diag),
        "rbxmx" => codegen::emit_rbxmx(&sheet, &codegen(None), &mut diag),
        "css" => codegen::emit_css(&sheet),
        other => return Err(JsValue::from_str(&format!("unknown output `{other}`"))),
    };
    result(diag.error_count() == 0, &output, &diag)
}

/// The approximation groups `compile` accepts, besides `all`.
#[wasm_bindgen]
pub fn approx_groups() -> Vec<String> {
    Group::concrete().iter().map(|g| g.name().to_string()).collect()
}

/// The repository's examples, used as the playground's starting files.
#[wasm_bindgen]
pub fn example_files() -> Object {
    let files = Object::new();
    set(&files, "showcase.scss", &include_str!("../../examples/showcase.scss").into());
    set(&files, "_theme.scss", &include_str!("../../examples/_theme.scss").into());
    files
}

fn result(ok: bool, output: &str, diag: &Diagnostics) -> Result<Object, JsValue> {
    let items = Array::new();
    for d in &diag.items {
        let item = Object::new();
        let level = match d.level {
            Level::Warning => "warning",
            Level::Debug => "debug",
            Level::Error => "error",
        };
        set(&item, "level", &level.into());
        set(&item, "message", &d.message.as_str().into());
        if let Some(span) = &d.span {
            set(&item, "file", &(&*span.file).into());
            set(&item, "line", &span.line.into());
            set(&item, "col", &span.col.into());
        }
        items.push(&item);
    }
    let out = Object::new();
    set(&out, "ok", &ok.into());
    set(&out, "output", &output.into());
    set(&out, "diagnostics", &items);
    Ok(out)
}

fn set(target: &Object, key: &str, value: &JsValue) {
    // Setting a property on a fresh plain object can't fail.
    let _ = Reflect::set(target, &key.into(), value);
}
