import { basicSetup } from "https://esm.sh/codemirror@6.0.2?deps=@codemirror/state@6.7.1,@codemirror/view@6.43.9";
import { EditorState, StateEffect, StateField } from "https://esm.sh/@codemirror/state@6.7.1";
import { Decoration, EditorView } from "https://esm.sh/@codemirror/view@6.43.9";
import { yaml } from "https://esm.sh/@codemirror/lang-yaml@6.1.3?deps=@codemirror/state@6.7.1,@codemirror/view@6.43.9";
import init, { run_command } from "./pkg/yaml_rt_wasm.js";

const baseSource = `# Production services — comments and style stay put
services:
  - name: api
    port: 8080 # public endpoint
    enabled: TRUE
  - {name: worker, port: 8081, enabled: false}
defaults: &defaults
  retries: 0x3
mirror: *defaults
`;

const examples = [
  { name: "Replace every service port (JSONPath)", command: "replace", selectorKind: "jsonpath", selector: "$.services[*].port", value: "9090" },
  { name: "Get an exact node (JSON Pointer)", command: "get", selectorKind: "pointer", selector: "/services/0" },
  { name: "Query enabled services", command: "query", selectorKind: "jsonpath", selector: "$.services[?@.enabled == true].name" },
  { name: "Add a nested value", command: "add", selectorKind: "pointer", selector: "/services/0/tls", value: "{enabled: true, mode: strict}" },
  { name: "Remove sequence entries safely", command: "remove", selectorKind: "jsonpath", selector: "$.services[0,1]" },
  { name: "Rename matching keys", command: "rename-key", selectorKind: "jsonpath", selector: "$.services[*].port", newKey: "listen" },
  { name: "Move a value", command: "move", from: "/services/0/port", destination: "/services/1/api-port" },
  { name: "Copy an anchored-free value", command: "copy", from: "/services/0/name", destination: "/services/1/source" },
  { name: "Test semantic equality", command: "test", selectorKind: "pointer", selector: "/defaults/retries", value: "3" },
  { name: "Transactional patch", command: "patch", patch: "- op: test\n  path: /services/0/port\n  value: 8080\n- op: replace\n  path: /services/0/port\n  value: 8443\n- op: add\n  path: /services/0/protocol\n  value: https\n" },
  { name: "Patch rollback on failure", command: "patch", patch: "- op: replace\n  path: /services/0/port\n  value: 8443\n- op: test\n  path: /services/1/port\n  value: 9999\n" },
  { name: "Edit a multi-document stream", source: "---\nname: development\nport: 3000\n---\nname: production\nport: 8080 # keep\n", command: "replace", selectorKind: "pointer", selector: "/port", value: "443", documentIndex: 1 },
];

const $ = (id) => document.getElementById(id);
const controls = {
  example: $("example"), command: $("command"), selectorKind: $("selector-kind"),
  selector: $("selector"), from: $("from"), destination: $("destination"),
  value: $("value"), newKey: $("new-key"), patch: $("patch"), documentIndex: $("document-index"),
};

const setChangedLines = StateEffect.define();
const setErrorLine = StateEffect.define();
const changedLines = StateField.define({
  create: () => Decoration.none,
  update(value, transaction) {
    value = value.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setChangedLines)) value = effect.value;
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});
const errorLine = StateField.define({
  create: () => Decoration.none,
  update(value, transaction) {
    value = value.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setErrorLine)) value = effect.value;
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});

function editor(parent, text, readOnly, onChange) {
  return new EditorView({
    parent,
    state: EditorState.create({
      doc: text,
      extensions: [
        basicSetup,
        yaml(),
        EditorView.lineWrapping,
        EditorState.readOnly.of(readOnly),
        changedLines,
        errorLine,
        EditorView.updateListener.of((update) => {
          if (update.docChanged && onChange) onChange();
        }),
      ],
    }),
  });
}

let sourceEditor;
let resultEditor;
let ready = false;
let debounce;
let lastValid = baseSource;
let activeExample = 0;

function text(view) { return view.state.doc.toString(); }
function replaceText(view, value) {
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
}

