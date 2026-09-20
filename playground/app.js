import {
  basicSetup,
  Decoration,
  EditorState,
  EditorView,
  GutterMarker,
  RangeSet,
  StateEffect,
  StateField,
  gutter,
  lineNumbers,
  yaml,
} from "./codemirror.js";
import init, { run_command } from "./pkg/yaml_rt_wasm.js";
import { copyText, lineDiff, resultPresentation, validationPresentation } from "./state.mjs";

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

const serviceSchema = `type: object
properties:
  services:
    type: array
    items:
      type: object
      properties:
        port:
          type: integer
`;
const portSchema = `type: object
properties:
  service:
    type: object
    required: [port]
    properties:
      port:
        type: integer
`;

const examples = [
  { name: "Replace ports and validate both sides", command: "replace", selectorKind: "jsonpath", selector: "$.services[*].port", value: "9090", inputSchema: serviceSchema, outputSchema: `${serviceSchema}          const: 9090\n` },
  { name: "Repair input that fails its schema", source: "service:\n  port: closed\n", command: "replace", selector: "/service/port", value: "8080", inputSchema: portSchema, outputSchema: portSchema },
  { name: "Detect output that fails its schema", source: "service:\n  port: 8080\n", command: "replace", selector: "/service/port", value: "closed", inputSchema: portSchema, outputSchema: portSchema },
  { name: "Generate a schema from input", source: "service:\n  name: api\n  port: 8080\n  enabled: true\n", command: "schema" },
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
  { name: "Validate YAML and locate an error", source: "services:\n  - name: api\n    ports: [8080, , 8443]\n", command: "validate" },
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
const setValidationLine = StateEffect.define();
const setReadValidationLine = StateEffect.define();
const setDeletedLines = StateEffect.define();
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
const validationLine = StateField.define({
  create: () => Decoration.none,
  update(value, transaction) {
    value = value.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setValidationLine)) value = effect.value;
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});
const readValidationLine = StateField.define({
  create: () => Decoration.none,
  update(value, transaction) {
    value = value.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setReadValidationLine)) value = effect.value;
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});
class DeletedLineMarker extends GutterMarker {
  constructor(removedLines) {
    super();
    this.removedLines = removedLines;
  }

  toDOM() {
    const marker = document.createElement("span");
    const count = this.removedLines.length;
    const label = `${count} deleted line${count === 1 ? "" : "s"}:\n${this.removedLines.join("\n")}`;
    marker.className = "cm-deleted-line-marker";
    marker.title = label;
    marker.setAttribute("aria-label", label);
    marker.textContent = "−";
    return marker;
  }
}
const deletedLines = StateField.define({
  create: () => RangeSet.empty,
  update(value, transaction) {
    value = value.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setDeletedLines)) value = effect.value;
    }
    return value;
  },
  provide: (field) => gutter({ class: "cm-deletion-gutter", markers: (view) => view.state.field(field) }),
});

function editor(parent, text, readOnly, onChange) {
  return new EditorView({
    parent,
    state: EditorState.create({
      doc: text,
      extensions: [
        basicSetup,
        lineNumbers(),
        yaml(),
        EditorView.lineWrapping,
        EditorState.readOnly.of(readOnly),
        changedLines,
        errorLine,
        validationLine,
        readValidationLine,
        deletedLines,
        EditorView.updateListener.of((update) => {
          if (update.docChanged && onChange) onChange();
        }),
      ],
    }),
  });
}

let sourceEditor;
let resultEditor;
let inputSchemaEditor;
let outputSchemaEditor;
let ready = false;
let debounce;
let activeExample = 0;
let loadedExample = null;
const exampleSchemaDrafts = new Map();

function selectTab(side, tab) {
  const yamlEditor = side === "input" ? "source-editor" : "result-editor";
  $(`${side}-schema-editor`).hidden = tab !== "schema";
  $(yamlEditor).hidden = tab !== "yaml";
  for (const choice of ["yaml", "schema"]) {
    const button = $(`${side}-${choice}-tab`);
    button.classList.toggle("active", choice === tab);
    button.setAttribute("aria-selected", String(choice === tab));
  }
}

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
  if (command === "validate") {
    if (text(inputSchemaEditor).trim()) preview += " --schema input.schema.yaml";
    $("command-preview").textContent = preview;
    return preview;
  }
  if (command === "schema") {
    preview += " -";
    $("command-preview").textContent = preview;
    return preview;
  }
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
  $("document-field").hidden = ["validate", "schema"].includes(command);
  if (command === "query") controls.selectorKind.value = "jsonpath";
  $("selector-label").textContent = controls.selectorKind.value === "jsonpath" ? "JSONPath (RFC 9535)" : "JSON Pointer (RFC 6901)";
  controls.selector.placeholder = controls.selectorKind.value === "jsonpath" ? "$.services[*].port" : "/services/0/port";
  commandPreview();
}

