
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

use outlass::approx::{self, ApproxOptions, Group};
use outlass::{builtins, codegen, eval};
use outlass::codegen::CodegenOptions;
use outlass::diag::{Diagnostics, Level};
use outlass::eval::{Options, Sheet, Syntax};
use outlass::roblox::ValueOptions;

const LONG_ABOUT: &str = "\
Compiles SCSS (plus indented .sass) into a Luau module that builds and \
returns a Roblox StyleSheet made of StyleRules.

Supported Sass features: variables (!default, !global), nesting and the parent selector \
(&, &-suffix), nested properties, #{} interpolation, @mixin/@include with arguments, \
defaults, rest args and @content (including `using`), @function/@return, @if/@else, \
@each, @for, @while, @extend and %placeholders, @use/@forward (namespaces, `as`, `with`), \
@import, @at-root, @debug/@warn/@error, maps, lists, arithmetic with units, calc(), and \
the built-in module functions (see `outlass functions`).

Roblox mapping:
  * Selectors: `Frame`, `.Tag`, `#Name`, `::UICorner`; the descendant combinator (space) \
becomes `>>`; :hover → :Hover, :active → :Press, :disabled → :NonInteractable.
  * Declarations with PascalCase names are Roblox properties. Their values are Roblox values \
(Enum.Font.Gotham, UDim2.new(0, 10, 0, 20), Color3.fromRGB(...), colors like #fff, quoted \
strings, numbers; px is dropped, 50% → 0.5, 1s/1000ms → 1). Anything else is ignored with a \
warning, so the generated code can only build a StyleSheet; luau(\"...\") inserts raw Luau, but \
only with --allow-raw-luau.
  * Custom properties (--Name: value) become StyleRule attributes (design tokens); in \
:root or at the top level they go on the StyleSheet. [data-theme=\"dark\"] and \
@media (prefers-color-scheme: dark) set a theme's tokens: theme StyleSheets the sheet derives from \
(switch with sheet.Theme.StyleSheet = sheet.Themes.dark). var(--Name) references a token (\"$Name\"). \
Their values are raw text, as in CSS and dart-sass, and are read back as a CSS value, so \
substitute variables with #{$var} rather than writing $var on its own.
  * The CSS cascade sets StyleRule.Priority: `!important` declarations beat normal ones, \
then later `@layer`s (styles outside any layer beat every layer; `!important` reverses the layer \
order), then more specific selectors, then later rules.
  * Like a browser's user-agent stylesheet, a lowest-priority rule turns RichText on for \
TextLabel, TextButton and TextBox, and (with --approx=size) sizes every class from its content, \
since a fresh Roblox element is 0x0. A rule that sets width and height turns that back off. Opt out \
with `RichText: false` or `AutomaticSize: Enum.AutomaticSize.None` on any rule.
  * `width: 100%` needs a parent with a definite width; under a parent that hugs its content it \
collapses to zero (Roblox resolves percentages against the parent's size, where CSS would treat one \
against an auto-sized parent as `auto`).
  * `Transition: <Property> <duration> <EasingStyle> <direction>, ...` sets property \
transitions (TweenInfo).
  * @media and @container become Roblox style queries: rules inside get an `@Name` selector \
prefix. Media features Roblox documents an equivalent for use the built-in query \
((max-width: 600px) → @ViewportDisplaySizeSmall, (pointer: coarse) → @PreferredInputTouch, \
(prefers-reduced-motion: reduce) → @ReducedMotionEnabledTrue, ...). Other width/height/aspect-ratio/\
orientation conditions, and combinations, become a `::StyleQuery` on the ScreenGui (@media) or \
on each element with `container-type`/`container-name` (@container). Any @PascalCase at-rule \
(`@PreferredInputTouch { ... }`) uses that query directly.
  * lowercase CSS properties (opacity, background-color, border-radius, ...) have no \
direct Roblox counterpart; with --approx they are translated into near equivalents \
(see `outlass properties`), otherwise they are ignored with a warning.";

const EXAMPLES: &str = "\
Examples:
  outlass ui.scss                         Write ui.luau next to ui.scss
  outlass src/styles/*.scss -d out        Compile each file into out/<name>.luau
  outlass theme.scss -o -                 Print the Luau to stdout
  outlass a.scss b.scss --merge -o all.luau
                                          Combine several files into one StyleSheet
  outlass ui.scss --approx                Translate CSS properties (all groups)
  outlass ui.scss --approx=opacity,color  Translate only some groups
  outlass ui.scss -D accent=#ff8800       Override `$accent: ... !default` in ui.scss
  outlass ui.scss --emit css -o -         Show the evaluated SCSS as plain CSS (debugging)
  outlass ui.scss --emit json -o -        Show the compiled StyleSheet as JSON
  outlass ui.scss --watch                 Recompile whenever an input or import changes
  outlass properties                      List every CSS property --approx understands
  outlass properties border-radius        Explain one property's translation
  outlass functions color                 List built-in functions matching \"color\"

Exit status: 0 on success, 1 if compilation failed (or warnings with --deny-warnings), \
2 on invalid command-line usage.";

#[derive(Parser)]
#[command(
    name = "outlass",
    version,
    about = "Compile SCSS into Luau that builds a Roblox StyleSheet",
    long_about = LONG_ABOUT,
    after_long_help = EXAMPLES,
    after_help = "Run `outlass --help` for the feature overview and examples.",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    build: BuildArgs,
}

#[derive(Subcommand)]
enum Command {
    /// List the CSS properties that --approx translates and what they become in Roblox
    #[command(long_about = "List the CSS properties that --approx translates, the approximation group each \
        belongs to, and the Roblox properties (or pseudo-instances such as ::UICorner) they produce.\n\n\
        Lowercase declarations that are not listed here have no Roblox counterpart and are ignored with a warning.")]
    Properties {
        /// Show only this property (vendor prefixes like -webkit- are ignored)
        #[arg(allow_hyphen_values = true)]
        property: Option<String>,
        /// Show only the properties in this approximation group
        #[arg(short, long, value_enum)]
        group: Option<Group>,
    },
    /// List the built-in Sass functions
    #[command(long_about = "List the built-in Sass functions. Module functions are called with their namespace \
        (math.div(...), color.adjust(...)); the sass: modules are always available under their own names, \
        and `@use \"sass:math\" as m` adds an alias. Legacy global names (lighten, map-get, ...) are shown \
        in the GLOBAL column.\n\n\
        Also available: if(), call(), get-function(), variable-exists(), global-variable-exists(), \
        function-exists(), mixin-exists(), content-exists(), module-variables(), module-functions(), \
        calc(), and the Roblox helpers var(--Token) / token(Name) (token reference \"$Name\") and \
        luau(\"code\") (raw Luau, only with --allow-raw-luau). Roblox constructors like UDim2.new(0, 4, 0, 4)         become the value they construct.")]
    Functions {
        /// Only show functions whose module, name or summary contains this text
        filter: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Emit {
    /// A Luau module that creates and returns the StyleSheet
    Luau,
    /// The compiled StyleSheet as JSON: the rules and typed values the Luau is generated from
    Json,
    /// A Roblox model file of the StyleSheet, for Studio or Rojo: data only, no code
    Rbxmx,
    /// The evaluated stylesheet as plain CSS, before Roblox translation (for debugging)
    Css,
}

#[derive(Args)]
struct BuildArgs {
    /// Input files or glob patterns (.scss, .sass; the syntax follows the extension). Use `-`
    /// to read SCSS from stdin.
    ///
    /// Files whose name starts with `_` (partials) are skipped when matched by a glob pattern.
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<String>,

    /// Output file, or `-` for stdout. Only valid with a single input or with --merge
    /// [default: the input path with a .luau (or .json/.css) extension]
    #[arg(short, long, value_name = "FILE")]
    output: Option<String>,

    /// Write outputs into this directory instead of next to each input
    #[arg(short = 'd', long, value_name = "DIR", conflicts_with = "output")]
    out_dir: Option<PathBuf>,

    /// Compile all inputs into a single StyleSheet (requires --output)
    #[arg(long, requires = "output")]
    merge: bool,

    /// Translate CSS properties into Roblox approximations. `--approx` alone enables every group;
    /// `--approx=a,b` (note the `=`) enables only those groups. See `outlass properties`
    #[arg(
        short,
        long,
        value_enum,
        value_name = "GROUP",
        num_args = 0..=1,
        require_equals = true,
        value_delimiter = ',',
        default_missing_value = "all"
    )]
    approx: Vec<Group>,

    /// Additional directory to search for @use/@forward/@import targets (repeatable)
    #[arg(short = 'I', long = "load-path", value_name = "DIR")]
    load_paths: Vec<PathBuf>,

    /// Define a global variable before compiling, e.g. `-D accent=#f80` (repeatable).
    /// The value is parsed as SassScript and takes precedence over `!default` declarations in the
    /// input file (to configure a @use'd module, use `@use "..." with (...)`)
    #[arg(short = 'D', long = "define", value_name = "NAME=VALUE")]
    defines: Vec<String>,

    /// What to generate
    #[arg(long, value_enum, default_value_t = Emit::Luau)]
    emit: Emit,

    /// A JSON file mapping each CollectionService tag (or `#Name`) to the GuiObject classes it's used
    /// on, e.g. `{"avatar": ["ImageLabel"], "card": ["Frame"]}`. Rules then drop the properties
    /// those classes don't have, and warn about the ones written for them
    #[arg(long, value_name = "FILE")]
    tags: Option<PathBuf>,

    /// Let `luau("...")` insert raw Luau into the generated code. Raw Luau can do anything a script
    /// can, so only allow it for stylesheets you trust; without it, the output can only build a
    /// StyleSheet
    #[arg(long)]
    allow_raw_luau: bool,

    /// Recompile whenever an input file or anything it imports changes
    #[arg(short, long)]
    watch: bool,

    /// Don't print warnings or @debug output
    #[arg(short, long)]
    quiet: bool,

    /// Treat warnings as errors (exit status 1)
    #[arg(long)]
    deny_warnings: bool,

    /// Fail on CSS that compiles, but lays out differently in Roblox than in a browser (e.g. a flex
    /// column that stretches its items, or `width: auto`); each error names the CSS that agrees
    #[arg(long)]
    strict: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Properties { property, group }) => list_properties(property.as_deref(), group),
        Some(Command::Functions { filter }) => list_functions(filter.as_deref()),
        None => build(&cli.build),
    }
}

// ----- listings -----

fn list_properties(property: Option<&str>, group: Option<Group>) -> ExitCode {
    if let Some(name) = property {
        let Some(doc) = approx::lookup(name) else {
            eprintln!("`{name}` is not translated by --approx (it has no Roblox approximation).");
            eprintln!("Run `outlass properties` for the full list.");
            return ExitCode::FAILURE;
        };
        println!("{}  (group: {}, enable with `--approx={}`)", doc.css, doc.group.name(), doc.group.name());
        println!("  → {}", doc.roblox);
        if !doc.notes.is_empty() {
            println!("  {}", doc.notes);
        }
        return ExitCode::SUCCESS;
    }
    let groups: Vec<Group> = match group {
        Some(g) => Group::expand(&[g]),
        None => Group::expand(&[Group::All]),
    };
    for g in groups {
        let props: Vec<_> = approx::PROPERTIES.iter().filter(|p| p.group == g).collect();
        if props.is_empty() {
            continue;
        }
        println!("{} (--approx={})", g.name(), g.name());
        let width = props.iter().map(|p| p.css.len()).max().unwrap_or(0);
        for p in props {
            println!("  {:width$}  → {}", p.css, p.roblox);
            if !p.notes.is_empty() {
                println!("  {:width$}    {}", "", p.notes);
            }
        }
        println!();
    }
    ExitCode::SUCCESS
}

fn list_functions(filter: Option<&str>) -> ExitCode {
    let filter = filter.map(str::to_ascii_lowercase);
    let docs: Vec<_> = builtins::BUILTINS
        .iter()
        .filter(|d| {
            filter.as_ref().is_none_or(|f| {
                d.module.contains(f.as_str())
                    || d.name.contains(f.as_str())
                    || d.global.is_some_and(|g| g.contains(f.as_str()))
                    || d.summary.to_ascii_lowercase().contains(f.as_str())
            })
        })
        .collect();
    if docs.is_empty() {
        eprintln!("No built-in functions match.");
        return ExitCode::FAILURE;
    }
    let call = |d: &builtins::BuiltinDoc| {
        if d.module.is_empty() {
            format!("{}{}", d.name, d.signature)
        } else {
            format!("{}.{}{}", d.module, d.name, d.signature)
        }
    };
    let w1 = docs.iter().map(|d| call(d).len()).max().unwrap_or(0).min(48);
    let w2 = docs.iter().map(|d| d.global.map_or(1, str::len)).max().unwrap_or(0).max(6);
    println!("{:w1$}  {:w2$}  SUMMARY", "FUNCTION", "GLOBAL");
    for d in docs {
        println!("{:w1$}  {:w2$}  {}", call(d), d.global.unwrap_or("-"), d.summary);
    }
    ExitCode::SUCCESS
}

// ----- building -----

struct Job {
    /// Input files (one, or several with --merge). Empty path = stdin.
    inputs: Vec<PathBuf>,
    /// None = stdout.
    output: Option<PathBuf>,
    sheet_name: String,
}

fn has_glob_chars(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

fn expand_inputs(patterns: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for pattern in patterns {
        if pattern == "-" {
            files.push(PathBuf::new());
            continue;
        }
        if !has_glob_chars(pattern) {
            let path = PathBuf::from(pattern);
            if !path.is_file() {
                return Err(format!("input file not found: {pattern}"));
            }
            files.push(path);
            continue;
        }
        let paths = glob::glob(pattern).map_err(|e| format!("invalid glob pattern {pattern}: {e}"))?;
        let mut matched = false;
        for entry in paths.flatten() {
            let is_partial = entry.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with('_'));
            if entry.is_file() && !is_partial {
                files.push(entry);
                matched = true;
            }
        }
        if !matched {
            return Err(format!("no files match {pattern}"));
        }
    }
    Ok(files)
}

fn plan(args: &BuildArgs) -> Result<Vec<Job>, String> {
    let files = expand_inputs(&args.inputs)?;
    let ext = match args.emit {
        Emit::Luau => "luau",
        Emit::Json => "json",
        Emit::Rbxmx => "rbxmx",
        Emit::Css => "css",
    };
    let stem = |p: &Path| p.file_stem().and_then(|s| s.to_str()).unwrap_or("StyleSheet").to_string();
    let to_output = |s: &str| if s == "-" { None } else { Some(PathBuf::from(s)) };

    if args.merge {
        let output = args.output.as_deref().map(to_output).unwrap_or_default();
        let name = output.as_deref().map(stem).unwrap_or_else(|| "StyleSheet".into());
        return Ok(vec![Job { inputs: files, output, sheet_name: name }]);
    }
    if args.output.is_some() && files.len() > 1 {
        return Err(format!(
            "--output needs a single input, but {} were given (use --merge or --out-dir)",
            files.len()
        ));
    }
    Ok(files
        .into_iter()
        .map(|input| {
            let is_stdin = input.as_os_str().is_empty();
            let output = if let Some(o) = &args.output {
                to_output(o)
            } else if is_stdin {
                None
            } else if let Some(dir) = &args.out_dir {
                Some(dir.join(format!("{}.{ext}", stem(&input))))
            } else {
                Some(input.with_extension(ext))
            };
            let sheet_name = if is_stdin { "StyleSheet".into() } else { stem(&input) };
            Job { inputs: vec![input], output, sheet_name }
        })
        .collect())
}

fn eval_options(args: &BuildArgs) -> Result<Options, String> {
    let mut defines = Vec::new();
    for def in &args.defines {
        let (name, value) = def.split_once('=').ok_or_else(|| format!("--define expects NAME=VALUE, got `{def}`"))?;
        let name = name.trim().trim_start_matches('$');
        let value = eval::evaluate_expression(value).map_err(|e| format!("--define {name}: {}", e.message))?;
        defines.push((name.to_string(), value));
    }
    Ok(Options { load_paths: args.load_paths.clone(), defines, ..Options::default() })
}

/// Reads `--tags`: a JSON object from each tag (or `#Name`) to a list of GuiObject class names.
fn read_tags(args: &BuildArgs) -> Result<HashMap<String, Vec<String>>, String> {
    let Some(path) = &args.tags else { return Ok(HashMap::new()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let tags = parse_tags(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    for (tag, classes) in &tags {
        if let Some(class) = classes.iter().find(|c| !codegen::GUI_OBJECT_CLASSES.contains(&c.as_str())) {
            return Err(format!(
                "{}: `{tag}` lists `{class}`, which isn't one of {}",
                path.display(),
                codegen::GUI_OBJECT_CLASSES.join(", ")
            ));
        }
    }
    Ok(tags)
}

/// `{"tag": ["Class", ...], ...}`; a single class may be a plain string.
fn parse_tags(text: &str) -> Result<HashMap<String, Vec<String>>, String> {
    let mut chars = text.chars().peekable();
    let skip_ws = |chars: &mut std::iter::Peekable<std::str::Chars>| {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
    };
    fn string(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
        if chars.next() != Some('"') {
            return Err("expected a string".into());
        }
        let mut s = String::new();
        loop {
            match chars.next().ok_or("unterminated string")? {
                '"' => return Ok(s),
                '\\' => match chars.next().ok_or("unterminated string")? {
                    'u' => {
                        let hex: String = chars.by_ref().take(4).collect();
                        let code = u32::from_str_radix(&hex, 16).map_err(|_| "bad \\u escape")?;
                        s.push(char::from_u32(code).ok_or("bad \\u escape")?);
                    }
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    c => s.push(c),
                },
                c => s.push(c),
            }
        }
    }
    let mut tags = HashMap::new();
    skip_ws(&mut chars);
    if chars.next() != Some('{') {
        return Err("expected a JSON object".into());
    }
    skip_ws(&mut chars);
    if chars.next_if_eq(&'}').is_none() {
        loop {
            skip_ws(&mut chars);
            let tag = string(&mut chars)?;
            skip_ws(&mut chars);
            if chars.next() != Some(':') {
                return Err(format!("expected `:` after \"{tag}\""));
            }
            skip_ws(&mut chars);
            let mut classes = Vec::new();
            if chars.next_if_eq(&'[').is_some() {
                skip_ws(&mut chars);
                if chars.next_if_eq(&']').is_none() {
                    loop {
                        skip_ws(&mut chars);
                        classes.push(string(&mut chars)?);
                        skip_ws(&mut chars);
                        match chars.next() {
                            Some(',') => {}
                            Some(']') => break,
                            _ => return Err(format!("expected `,` or `]` in the list for \"{tag}\"")),
                        }
                    }
                }
            } else {
                classes.push(string(&mut chars)?);
            }
            tags.insert(tag, classes);
            skip_ws(&mut chars);
            match chars.next() {
                Some(',') => {}
                Some('}') => break,
                _ => return Err("expected `,` or `}`".into()),
            }
        }
    }
    skip_ws(&mut chars);
    if chars.next().is_some() {
        return Err("unexpected text after the object".into());
    }
    Ok(tags)
}

fn codegen_options(
    args: &BuildArgs,
    sheet_name: &str,
    header: Option<String>,
    tags: &HashMap<String, Vec<String>>,
) -> CodegenOptions {
    let values = ValueOptions { allow_raw_luau: args.allow_raw_luau };
    CodegenOptions {
        approx: ApproxOptions {
            groups: Group::expand(&args.approx),
            strict: args.strict,
            tokens: HashMap::new(),
            inherited_family: None,
        },
        values,
        sheet_name: sheet_name.to_string(),
        header,
        tags: tags.clone(),
    }
}

/// Compiles one job. Returns the files it read (for --watch) and whether it succeeded.
fn run_job(job: &Job, args: &BuildArgs, opts: &Options, tags: &HashMap<String, Vec<String>>) -> (Vec<PathBuf>, bool) {
    let mut diag = Diagnostics::default();
    let mut combined = Sheet::default();
    let mut files = Vec::new();
    let mut ok = true;
    for input in &job.inputs {
        let result = if input.as_os_str().is_empty() {
            let mut source = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut source) {
                eprintln!("error: can't read stdin: {e}");
                return (files, false);
            }
            eval::compile_source(&source, Path::new("<stdin>"), Syntax::Scss, opts, &mut diag)
        } else {
            eval::compile_file(input, opts, &mut diag)
        };
        match result {
            Ok(sheet) => {
                files.extend(sheet.files.iter().cloned());
                combined.append(sheet);
            }
            Err(e) => {
                files.push(input.clone());
                print_diagnostics(&diag, args);
                eprintln!("{e}");
                return (files, false);
            }
        }
    }

    let label =
        job.inputs
            .iter()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    "<stdin>".to_string()
                } else {
                    p.display().to_string().replace('\\', "/")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
    let output = match args.emit {
        Emit::Luau => {
            codegen::emit_luau(&combined, &codegen_options(args, &job.sheet_name, Some(label), tags), &mut diag)
        }
        Emit::Json => codegen::emit_json(&combined, &codegen_options(args, &job.sheet_name, None, tags), &mut diag),
        Emit::Rbxmx => codegen::emit_rbxmx(&combined, &codegen_options(args, &job.sheet_name, None, tags), &mut diag),
        Emit::Css => codegen::emit_css(&combined),
    };
    print_diagnostics(&diag, args);
    if diag.error_count() > 0 {
        if args.strict {
            eprintln!(
                "error: {} layout difference(s) from CSS and --strict is set; no output written",
                diag.error_count()
            );
        } else {
            eprintln!("error: {} error(s); no output written", diag.error_count());
        }
        return (files, false);
    }
    if args.deny_warnings && diag.warning_count() > 0 {
        eprintln!("error: {} warning(s) and --deny-warnings is set; no output written", diag.warning_count());
        return (files, false);
    }
    match &job.output {
        None => {
            let _ = std::io::stdout().write_all(output.as_bytes());
        }
        Some(path) => {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
                && let Err(e) = std::fs::create_dir_all(dir)
            {
                eprintln!("error: can't create {}: {e}", dir.display());
                return (files, false);
            }
            if let Err(e) = std::fs::write(path, output) {
                eprintln!("error: can't write {}: {e}", path.display());
                ok = false;
            } else if !args.quiet {
                eprintln!("wrote {}", path.display());
            }
        }
    }
    (files, ok)
}

fn print_diagnostics(diag: &Diagnostics, args: &BuildArgs) {
    if args.quiet {
        return;
    }
    let color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    for d in &diag.items {
        let text = d.to_string();
        if color {
            let code = match d.level {
                Level::Warning => "33",
                Level::Error => "31",
                Level::Debug => "36",
            };
            eprintln!("\x1b[{code}m{text}\x1b[0m");
        } else {
            eprintln!("{text}");
        }
    }
}

fn usage_error(msg: String) -> ! {
    Cli::command().error(clap::error::ErrorKind::ValueValidation, msg).exit()
}

fn build(args: &BuildArgs) -> ExitCode {
    let jobs = plan(args).unwrap_or_else(|e| usage_error(e));
    // Every strict check is about how translated CSS lays out, so there's nothing to check without it.
    if args.strict && args.approx.is_empty() && !args.quiet {
        eprintln!(
            "warning: --strict checks how CSS properties are translated, which needs --approx; nothing was checked"
        );
    }
    let opts = eval_options(args).unwrap_or_else(|e| usage_error(e));
    let mut tags = read_tags(args).unwrap_or_else(|e| usage_error(e));
    if args.watch && jobs.iter().any(|j| j.inputs.iter().any(|i| i.as_os_str().is_empty())) {
        usage_error("--watch can't be used with stdin input".into());
    }

    let mut all_ok = true;
    let mut watched: Vec<(PathBuf, Option<SystemTime>)> = args.tags.iter().map(|p| (p.clone(), mtime(p))).collect();
    for job in &jobs {
        let (files, ok) = run_job(job, args, &opts, &tags);
        all_ok &= ok;
        watched.extend(files.into_iter().map(|f| {
            let t = mtime(&f);
            (f, t)
        }));
    }
    if !args.watch {
        return if all_ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }

    eprintln!("watching for changes (Ctrl+C to stop)...");
    loop {
        std::thread::sleep(Duration::from_millis(300));
        if !watched.iter().any(|(f, t)| mtime(f) != *t) {
            continue;
        }
        // Re-expand globs so new files are picked up.
        let jobs = plan(args).unwrap_or_default();
        match read_tags(args) {
            Ok(t) => tags = t,
            Err(e) => eprintln!("error: {e} (keeping the previous tags)"),
        }
        watched.clear();
        watched.extend(args.tags.iter().map(|p| (p.clone(), mtime(p))));
        for job in &jobs {
            let (files, _) = run_job(job, args, &opts, &tags);
            watched.extend(files.into_iter().map(|f| {
                let t = mtime(&f);
                (f, t)
            }));
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_files_are_json_objects_of_class_lists() {
        let tags =
            parse_tags(" {\"a\": [\"Frame\", \"TextLabel\"], \"#B\": \"ImageLabel\", \"c\\u0021\": []} ").unwrap();
        assert_eq!(tags["a"], vec!["Frame", "TextLabel"]);
        assert_eq!(tags["#B"], vec!["ImageLabel"]);
        assert!(tags["c!"].is_empty());
        assert!(parse_tags("{}").unwrap().is_empty());
        for bad in ["", "[]", "{\"a\": [\"Frame\"", "{\"a\" [\"Frame\"]}", "{\"a\": 1}", "{} x"] {
            assert!(parse_tags(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// Every command line shown in the `--help` examples must be accepted by the parser.
    #[test]
    fn help_examples_parse() {
        let mut count = 0;
        for line in EXAMPLES.lines() {
            let Some(cmd) = line.trim_start().strip_prefix("outlass ") else { continue };
            // The description is separated from the command by two or more spaces.
            let cmd = cmd.split("  ").next().unwrap();
            let argv = std::iter::once("outlass").chain(cmd.split_whitespace());
            if let Err(e) = Cli::try_parse_from(argv) {
                panic!("help example `outlass {cmd}` doesn't parse:\n{e}");
            }
            count += 1;
        }
        assert!(count >= 10, "expected to find the examples, found {count}");
    }

    #[test]
    fn approx_flag_does_not_swallow_inputs() {
        let cli = Cli::try_parse_from(["outlass", "--approx", "ui.scss"]).unwrap();
        assert_eq!(cli.build.inputs, vec!["ui.scss"]);
        assert_eq!(cli.build.approx, vec![Group::All]);
        let cli = Cli::try_parse_from(["outlass", "ui.scss", "--approx=opacity,box"]).unwrap();
        assert_eq!(cli.build.approx, vec![Group::Opacity, Group::Box]);
    }

    #[test]
    fn help_mentions_every_group() {
        let help = Cli::command().render_long_help().to_string();
        for g in Group::expand(&[Group::All]) {
            assert!(help.contains(g.name()), "--help doesn't mention group {}", g.name());
        }
    }
}