function shellQuote(value) {
  if (!value) return "''";
  return `'${value.replaceAll("'", `'\\''`)}'`;
}

function commandPreview() {
  const command = controls.command.value;
  let preview = `yaml-rt ${command}`;
  if (command === "query") preview += ` ${shellQuote(controls.selector.value)}`;
  else if (["move", "copy"].includes(command)) preview += ` ${shellQuote(controls.from.value)} ${shellQuote(controls.destination.value)}`;
  else if (command === "patch") preview += ` --patch ${shellQuote(controls.patch.value)}`;
  else {
    preview += controls.selectorKind.value === "jsonpath"
      ? ` --query ${shellQuote(controls.selector.value)}`
      : ` ${shellQuote(controls.selector.value)}`;
    if (["add", "replace", "test"].includes(command)) preview += ` --value ${shellQuote(controls.value.value)}`;
    if (command === "rename-key") preview += ` --to ${shellQuote(controls.newKey.value)}`;
  }
  if (Number(controls.documentIndex.value)) preview += ` --doc ${controls.documentIndex.value}`;
  $("command-preview").textContent = preview;
  return preview;
}

function updateFields() {
  const command = controls.command.value;
  const dualSelector = ["get", "add", "remove", "replace", "rename-key", "test"].includes(command);
  const hasSelector = command === "query" || dualSelector;
  $("selector-kind-field").hidden = !dualSelector;
  $("selector-field").hidden = !hasSelector;
  $("from-field").hidden = !["move", "copy"].includes(command);
  $("destination-field").hidden = !["move", "copy"].includes(command);
  $("value-field").hidden = !["add", "replace", "test"].includes(command);
  $("new-key-field").hidden = command !== "rename-key";
  $("patch-field").hidden = command !== "patch";
  if (command === "query") controls.selectorKind.value = "jsonpath";
  $("selector-label").textContent = controls.selectorKind.value === "jsonpath" ? "JSONPath (RFC 9535)" : "JSON Pointer (RFC 6901)";
  controls.selector.placeholder = controls.selectorKind.value === "jsonpath" ? "$.services[*].port" : "/services/0/port";
  commandPreview();
}

function changedLineNumbers(before, after) {
  const left = before.split(/\r?\n/);
  const right = after.split(/\r?\n/);
  if (left.length * right.length <= 250000) {
    const lengths = Array.from({ length: left.length + 1 }, () => new Uint32Array(right.length + 1));
    for (let i = left.length - 1; i >= 0; i--) {
      for (let j = right.length - 1; j >= 0; j--) {
        lengths[i][j] = left[i] === right[j]
          ? lengths[i + 1][j + 1] + 1
          : Math.max(lengths[i + 1][j], lengths[i][j + 1]);
      }
    }
    const changed = [];
    let i = 0;
    let j = 0;
    while (i < left.length && j < right.length) {
      if (left[i] === right[j]) { i++; j++; }
      else if (lengths[i + 1][j] >= lengths[i][j + 1]) i++;
      else { changed.push(j + 1); j++; }
    }
    while (j < right.length) changed.push(++j);
    return changed;
  }
  let prefix = 0;
  while (prefix < left.length && prefix < right.length && left[prefix] === right[prefix]) prefix++;
  let suffix = 0;
  while (suffix < left.length - prefix && suffix < right.length - prefix && left[left.length - 1 - suffix] === right[right.length - 1 - suffix]) suffix++;
  const lines = [];
  for (let line = prefix + 1; line <= right.length - suffix; line++) lines.push(line);
  return lines;
}

function markChanges(before, after) {
  const decorations = [];
  for (const number of changedLineNumbers(before, after)) {
    if (number <= resultEditor.state.doc.lines) {
      decorations.push(Decoration.line({ class: "cm-changed-line" }).range(resultEditor.state.doc.line(number).from));
    }
  }
  resultEditor.dispatch({ effects: setChangedLines.of(Decoration.set(decorations, true)) });
}

function setDocuments(count) {
  const selected = Math.min(Number(controls.documentIndex.value), Math.max(0, count - 1));
  controls.documentIndex.replaceChildren(...Array.from({ length: Math.max(1, count) }, (_, index) => new Option(String(index), String(index))));
  controls.documentIndex.value = String(selected);
  $("document-count").textContent = count ? `${count} document${count === 1 ? "" : "s"}` : "";
}

function clearDiagnostics() {
  sourceEditor.dispatch({ effects: setErrorLine.of(Decoration.none) });
  for (const control of Object.values(controls)) {
    control.classList.remove("invalid");
    control.removeAttribute("aria-invalid");
  }
}

function markDiagnostic(result) {
  const field = {
    patch: controls.patch,
    selector: controls.selector,
    value: controls.value,
    command: controls.command,
  }[result.error_source];
  if (field) {
    field.classList.add("invalid");
    field.setAttribute("aria-invalid", "true");
  }
  if (result.error_source === "document" && result.line && result.line <= sourceEditor.state.doc.lines) {
    const position = sourceEditor.state.doc.line(result.line).from;
    sourceEditor.dispatch({ effects: setErrorLine.of(Decoration.set([Decoration.line({ class: "cm-error-line" }).range(position)])) });
  }
}

function run() {
  commandPreview();
  if (!ready) return;
  clearDiagnostics();
  const source = text(sourceEditor);
  const wasmResult = run_command(
    source,
    Number(controls.documentIndex.value),
    controls.command.value,
    controls.selectorKind.value,
    controls.selector.value,
    controls.from.value,
    controls.destination.value,
    controls.value.value,
    controls.newKey.value,
    controls.patch.value,
  );
  const result = {
    ok: wasmResult.ok,
    output_yaml: wasmResult.output_yaml,
    command_output: wasmResult.command_output,
    matched_pointers: wasmResult.matched_pointers,
    document_count: wasmResult.document_count,
    error_source: wasmResult.error_source,
    message: wasmResult.message,
    operation_index: wasmResult.operation_index,
    line: wasmResult.line,
    column: wasmResult.column,
  };
  wasmResult.free();
  setDocuments(result.document_count);
  if (result.ok) {
    lastValid = result.output_yaml;
    replaceText(resultEditor, lastValid);
    markChanges(source, lastValid);
    $("run-state").textContent = "Ready";
    $("run-state").className = "status success";
    $("diagnostic").hidden = true;
    $("stale").hidden = true;
    const showOutput = Boolean(result.command_output) || ["query", "get", "test"].includes(controls.command.value);
    $("command-output-wrap").hidden = !showOutput;
    $("command-output").textContent = result.command_output || "No matches.";
    const count = result.matched_pointers.length;
    $("match-count").textContent = count ? `${count} match${count === 1 ? "" : "es"}` : "";
  } else {
    $("run-state").textContent = "Error";
    $("run-state").className = "status error";
    const location = result.line ? ` at ${result.line}:${result.column}` : "";
    const operation = result.operation_index != null ? ` (operation ${result.operation_index})` : "";
    $("diagnostic").textContent = `${result.error_source || "command"}${operation}${location}: ${result.message || "Unknown error"}`;
    $("diagnostic").hidden = false;
    $("stale").hidden = false;
    markDiagnostic(result);
  }
}

function scheduleRun() {
  updateFields();
  clearTimeout(debounce);
  debounce = setTimeout(run, 280);
}

function loadExample(index) {
  activeExample = index;
  const example = examples[index];
  replaceText(sourceEditor, example.source || baseSource);
  controls.command.value = example.command;
  controls.selectorKind.value = example.selectorKind || "pointer";
  controls.selector.value = example.selector || "";
  controls.from.value = example.from || "";
  controls.destination.value = example.destination || "";
  controls.value.value = example.value || "";
  controls.newKey.value = example.newKey || "";
  controls.patch.value = example.patch || "";
  const requestedDocument = example.documentIndex || 0;
  if (![...controls.documentIndex.options].some((option) => Number(option.value) === requestedDocument)) {
    controls.documentIndex.add(new Option(String(requestedDocument), String(requestedDocument)));
  }
  controls.documentIndex.value = String(requestedDocument);
  updateFields();
  run();
}

async function copy(value, button) {
  await navigator.clipboard.writeText(value);
  const previous = button.textContent;
  button.textContent = "Copied";
  setTimeout(() => { button.textContent = previous; }, 1000);
}

async function start() {
  sourceEditor = editor($("source-editor"), baseSource, false, scheduleRun);
  resultEditor = editor($("result-editor"), baseSource, true);
  examples.forEach((example, index) => controls.example.add(new Option(example.name, String(index))));
  Object.values(controls).forEach((control) => control.addEventListener("input", scheduleRun));
  controls.example.addEventListener("change", () => loadExample(Number(controls.example.value)));
  $("run").addEventListener("click", run);
  $("reset").addEventListener("click", () => loadExample(activeExample));
  $("copy-result").addEventListener("click", () => copy(text(resultEditor), $("copy-result")));
  $("copy-command").addEventListener("click", () => copy(commandPreview(), $("copy-command")));
  updateFields();
  try {
    await init();
    ready = true;
    loadExample(0);
  } catch (error) {
    $("run-state").textContent = "Load failed";
    $("run-state").className = "status error";
    $("diagnostic").textContent = `Unable to initialize the WebAssembly module: ${error}`;
    $("diagnostic").hidden = false;
  }
}

start();