function markChanges(before, after) {
  const decorations = [];
  const deletionMarkers = [];
  const diff = lineDiff(before, after);
  for (const number of diff.changedLines) {
    if (number <= resultEditor.state.doc.lines) {
      decorations.push(Decoration.line({ class: "cm-changed-line" }).range(resultEditor.state.doc.line(number).from));
    }
  }
  for (const deletion of diff.deletions) {
    const line = resultEditor.state.doc.line(Math.max(1, deletion.line));
    deletionMarkers.push(new DeletedLineMarker(deletion.removedLines).range(line.from));
  }
  resultEditor.dispatch({ effects: [
    setChangedLines.of(Decoration.set(decorations, true)),
    setDeletedLines.of(RangeSet.of(deletionMarkers, true)),
  ] });
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

function markValidation(view, result, effect = setValidationLine) {
  let decoration = Decoration.none;
  if (result.status === "invalid" && result.line && result.line <= view.state.doc.lines) {
    const source = text(view);
    const start = codeUnitOffset(source, result.span_start ?? 0);
    const end = codeUnitOffset(source, result.span_end ?? result.span_start ?? 0);
    const range = end > start
      ? Decoration.mark({ class: "cm-error-range" }).range(start, end)
      : Decoration.line({ class: "cm-error-line" }).range(view.state.doc.line(result.line).from);
    decoration = Decoration.set([range]);
  }
  view.dispatch({ effects: effect.of(decoration) });
}

function showValidation(side, result, command) {
  const state = $(`${side}-validation`);
  const detail = $(`${side}-validation-detail`);
  const presentation = validationPresentation(result);
  state.className = `validation-state ${result.status}`;
  state.textContent = presentation.label;
  detail.className = `validation-detail ${result.status}`;
  detail.textContent = presentation.detail;
  detail.hidden = !presentation.detail;
  const schemaEditor = side === "input" ? inputSchemaEditor : outputSchemaEditor;
  const yamlEditor = side === "input" ? sourceEditor : resultEditor;
  markValidation(schemaEditor, result.target === "schema" ? result : { status: "skipped" });
  markValidation(yamlEditor, result.target === "yaml" && (side === "input" || !["query", "get", "validate"].includes(command))
    ? result : { status: "skipped" });
}

function codeUnitOffset(source, byteOffset) {
  const target = Math.max(0, byteOffset);
  let bytes = 0;
  let units = 0;
  for (const character of source) {
    const width = new TextEncoder().encode(character).length;
    if (bytes + width > target) break;
    bytes += width;
    units += character.length;
  }
  return units;
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
    const source = text(sourceEditor);
    const start = codeUnitOffset(source, result.span_start ?? 0);
    const end = codeUnitOffset(source, result.span_end ?? result.span_start ?? 0);
    const decoration = end > start
      ? Decoration.mark({ class: "cm-error-range" }).range(start, end)
      : Decoration.line({ class: "cm-error-line" }).range(sourceEditor.state.doc.line(result.line).from);
    sourceEditor.dispatch({ effects: setErrorLine.of(Decoration.set([decoration])) });
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
    text(inputSchemaEditor),
    text(outputSchemaEditor),
  );
  const validationResult = (value) => ({
    status: value.status, message: value.message, target: value.target,
    span_start: value.span_start, span_end: value.span_end,
    line: value.line, column: value.column, document_index: value.document_index,
  });
  const inputValidation = wasmResult.input_validation;
  const outputValidation = wasmResult.output_validation;
  const result = {
    ok: wasmResult.ok,
    output_yaml: wasmResult.output_yaml,
    command_output: wasmResult.command_output,
    matched_pointers: wasmResult.matched_pointers,
    document_count: wasmResult.document_count,
    error_source: wasmResult.error_source,
    message: wasmResult.message,
    rendered_diagnostic: wasmResult.rendered_diagnostic,
    operation_index: wasmResult.operation_index,
    span_start: wasmResult.span_start,
    span_end: wasmResult.span_end,
    line: wasmResult.line,
    column: wasmResult.column,
  };
  result.input_validation = validationResult(inputValidation);
  result.output_validation = validationResult(outputValidation);
  inputValidation.free();
  outputValidation.free();
  wasmResult.free();
  setDocuments(result.document_count);
  const presentation = resultPresentation(result, controls.command.value, source);
  replaceText(resultEditor, presentation.content);
  $("result-title").textContent = presentation.title;
  if (presentation.highlightChanges) markChanges(source, presentation.content);
  else resultEditor.dispatch({ effects: [
    setChangedLines.of(Decoration.none),
    setDeletedLines.of(RangeSet.empty),
  ] });
  $("match-summary").hidden = !presentation.showMatchCount;
  $("copy-result").hidden = !presentation.showCopyResult;
  $("use-schema").hidden = !(controls.command.value === "schema" && result.ok);
  if (presentation.showMatchCount) {
    const count = result.matched_pointers.length;
    $("match-count").textContent = `${count} match${count === 1 ? "" : "es"}`;
  }
  if (result.ok) {
    $("run-state").textContent = "Ready";
    $("run-state").className = "status success";
    $("diagnostic").hidden = true;
  } else {
    $("run-state").textContent = "Error";
    $("run-state").className = "status error";
    const location = result.line ? ` at ${result.line}:${result.column}` : "";
    const operation = result.operation_index != null ? ` (operation ${result.operation_index})` : "";
    $("diagnostic").textContent = result.rendered_diagnostic
      || `${result.error_source || "command"}${operation}${location}: ${result.message || "Unknown error"}`;
    $("diagnostic").hidden = false;
    markDiagnostic(result);
  }
  showValidation("input", result.input_validation, controls.command.value);
  showValidation("output", result.output_validation, controls.command.value);
  markValidation(sourceEditor, ["get", "query"].includes(controls.command.value)
    && result.output_validation.target === "yaml"
    ? result.output_validation : { status: "skipped" }, setReadValidationLine);
}

