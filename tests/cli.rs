//! Integration tests proving that every claim made in `outlass --help` (and the
//! `properties`/`functions` subcommand help) is actually true of the compiled binary.
//!
//! Every test shells out to the real binary via `CARGO_BIN_EXE_outlass`. No src/ code is
//! touched or imported directly.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------- helpers ----------

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_outlass")
}

/// Runs the binary with the given args and optional stdin, returning (exit_code, stdout, stderr).
fn run(args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd.spawn().expect("failed to spawn outlass binary");
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().expect("failed to wait on outlass");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Creates a fresh, empty temp directory under std::env::temp_dir() unique to this test process
/// and call site. The caller is responsible for removing it (via `TempDir`'s Drop, see below).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("outlass-cli-test-{pid}-{tag}-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compiles `scss` (written to a temp .scss file) with `extra_args`, printing to stdout.
/// Panics with full diagnostics if the compile fails. Returns (stdout, stderr).
fn compile(scss: &str, extra_args: &[&str]) -> (String, String) {
    let dir = TempDir::new("compile");
    let input = dir.write("in.scss", scss);
    let mut args: Vec<String> = vec![input.to_str().unwrap().to_string(), "-o".into(), "-".into()];
    args.extend(extra_args.iter().map(|s| s.to_string()));
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (code, stdout, stderr) = run(&arg_refs, None);
    assert_eq!(code, 0, "compile failed.\nargs: {:?}\nstdout: {stdout}\nstderr: {stderr}", arg_refs);
    (stdout, stderr)
}

// ---------- 2. default output paths ----------

#[test]
fn default_output_writes_luau_next_to_input() {
    let dir = TempDir::new("default-out");
    let input = dir.write("x.scss", ".a { Color: 1 }\n");
    let (code, _stdout, stderr) = run(&[input.to_str().unwrap()], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(dir.path().join("x.luau").is_file(), "expected x.luau next to x.scss");
}

#[test]
fn emit_css_writes_css_extension() {
    let dir = TempDir::new("emit-css");
    let input = dir.write("x.scss", ".a { Color: 1 }\n");
    let (code, _stdout, stderr) = run(&[input.to_str().unwrap(), "--emit", "css"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(dir.path().join("x.css").is_file(), "expected x.css next to x.scss");
}

#[test]
fn out_dir_creates_directory_and_writes_there() {
    let dir = TempDir::new("out-dir");
    let input = dir.write("x.scss", ".a { Color: 1 }\n");
    let outdir = dir.path().join("outdir");
    assert!(!outdir.exists());
    let (code, _stdout, stderr) = run(&[input.to_str().unwrap(), "-d", outdir.to_str().unwrap()], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(outdir.join("x.luau").is_file(), "expected outdir/x.luau to be created");
}

// ---------- 3. stdout / stdin ----------

#[test]
fn dash_output_prints_to_stdout_without_writing_a_file() {
    let dir = TempDir::new("dash-out");
    let input = dir.write("x.scss", ".a { Color: 1 }\n");
    let (code, stdout, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("return sheet"));
    assert!(!dir.path().join("x.luau").is_file(), "no file should have been written when -o - is used");
}

#[test]
fn dash_input_reads_scss_from_stdin() {
    let (code, stdout, stderr) = run(&["-", "-o", "-"], Some(".a { BackgroundTransparency: 1 }"));
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("[\"BackgroundTransparency\"] = 1"), "stdout:\n{stdout}");
}

// ---------- 4. --output / --merge ----------

#[test]
fn output_with_two_inputs_is_a_usage_error_mentioning_merge() {
    let dir = TempDir::new("output-two-inputs");
    let a = dir.write("a.scss", ".a { Color: 1 }\n");
    let b = dir.write("b.scss", ".b { Color: 2 }\n");
    let out = dir.path().join("out.luau");
    let (code, _stdout, stderr) = run(&["-o", out.to_str().unwrap(), a.to_str().unwrap(), b.to_str().unwrap()], None);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("--merge"), "stderr should mention --merge:\n{stderr}");
}

#[test]
fn merge_without_output_is_a_usage_error() {
    let dir = TempDir::new("merge-no-output");
    let a = dir.write("a.scss", ".a { Color: 1 }\n");
    let b = dir.write("b.scss", ".b { Color: 2 }\n");
    let (code, _stdout, stderr) = run(&["--merge", a.to_str().unwrap(), b.to_str().unwrap()], None);
    assert_eq!(code, 2, "stderr: {stderr}");
}

#[test]
fn merge_combines_inputs_into_one_sheet_named_after_output() {
    let dir = TempDir::new("merge-ok");
    let a = dir.write("a.scss", ".a { Color: 1 }\n");
    let b = dir.write("b.scss", ".b { Color: 2 }\n");
    let out = dir.path().join("all.luau");
    let (code, _stdout, stderr) =
        run(&["--merge", "-o", out.to_str().unwrap(), a.to_str().unwrap(), b.to_str().unwrap()], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains(r#"sheet.Name = "all""#), "content:\n{content}");
    assert!(content.contains(r#"rule(sheet, ".a", "#), "content:\n{content}");
    assert!(content.contains(r#"rule(sheet, ".b", "#), "content:\n{content}");
    // Only one `local sheet = Instance.new` — a single combined StyleSheet.
    assert_eq!(content.matches("local sheet = Instance.new(\"StyleSheet\")").count(), 1, "content:\n{content}");
}

// ---------- 5. globs ----------

#[test]
fn glob_compiles_matches_and_skips_partials() {
    let dir = TempDir::new("glob");
    dir.write("a.scss", ".a { Color: 1 }\n");
    dir.write("_p.scss", ".p { Color: 1 }\n");
    let pattern = dir.path().join("*.scss");
    let (code, _stdout, stderr) = run(&[pattern.to_str().unwrap()], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(dir.path().join("a.luau").is_file(), "a.luau should have been written");
    assert!(!dir.path().join("_p.luau").is_file(), "_p.luau (a partial) should be skipped");
}

#[test]
fn glob_matching_nothing_is_an_error() {
    let dir = TempDir::new("glob-empty");
    let pattern = dir.path().join("*.nonexistent");
    let (code, _stdout, stderr) = run(&[pattern.to_str().unwrap()], None);
    assert_eq!(code, 2, "stderr: {stderr}");
}

// ---------- 6. --approx ----------

#[test]
fn approx_before_input_translates_opacity() {
    let dir = TempDir::new("approx-before");
    let input = dir.write("x.scss", ".a { opacity: 0.25; }\n");
    let (code, stdout, stderr) = run(&["--approx", input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("[\"TextTransparency\"] = 0.75"), "stdout:\n{stdout}");
}

#[test]
fn approx_group_filter_translates_only_that_group() {
    let (stdout, stderr) = compile(".a { opacity: 0.25; border-radius: 4px; }", &["--approx=opacity"]);
    assert!(stdout.contains("[\"TextTransparency\"] = 0.75"), "stdout:\n{stdout}");
    assert!(!stdout.contains("UICorner"), "border-radius should not have been translated:\n{stdout}");
    assert!(stderr.contains("--approx=box"), "stderr should point at the box group:\n{stderr}");
}

#[test]
fn no_approx_warns_and_drops_css_property() {
    let (stdout, stderr) = compile(".a { opacity: 0.25; }", &[]);
    assert!(stderr.contains("--approx"), "stderr:\n{stderr}");
    assert!(!stdout.contains("Transparency"), "no Roblox property should have been produced:\n{stdout}");
}

// ---------- 7. -D / --define ----------

#[test]
fn define_overrides_default_declaration() {
    let (stdout, _stderr) =
        compile("$accent: red !default;\n.a { BackgroundColor3: $accent }", &["-D", "accent=#00ff00"]);
    assert!(stdout.contains("[\"BackgroundColor3\"] = Color3.fromRGB(0, 255, 0)"), "stdout:\n{stdout}");
}

#[test]
fn non_default_declaration_still_wins_over_define() {
    let (stdout, _stderr) = compile("$accent: blue;\n.a { BackgroundColor3: $accent }", &["-D", "accent=#00ff00"]);
    assert!(stdout.contains("[\"BackgroundColor3\"] = Color3.fromRGB(0, 0, 255)"), "stdout:\n{stdout}");
}

// ---------- 8. -I / --load-path ----------

#[test]
fn load_path_resolves_use_target() {
    let dir = TempDir::new("load-path");
    let libs = dir.path().join("libs");
    std::fs::create_dir_all(&libs).unwrap();
    std::fs::write(libs.join("_lib.scss"), "$foo: 1;\n.lib { Color: $foo }\n").unwrap();
    let input = dir.write("use.scss", "@use \"lib\";\n.a { Color: lib.$foo }\n");
    let (code, stdout, stderr) = run(&["-I", libs.to_str().unwrap(), input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains(r#"rule(sheet, ".lib", "#), "stdout:\n{stdout}");
    assert!(stdout.contains(r#"rule(sheet, ".a", "#), "stdout:\n{stdout}");
}

// ---------- 9. syntax detection ----------

#[test]
fn dot_sass_extension_is_autodetected() {
    let dir = TempDir::new("sass-auto");
    let input = dir.write("auto.sass", ".a\n  BackgroundTransparency: 1\n");
    let (code, stdout, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("[\"BackgroundTransparency\"] = 1"), "stdout:\n{stdout}");
}

// ---------- 10. the cascade and @layer ----------

/// The priority each selector got, in output order.
fn priorities(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix("rule(sheet, \"").or_else(|| l.split("= rule(sheet, \"").nth(1)))
        .map(|rest| {
            let (sel, rest) = rest.split_once("\", ").unwrap();
            (sel.to_string(), rest.split(',').next().unwrap().to_string())
        })
        .collect()
}

fn priority_of(stdout: &str, selector: &str) -> f64 {
    priorities(stdout)
        .into_iter()
        .find(|(s, _)| s == selector)
        .unwrap_or_else(|| panic!("no rule for {selector}:\n{stdout}"))
        .1
        .parse()
        .unwrap()
}

#[test]
fn css_cascade_orders_by_specificity_then_source_order() {
    let (out, _) = compile("#b { Color: 1 }\n.a { Color: 2 }\nFrame { Color: 3 }\n.c { Color: 4 }\n", &[]);
    assert!(priority_of(&out, "Frame") < priority_of(&out, ".a"), "{out}");
    assert!(priority_of(&out, ".a") < priority_of(&out, ".c"), "later wins at equal specificity:\n{out}");
    assert!(priority_of(&out, ".c") < priority_of(&out, "#b"), "{out}");
}

#[test]
fn layers_beat_specificity_and_unlayered_styles_beat_layers() {
    let (out, _) = compile(
        "@layer base, theme;\n@layer theme { .a { Color: 1 } }\n@layer base { #b { Color: 2 } }\n.c { Color: 3 }\n",
        &[],
    );
    assert!(priority_of(&out, ".a") > priority_of(&out, "#b"), "a later layer beats specificity:\n{out}");
    assert!(priority_of(&out, ".c") > priority_of(&out, ".a"), "unlayered styles beat every layer:\n{out}");
}

#[test]
fn important_declarations_beat_everything_normal() {
    let (out, stderr) = compile("#x.y { Color: 3 }\n.a { Color: 1 !important; Other: 2 }\n", &[]);
    assert!(!stderr.contains("important"), "{stderr}");
    let rules = priorities(&out);
    let a: Vec<_> = rules.iter().filter(|(s, _)| s == ".a").collect();
    assert_eq!(a.len(), 2, "normal and !important parts:\n{out}");
    let top = rules.iter().map(|(_, p)| p.parse::<f64>().unwrap()).fold(f64::MIN, f64::max);
    assert_eq!(a[1].1.parse::<f64>().unwrap(), top, "{out}");
    let important_part = &out[out.rfind("\".a\"").unwrap()..];
    assert!(important_part[..important_part.find('}').unwrap()].contains("[\"Color\"] = 1"), "{out}");
    assert!(!important_part[..important_part.find('}').unwrap()].contains("Other"), "{out}");
}

#[test]
fn important_reverses_the_layer_order() {
    let (out, _) = compile(
        "@layer base, theme;\n@layer base { .a { Color: 1 !important } }\n@layer theme { .b { Color: 2 !important } }\n\
         .c { Color: 3 !important }\n",
        &[],
    );
    assert!(priority_of(&out, ".a") > priority_of(&out, ".b"), "an earlier layer's !important wins:\n{out}");
    assert!(priority_of(&out, ".b") > priority_of(&out, ".c"), "a layer's !important beats an unlayered one:\n{out}");
}
// ---------- 11. colors ----------

#[test]
fn colors_become_from_rgb() {
    let (stdout, _stderr) = compile(".a { Color: rgb(255, 0, 0) }", &[]);
    assert!(stdout.contains("Color3.fromRGB(255, 0, 0)"), "stdout:\n{stdout}");
}

// ---------- 12. rem ----------

#[test]
fn rem_is_16px_on_roblox_properties() {
    let (stdout, _stderr) = compile(".a { TextSize: 2rem }", &[]);
    assert!(stdout.contains("[\"TextSize\"] = 32"), "stdout:\n{stdout}");
}

#[test]
fn rem_is_16px_in_approx_font_size() {
    let (stdout, _stderr) = compile(".a { font-size: 1.5rem }", &["--approx"]);
    assert!(stdout.contains("[\"TextSize\"] = 24"), "stdout:\n{stdout}");
}

// ---------- 13. default font ----------

#[test]
fn default_font_used_when_only_weight_given() {
    let (stdout, _stderr) = compile(".a { font-weight: bold }", &["--approx"]);
    assert!(stdout.contains("rbxasset://fonts/families/SourceSansPro.json"), "stdout:\n{stdout}");
}

// ---------- 14. sheet name / header ----------

#[test]
fn stylesheet_is_named_after_the_input_and_has_a_header() {
    let (stdout, _stderr) = compile(".a { Color: 1 }", &[]);
    assert!(stdout.contains(r#"sheet.Name = "in""#), "stdout:\n{stdout}");
    assert!(stdout.contains("Generated by outlass"), "stdout:\n{stdout}");
}

// ---------- 15. -q / --deny-warnings ----------

#[test]
fn quiet_suppresses_warnings() {
    let dir = TempDir::new("quiet");
    let input = dir.write("warn.scss", ".a { opacity: 0.5 }\n");
    let (code, _stdout, stderr) = run(&["-q", input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0);
    assert!(stderr.is_empty(), "stderr should be empty with -q:\n{stderr}");
}

#[test]
fn deny_warnings_fails_and_writes_no_file_when_warnings_present() {
    let dir = TempDir::new("deny-warn");
    let input = dir.write("warn.scss", ".a { opacity: 0.5 }\n");
    let out = dir.path().join("dw.luau");
    let (code, _stdout, stderr) = run(&["--deny-warnings", input.to_str().unwrap(), "-o", out.to_str().unwrap()], None);
    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(!out.exists(), "no output file should be written when --deny-warnings trips");
}

#[test]
fn deny_warnings_succeeds_when_no_warnings() {
    let dir = TempDir::new("deny-warn-ok");
    let input = dir.write("ok.scss", ".a { Color: 1 }\n");
    let out = dir.path().join("dw.luau");
    let (code, _stdout, stderr) = run(&["--deny-warnings", input.to_str().unwrap(), "-o", out.to_str().unwrap()], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(out.is_file());
}

// ---------- 16. --watch + stdin ----------

#[test]
fn watch_cannot_be_combined_with_stdin() {
    let (code, _stdout, stderr) = run(&["--watch", "-", "-o", "-"], Some(".a { Color: 1 }"));
    assert_eq!(code, 2, "stderr: {stderr}");
}

// ---------- 17. compile errors ----------

#[test]
fn compile_error_has_file_line_col_location() {
    let dir = TempDir::new("compile-error");
    let input = dir.write("err.scss", ".a {\n  Color: $undefined;\n}\n");
    let (code, _stdout, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(stderr.contains("error:"), "stderr:\n{stderr}");
    assert!(stderr.contains(":2:"), "stderr should reference line 2:\n{stderr}");
}

// ---------- 18. `properties` subcommand ----------

#[test]
fn properties_lists_group_headings() {
    let (code, stdout, stderr) = run(&["properties"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("box (--approx=box)"), "stdout:\n{stdout}");
}

#[test]
fn properties_single_property_mentions_uicorner() {
    let (code, stdout, stderr) = run(&["properties", "border-radius"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("UICorner"), "stdout:\n{stdout}");
}

#[test]
fn properties_group_filter_shows_only_that_group() {
    let (code, stdout, stderr) = run(&["properties", "-g", "opacity"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("opacity"), "stdout:\n{stdout}");
    assert!(!stdout.contains("border-radius"), "stdout:\n{stdout}");
}

#[test]
fn properties_unknown_property_is_an_error() {
    let (code, _stdout, _stderr) = run(&["properties", "float"], None);
    assert_eq!(code, 1);
}

// ---------- 19. `functions` subcommand ----------

#[test]
fn functions_lists_math_div_and_lighten() {
    let (code, stdout, stderr) = run(&["functions"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("math.div"), "stdout:\n{stdout}");
    assert!(stdout.contains("lighten"), "stdout:\n{stdout}");
}

#[test]
fn functions_filter_color_shows_color_functions_only() {
    let (code, stdout, stderr) = run(&["functions", "color"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("color.adjust"), "stdout:\n{stdout}");
    assert!(!stdout.contains("math.div"), "stdout:\n{stdout}");
}

#[test]
fn functions_filter_matching_nothing_is_an_error() {
    let (code, _stdout, _stderr) = run(&["functions", "zzzz"], None);
    assert_eq!(code, 1);
}

// ---------- 20. --version / no args ----------

#[test]
fn version_prints_cargo_version() {
    let (code, stdout, stderr) = run(&["--version"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    let expected = env!("CARGO_PKG_VERSION");
    assert!(stdout.contains(expected), "stdout `{stdout}` should contain version `{expected}`");
}

#[test]
fn no_args_is_a_usage_error() {
    let (code, _stdout, _stderr) = run(&[], None);
    assert_eq!(code, 2);
}

// ---------- 21. Roblox mapping claims ----------

#[test]
fn descendant_combinator_becomes_double_gt() {
    let (stdout, _stderr) = compile(".a .b { Color: 1 }", &[]);
    assert!(stdout.contains(r#"".a >> .b""#), "stdout:\n{stdout}");
}

#[test]
fn pseudo_classes_map_to_roblox_states() {
    let (stdout, _stderr) = compile(".a:hover { Color: 1 }\n.b:active { Color: 2 }\n.c:disabled { Color: 3 }\n", &[]);
    assert!(stdout.contains(r#"".a:Hover""#), "stdout:\n{stdout}");
    assert!(stdout.contains(r#"".b:Press""#), "stdout:\n{stdout}");
    assert!(stdout.contains(r#"".c:NonInteractable""#), "stdout:\n{stdout}");
}

#[test]
fn custom_property_in_rule_becomes_set_attribute() {
    let (stdout, _stderr) = compile(".a { --Name: blue; }", &[]);
    assert!(stdout.contains(r#"SetAttribute("Name", Color3.fromRGB(0, 0, 255))"#), "stdout:\n{stdout}");
}

#[test]
fn custom_property_in_root_goes_on_stylesheet() {
    let (stdout, _stderr) = compile(":root { --Red: red; }", &[]);
    assert!(stdout.contains(r#"sheet:SetAttribute("Red", Color3.fromRGB(255, 0, 0))"#), "stdout:\n{stdout}");
}

#[test]
fn var_reference_becomes_token_string() {
    let (stdout, _stderr) = compile(":root { --Brand: red; }\n.a { Color: var(--Brand); }", &[]);
    assert!(stdout.contains(r#"["Color"] = "$Brand""#), "stdout:\n{stdout}");
}

#[test]
fn nested_layers_rank_below_their_parents_own_rules() {
    let scss = "@layer a {\n  .x {\n    Color: 1;\n    &:hover { Color: 2 }\n  }\n  @layer inner { #y { Color: 3 } }\n}\n\
                .z { @layer a { Color: 4 } }\n";
    let (out, _) = compile(scss, &[]);
    assert!(priority_of(&out, ".x") > priority_of(&out, "#y"), "a layer's own rules beat its sublayers:\n{out}");
    assert!(priority_of(&out, ".x:Hover") > priority_of(&out, ".x"), "{out}");
    assert!(priority_of(&out, ".z") > priority_of(&out, "#y"), "a layer inside a rule is the same layer:\n{out}");
}

#[test]
fn grid_tracks_become_a_cell_count_and_cell_size() {
    let scss = ".grid {\n  display: grid;\n  grid-template-columns: repeat(3, 68px);\n  grid-auto-rows: 68px;\n  gap: 6px;\n}\n";
    let (stdout, stderr) = compile(scss, &["--approx"]);
    assert!(stdout.contains("[\"FillDirectionMaxCells\"] = 3"), "stdout:\n{stdout}");
    assert!(stdout.contains("[\"CellSize\"] = UDim2.new(0, 68, 0, 68)"), "stdout:\n{stdout}");
    assert!(stdout.contains("[\"CellPadding\"] = UDim2.new(0, 6, 0, 6)"), "stdout:\n{stdout}");
    assert!(!stderr.contains("warning"), "stderr:\n{stderr}");
}

#[test]
fn transition_property_becomes_tween_info() {
    let (stdout, _stderr) = compile(".a { Transition: BackgroundColor3 0.2s Linear In; }", &[]);
    assert!(
        stdout.contains("TweenInfo.new(0.2, Enum.EasingStyle.Linear, Enum.EasingDirection.In)"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn pascal_case_at_rule_prefixes_its_rules_with_the_query() {
    let (stdout, _stderr) = compile("@PreferredInputTouch { Frame { Visible: false } }", &[]);
    assert!(stdout.contains(r#"rule(sheet, "@PreferredInputTouch Frame", "#), "stdout:\n{stdout}");
    // A hand-made query (a StyleQuery the author created) is referenced by name.
    let (stdout, _stderr) = compile("@MyQuery { .a { Visible: false } }", &[]);
    assert!(stdout.contains(r#""@MyQuery .a""#), "stdout:\n{stdout}");
}

#[test]
fn media_features_with_documented_equivalents_use_builtin_queries() {
    let cases = [
        ("(prefers-reduced-motion: reduce)", "@ReducedMotionEnabledTrue"),
        ("(prefers-reduced-motion: no-preference)", "@ReducedMotionEnabledFalse"),
        ("(pointer: coarse)", "@PreferredInputTouch"),
        ("(pointer: fine)", "@PreferredInputKeyboardAndMouse"),
        ("(any-pointer: coarse)", "@PreferredInputGamepad"),
        ("(max-width: 600px)", "@ViewportDisplaySizeSmall"),
        ("(min-width: 601px) and (max-width: 1200px)", "@ViewportDisplaySizeMedium"),
        ("(min-width: 1201px)", "@ViewportDisplaySizeLarge"),
    ];
    for (media, query) in cases {
        let (stdout, _stderr) = compile(&format!("@media {media} {{ .c {{ Color: 1 }} }}"), &[]);
        assert!(stdout.contains(&format!("\"{query} .c\"")), "{media} should be {query}:\n{stdout}");
        assert!(!stdout.contains("::StyleQuery"), "built-ins need no StyleQuery:\n{stdout}");
    }
}

#[test]
fn other_media_queries_become_a_style_query_on_the_screen_gui() {
    let (out, stderr) = compile(".a { @media (min-width: 900px) and (orientation: landscape) { ZIndex: 2; } }", &[]);
    assert!(stderr.is_empty(), "{stderr}");
    assert!(out.contains("rule(sheet, \"ScreenGui::StyleQuery #MediaMinWidth900MinAspect1\", nil, {\n\t[\"MinSize\"] = Vector2.new(900, 0),\n\t[\"AspectRatioRange\"] = NumberRange.new(1, math.huge),"), "{out}");
    assert!(out.contains("\"@MediaMinWidth900MinAspect1 .a\""), "{out}");
}

#[test]
fn container_queries_attach_a_style_query_to_each_container() {
    let scss = ".panel { container-type: inline-size; }\n.side { container: sidebar / inline-size; }\n\
                @container (min-width: 400px) { .title { ZIndex: 2; } }\n\
                @container sidebar (max-width: 200px) { .icon { ZIndex: 3; } }";
    let (out, stderr) = compile(scss, &["--approx"]);
    assert!(!stderr.contains("container"), "container declarations are consumed, not warned about:\n{stderr}");
    assert!(
        out.contains("\".panel::StyleQuery #ContainerMinWidth400, .side::StyleQuery #ContainerMinWidth400\""),
        "{out}"
    );
    assert!(out.contains("\".side::StyleQuery #ContainerSidebarMaxWidth200\""), "{out}");
    assert!(
        !out.contains(".panel::StyleQuery #ContainerSidebar"),
        "named queries only use matching containers:\n{out}"
    );
    assert!(out.contains("\"@ContainerMinWidth400 .title\""), "{out}");
    assert!(out.contains("\"@ContainerSidebarMaxWidth200 .icon\""), "{out}");
    let (_, stderr) = compile("@container (min-width: 400px) { .t { ZIndex: 2; } }", &[]);
    assert!(stderr.contains("no element is declared a container"), "{stderr}");
}

#[test]
fn nested_and_listed_queries() {
    let (out, _) = compile("@media (pointer: coarse) { @media (min-width: 800px) { .a { ZIndex: 1; } } }", &[]);
    assert!(out.contains("\"@MediaMinWidth800Touch .a\""), "nested queries combine into one:\n{out}");
    let (out, _) = compile("@media (pointer: coarse), (max-width: 600px) { .a { ZIndex: 1; } }", &[]);
    assert!(out.contains("\"@PreferredInputTouch .a\"") && out.contains("\"@ViewportDisplaySizeSmall .a\""), "{out}");
    let (out, stderr) = compile("@media print { .a { ZIndex: 1; } } @media not screen { .b { ZIndex: 1; } }", &[]);
    assert!(!out.contains(".a\"") && !out.contains(".b\""), "{out}");
    assert!(stderr.contains("not"), "{stderr}");
}

#[test]
fn value_units_are_converted() {
    let (stdout, _stderr) = compile(".vals {\n  Size: 10px;\n  Opacity: 50%;\n  Time: 1000ms;\n}\n", &[]);
    assert!(stdout.contains("[\"Size\"] = 10"), "stdout:\n{stdout}");
    assert!(stdout.contains("[\"Opacity\"] = 0.5"), "stdout:\n{stdout}");
    assert!(stdout.contains("[\"Time\"] = 1"), "stdout:\n{stdout}");
}

// ----- regressions found during review -----

#[test]
fn grid_gap_does_not_create_list_layout() {
    let (out, _) = compile(".g { display: grid; gap: 4px 8px; }", &["--approx"]);
    assert!(out.contains("[\"CellPadding\"] = UDim2.new(0, 8, 0, 4)"), "{out}");
    assert!(!out.contains("UIListLayout"), "grid gap leaked into a UIListLayout:\n{out}");
}

#[test]
fn clamp_folds_compatible_numbers() {
    let (out, _) = compile(".a { TextSize: clamp(10px, 30px, 24px); ZIndex: clamp(1, 0, 5); }", &[]);
    assert!(out.contains("[\"TextSize\"] = 24"), "{out}");
    assert!(out.contains("[\"ZIndex\"] = 1"), "{out}");
}

#[test]
fn properties_accepts_vendor_prefixed_names() {
    let (code, stdout, stderr) = run(&["properties", "-webkit-text-stroke"], None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("UIStroke"), "{stdout}");
}

#[test]
fn raw_color_alpha_is_warned_about() {
    let (_, stderr) = compile(".a { BackgroundColor3: rgba(0, 0, 0, 0.5); }", &[]);
    assert!(stderr.contains("Color3 has no alpha channel"), "{stderr}");
}

#[test]
fn single_axis_size_warns() {
    let (out, stderr) = compile(".a { height: 48px; }", &["--approx=size"]);
    assert!(out.contains("[\"AutomaticSize\"] = Enum.AutomaticSize.X"), "{out}");
    assert!(stderr.contains("only `height` is set"), "{stderr}");
}

// ----- CSS fidelity: idiomatic CSS maps onto faithful Roblox results -----

#[test]
fn absolute_edges_stretch_like_css() {
    let (out, stderr) = compile(
        ".fill { position: absolute; inset: 0; } .bar { position: absolute; top: 8px; left: 12px; right: 12px; height: 4px; }",
        &["--approx"],
    );
    assert!(out.contains("[\"Size\"] = UDim2.new(1, 0, 1, 0)"), "{out}");
    assert!(out.contains("[\"Size\"] = UDim2.new(1, -24, 0, 4)"), "{out}");
    assert!(out.contains("[\"Position\"] = UDim2.new(0, 12, 0, 8)"), "{out}");
    assert!(!stderr.contains("both set"), "{stderr}");
    assert!(!stderr.contains("only `height`"), "{stderr}");
}

#[test]
fn composite_properties_cascade_per_axis() {
    let (out, _) = compile(
        ".bar { position: absolute; top: 36px; width: 3px; height: 9px; &.left { left: 34%; } }",
        &["--approx"],
    );
    let left_rule = &out[out.find("\".bar.left\"").unwrap()..];
    assert!(left_rule.contains("[\"Position\"] = UDim2.new(0.34, 0, 0, 36)"), "{out}");
}

#[test]
fn opacity_fades_canvas_groups() {
    let (out, _) = compile(".card { opacity: 0.25; transition: opacity 0.2s ease-out; }", &["--approx"]);
    assert!(out.contains("[\"GroupTransparency\"] = 0.75"), "{out}");
    assert!(out.contains("[\"TextTransparency\"] = 0.75"), "{out}");
    assert!(out.contains("[\"GroupTransparency\"] = TweenInfo.new(0.2"), "{out}");
}

#[test]
fn mask_image_becomes_gradient_transparency() {
    let (out, stderr) = compile(
        ".track { mask-image: linear-gradient(to right, transparent, black 14%, black 86%, transparent); }",
        &["--approx"],
    );
    assert!(out.contains("[\"Transparency\"] = NumberSequence.new({NumberSequenceKeypoint.new(0, 1), NumberSequenceKeypoint.new(0.14, 0), NumberSequenceKeypoint.new(0.86, 0), NumberSequenceKeypoint.new(1, 1)})"), "{out}");
    assert!(out.contains("[\"Rotation\"] = 0"), "{out}");
    assert!(!stderr.contains("mask-image"), "{stderr}");
}

#[test]
fn gradient_text_idiom() {
    let (out, stderr) = compile(
        ".gold { background: linear-gradient(#ffe29a, #ffb347); background-clip: text; color: transparent; }",
        &["--approx"],
    );
    assert!(out.contains("[\"TextColor3\"] = Color3.fromRGB(255, 255, 255)"), "{out}");
    assert!(out.contains("[\"BackgroundTransparency\"] = 1"), "{out}");
    assert!(!out.contains("[\"TextTransparency\"] = 1"), "text must stay visible:\n{out}");
    assert!(out.contains("\".gold::UIGradient\""), "{out}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn cubic_bezier_maps_to_roblox_easing() {
    // Properties with distinct Roblox targets, so each entry's easing is visible.
    let (out, stderr) = compile(
        ".a { transition: left 0.3s cubic-bezier(0.34, 1.56, 0.64, 1), color 1s cubic-bezier(0.16, 1, 0.3, 1), z-index 1s cubic-bezier(0.25, 0.25, 0.75, 0.75); }",
        &["--approx"],
    );
    assert!(out.contains("TweenInfo.new(0.3, Enum.EasingStyle.Back, Enum.EasingDirection.Out)"), "{out}");
    assert!(out.contains("TweenInfo.new(1, Enum.EasingStyle.Quint, Enum.EasingDirection.Out)"), "{out}");
    assert!(out.contains("TweenInfo.new(1, Enum.EasingStyle.Linear, Enum.EasingDirection.InOut)"), "{out}");
    assert!(!stderr.contains("cubic-bezier"), "{stderr}");
}

#[test]
fn transitions_reach_pseudo_instances() {
    let (out, _) = compile(".pop { transform: scale(1); transition: transform 0.2s ease-out; }", &["--approx"]);
    let scale_rule = &out[out.find("\".pop::UIScale\"").unwrap()..];
    assert!(scale_rule.contains("[\"Scale\"] = TweenInfo.new(0.2"), "{out}");
}

#[test]
fn borders_clear_the_legacy_border() {
    let (out, _) = compile("Frame { border: none; } .card { border: 1px solid #fff; }", &["--approx"]);
    // Both rules, and the user-agent default under every GuiObject (a CSS box has no border).
    assert_eq!(out.matches("[\"BorderSizePixel\"] = 0").count(), 3, "{out}");
    assert!(out.contains("[\"Enabled\"] = false"), "{out}");
    assert!(out.contains("[\"Enabled\"] = true"), "{out}");
}

#[test]
fn background_none_only_clears_the_background() {
    let (out, _) = compile(".label { background: none; }", &["--approx"]);
    assert!(out.contains("[\"BackgroundTransparency\"] = 1"), "{out}");
    assert!(!out.contains("BackgroundColor3"), "{out}");
}

#[test]
fn repeating_linear_gradient_is_written_out_stop_by_stop() {
    // Roblox has no repeating gradient, but a UIGradient holds 20 keypoints, which is room for
    // the pattern written out by hand.
    let (out, stderr) = compile(
        ".stripes { width: 100px; height: 100px; background: repeating-linear-gradient(45deg, #222 0 10%, #444 10% 20%); }",
        &["--approx"],
    );
    let gradient = &out[out.find("\".stripes::UIGradient\"").unwrap()..];
    // Five copies of the two-color pattern, each ending in a hard stop (two keypoints at one time).
    assert_eq!(gradient[..gradient.find('}').unwrap()].matches("ColorSequenceKeypoint.new(").count(), 20, "{out}");
    assert!(gradient.contains(r##"ColorSequenceKeypoint.new(0, Color3.fromRGB(34, 34, 34)), ColorSequenceKeypoint.new(0.1, Color3.fromRGB(34, 34, 34)), ColorSequenceKeypoint.new(0.1, Color3.fromRGB(68, 68, 68)), ColorSequenceKeypoint.new(0.2, Color3.fromRGB(68, 68, 68)), ColorSequenceKeypoint.new(0.2, Color3.fromRGB(34, 34, 34))"##), "{out}");
    assert!(gradient.contains("[\"Rotation\"] = -45"), "{out}");
    assert!(stderr.is_empty(), "{stderr}");

    // A mask repeats the same way, and its hard stops survive as two keypoints at one time.
    let (out, _) = compile(
        ".fade { mask-image: repeating-linear-gradient(to right, black 0 10%, transparent 10% 20%); }",
        &["--approx"],
    );
    assert!(
        out.contains(
            "NumberSequenceKeypoint.new(0, 0), NumberSequenceKeypoint.new(0.1, 0), NumberSequenceKeypoint.new(0.1, 1)"
        ),
        "{out}"
    );
}

#[test]
fn a_repeating_gradient_that_does_not_fit_warns_instead_of_erroring_in_roblox() {
    let (out, stderr) =
        compile(".fine { background: repeating-linear-gradient(90deg, #f00 0 2%, #00f 2% 4%); }", &["--approx"]);
    let gradient = &out[out.find("\".fine::UIGradient\"").unwrap()..];
    let gradient = &gradient[..gradient.find('}').unwrap()];
    assert_eq!(gradient.matches("ColorSequenceKeypoint.new(").count(), 20, "{out}");
    // Roblox rejects a sequence that doesn't end at 1.
    assert!(gradient.contains("ColorSequenceKeypoint.new(1, "), "{out}");
    assert!(stderr.contains("a Roblox sequence holds 20"), "{stderr}");
}

#[test]
fn gradient_stop_positions_follow_css() {
    // A first stop past 0 holds its color back to the start, rather than being dragged to 0.
    let (out, stderr) = compile(".a { background: linear-gradient(to right, red 50%, blue); }", &["--approx"]);
    assert!(out.contains(r##"ColorSequenceKeypoint.new(0, Color3.fromRGB(255, 0, 0)), ColorSequenceKeypoint.new(0.5, Color3.fromRGB(255, 0, 0)), ColorSequenceKeypoint.new(1, Color3.fromRGB(0, 0, 255))"##), "{out}");
    assert!(stderr.is_empty(), "{stderr}");

    // A bare percentage is a color hint: the two colors are mixed half and half there.
    let (out, _) = compile(".b { background: linear-gradient(black, 25%, white); }", &["--approx"]);
    assert!(out.contains(r##"ColorSequenceKeypoint.new(0.25, Color3.fromRGB(128, 128, 128))"##), "{out}");

    // Radial and conic gradients say what's wrong instead of "expected a color".
    let (_, stderr) = compile(".c { background: radial-gradient(red, blue); }", &["--approx"]);
    assert!(stderr.contains("a UIGradient is linear"), "{stderr}");

    // A stack of background layers still finds the gradient in it, instead of dropping the whole
    // declaration on the floor.
    let (out, stderr) = compile(".d { background: linear-gradient(#000, #111), #222; }", &["--approx"]);
    assert!(out.contains("\".d::UIGradient\""), "{out}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn a_gradient_is_painted_over_the_background_color_like_css() {
    // A UIGradient multiplies BackgroundColor3 and has no way to cover it, where CSS paints
    // `background-image` over `background-color`. So the color is flattened into the stops and the
    // element is painted white, which makes the multiply a no-op.
    //
    // Opaque stops hide the color completely, exactly as they do in a browser.
    let (out, stderr) = compile(
        ".plate { background-color: #204060; background-image: linear-gradient(#ffffff, #000000); }",
        &["--approx"],
    );
    assert!(out.contains(r##"["BackgroundColor3"] = Color3.fromRGB(255, 255, 255)"##), "{out}");
    assert!(
        !out.contains(r##"Color3.fromRGB(32, 64, 96)"##),
        "an opaque gradient hides the color:
{out}"
    );
    assert!(out.contains(r##"ColorSequenceKeypoint.new(0, Color3.fromRGB(255, 255, 255)), ColorSequenceKeypoint.new(1, Color3.fromRGB(0, 0, 0))"##), "{out}");
    assert!(stderr.is_empty(), "{stderr}");

    // Translucent stops blend with it instead: 50% red over black is half-brightness red, opaque.
    let (out, _) = compile(
        ".tint { background-color: #000000; background-image: linear-gradient(rgba(255, 0, 0, 0.5), rgba(255, 0, 0, 0.5)); }",
        &["--approx"],
    );
    assert!(out.contains(r##"ColorSequenceKeypoint.new(0, Color3.fromRGB(128, 0, 0))"##), "{out}");
    assert!(
        !out.contains("[\"Transparency\"] = NumberSequence"),
        "the blend is opaque:
{out}"
    );

    // With nothing underneath, the stop alphas stay in the UIGradient's Transparency.
    let (out, _) = compile(".fade { background-image: linear-gradient(#f00, transparent); }", &["--approx"]);
    assert!(
        out.contains(
            "[\"Transparency\"] = NumberSequence.new({NumberSequenceKeypoint.new(0, 0), NumberSequenceKeypoint.new(1, 1)})"
        ),
        "{out}"
    );
}

#[test]
fn text_stroke_survives_a_border_reset() {
    let (out, _) = compile("TextLabel { border: none; } .title { -webkit-text-stroke: 1px black; }", &["--approx"]);
    let title = &out[out.find("\".title::UIStroke\"").unwrap()..];
    assert!(title[..title.find('}').unwrap()].contains("[\"Enabled\"] = true"), "{out}");
}

#[test]
fn webkit_text_stroke_longhands_and_current_color() {
    // -webkit-text-stroke is the property browsers implement; the unprefixed spelling isn't CSS.
    let (code, stdout, _) = run(&["properties", "-webkit-text-stroke-color"], None);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.starts_with("-webkit-text-stroke-color"), "{stdout}");

    // An omitted color is currentColor, i.e. the rule's own `color`.
    let (out, stderr) = compile(
        ".a { color: #ff0000; -webkit-text-stroke: 2px; }          .b { -webkit-text-stroke-width: 1px; -webkit-text-stroke-color: #00ff00; }          .c { -webkit-text-stroke: 0 #000; }",
        &["--approx"],
    );
    let a = &out[out.find("\".a::UIStroke\"").unwrap()..];
    assert!(a[..a.find('}').unwrap()].contains(r##"["Color"] = Color3.fromRGB(255, 0, 0)"##), "{out}");
    let b = &out[out.find("\".b::UIStroke\"").unwrap()..];
    let b = &b[..b.find('}').unwrap()];
    assert!(b.contains("[\"Thickness\"] = 1") && b.contains(r##"["Color"] = Color3.fromRGB(0, 255, 0)"##), "{out}");
    // A zero width draws nothing, and says so, so it can undo a weaker rule's stroke.
    let c = &out[out.find("\".c::UIStroke\"").unwrap()..];
    assert!(c[..c.find('}').unwrap()].contains("[\"Enabled\"] = false"), "{out}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn star_transition_is_the_default_transition() {
    let (out, _) = compile("Frame { Transition: * 0.5s; }", &[]);
    assert!(
        out.contains(
            "SetDefaultPropertyTransition(TweenInfo.new(0.5, Enum.EasingStyle.Quad, Enum.EasingDirection.Out))"
        ),
        "{out}"
    );
}

#[test]
fn cascaded_state_rules_do_not_add_properties() {
    // `.t.positive` sets a color, so it emits that color and the transparency the color implies
    // (combined with the opacity it inherits from `.t`) — but nothing else: it must not re-emit the
    // base rule's Size, which would compete with the sibling `.wide` state.
    let (out, _) = compile(
        ".t { opacity: 0; width: 10px; height: 10px; } .t.positive { color: red; } .wide .t { width: 50px; height: 10px; }",
        &["--approx"],
    );
    let positive = &out[out.find("\".t.positive\"").unwrap()..];
    let positive = &positive[..positive.find('}').unwrap()];
    assert!(positive.contains("TextColor3"), "{out}");
    assert!(
        positive.contains("[\"TextTransparency\"] = 1"),
        "the inherited `opacity: 0` still applies:
{out}"
    );
    assert!(!positive.contains("Size"), "{out}");
}

#[test]
fn an_opaque_color_undoes_a_weaker_rules_transparency() {
    // Roblox has one Transparency slot where CSS has a color alpha and an `opacity`, so a rule that
    // paints an opaque background has to say so; otherwise the weaker rule's transparency sticks and
    // the element stays invisible.
    let (out, _) = compile(
        ".panel { background-color: transparent; color: rgba(255, 0, 0, 0.5); }          .panel.solid { background-color: #123456; color: #fff; }",
        &["--approx"],
    );
    let solid = &out[out.find("\".panel.solid\"").unwrap()..];
    let solid = &solid[..solid.find('}').unwrap()];
    assert!(solid.contains("[\"BackgroundTransparency\"] = 0"), "{out}");
    assert!(solid.contains("[\"TextTransparency\"] = 0"), "{out}");
    assert!(priority_of(&out, ".panel.solid") > priority_of(&out, ".panel"), "{out}");
}

#[test]
fn utf8_byte_order_mark_is_ignored() {
    let (out, _) = compile("\u{FEFF}// comment\n.a { ZIndex: 2; }", &[]);
    assert!(out.contains("[\"ZIndex\"] = 2"), "{out}");
}

#[test]
fn transition_only_pseudo_rules_are_limited_to_neutral_classes() {
    let (out, _) =
        compile(".t { opacity: 0; transition: opacity 0.2s, transform 0.2s, border-radius 0.2s; }", &["--approx"]);
    assert!(out.contains("\".t::UIScale\""), "{out}");
    assert!(!out.contains("\".t::UIStroke\""), "a default UIStroke would draw an outline:\n{out}");
    assert!(!out.contains("\".t::UICorner\""), "a default UICorner would round the corners:\n{out}");
}

#[test]
fn stroke_resets_are_dropped_when_nothing_enables_a_stroke() {
    let (out, _) = compile("Frame { border: none; }", &["--approx"]);
    assert!(out.contains("[\"BorderSizePixel\"] = 0"), "{out}");
    assert!(!out.contains("UIStroke"), "{out}");
    let (out, _) = compile("Frame { border: none; } .card { border: 1px solid red; }", &["--approx"]);
    assert!(out.contains("\"Frame::UIStroke\""), "the reset matters once a stroke exists:\n{out}");
}

#[test]
fn zero_percent_translate_still_sets_the_anchor() {
    let (out, _) =
        compile(".tl { position: absolute; left: 0%; top: 0%; transform: translate(-0%, -0%); }", &["--approx"]);
    assert!(out.contains("[\"AnchorPoint\"] = Vector2.new(0, 0)"), "{out}");
}

#[test]
fn fit_content_sizes_from_the_content_even_over_a_weaker_size() {
    let (out, stderr) = compile(
        ".as-x { width: fit-content; } .wide { width: 100%; height: 20px; } .row > .wide { width: fit-content; height: fit-content; }",
        &["--approx"],
    );
    // AutomaticSize only grows an element past its Size, so a weaker rule's Size has to be zeroed.
    for selector in [".as-x", ".row > .wide"] {
        let rule = rule_of(&out, selector);
        assert!(rule.contains("[\"Size\"] = UDim2.new(0, 0, 0, 0)"), "{out}");
        assert!(rule.contains("[\"AutomaticSize\"] = Enum.AutomaticSize.XY"), "{out}");
    }
    assert!(!stderr.contains("only `width`"), "{stderr}");
}

#[test]
fn display_flex_only_forces_visible_when_something_hides() {
    let (out, _) = compile(".row { display: flex; }", &["--approx"]);
    assert!(!out.contains("Visible"), "{out}");
    let (out, _) = compile(".row { display: flex; } .row.gone { display: none; }", &["--approx"]);
    assert!(out.contains("[\"Visible\"] = true"), "{out}");
    assert!(out.contains("[\"Visible\"] = false"), "{out}");
}

#[test]
fn content_and_appearance_map_to_text_and_auto_button_color() {
    let (out, stderr) = compile("TextButton { content: \"\"; appearance: none; }", &["--approx"]);
    assert!(out.contains("[\"Text\"] = \"\""), "{out}");
    assert!(out.contains("[\"AutoButtonColor\"] = false"), "{out}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn rich_text_is_on_by_default_below_every_rule() {
    let (out, _) = compile(".plain { RichText: false; }", &[]);
    assert!(rule_of(&out, "TextLabel, TextButton, TextBox").contains("[\"RichText\"] = true,"), "{out}");
    // Text wraps by default, as `white-space: normal` does in CSS.
    let (wrapped, _) = compile(".nowrap { white-space: nowrap; }", &["--approx"]);
    assert!(rule_of(&wrapped, "TextLabel, TextButton, TextBox").contains("[\"TextWrapped\"] = true,"), "{wrapped}");
    assert!(wrapped.contains("[\"TextWrapped\"] = false"), "an author rule must be able to opt out:\n{wrapped}");
    assert!(priority_of(&out, ".plain") > 0.0, "an author rule must override the default:\n{out}");
}

#[test]
fn elements_are_content_sized_by_default_and_a_css_size_turns_that_off() {
    // A fresh GuiObject is 0x0, so the user-agent sheet sizes every class from its content, as CSS
    // sizes a box with no width or height of its own.
    let (out, _) = compile(".chip { width: 100px; height: 20px; }\n.grow { width: fit-content; }", &["--approx"]);
    let every_class = rule_of(
        &out,
        "Frame, TextLabel, TextButton, TextBox, ImageLabel, ImageButton, ScrollingFrame, CanvasGroup, VideoFrame, ViewportFrame",
    );
    assert!(every_class.contains("[\"AutomaticSize\"] = Enum.AutomaticSize.XY,"), "{out}");
    assert!(
        out.contains("[\"Size\"] = UDim2.new(0, 100, 0, 20),\n\t[\"AutomaticSize\"] = Enum.AutomaticSize.None,"),
        "{out}"
    );
    assert!(rule_of(&out, ".grow").contains("[\"AutomaticSize\"] = Enum.AutomaticSize.XY,"), "{out}");
    // Without the size approximations nothing translates to AutomaticSize, so the default is not
    // overridable and isn't emitted.
    let (out, _) = compile(".chip { Size: UDim2.new(0, 100, 0, 20); }", &[]);
    assert!(!out.contains("AutomaticSize"), "{out}");
}

// ----- dart-sass parity -----

#[test]
fn extend_reaches_a_used_module_but_not_the_stylesheet_that_uses_it() {
    // dart-sass scopes extension to the stylesheet the @extend is written in plus everything that
    // stylesheet loads, transitively — never the other way round.
    let dir = TempDir::new("extend-scope");
    dir.write("_base.scss", "%box { border: none; }\n.plain { border-radius: 4px; }\n");
    dir.write("_leaf.scss", ".leaf { @extend .later !optional; color: red; }\n");
    let input = dir.write(
        "main.scss",
        "@use \"base\";\n@use \"leaf\";\n.card { @extend %box; @extend .plain; }\n.later { z-index: 1; }\n",
    );
    let (code, out, stderr) = run(&[input.to_str().unwrap(), "-o", "-", "--approx", "--emit", "css"], None);
    assert_eq!(code, 0, "{stderr}");
    // Downstream: main's @extend rewrites the module it uses.
    assert!(out.contains(".plain, .card"), "{out}");
    assert!(out.contains(".card {"), "{out}");
    // Upstream: leaf's @extend cannot reach the stylesheet that loaded it.
    assert!(!out.contains(".later, .leaf"), "an @extend must not reach the sheet that uses it:\n{out}");
}

#[test]
fn an_unfound_extend_target_is_an_error_like_dart_sass() {
    let dir = TempDir::new("extend-missing");
    let input = dir.write("main.scss", ".card { @extend %nope; }\n");
    let (code, _out, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("The target selector \"%nope\" of @extend was not found"), "{stderr}");
    assert!(stderr.contains("!optional"), "{stderr}");

    // ...and !optional still silences it.
    let input = dir.write("ok.scss", ".card { @extend %nope !optional; }\n");
    let (code, _out, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn imported_files_share_the_importers_extend_scope() {
    // @import has no module of its own, so its rules and its @extends belong to the importer.
    let dir = TempDir::new("extend-import");
    dir.write("_old.scss", "%chip { border-radius: 4px; }\n");
    let input = dir.write("main.scss", "@import \"old\";\n.tag { @extend %chip; }\n");
    let (code, out, stderr) = run(&[input.to_str().unwrap(), "-o", "-", "--approx"], None);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("\".tag::UICorner\""), "{out}");
}

#[test]
fn custom_property_values_are_raw_text_read_back_as_css() {
    // dart-sass never evaluates a custom property's value; only #{} substitutes into it. outlass
    // reads the text back as a CSS value so a token still becomes a real Luau value.
    let (out, stderr) = compile(
        "$metal: #8a8f98;\n:root { --Metal: #{$metal}; --Elevation: 2; --Font: Enum.Font.Gotham; --Pad: UDim.new(0, 4); }",
        &[],
    );
    assert!(out.contains(r#"sheet:SetAttribute("Metal", Color3.fromRGB(138, 143, 152))"#), "{out}");
    assert!(out.contains(r#"sheet:SetAttribute("Elevation", 2)"#), "{out}");
    assert!(out.contains(r#"sheet:SetAttribute("Font", Enum.Font.Gotham)"#), "{out}");
    assert!(out.contains(r#"sheet:SetAttribute("Pad", UDim.new(0, 4))"#), "{out}");
    assert!(stderr.is_empty(), "{stderr}");

    // A bare $var is four characters of text, as in dart-sass — said out loud, and never emitted
    // as Luau that wouldn't parse.
    let (out, stderr) = compile("$metal: #8a8f98;\n:root { --Metal: $metal; }", &[]);
    assert!(out.contains(r#"sheet:SetAttribute("Metal", "$metal")"#), "{out}");
    assert!(stderr.contains("write #{$var}"), "{stderr}");
}

#[test]
fn opacity_scales_the_background_instead_of_replacing_it() {
    // CSS `opacity` fades whatever background the element already has. A rule that sets only
    // `opacity` can't see a background set by a rule that doesn't cover it, so it must leave
    // BackgroundTransparency alone rather than guess an opaque one over `transparent`.
    let (out, _) = compile("Frame { background-color: transparent; } .a { opacity: 0.75; }", &["--approx"]);
    let a = &out[out.find("\".a\"").unwrap()..];
    let a = &a[..a.find('}').unwrap()];
    assert!(!a.contains("BackgroundTransparency"), "{out}");
    assert!(a.contains("[\"TextTransparency\"] = 0.25"), "{out}");

    // A background it does inherit is scaled, and so is an inherited border.
    let (out, _) = compile(
        ".a { background-color: rgba(0, 0, 0, 0.5); border: 1px solid red; } .a:hover { opacity: 0.5; }",
        &["--approx"],
    );
    let hover = &out[out.find("\".a:Hover\"").unwrap()..];
    let hover = &hover[..hover.find('}').unwrap()];
    assert!(hover.contains("[\"BackgroundTransparency\"] = 0.75"), "{out}");
    assert!(!hover.contains("BackgroundColor3"), "{out}");
    let stroke = &out[out.find("\".a:Hover::UIStroke\"").unwrap()..];
    let stroke = &stroke[..stroke.find('}').unwrap()];
    assert!(stroke.contains("[\"Transparency\"] = 0.5"), "{out}");
    assert!(!stroke.contains("Thickness"), "{out}");
}

#[test]
fn scrolling_frames_size_their_canvas_to_the_content() {
    // A CSS scroll container scrolls exactly its content; a ScrollingFrame's canvas defaults to twice
    // its height. CanvasSize is a floor under AutomaticCanvasSize, so it has to be zeroed too.
    let (out, _) = compile(".list { overflow-y: auto; } .list.wide { overflow-x: scroll; }", &["--approx"]);
    let ua = &out[out.find("\"ScrollingFrame\"").unwrap()..];
    let ua = &ua[..ua.find('}').unwrap()];
    assert!(ua.contains("[\"AutomaticCanvasSize\"] = Enum.AutomaticSize.XY"), "{out}");
    assert!(ua.contains("[\"CanvasSize\"] = UDim2.new(0, 0, 0, 0)"), "{out}");

    let list = &out[out.find("\".list\"").unwrap()..];
    let list = &list[..list.find('}').unwrap()];
    assert!(list.contains("[\"ScrollingDirection\"] = Enum.ScrollingDirection.Y"), "{out}");
    assert!(list.contains("[\"AutomaticCanvasSize\"] = Enum.AutomaticSize.Y"), "{out}");

    // `.list.wide` inherits `overflow-y: auto` from `.list`, so it scrolls both ways.
    let wide = &out[out.find("\".list.wide\"").unwrap()..];
    let wide = &wide[..wide.find('}').unwrap()];
    assert!(wide.contains("[\"ScrollingDirection\"] = Enum.ScrollingDirection.XY"), "{out}");
    assert!(wide.contains("[\"AutomaticCanvasSize\"] = Enum.AutomaticSize.XY"), "{out}");
}

#[test]
fn scrollbar_width_is_translated_not_rejected() {
    let (out, stderr) = compile(
        ".list { overflow-y: auto; scrollbar-width: thin; scrollbar-color: #888 transparent; transition: scrollbar-color 0.2s; } .list:hover { opacity: 0.5; }",
        &["--approx"],
    );
    assert!(!stderr.contains("no Roblox equivalent"), "{stderr}");
    assert!(out.contains("[\"ScrollBarThickness\"] = 8"), "{out}");
    assert!(out.contains("[\"ScrollBarImageColor3\"] = Color3.fromRGB(136, 136, 136)"), "{out}");
    assert!(out.contains("[\"ScrollBarImageTransparency\"] = TweenInfo.new(0.2"), "{out}");
    // `:hover`'s opacity fades the thumb `.list` gives it.
    let hover = &out[out.find("\".list:Hover\"").unwrap()..];
    let hover = &hover[..hover.find('}').unwrap()];
    assert!(hover.contains("[\"ScrollBarImageTransparency\"] = 0.5"), "{out}");
    assert!(!hover.contains("ScrollBarImageColor3"), "{out}");
}

#[test]
fn unknown_fonts_warn_and_fall_back_like_css() {
    // `Gotham` is an easy slip: the built-in family is `GothamSSm`.
    let (out, stderr) = compile(".a { font-family: Gotham; }", &["--approx"]);
    assert!(stderr.contains("`Gotham` is not a built-in Roblox font"), "{stderr}");
    assert!(stderr.contains("did you mean `GothamSSm`?"), "{stderr}");
    assert!(out.contains("rbxasset://fonts/families/SourceSansPro.json"), "the default font stands in:\n{out}");
    assert!(!out.contains("Gotham.json"), "{out}");

    // The first family that exists wins, in the shorthand too.
    let (out, stderr) = compile(".a { font: bold 20px \"Inter\", \"Source Sans Pro\", sans-serif; }", &["--approx"]);
    assert!(stderr.contains("`Inter`"), "{stderr}");
    assert!(stderr.contains("using `Source Sans Pro`"), "{stderr}");
    assert!(out.contains("Font.new(\"rbxasset://fonts/families/SourceSansPro.json\", Enum.FontWeight.Bold"), "{out}");
    // Roblox's TextSize is a line's height: 20px x Source Sans Pro's 1.257.
    assert!(out.contains("[\"TextSize\"] = 25"), "{out}");

    // Built-in names in any spacing or case, generic families and uploaded fonts are all fine.
    let (out, stderr) = compile(
        ".a { font-family: \"builder sans\"; } .b { font-family: monospace; } .c { font-family: \"rbxassetid://123\"; }",
        &["--approx"],
    );
    assert!(!stderr.contains("warning"), "{stderr}");
    assert!(out.contains("rbxasset://fonts/families/BuilderSans.json"), "{out}");
    assert!(out.contains("rbxasset://fonts/families/RobotoMono.json"), "{out}");
    assert!(out.contains("Font.new(\"rbxassetid://123\""), "{out}");
}

// ---------- 22. layout parity ----------

/// The body of the StyleRule emitted for exactly this selector.
fn rule_of<'a>(out: &'a str, selector: &str) -> &'a str {
    let start = ["sheet", "userAgent"]
        .iter()
        .find_map(|parent| out.find(&format!("rule({parent}, \"{selector}\"")))
        .unwrap_or_else(|| panic!("no `{selector}` rule in:\n{out}"));
    let rest = &out[start..];
    &rest[..rest.find("\n})").map_or(rest.len(), |i| i + 3)]
}

#[test]
fn flex_items_stretch_by_default_but_sized_axes_keep_their_size() {
    let (out, _) = compile(
        ".row { display: flex; } .row.mid { align-items: center; } .icon { width: 16px; height: 16px; } \
         .bar { height: 4px; width: 50%; } .grow { width: 100px; height: 20px; flex: 1; }",
        &["--approx"],
    );
    assert!(
        rule_of(&out, ".row::UIListLayout").contains("[\"ItemLineAlignment\"] = Enum.ItemLineAlignment.Stretch"),
        "{out}"
    );
    // Any other alignment has to undo the stretch a weaker `display: flex` rule gave.
    assert!(
        rule_of(&out, ".row.mid::UIListLayout").contains("[\"ItemLineAlignment\"] = Enum.ItemLineAlignment.Automatic")
    );
    // Roblox stretches explicitly sized items too; a MaxSize at their own size keeps them out of it.
    assert!(rule_of(&out, ".icon::UISizeConstraint").contains("[\"MaxSize\"] = Vector2.new(16, 16)"), "{out}");
    assert!(rule_of(&out, ".bar::UISizeConstraint").contains("[\"MaxSize\"] = Vector2.new(math.huge, 4)"), "{out}");
    // A cap would stop an item from growing along the line.
    assert!(!out.contains("\".grow::UISizeConstraint\""), "{out}");
}

#[test]
fn a_scrolling_axis_is_never_capped_at_the_window() {
    // A UISizeConstraint caps a ScrollingFrame's canvas too, so a cap at its own height would leave
    // nothing to scroll. The other axis keeps its cap.
    let (out, _) = compile(
        ".log { width: 300px; height: 100px; overflow-y: auto; } \
         .hug { width: 300px; height: fit-content; overflow-y: auto; } .hug.fixed { height: 100px; } \
         .box { width: 300px; height: 100px; } .box.scrolls { overflow: auto; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".log::UISizeConstraint").contains("[\"MaxSize\"] = Vector2.new(300, math.huge)"), "{out}");
    // The scrolling comes from a weaker rule.
    assert!(
        rule_of(&out, ".hug.fixed::UISizeConstraint").contains("[\"MaxSize\"] = Vector2.new(300, math.huge)"),
        "{out}"
    );
    // Scrolling lifts a weaker rule's cap.
    assert!(rule_of(&out, ".box::UISizeConstraint").contains("[\"MaxSize\"] = Vector2.new(300, 100)"), "{out}");
    assert!(
        rule_of(&out, ".box.scrolls::UISizeConstraint").contains("[\"MaxSize\"] = Vector2.new(math.huge, math.huge)"),
        "{out}"
    );
}

#[test]
fn flex_rows_let_their_items_shrink_like_css() {
    let (out, _) = compile(
        ".row { display: flex; } .row.down { flex-direction: column; } .col { display: flex; flex-direction: column; } \
         .strip { display: flex; overflow-x: auto; } .item { flex: none; } \
         @media (pointer: coarse) { .touch { display: flex; } }",
        &["--approx"],
    );
    // Below every author rule, so an item's own `flex` wins.
    let row = rule_of(&out, ".row > GuiObject::UIFlexItem");
    assert!(row.contains("[\"FlexMode\"] = Enum.UIFlexMode.Shrink") && row.contains("[\"ShrinkRatio\"] = 1"), "{out}");
    assert!(row.contains(", -"), "a negative priority:\n{out}");
    assert!(rule_of(&out, ".item::UIFlexItem").contains("[\"FlexMode\"] = Enum.UIFlexMode.None"), "{out}");
    // A column would crush its content-sized items, and a scrolling row squeeze them into the window.
    assert!(
        rule_of(&out, ".row.down > GuiObject::UIFlexItem").contains("[\"FlexMode\"] = Enum.UIFlexMode.None"),
        "{out}"
    );
    assert!(!out.contains("\".col > GuiObject::UIFlexItem\""), "{out}");
    assert!(!out.contains("\".strip > GuiObject::UIFlexItem\""), "{out}");
    assert!(out.contains("\"@PreferredInputTouch .touch > GuiObject::UIFlexItem\""), "{out}");
}

#[test]
fn max_width_and_min_width_combine_with_the_stretch_cap() {
    let (out, _) = compile(".a { width: 300px; height: 20px; max-width: 200px; min-height: 30px; }", &["--approx"]);
    let cap = rule_of(&out, ".a::UISizeConstraint");
    assert!(cap.contains("[\"MaxSize\"] = Vector2.new(200, 30)"), "{out}");
    assert!(cap.contains("[\"MinSize\"] = Vector2.new(0, 30)"), "{out}");
}

#[test]
fn margins_offset_positioned_elements_like_css() {
    let (out, stderr) = compile(
        ".card { position: absolute; inset: 0; margin: 8px; } \
         .pin { position: absolute; right: 10px; top: 0; margin-right: 5px; margin-top: 2px; width: 10px; height: 10px; } \
         .mid { position: absolute; left: 0; right: 0; width: 200px; height: 40px; margin-inline: auto; }",
        &["--approx"],
    );
    let card = rule_of(&out, ".card");
    assert!(card.contains("[\"Size\"] = UDim2.new(1, -16, 1, -16)"), "{out}");
    assert!(card.contains("[\"Position\"] = UDim2.new(0, 8, 0, 8)"), "{out}");
    let pin = rule_of(&out, ".pin");
    assert!(pin.contains("[\"Position\"] = UDim2.new(1, -15, 0, 2)"), "{out}");
    assert!(pin.contains("[\"AnchorPoint\"] = Vector2.new(1, 0)"), "{out}");
    let mid = rule_of(&out, ".mid");
    assert!(mid.contains("[\"Position\"] = UDim2.new(0.5, 0, 0, 0)"), "{out}");
    assert!(mid.contains("[\"AnchorPoint\"] = Vector2.new(0.5, 0)"), "{out}");
    assert!(!stderr.contains("margin"), "positioned margins are exact:\n{stderr}");
}

#[test]
fn margins_in_flow_move_the_element_and_warn_about_siblings() {
    let (out, stderr) = compile(
        ".a { width: 200px; height: 40px; margin: 0 auto; } .b { margin-left: auto; width: 10px; height: 10px; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".a").contains("[\"AnchorPoint\"] = Vector2.new(0.5, 0)"), "{out}");
    let b = rule_of(&out, ".b");
    assert!(b.contains("[\"Position\"] = UDim2.new(1, 0, 0, 0)"), "{out}");
    assert!(b.contains("[\"AnchorPoint\"] = Vector2.new(1, 0)"), "{out}");
    assert!(stderr.contains("siblings don't make room"), "{stderr}");
    assert!(!stderr.contains("no Roblox equivalent"), "{stderr}");
}

#[test]
fn line_height_pads_the_half_leading_like_css() {
    let (out, _) = compile(
        ".label { font-size: 20px; line-height: 1.5; padding: 4px; &.big { padding: 6px; } } \
         .px { font-size: 20px; line-height: 24px; } .one { font-size: 20px; line-height: 1; }",
        &["--approx"],
    );
    // (1.5 - 1) x 20 / 2 = 5 above and below, on top of the padding.
    let label = rule_of(&out, ".label::UIPadding");
    assert!(label.contains("[\"PaddingTop\"] = UDim.new(0, 9)"), "{out}");
    assert!(label.contains("[\"PaddingLeft\"] = UDim.new(0, 4)"), "{out}");
    // A rule that only changes the padding keeps the line height from the cascade.
    assert!(rule_of(&out, ".label.big::UIPadding").contains("[\"PaddingTop\"] = UDim.new(0, 11)"), "{out}");
    assert!(rule_of(&out, ".px::UIPadding").contains("[\"PaddingBottom\"] = UDim.new(0, 2)"), "{out}");
    assert!(!out.contains("\".one::UIPadding\""), "{out}");
}

#[test]
fn align_content_aligns_a_blocks_text() {
    let (out, stderr) = compile(
        ".a { align-content: center; } .b { display: flex; flex-wrap: wrap; align-content: center; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".a").contains("[\"TextYAlignment\"] = Enum.TextYAlignment.Center"), "{out}");
    // `.a` and the user-agent default (text starts at the top, as in CSS); `.b` has none.
    assert_eq!(out.matches("[\"TextYAlignment\"] =").count(), 2, "only `.a` aligns its text:\n{out}");
    assert!(stderr.contains("`align-content` on a flex or grid container"), "{stderr}");
}

#[test]
fn box_sizing_border_box_is_what_roblox_does() {
    let (_, stderr) = compile(".a { box-sizing: border-box; }", &["--approx"]);
    assert!(!stderr.contains("warning"), "{stderr}");
    let (_, stderr) = compile(".a { box-sizing: content-box; }", &["--approx"]);
    assert!(stderr.contains("`box-sizing: content-box` isn't supported"), "{stderr}");
}

#[test]
fn strict_fails_on_css_that_lays_out_differently() {
    /// Compiles with `--approx --strict`; returns (exit code, stdout, stderr).
    fn strict(scss: &str) -> (i32, String, String) {
        let dir = TempDir::new("strict");
        let input = dir.write("in.scss", scss);
        run(&[input.to_str().unwrap(), "-o", "-", "--approx", "--strict"], None)
    }
    let scss = ".fill { width: auto; height: 10px; } .text { font-size: 14px; } .v { vertical-align: middle; } \
                .tall { display: flex; height: 100px; width: 10px; } .ok { font-family: Roboto; font-size: 14px; }";
    let (_, stderr) = compile(scss, &["--approx"]);
    assert!(!stderr.contains("strict:"), "only with --strict:\n{stderr}");
    let (code, stdout, stderr) = strict(scss);
    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.is_empty(), "no output is written:\n{stdout}");
    assert!(stderr.contains("error: strict: `width: auto` fills the parent"), "{stderr}");
    assert!(stderr.contains("strict: Roblox's TextSize is the height of a line"), "{stderr}");
    assert!(stderr.contains("strict: `vertical-align`"), "{stderr}");
    assert!(stderr.contains("not to the container's height"), "{stderr}");
    assert_eq!(stderr.matches("strict: Roblox's TextSize").count(), 1, "`.ok` has a built-in family:\n{stderr}");
    assert!(stderr.contains("--strict is set; no output written"), "{stderr}");

    let (code, _, stderr) = strict(".pad { width: 100px; height: 20px; padding: 4px; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("set `box-sizing: border-box`"), "{stderr}");
    let (code, _, stderr) = strict(".pad { box-sizing: border-box; width: 100px; height: 20px; padding: 4px; }");
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("warning") && !stderr.contains("error"), "{stderr}");

    // A column stretches to its own width in CSS, to its widest item in Roblox.
    let (code, _, stderr) = strict(".col { display: flex; flex-direction: column; width: 300px; height: 100px; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("not to the container's width"), "{stderr}");
    for ok in [
        ".col { display: flex; flex-direction: column; align-items: flex-start; width: 300px; height: 100px; }",
        ".col { display: flex; flex-direction: column; width: fit-content; height: fit-content; }",
        ".row { display: flex; width: 300px; height: fit-content; }",
    ] {
        let (code, _, stderr) = strict(ok);
        assert_eq!(code, 0, "{ok}\n{stderr}");
    }

    // Absolutely positioned children of a layout container join the layout.
    let (code, _, stderr) =
        strict(".card { position: relative; display: flex; align-items: flex-start; width: 10px; height: 10px; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("takes part in Roblox's layout"), "{stderr}");

    // A percentage of a parent that is sized by its content.
    let (code, _, stderr) = strict(
        ".card { width: fit-content; height: fit-content; } .card > .bar { width: 100%; height: 6px; } \
         .box { width: 200px; height: fit-content; } .box > .bar { width: 50%; height: 6px; }",
    );
    assert_eq!(code, 1);
    assert!(stderr.contains("`width: 100%` is a percentage of `.card`"), "{stderr}");
    assert!(!stderr.contains("of `.box`"), "{stderr}");

    // `fr` tracks need a grid with a size to share out, from its own rule or another.
    let (code, _, stderr) =
        strict(".grid { display: grid; grid-template-columns: repeat(4, 1fr); grid-auto-rows: 40px; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("`fr` tracks share out the grid's width"), "{stderr}");
    let (code, _, stderr) = strict(
        ".grid { display: grid; grid-template-columns: repeat(4, 1fr); grid-auto-rows: 40px; } \
         .grid { width: 100%; height: fit-content; }",
    );
    assert_eq!(code, 0, "{stderr}");

    // A rule that only changes the border inherits the padding, and whether it's inside the size.
    let (code, _, stderr) = strict(
        ".card { box-sizing: border-box; width: 100px; height: 20px; padding: 4px; border: 1px solid #fff; }          .card.hot { border: 2px solid red; }",
    );
    assert_eq!(code, 0, "{stderr}");

    // A cap on a scroll container caps its canvas; on anything sized by its content but text, it
    // doesn't cap the box.
    let (code, _, stderr) = strict(".log { width: 300px; height: fit-content; max-height: 100px; overflow-y: auto; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("`max-height` on a scroll container caps its canvas"), "{stderr}");
    let (code, _, stderr) = strict(".box { width: 300px; height: fit-content; max-height: 100px; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("past its `max-height`"), "{stderr}");
    for ok in [
        ".box { width: 300px; height: 200px; max-height: 100px; }",
        ".box { width: 300px; height: fit-content; max-height: none; }",
        ".tip { width: fit-content; height: fit-content; max-width: 200px; font-family: Roboto; font-size: 14px; }",
    ] {
        let (code, _, stderr) = strict(ok);
        assert_eq!(code, 0, "{ok}\n{stderr}");
    }

    // No width fills a block parent in CSS and fits the content in Roblox.
    let (code, _, stderr) = strict(
        ".card { width: 200px; height: fit-content; } .card > .caption { height: 20px; } \
         .row { display: flex; width: 200px; height: fit-content; } .row > .caption { height: 20px; } \
         .card > .pin { position: absolute; height: 20px; } .card > .sized { width: 50%; height: 20px; }",
    );
    assert_eq!(code, 1);
    assert!(stderr.contains("`.card > .caption` has no `width`"), "{stderr}");
    assert_eq!(stderr.matches("has no `width`").count(), 1, "{stderr}");

    // Grids need a row height.
    let (code, _, stderr) =
        strict(".grid { display: grid; grid-template-columns: repeat(3, 1fr); width: 300px; height: fit-content; }");
    assert_eq!(code, 1);
    assert!(stderr.contains("error: strict: grid cells need a size on both axes"), "{stderr}");
}

#[test]
fn font_size_is_the_em_so_text_size_scales_by_the_familys_line_height() {
    let (out, _) = compile(
        ".mono { font-family: \"Roboto Mono\"; font-size: 13px; line-height: 1; } \
         .mono.sans { font-family: \"Source Sans Pro\"; } .big { font-family: Roboto; font-size: 20px; }",
        &["--approx"],
    );
    // 13px x Roboto Mono's 1.3188 is a 17px line; `line-height: 1` pulls it back to 13px.
    let mono = rule_of(&out, ".mono");
    assert!(mono.contains("[\"TextSize\"] = 17"), "{out}");
    let pad = rule_of(&out, ".mono::UIPadding");
    assert!(
        pad.contains("[\"PaddingTop\"] = UDim.new(0, -2)") && pad.contains("[\"PaddingBottom\"] = UDim.new(0, -2)"),
        "{out}"
    );
    // Changing only the family re-sizes the text, and splits an odd leading into whole pixels.
    assert!(rule_of(&out, ".mono.sans").contains("[\"TextSize\"] = 16"), "{out}");
    let pad = rule_of(&out, ".mono.sans::UIPadding");
    assert!(
        pad.contains("[\"PaddingTop\"] = UDim.new(0, -2)") && pad.contains("[\"PaddingBottom\"] = UDim.new(0, -1)"),
        "{out}"
    );
    // With `line-height: normal` the family's own line is already what a browser draws.
    assert!(rule_of(&out, ".big").contains("[\"TextSize\"] = 23"), "{out}");
    assert!(!out.contains("\".big::UIPadding\""), "{out}");
}

#[test]
fn tokens_are_compiled_in_where_roblox_cant_look_them_up() {
    let (out, stderr) = compile(
        ":root { --accent: #7c5cff; --font-body: \"Source Sans Pro\", sans-serif; --radius: 8px; \
         --grad: linear-gradient(90deg, #000 0%, #fff 100%); } \
         .a { font-family: var(--font-body); font-size: 13px; border-radius: var(--radius); \
              background: var(--grad); color: var(--accent); }",
        &["--approx"],
    );
    assert!(!stderr.contains("ignored") && !stderr.contains("unsupported"), "{stderr}");
    let a = rule_of(&out, ".a");
    assert!(a.contains("rbxasset://fonts/families/SourceSansPro.json"), "{out}");
    assert!(a.contains("[\"TextSize\"] = 16"), "the family is known, so the size scales:\n{out}");
    // A color token stays a live reference, so a theme can still change it.
    assert!(a.contains("[\"TextColor3\"] = \"$accent\""), "{out}");
    // So does a length a property uses whole.
    assert!(rule_of(&out, ".a::UICorner").contains("[\"CornerRadius\"] = \"$radius\""), "{out}");
    assert!(out.contains("\".a::UIGradient\""), "{out}");
    // The gradient token itself can only be an attribute as text, never as a Luau call.
    assert!(out.contains("SetAttribute(\"grad\", \"linear-gradient("), "{out}");
    assert!(!out.contains("linear-gradient(90, "), "{out}");
}

#[test]
fn border_colors_from_tokens() {
    let (out, _) = compile(
        ":root { --line: #445566; --faint: rgba(255, 255, 255, 0.08); } \
         .a { border: 1px solid var(--line); } .b { border: 1px solid var(--faint); }",
        &["--approx"],
    );
    // An opaque token stays a live reference, in the shorthand as in `border-color`.
    assert!(rule_of(&out, ".a::UIStroke").contains("[\"Color\"] = \"$line\""), "{out}");
    // A translucent one is compiled in: a Color3 attribute would drop its alpha.
    let b = rule_of(&out, ".b::UIStroke");
    assert!(b.contains("[\"Color\"] = Color3.fromRGB(255, 255, 255)"), "{out}");
    assert!(b.contains("[\"Transparency\"] = 0.92"), "{out}");
}

#[test]
fn a_gradient_background_under_text_warns() {
    let (_, stderr) =
        compile(".btn { background: linear-gradient(90deg, #7c5cff, #ff5c8a); color: white; }", &["--approx"]);
    assert!(stderr.contains("a gradient background also tints this element's text"), "{stderr}");
    let (_, stderr) = compile(".swatch { background: linear-gradient(90deg, #7c5cff, #ff5c8a); }", &["--approx"]);
    assert!(!stderr.contains("tints"), "{stderr}");
    // The gradient-text idiom is meant to tint the text.
    let (_, stderr) = compile(
        ".t { background: linear-gradient(90deg, #7c5cff, #ff5c8a); background-clip: text; color: transparent; }",
        &["--approx"],
    );
    assert!(!stderr.contains("tints"), "{stderr}");
}

#[test]
fn gradient_angles_follow_the_elements_shape() {
    let (out, stderr) = compile(
        ".bar { width: 200px; height: 10px; background: linear-gradient(135deg, #000, #fff); } \
         .sq { width: 50px; height: 50px; background: linear-gradient(135deg, #000, #fff); } \
         .corner { background: linear-gradient(to bottom right, #000, #fff); } \
         .flex { width: 50%; height: 6px; background: linear-gradient(135deg, #000, #fff); } \
         .near { width: 50%; height: 6px; background: linear-gradient(95deg, #000, #fff); }",
        &["--approx"],
    );
    // A real 135deg on a 200x10 bar runs almost along it: atan(10 / 200).
    assert!(rule_of(&out, ".bar::UIGradient").contains("[\"Rotation\"] = 2.862405"), "{out}");
    assert!(rule_of(&out, ".sq::UIGradient").contains("[\"Rotation\"] = 45"), "{out}");
    // Corner to corner in both, whatever the shape.
    assert!(rule_of(&out, ".corner::UIGradient").contains("[\"Rotation\"] = 45"), "{out}");
    assert!(stderr.contains("a `135deg` gradient's direction depends on the element's shape"), "{stderr}");
    // `.near` is close enough to a right angle that the shape barely moves it.
    assert_eq!(stderr.matches("depends on the element's shape").count(), 1, "only `.flex`:\n{stderr}");
}

#[test]
fn a_border_takes_room_inside_the_box_like_css() {
    let (out, _) = compile(
        ".card { border: 1px solid #fff; padding: 8px; } .card.hot { border: 3px solid red; } \
         .card.flat { border: none; } .plain { border: 2px solid #fff; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".card::UIPadding").contains("[\"PaddingTop\"] = UDim.new(0, 9)"), "{out}");
    // A rule that only changes the border keeps the padding it inherits.
    assert!(rule_of(&out, ".card.hot::UIPadding").contains("[\"PaddingLeft\"] = UDim.new(0, 11)"), "{out}");
    assert!(rule_of(&out, ".card.flat::UIPadding").contains("[\"PaddingLeft\"] = UDim.new(0, 8)"), "{out}");
    assert!(rule_of(&out, ".plain::UIPadding").contains("[\"PaddingRight\"] = UDim.new(0, 2)"), "{out}");
}

#[test]
fn a_gradient_angle_uses_a_size_from_a_weaker_rule() {
    let (out, stderr) = compile(
        ".card { width: 200px; height: 10px; } .card.hot { background: linear-gradient(135deg, #000, #fff); }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".card.hot::UIGradient").contains("[\"Rotation\"] = 2.862405"), "{out}");
    assert!(!stderr.contains("depends on the element's shape"), "{stderr}");
    // The size stays the weaker rule's: the gradient rule doesn't restate it.
    assert!(!rule_of(&out, ".card.hot").contains("Size ="), "{out}");
}

#[test]
fn strict_without_approx_says_it_checked_nothing() {
    let (_, stderr) = compile(".a { Size: UDim2.new(0, 10, 0, 10); }", &["--strict"]);
    assert!(stderr.contains("--strict checks how CSS properties are translated, which needs --approx"), "{stderr}");
}

#[test]
fn a_redefined_token_stays_a_reference() {
    let (out, _) =
        compile(":root { --gap: 8px; } .dense { --gap: 4px; } .a { display: flex; gap: var(--gap); }", &["--approx"]);
    assert!(!rule_of(&out, ".a::UIListLayout").contains("UDim.new(0, 8)"), "{out}");
}

#[test]
fn css_defaults_for_backgrounds_borders_and_text_alignment() {
    let (out, _) = compile(".a { width: 10px; height: 10px; }", &["--approx"]);
    let gui = rule_of(
        &out,
        "Frame, TextLabel, TextButton, TextBox, ImageLabel, ImageButton, ScrollingFrame, CanvasGroup, VideoFrame, ViewportFrame",
    );
    assert!(gui.contains("[\"BackgroundTransparency\"] = 1") && gui.contains("[\"BorderSizePixel\"] = 0"), "{out}");
    assert!(rule_of(&out, "TextButton, ImageButton").contains("[\"AutoButtonColor\"] = false"), "{out}");
    let text = rule_of(&out, "TextLabel, TextBox");
    assert!(text.contains("[\"TextXAlignment\"] = Enum.TextXAlignment.Left"), "{out}");
    assert!(text.contains("[\"TextYAlignment\"] = Enum.TextYAlignment.Top"), "{out}");
}

#[test]
fn user_agent_rules_live_in_a_stylesheet_the_sheet_derives_from() {
    // A deriving sheet's rules beat the derived sheet's whatever their priorities (measured in
    // Studio), so the defaults sit under every author rule, like a browser's.
    let (out, _) = compile(".a { RichText: false; }", &["--approx"]);
    assert!(out.contains("rule(userAgent, \"TextLabel, TextButton, TextBox\", "), "{out}");
    assert!(!out.contains("rule(sheet, \"TextLabel, TextButton, TextBox\""), "{out}");
    assert!(out.contains("derive.Name = \"UserAgent\"\nderive.StyleSheet = userAgent\nuserAgent.Parent = derive\nderive.Parent = sheet\n"), "{out}");
    let (json, _) = compile(".a { RichText: false; }", &["--approx", "--emit", "json"]);
    assert!(json.contains("\"userAgent\": ["), "{json}");
    let (model, _) = compile(".a { RichText: false; }", &["--approx", "--emit", "rbxmx"]);
    assert!(model.contains("<Item class=\"StyleDerive\""), "{model}");
    assert!(model.contains("<string name=\"Name\">UserAgent</string>"), "{model}");
    let (bare, _) = compile(".a { RichText: false; }", &["--approx", "--no-user-agent-styles"]);
    assert!(!bare.contains("UserAgent"), "{bare}");
}

#[test]
fn no_user_agent_styles_leaves_out_the_default_rules() {
    let scss = ".row { display: flex; } .row > .a { width: 50px; } TextLabel { TextSize: 14; }";
    let (out, _) = compile(scss, &["--approx", "--no-user-agent-styles"]);
    for property in ["RichText", "TextWrapped", "AutoButtonColor", "AutomaticCanvasSize", "BorderSizePixel"] {
        assert!(!out.contains(property), "{property} in {out}");
    }
    // Rules generated from the stylesheet's own flex containers stay.
    assert!(out.contains("UIFlexItem"), "{out}");
    assert!(rule_of(&out, "TextLabel").contains("[\"TextSize\"] = 14"), "{out}");
}

#[test]
fn a_growing_item_without_a_width_starts_from_zero() {
    let (out, stderr) =
        compile(".card { flex-grow: 1; } .fixed { flex-grow: 1; width: 120px; height: 20px; }", &["--approx"]);
    // Sized by its content, a percentage-wide child would inflate it to the whole line.
    let card = rule_of(&out, ".card");
    assert!(card.contains("[\"Size\"] = UDim2.new(0, 0, 0, 0)"), "{out}");
    assert!(card.contains("[\"AutomaticSize\"] = Enum.AutomaticSize.Y,"), "{out}");
    assert!(rule_of(&out, ".fixed").contains("[\"Size\"] = UDim2.new(0, 120, 0, 20)"), "{out}");
    assert!(!stderr.contains("only `width`"), "{stderr}");
}

// ---------- generated code can only build a StyleSheet ----------

#[test]
fn stylesheet_text_never_becomes_luau_code() {
    // Each of these once compiled into a call to `require`.
    for value in [
        r#"luau("require(1)")"#,
        "require(2)",
        r#"unquote("3)) require(3")"#,
        r"a\29 require\28 4\29",
        "game.GetService(x)",
        r#"Enum.Font.#{"x"}#{"(require(5))"}"#,
    ] {
        let dir = TempDir::new("no-code");
        let input = dir.write("in.scss", &format!(".a {{ Text: {value}; }}"));
        let (_, out, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
        assert!(!out.contains("require") && !out.contains("GetService"), "{value}:\n{out}\n{stderr}");
    }
}

#[test]
fn allow_raw_luau_lets_luau_through() {
    let scss = r#".a { Text: luau("game.Players.LocalPlayer.Name"); }"#;
    let dir = TempDir::new("raw-luau");
    let input = dir.write("in.scss", scss);
    let (code, out, stderr) = run(&[input.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 1, "luau() without --allow-raw-luau is an error:\n{out}");
    assert!(stderr.contains("error:") && stderr.contains("--allow-raw-luau"), "{stderr}");
    let (out, _) = compile(scss, &["--allow-raw-luau"]);
    assert!(out.contains(r#"["Text"] = game.Players.LocalPlayer.Name,"#), "{out}");
}

#[test]
fn raw_luau_never_goes_into_json_or_a_model_file() {
    let dir = TempDir::new("raw-luau-data");
    let input = dir.write("in.scss", ".a { Text: luau(\"require(1)\"); }");
    let (code, out, stderr) = run(&[input.to_str().unwrap(), "--emit", "json", "--allow-raw-luau", "-o", "-"], None);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("JSON holds the stylesheet as data, so it can't hold raw Luau"), "{stderr}");
    assert!(!out.contains("require"), "{out}");
    let (code, _, stderr) = run(&[input.to_str().unwrap(), "--emit", "rbxmx", "--allow-raw-luau", "-o", "-"], None);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("a model file can't hold raw Luau"), "{stderr}");
}

#[test]
fn the_universal_selector_is_every_gui_object() {
    // A class selector matches by IsA (measured in Studio), so `*` is `GuiObject`; next to a tag,
    // name or type it adds nothing, as in CSS.
    let (out, stderr) = compile(
        "* { ZIndex: 1; } .a > * { ZIndex: 2; } .b *:hover { ZIndex: 3; } *.c { ZIndex: 4; } *::UICorner { CornerRadius: UDim.new(0, 4); }",
        &[],
    );
    assert!(!stderr.contains("universal"), "{stderr}");
    for selector in ["GuiObject", ".a > GuiObject", ".b >> GuiObject:Hover", ".c", "GuiObject::UICorner"] {
        rule_of(&out, selector);
    }
    assert!(!out.contains('*'), "{out}");
    // The debug CSS keeps it.
    let (css, _) = compile(".a > * { ZIndex: 2; }", &["--emit", "css"]);
    assert!(css.contains(".a > *"), "{css}");
}

#[test]
fn the_background_shorthand_draws_its_picture() {
    let (out, stderr) = compile(
        r#".c { background: url("rbxassetid://1") no-repeat center / cover; }
           .h { background: #fff url("rbxassetid://1"); }
           .k { background: url("rbxassetid://1"); background-size: contain; }
           .n { background: linear-gradient(red, blue) no-repeat; }
           .o { background-image: url("rbxassetid://1"); background: red; }"#,
        &["--approx"],
    );
    let c = rule_of(&out, ".c");
    assert!(c.contains(r#"["Image"] = "rbxassetid://1""#) && c.contains("Enum.ScaleType.Crop"), "{out}");
    // The color in the same layer still paints the background.
    let h = rule_of(&out, ".h");
    assert!(h.contains("Color3.fromRGB(255, 255, 255)") && h.contains(r#"["Image"]"#), "{out}");
    // A longhand after the shorthand overrides it.
    assert!(rule_of(&out, ".k").contains("Enum.ScaleType.Fit"), "{out}");
    assert!(out.contains("\".n::UIGradient\""), "{out}");
    // A shorthand without a picture clears one set before it.
    assert!(rule_of(&out, ".o").contains(r#"["Image"] = """#), "{out}");
    assert!(!stderr.contains("warning"), "{stderr}");
}

#[test]
fn a_border_image_hides_the_background_picture_with_a_warning() {
    let (out, stderr) = compile(
        r#".e { background-image: url("rbxassetid://1"); border-image: url("rbxassetid://2#96x96") 32 fill; }"#,
        &["--approx"],
    );
    assert!(rule_of(&out, ".e").contains(r#"["Image"] = "rbxassetid://2""#), "{out}");
    assert!(stderr.contains("Roblox has one Image"), "{stderr}");
}

#[test]
fn roblox_constructors_become_typed_values() {
    let (out, stderr) = compile(
        ".a { Size: UDim2.fromScale(1, 0.5); Position: UDim2.fromOffset(4, 8); BackgroundColor3: Color3.new(1, 0, 0); \
         FontFace: Font.fromName(\"Gotham\"); Font: Enum.Font.Arial; }",
        &[],
    );
    assert!(out.contains("[\"Size\"] = UDim2.new(1, 0, 0.5, 0),"), "{out}\n{stderr}");
    assert!(out.contains("[\"Position\"] = UDim2.new(0, 4, 0, 8),"), "{out}");
    assert!(out.contains("[\"BackgroundColor3\"] = Color3.fromRGB(255, 0, 0),"), "{out}");
    assert!(out.contains(r#"["FontFace"] = Font.new("rbxasset://fonts/families/Gotham.json"),"#), "{out}");
    assert!(out.contains("[\"Font\"] = Enum.Font.Arial,"), "{out}");
}

#[test]
fn emit_json_writes_the_typed_stylesheet() {
    let (out, _) = compile(
        ":root { --Accent: #ff8000; } .a { Size: UDim2.new(0, 4, 1, -8); BackgroundColor3: var(--Accent); \
         Text: \"hi\"; Transition: BackgroundColor3 0.2s; }",
        &["--emit", "json"],
    );
    assert!(out.contains(r#""Accent": {"type": "Color3", "r": 255, "g": 128, "b": 0}"#), "{out}");
    assert!(
        out.contains(r#""Size": {"type": "UDim2", "x": {"scale": 0, "offset": 4}, "y": {"scale": 1, "offset": -8}}"#),
        "{out}"
    );
    assert!(out.contains(r#""BackgroundColor3": {"type": "token", "name": "Accent"}"#), "{out}");
    assert!(out.contains(r#""Text": "hi""#), "{out}");
    let transitions = &out[out.find(r#""transitions""#).expect(&out)..];
    assert!(transitions.contains(r#""type": "TweenInfo""#) && transitions.contains(r#""time": 0.2,"#), "{out}");
}

#[test]
fn emit_json_writes_a_json_file_by_default() {
    let dir = TempDir::new("emit-json");
    let input = dir.write("x.scss", ".a { Visible: true; }");
    let (code, _, stderr) = run(&[input.to_str().unwrap(), "--emit", "json"], None);
    assert_eq!(code, 0, "{stderr}");
    let json = std::fs::read_to_string(dir.path().join("x.json")).unwrap();
    assert!(json.contains(r#""properties": {"Visible": true}"#), "{json}");
}

// ---------- CSS features ----------

#[test]
fn text_inside_an_element_inherits_its_text_properties() {
    let (out, _) = compile(
        ".card { color: red; text-align: center; } TextLabel { color: blue; } .card .title { color: green; }",
        &["--approx"],
    );
    let inherited = ".card >> TextLabel, .card >> TextButton, .card >> TextBox";
    let rule = rule_of(&out, inherited);
    assert!(rule.contains("[\"TextColor3\"] = Color3.fromRGB(255, 0, 0)"), "{out}");
    assert!(rule.contains("[\"TextXAlignment\"] = Enum.TextXAlignment.Center"), "{out}");
    assert!(priority_of(&out, inherited) > 0.0, "inherited values beat the user-agent defaults:\n{out}");
    assert!(priority_of(&out, inherited) < priority_of(&out, "TextLabel"), "a rule for the text itself wins:\n{out}");
    assert!(priority_of(&out, inherited) < priority_of(&out, ".card >> .title"), "{out}");
}

#[test]
fn a_rule_that_redefines_a_token_recompiles_the_declarations_using_it() {
    let (out, _) = compile(".a { --gap: 6px; padding: var(--gap); } .a.b { --gap: 10px; }", &["--approx"]);
    assert!(rule_of(&out, ".a::UIPadding").contains("[\"PaddingTop\"] = UDim.new(0, 6)"), "{out}");
    assert!(rule_of(&out, ".a.b::UIPadding").contains("[\"PaddingTop\"] = UDim.new(0, 10)"), "{out}");
}

#[test]
fn min_max_and_clamp_sizes_become_size_constraints() {
    let (out, stderr) = compile(".a { width: min(100%, 300px); height: clamp(10px, 50%, 200px); }", &["--approx"]);
    assert!(rule_of(&out, ".a").contains("[\"Size\"] = UDim2.new(1, 0, 0.5, 0)"), "{out}");
    let constraint = rule_of(&out, ".a::UISizeConstraint");
    assert!(constraint.contains("[\"MinSize\"] = Vector2.new(0, 10)"), "{out}");
    assert!(constraint.contains("[\"MaxSize\"] = Vector2.new(300, 200)"), "{out}");
    assert!(!stderr.contains("width") && !stderr.contains("height"), "{stderr}");
}

#[test]
fn a_clamped_font_size_scales_the_text_between_its_bounds() {
    let (out, _) =
        compile(".a { font-size: clamp(12px, 2vw, 20px); } .b { font-size: clamp(12px, 30px, 20px); }", &["--approx"]);
    assert!(rule_of(&out, ".a").contains("[\"TextScaled\"] = true"), "{out}");
    let constraint = rule_of(&out, ".a::UITextSizeConstraint");
    assert!(constraint.contains("[\"MinTextSize\"] = 12") && constraint.contains("[\"MaxTextSize\"] = 20"), "{out}");
    assert!(rule_of(&out, ".b").contains("[\"TextSize\"] = 20"), "a px value is clamped directly:\n{out}");
}

#[test]
fn standalone_transform_properties_act_like_transform() {
    let (out, _) = compile(".a { translate: 4px 2px; rotate: 45deg; scale: 2; }", &["--approx"]);
    let a = rule_of(&out, ".a");
    assert!(a.contains("[\"Position\"] = UDim2.new(0, 4, 0, 2)") && a.contains("[\"Rotation\"] = 45"), "{out}");
    assert!(rule_of(&out, ".a::UIScale").contains("[\"Scale\"] = 2"), "{out}");
}

#[test]
fn image_fitting_and_resampling() {
    let (out, _) = compile(
        ".a { object-fit: cover; image-rendering: pixelated; } \
         .b { background-image: url(\"rbxassetid://1\"); background-size: 32px 32px; } .c { background-size: contain; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".a").contains("[\"ScaleType\"] = Enum.ScaleType.Crop"), "{out}");
    assert!(rule_of(&out, ".a").contains("[\"ResampleMode\"] = Enum.ResamplerMode.Pixelated"), "{out}");
    let b = rule_of(&out, ".b");
    assert!(
        b.contains("[\"ScaleType\"] = Enum.ScaleType.Tile") && b.contains("[\"TileSize\"] = UDim2.new(0, 32, 0, 32)"),
        "{out}"
    );
    assert!(rule_of(&out, ".c").contains("[\"ScaleType\"] = Enum.ScaleType.Fit"), "{out}");
}

#[test]
fn placeholder_color_styles_the_textbox() {
    let (out, stderr) = compile(".search::placeholder { color: #888; } ::placeholder { color: red; }", &["--approx"]);
    assert!(rule_of(&out, ".search").contains("[\"PlaceholderColor3\"] = Color3.fromRGB(136, 136, 136)"), "{out}");
    assert!(rule_of(&out, "TextBox").contains("[\"PlaceholderColor3\"] = Color3.fromRGB(255, 0, 0)"), "{out}");
    assert!(!stderr.contains("placeholder"), "{stderr}");
}

#[test]
fn current_color_is_the_rules_color() {
    let (out, _) = compile(".a { color: red; border: 1px solid currentColor; }", &["--approx"]);
    assert!(rule_of(&out, ".a::UIStroke").contains("[\"Color\"] = Color3.fromRGB(255, 0, 0)"), "{out}");
}

#[test]
fn font_face_names_a_font_asset() {
    let (out, stderr) = compile(
        "@font-face { font-family: Brand; src: url(\"rbxassetid://123\") format(\"truetype\"); } \
         .a { font-family: Brand, sans-serif; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".a").contains("[\"FontFace\"] = Font.new(\"rbxassetid://123\""), "{out}");
    assert!(!stderr.contains("font-face") && !stderr.contains("Brand"), "{stderr}");
}

#[test]
fn is_and_where_expand_into_selector_lists() {
    let (out, stderr) = compile(
        ".a:is(.b, #c) .d { Visible: true; } .y:is(Frame, TextLabel) { Visible: true; } \
         :where(#x) .e { Visible: true; } .f .e { Visible: false; }",
        &[],
    );
    assert!(out.contains("\".a.b >> .d, .a#c >> .d\""), "{out}");
    assert!(out.contains("\"Frame.y, TextLabel.y\""), "{out}");
    assert!(priority_of(&out, "#x >> .e") < priority_of(&out, ".f >> .e"), ":where adds no specificity:\n{out}");
    assert!(!stderr.contains(":is") && !stderr.contains(":where"), "{stderr}");
}

#[test]
fn tags_drop_properties_the_tagged_classes_dont_have() {
    let dir = TempDir::new("tags");
    let tags = dir.write("tags.json", r##"{ "avatar": ["ImageLabel", "ImageButton"], "card": "Frame" }"##);
    let input = dir.write(
        "in.scss",
        ".card { background-image: url(\"rbxassetid://1\"); color: red; } .avatar { object-fit: cover; opacity: 0.5; color: red; }",
    );
    let (code, out, stderr) =
        run(&[input.to_str().unwrap(), "--approx", "--tags", tags.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("`background-image` sets Image, which Frame doesn't have"), "{stderr}");
    assert!(!stderr.contains("`color`"), "an inherited property styles the text inside:\n{stderr}");
    assert!(!out.contains("rule(sheet, \".card\""), "nothing is left for .card itself:\n{out}");
    let avatar = rule_of(&out, ".avatar");
    assert!(
        avatar.contains("[\"ScaleType\"] = Enum.ScaleType.Crop") && avatar.contains("[\"ImageTransparency\"] = 0.5"),
        "{out}"
    );
    assert!(!avatar.contains("TextColor3") && !avatar.contains("GroupTransparency"), "{out}");
    assert!(rule_of(&out, ".card >> TextLabel, .card >> TextButton, .card >> TextBox").contains("TextColor3"), "{out}");
}

#[test]
fn a_type_selector_narrows_the_classes_without_tags() {
    let (out, stderr) = compile("Frame.x { Text: \"hi\"; Visible: true; }", &[]);
    assert!(stderr.contains("`Text` sets Text, which Frame doesn't have"), "{stderr}");
    assert!(!rule_of(&out, "Frame.x").contains("Text ="), "{out}");
}

#[test]
fn tags_must_name_gui_object_classes() {
    let dir = TempDir::new("bad-tags");
    let tags = dir.write("tags.json", r#"{ "x": ["Part"] }"#);
    let input = dir.write("in.scss", ".x { Visible: true; }");
    let (code, _, stderr) = run(&[input.to_str().unwrap(), "--tags", tags.to_str().unwrap(), "-o", "-"], None);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("`x` lists `Part`"), "{stderr}");
}

// ---------- themes, live tokens, model files ----------

#[test]
fn data_theme_and_color_scheme_rules_become_theme_sheets() {
    let (out, stderr) = compile(
        ":root { --bg: #fff; --gap: 4px; --keep: #123456; } [data-theme=\"dark\"] { --bg: #111; } \
         @media (prefers-color-scheme: light) { :root { --gap: 12px; } } .a { background-color: var(--bg); }",
        &["--approx"],
    );
    assert!(stderr.is_empty(), "{stderr}");
    // A sheet's own attribute beats its theme's, so themed tokens live only on the themes.
    assert!(out.contains("sheet:SetAttribute(\"keep\""), "{out}");
    assert!(!out.contains("sheet:SetAttribute(\"bg\""), "{out}");
    assert!(out.contains("local theme = Instance.new(\"StyleDerive\")\ntheme.Name = \"Theme\""), "{out}");
    let default = &out[out.find("t.Name = \"default\"").unwrap()..];
    assert!(default[..default.find("end").unwrap()].contains("theme.StyleSheet = t"), "{out}");
    let dark = &out[out.find("t.Name = \"dark\"").unwrap()..];
    let dark = &dark[..dark.find("end").unwrap()];
    assert!(dark.contains("t:SetAttribute(\"bg\", Color3.fromRGB(17, 17, 17))"), "{out}");
    assert!(dark.contains("t:SetAttribute(\"gap\", 4)"), "a theme starts from the defaults:\n{out}");
    let light = &out[out.find("t.Name = \"light\"").unwrap()..];
    assert!(light[..light.find("end").unwrap()].contains("t:SetAttribute(\"gap\", 12)"), "{out}");
    assert!(rule_of(&out, ".a").contains("[\"BackgroundColor3\"] = \"$bg\""), "{out}");
}

#[test]
fn only_root_custom_properties_can_depend_on_a_theme() {
    let (out, stderr) =
        compile("@media (prefers-color-scheme: dark) { .a { Visible: false; } :root { color: red; } }", &[]);
    assert!(stderr.contains("depends on the theme `dark`"), "{stderr}");
    assert!(stderr.contains("property \"color\" in a theme is ignored"), "{stderr}");
    assert!(!out.contains("\".a\""), "{out}");
}

#[test]
fn length_tokens_stay_live_where_a_udim_property_uses_them_whole() {
    let (out, stderr) = compile(
        ":root { --radius: 8px; --half: 50%; --pad: 6px; --w: 30px; } \
         .a { border-radius: var(--radius); padding: var(--pad); width: var(--w); height: 4px; } \
         .b { border-radius: var(--half); } .c { padding: var(--pad); border: 2px solid red; }",
        &["--approx"],
    );
    assert!(stderr.is_empty(), "{stderr}");
    // A number attribute in a UDim property never updates in Roblox, so these are UDims.
    assert!(out.contains("sheet:SetAttribute(\"radius\", UDim.new(0, 8))"), "{out}");
    assert!(out.contains("sheet:SetAttribute(\"half\", UDim.new(0.5, 0))"), "{out}");
    assert!(out.contains("sheet:SetAttribute(\"w\", 30)"), "a compiled-in token stays a number:\n{out}");
    assert!(rule_of(&out, ".a::UICorner").contains("[\"CornerRadius\"] = \"$radius\""), "{out}");
    assert!(rule_of(&out, ".a::UIPadding").contains("[\"PaddingTop\"] = \"$pad\""), "{out}");
    assert!(rule_of(&out, ".a").contains("[\"Size\"] = UDim2.new(0, 30, 0, 4)"), "{out}");
    assert!(rule_of(&out, ".b::UICorner").contains("[\"CornerRadius\"] = \"$half\""), "{out}");
    // The border adds to the padding, which can't be the token then.
    assert!(rule_of(&out, ".c::UIPadding").contains("[\"PaddingTop\"] = UDim.new(0, 8)"), "{out}");
}

#[test]
fn a_redefined_token_is_resolved_in_the_redefining_rule() {
    let (out, _) = compile(
        ":root { --c: #000; --r: 4px; } .a { color: var(--c); border-radius: var(--r); } .a.b { --c: #f00; --r: 9px; }",
        &["--approx"],
    );
    // A `"$Name"` reference only sees its own rule's tokens: the redefining rule repeats the
    // declarations, and its pseudo-instance rule gets the attribute too.
    let b = &out[out.find("rule(sheet, \".a.b\"").unwrap()..];
    let b = &b[..b.find("\nend").unwrap()];
    assert!(
        b.contains("[\"TextColor3\"] = \"$c\"") && b.contains("SetAttribute(\"c\", Color3.fromRGB(255, 0, 0))"),
        "{out}"
    );
    let corner = &out[out.find("rule(sheet, \".a.b::UICorner\"").unwrap()..];
    let corner = &corner[..corner.find("\nend").unwrap()];
    assert!(
        corner.contains("[\"CornerRadius\"] = \"$r\"") && corner.contains("SetAttribute(\"r\", UDim.new(0, 9))"),
        "{out}"
    );
}

#[test]
fn a_theme_token_compiled_in_warns() {
    let (_, stderr) = compile(
        ":root { --w: 10px; } [data-theme=dark] { --w: 20px; } .a { width: var(--w); height: 1px; }",
        &["--approx"],
    );
    assert!(stderr.contains("`--w` changes with the theme, but `width` compiles it in"), "{stderr}");
}

#[test]
fn emit_rbxmx_writes_a_model_without_code() {
    let (out, stderr) = compile(
        ":root { --bg: #fff; } [data-theme=dark] { --bg: #111; } \
         .card { BackgroundColor3: var(--bg); Size: UDim2.new(0, 100, 0, 20); Transition: BackgroundColor3 0.2s; } \
         .all { Transition: * 1s; }",
        &["--emit", "rbxmx"],
    );
    assert!(out.starts_with("<roblox version=\"4\">"), "{out}");
    assert!(out.contains("<Item class=\"StyleDerive\""), "{out}");
    assert!(out.contains("<Ref name=\"StyleSheet\">RBX2</Ref>"), "{out}");
    let card = &out[out.find("<string name=\"Selector\">.card</string>").unwrap()..];
    // BackgroundColor3 = "$bg", Size = {0, 100}, {0, 20} — bytes Studio saves for the same values.
    assert!(card.contains(
        "<BinaryString name=\"PropertiesSerialize\">AgAAABAAAABCYWNrZ3JvdW5kQ29sb3IzAgMAAAAkYmcEAAAAU2l6ZQoAAAAAZAAAAAAAAAAUAAAA</BinaryString>"
    ), "{out}");
    assert!(card.contains("<BinaryString name=\"PropertyTransitionsSerialize\">"), "{out}");
    assert!(stderr.contains("a default transition (`all`) isn't saved in a model file"), "{stderr}");
    assert!(!out.contains("Instance.new") && !out.contains("local "), "{out}");
}

#[test]
fn overflow_wrap_break_word_is_what_roblox_does() {
    let (_, stderr) = compile(
        ".a { overflow-wrap: break-word; } .b { word-wrap: anywhere; } .c { overflow-wrap: normal; }",
        &["--approx"],
    );
    assert_eq!(stderr.matches("warning").count(), 1, "{stderr}");
    assert!(stderr.contains("always breaks a word too long for its line"), "{stderr}");
}

// ---------- pictures ----------

#[test]
fn border_image_is_a_nine_slice_of_a_picture_whose_size_its_url_gives() {
    let (out, stderr) = compile(
        ".p { border-image: url(\"rbxassetid://1#192x44\") 18 fill; } \
         .p:hover { border-image-source: url(\"rbxassetid://2#192x44\"); } \
         .bar { border-image: url(\"rbxassetid://3#64x12\") 6 8 fill; border-image-width: 12px; }",
        &["--approx"],
    );
    assert!(stderr.is_empty(), "{stderr}");
    let p = rule_of(&out, ".p");
    assert!(p.contains("[\"Image\"] = \"rbxassetid://1\","), "the size isn't part of the id:\n{out}");
    assert!(
        p.contains("[\"ScaleType\"] = Enum.ScaleType.Slice")
            && p.contains("[\"SliceCenter\"] = Rect.new(18, 18, 174, 26)"),
        "{out}"
    );
    // A rule that only swaps the picture keeps the slicing.
    let hover = rule_of(&out, ".p:Hover");
    assert!(
        hover.contains("[\"Image\"] = \"rbxassetid://2\"")
            && hover.contains("[\"SliceCenter\"] = Rect.new(18, 18, 174, 26)"),
        "{out}"
    );
    let bar = rule_of(&out, ".bar");
    assert!(bar.contains("[\"SliceCenter\"] = Rect.new(8, 6, 56, 6)") && bar.contains("[\"SliceScale\"] = 2"), "{out}");
}

#[test]
fn border_image_says_what_roblox_needs() {
    let (_, stderr) = compile(
        ".a { border-image: url(\"rbxassetid://1\") 6 fill; } .b { border-image: url(\"rbxassetid://2#10x10\") 2; }",
        &["--approx"],
    );
    assert!(stderr.contains("end its url with its size"), "{stderr}");
    assert!(stderr.contains("always draws the middle of a sliced picture; add `fill`"), "{stderr}");
}

#[test]
fn contain_size_stops_an_element_sizing_itself_by_its_content() {
    let (out, _) = compile(".fill { contain: size; background-color: red; }", &["--approx"]);
    assert!(rule_of(&out, ".fill").contains("[\"AutomaticSize\"] = Enum.AutomaticSize.None"), "{out}");
}

#[test]
fn multiply_tints_the_picture() {
    let (out, _) = compile(
        ".f { background-image: url(\"rbxassetid://7\"); background-color: rgb(176, 160, 146); \
         background-blend-mode: multiply; } .f.full { background-color: white; }",
        &["--approx"],
    );
    let f = rule_of(&out, ".f");
    assert!(
        f.contains("[\"ImageColor3\"] = Color3.fromRGB(176, 160, 146)") && !f.contains("BackgroundColor3"),
        "{out}"
    );
    assert!(f.contains("[\"BackgroundTransparency\"] = 1"), "{out}");
    assert!(rule_of(&out, ".f.full").contains("[\"ImageColor3\"] = Color3.fromRGB(255, 255, 255)"), "{out}");
}

#[test]
fn background_image_none_clears_the_picture() {
    let (out, _) = compile(
        ".x { background-image: url(\"rbxassetid://8#4x4\"); } .x.worn { background-image: none; }",
        &["--approx"],
    );
    assert!(rule_of(&out, ".x").contains("[\"Image\"] = \"rbxassetid://8\""), "{out}");
    assert!(rule_of(&out, ".x.worn").contains("[\"Image\"] = \"\""), "{out}");
}

#[test]
fn text_shadow_is_the_text_stroke() {
    let (out, stderr) = compile(
        ".a { text-shadow: 0 0 1px rgba(0, 0, 0, 0.4); } .b { text-shadow: 0 0 rgb(120, 30, 10); } .c { text-shadow: 2px 2px 4px black; }",
        &["--approx"],
    );
    let a = rule_of(&out, ".a");
    assert!(
        a.contains("[\"TextStrokeColor3\"] = Color3.fromRGB(0, 0, 0)")
            && a.contains("[\"TextStrokeTransparency\"] = 0.6"),
        "{out}"
    );
    let b = rule_of(&out, ".b");
    assert!(
        b.contains("[\"TextStrokeColor3\"] = Color3.fromRGB(120, 30, 10)")
            && b.contains("[\"TextStrokeTransparency\"] = 0"),
        "{out}"
    );
    assert!(stderr.contains("no offset or blur"), "{stderr}");
    assert_eq!(stderr.matches("warning").count(), 1, "{stderr}");
}

#[test]
fn url_of_an_expression_is_a_function_call() {
    let (out, stderr) = compile(
        "$pictures: (\"rbxassetid://1#4x4\", \"rbxassetid://2#4x4\"); $one: \"rbxassetid://3\";          .a { background-image: url(nth($pictures, 2)); } .b { background-image: url($one); }          .c { background-image: url(rbxassetid://4); }",
        &["--approx"],
    );
    assert!(stderr.is_empty(), "{stderr}");
    assert!(rule_of(&out, ".a").contains("[\"Image\"] = \"rbxassetid://2\""), "{out}");
    assert!(rule_of(&out, ".b").contains("[\"Image\"] = \"rbxassetid://3\""), "{out}");
    assert!(rule_of(&out, ".c").contains("[\"Image\"] = \"rbxassetid://4\""), "{out}");
}

#[test]
fn text_sizes_and_weights_in_the_family_the_root_gives_it() {
    let (out, _) = compile(
        "ScreenGui { font: 14px Merriweather; } .a { font-size: 20px; } .b { font-weight: bold; }",
        &["--approx"],
    );
    // Merriweather's line is taller than its em, so 20px is a larger TextSize than 20.
    assert!(!rule_of(&out, ".a").contains("[\"TextSize\"] = 20,"), "{out}");
    assert!(rule_of(&out, ".b").contains("Merriweather.json\", Enum.FontWeight.Bold"), "{out}");
}
