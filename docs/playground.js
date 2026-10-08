import init, { compile, approx_groups, example_files } from "./pkg/outlass_web.js";

const STORAGE_KEY = "outlass-playground";
const $ = (id) => document.getElementById(id);

const source = $("source");
const gutter = $("gutter");
const output = $("output");
const status = $("status");

/** files: [{ name, text }] in tab order; entry is the file compiled. */
let state = { files: [], active: "", entry: "", emit: "luau", approx: ["all"], strict: false, userAgent: true };
let lastResult = null;

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY));
    if (saved?.files?.length) return { ...state, ...saved };
  } catch {}
  return null;
}

function save() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {}
}

function exampleState() {
  const examples = example_files();
  const files = Object.entries(examples).map(([name, text]) => ({ name, text }));
  return { ...state, files, active: "showcase.scss", entry: "showcase.scss" };
}

const file = (name) => state.files.find((f) => f.name === name);
const isPartial = (name) => name.split("/").pop().startsWith("_");

// ---------- compiling ----------

let timer = 0;
function scheduleCompile() {
  clearTimeout(timer);
  output.classList.add("stale");
  timer = setTimeout(run, 120);
}

function run() {
  save();
  const files = Object.fromEntries(state.files.map((f) => [f.name, f.text]));
  const started = performance.now();
  let result;
  try {
    result = file(state.entry)
      ? compile(files, state.entry, state.emit, state.approx, state.strict, state.userAgent)
      : { ok: false, output: "", diagnostics: [{ level: "error", message: "Choose a file to compile (the dot on its tab)." }] };
  } catch (e) {
    // Rust panics surface here; keep the playground usable.
    result = { ok: false, output: "", diagnostics: [{ level: "error", message: `internal compiler error: ${e}` }] };
  }
  const ms = performance.now() - started;
  lastResult = result;
  output.classList.remove("stale");

  if (result.ok) output.textContent = result.output;
  output.classList.toggle("stale", !result.ok && output.textContent !== "");

  const errors = result.diagnostics.filter((d) => d.level === "error").length;
  const warnings = result.diagnostics.filter((d) => d.level === "warning").length;
  status.className = "status" + (errors ? " error" : "");
  status.textContent = errors
    ? `${errors} error${errors > 1 ? "s" : ""}`
    : `${ms.toFixed(1)} ms` + (warnings ? ` · ${warnings} warning${warnings > 1 ? "s" : ""}` : "");

  renderDiagnostics(result.diagnostics);
  renderGutter();
}

function renderDiagnostics(diagnostics) {
  const list = $("diagnostics");
  list.replaceChildren(
    ...diagnostics.map((d) => {
      const li = document.createElement("li");
      li.className = d.level;
      const level = Object.assign(document.createElement("span"), { className: "level", textContent: d.level });
      const parts = [level];
      if (d.file) {
        parts.push(Object.assign(document.createElement("span"), { className: "where", textContent: `${d.file}:${d.line}:${d.col}` }));
        li.dataset.file = d.file;
        li.dataset.line = d.line;
        li.dataset.col = d.col;
      }
      parts.push(document.createTextNode(d.message));
      li.append(...parts);
      return li;
    }),
  );
}

$("diagnostics").addEventListener("click", (e) => {
  const li = e.target.closest("li[data-line]");
  if (!li) return;
  if (file(li.dataset.file)) selectFile(li.dataset.file);
  goTo(Number(li.dataset.line), Number(li.dataset.col));
});

function goTo(line, col) {
  const lines = source.value.split("\n");
  let offset = 0;
  for (let i = 0; i < line - 1 && i < lines.length; i++) offset += lines[i].length + 1;
  offset += Math.max(0, col - 1);
  source.focus();
  source.setSelectionRange(offset, offset);
  source.scrollTop = Math.max(0, (line - 5) * 20);
  gutter.scrollTop = source.scrollTop;
}

// ---------- editor ----------