function scheduleRun() {
  updateFields();
  clearTimeout(debounce);
  debounce = setTimeout(run, 280);
}

function loadExample(index, reset = false) {
  if (loadedExample != null && !reset) {
    exampleSchemaDrafts.set(loadedExample, {
      input: text(inputSchemaEditor), output: text(outputSchemaEditor),
    });
  }
  if (reset) exampleSchemaDrafts.delete(index);
  activeExample = index;
  const example = examples[index];
  const schemas = exampleSchemaDrafts.get(index);
  replaceText(inputSchemaEditor, schemas?.input ?? example.inputSchema ?? "");
  replaceText(outputSchemaEditor, schemas?.output ?? example.outputSchema ?? "");
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
  loadedExample = index;
}

function legacyCopy(value) {
  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.readOnly = true;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  try {
    return document.execCommand("copy");
  } finally {
    textarea.remove();
  }
}

async function copy(value, button) {
  const previous = button.textContent;
  const copied = await copyText(value, {
    clipboard: navigator.clipboard,
    fallback: legacyCopy,
  });
  button.textContent = copied ? "Copied" : "Copy failed";
  $("clipboard-status").textContent = copied
    ? `${previous} copied to clipboard.`
    : `Unable to copy ${previous.toLowerCase()}.`;
  setTimeout(() => { button.textContent = previous; }, 1000);
}

async function start() {
  sourceEditor = editor($("source-editor"), baseSource, false, scheduleRun);
  resultEditor = editor($("result-editor"), baseSource, true);
  inputSchemaEditor = editor($("input-schema-editor"), "", false, scheduleRun);
  outputSchemaEditor = editor($("output-schema-editor"), "", false, scheduleRun);
  for (const side of ["input", "output"]) {
    for (const tab of ["yaml", "schema"]) {
      $(`${side}-${tab}-tab`).addEventListener("click", () => selectTab(side, tab));
    }
  }
  examples.forEach((example, index) => controls.example.add(new Option(example.name, String(index))));
  Object.values(controls).forEach((control) => control.addEventListener("input", scheduleRun));
  controls.example.addEventListener("change", () => loadExample(Number(controls.example.value)));
  $("run").addEventListener("click", run);
  $("reset").addEventListener("click", () => loadExample(activeExample, true));
  $("copy-result").addEventListener("click", () => copy(text(resultEditor), $("copy-result")));
  $("use-schema").addEventListener("click", () => {
    replaceText(inputSchemaEditor, text(resultEditor));
    selectTab("input", "schema");
    scheduleRun();
  });
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