function renderGutter() {
  const count = source.value.split("\n").length;
  const marks = new Map();
  for (const d of lastResult?.diagnostics ?? []) {
    if (d.file !== state.active || !d.line) continue;
    if (d.level === "error") marks.set(d.line, "bad");
    else if (d.level === "warning" && !marks.has(d.line)) marks.set(d.line, "warn");
  }
  const frag = document.createDocumentFragment();
  for (let i = 1; i <= count; i++) {
    const mark = marks.get(i);
    if (mark) frag.append(Object.assign(document.createElement("span"), { className: mark, textContent: String(i) }));
    else frag.append(String(i));
    frag.append("\n");
  }
  // Room for the textarea's horizontal scrollbar.
  frag.append("\n");
  gutter.replaceChildren(frag);
  gutter.scrollTop = source.scrollTop;
}

source.addEventListener("input", () => {
  file(state.active).text = source.value;
  renderGutter();
  scheduleCompile();
});

source.addEventListener("scroll", () => (gutter.scrollTop = source.scrollTop));

source.addEventListener("keydown", (e) => {
  if (e.key !== "Tab" || e.ctrlKey || e.metaKey || e.altKey) return;
  e.preventDefault();
  const { selectionStart: start, selectionEnd: end, value } = source;
  const lineStart = value.lastIndexOf("\n", start - 1) + 1;
  if (start === end && !e.shiftKey) {
    source.setRangeText("  ", start, end, "end");
  } else {
    // Indent or outdent every selected line.
    const block = value.slice(lineStart, end);
    const changed = e.shiftKey ? block.replace(/^ {1,2}/gm, "") : block.replace(/^/gm, "  ");
    source.setRangeText(changed, lineStart, end, "select");
  }
  source.dispatchEvent(new Event("input"));
});

// ---------- file tabs ----------

function renderFileTabs() {
  const tabs = $("file-tabs");
  tabs.replaceChildren();
  for (const f of state.files) {
    const tab = document.createElement("button");
    tab.setAttribute("role", "tab");
    tab.setAttribute("aria-selected", String(f.name === state.active));
    tab.title = isPartial(f.name) ? "Partial: load it with @use" : "Click the dot to compile this file";

    if (!isPartial(f.name)) {
      const dot = document.createElement("span");
      dot.className = "dot" + (f.name === state.entry ? " on" : "");
      dot.title = "Compile this file";
      dot.addEventListener("click", (e) => {
        e.stopPropagation();
        state.entry = f.name;
        renderFileTabs();
        scheduleCompile();
      });
      tab.append(dot);
    }
    tab.append(f.name);
    if (state.files.length > 1) {
      const close = Object.assign(document.createElement("span"), { className: "close", textContent: "×", title: "Delete file" });
      close.addEventListener("click", (e) => {
        e.stopPropagation();
        deleteFile(f.name);
      });
      tab.append(close);
    }
    tab.addEventListener("click", () => selectFile(f.name));
    tab.addEventListener("dblclick", () => startRename(tab, f));
    tabs.append(tab);
  }
  const add = Object.assign(document.createElement("button"), { className: "add", textContent: "+", title: "New file" });
  add.addEventListener("click", newFile);
  tabs.append(add);
}

function selectFile(name) {
  state.active = name;
  source.value = file(name).text;
  source.scrollTop = 0;
  renderFileTabs();
  renderGutter();
  save();
}

function newFile() {
  let n = 1;
  while (file(`_partial${n}.scss`)) n++;
  const name = `_partial${n}.scss`;
  state.files.push({ name, text: "" });
  selectFile(name);
  startRename([...$("file-tabs").children].at(-2), file(name));
}

function deleteFile(name) {
  if (!confirm(`Delete ${name}?`)) return;
  const index = state.files.findIndex((f) => f.name === name);
  state.files.splice(index, 1);
  if (state.entry === name) state.entry = state.files.find((f) => !isPartial(f.name))?.name ?? "";
  if (state.active === name) state.active = state.files[Math.max(0, index - 1)].name;
  selectFile(state.active);
  scheduleCompile();
}

function startRename(tab, f) {
  const input = Object.assign(document.createElement("input"), { className: "rename", value: f.name });
  tab.replaceWith(input);
  input.focus();
  input.setSelectionRange(0, f.name.replace(/\.(scss|sass|css)$/, "").length);
  let done = false;
  const finish = (commit) => {
    if (done) return;
    done = true;
    const name = input.value.trim().replace(/\\/g, "/");
    if (commit && name && name !== f.name && !file(name)) {
      if (state.entry === f.name) state.entry = isPartial(name) ? "" : name;
      if (state.active === f.name) state.active = name;
      f.name = name;
      if (!state.entry && !isPartial(name)) state.entry = name;
      scheduleCompile();
    }
    renderFileTabs();
    save();
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") finish(true);
    if (e.key === "Escape") finish(false);
  });
  input.addEventListener("blur", () => finish(true));
}

// ---------- output ----------

for (const tab of $("emit-tabs").querySelectorAll("[data-emit]")) {
  tab.addEventListener("click", () => {
    state.emit = tab.dataset.emit;
    renderEmitTabs();
    run();
  });
}

function renderEmitTabs() {
  for (const tab of $("emit-tabs").querySelectorAll("[data-emit]")) {
    tab.setAttribute("aria-selected", String(tab.dataset.emit === state.emit));
  }
}

$("copy").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(output.textContent);
    flash($("copy"), "Copied");
  } catch {
    flash($("copy"), "Failed");
  }
});

$("download").addEventListener("click", () => {
  const stem = state.entry.split("/").pop().replace(/\.(scss|sass|css)$/, "") || "StyleSheet";
  const blob = new Blob([output.textContent], { type: "text/plain" });
  const link = Object.assign(document.createElement("a"), { href: URL.createObjectURL(blob), download: `${stem}.${state.emit}` });
  link.click();
  URL.revokeObjectURL(link.href);
});

function flash(button, text) {
  const original = button.textContent;
  button.textContent = text;
  setTimeout(() => (button.textContent = original), 1200);
}

// ---------- options ----------

function renderApprox() {
  const all = state.approx.includes("all");
  $("approx-all").checked = all;
  for (const box of $("approx-groups").querySelectorAll("input")) {
    box.checked = all || state.approx.includes(box.value);
    box.disabled = all;
  }
  $("approx-summary").textContent = all ? "all" : state.approx.length ? state.approx.join(", ") : "off";
}

function setupOptions() {
  for (const group of approx_groups()) {
    const label = document.createElement("label");
    const box = Object.assign(document.createElement("input"), { type: "checkbox", value: group });
    box.addEventListener("change", () => {
      state.approx = [...$("approx-groups").querySelectorAll("input:checked")].map((b) => b.value);
      renderApprox();
      scheduleCompile();
    });
    label.append(box, group);
    $("approx-groups").append(label);
  }
  $("approx-all").addEventListener("change", (e) => {
    state.approx = e.target.checked ? ["all"] : approx_groups();
    renderApprox();
    scheduleCompile();
  });
  $("strict").addEventListener("change", (e) => {
    state.strict = e.target.checked;
    scheduleCompile();
  });
  $("user-agent").addEventListener("change", (e) => {
    state.userAgent = e.target.checked;
    scheduleCompile();
  });
  $("reset").addEventListener("click", () => {
    if (!confirm("Replace your files with the repository examples?")) return;
    state = { ...exampleState(), emit: state.emit, approx: state.approx, strict: state.strict, userAgent: state.userAgent };
    selectFile(state.active);
    run();
  });
  document.addEventListener("click", (e) => {
    if (!$("approx").contains(e.target)) $("approx").open = false;
  });
}

// ---------- start ----------

await init();
state = load() ?? exampleState();
if (!file(state.active)) state.active = state.files[0].name;
setupOptions();
renderApprox();
renderEmitTabs();
$("strict").checked = state.strict;
$("user-agent").checked = state.userAgent;
selectFile(state.active);
run();
