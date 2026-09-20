import test from "node:test";
import assert from "node:assert/strict";

import { copyText, lineDiff, resultPresentation, validationPresentation } from "./state.mjs";

test("schema validation labels distinguish missing, bad, and mismatched schemas", () => {
  assert.equal(validationPresentation({ status: "skipped" }).label, "No schema");
  assert.equal(validationPresentation({ status: "invalid", target: "schema", message: "bad schema" }).label, "Invalid schema");
  assert.deepEqual(validationPresentation({ status: "invalid", target: "yaml", document_index: 1, message: "wrong type" }), {
    label: "Schema mismatch",
    detail: "Document 1: wrong type",
  });
  assert.equal(validationPresentation({ status: "unavailable" }).label, "Not available");
});

test("read commands put command output in the right pane", () => {
  const presentation = resultPresentation(
    { ok: true, command_output: "/value: 1\n", output_yaml: "value: 1\n" },
    "query",
    "value: 1\n",
  );
  assert.equal(presentation.content, "/value: 1\n");
  assert.equal(presentation.title, "Command Output");
  assert.equal(presentation.highlightChanges, false);
  assert.equal(presentation.showMatchCount, true);
});

test("mutations put edited YAML in the right pane", () => {
  const presentation = resultPresentation(
    { ok: true, command_output: "", output_yaml: "value: 2\n" },
    "replace",
    "value: 1\n",
  );
  assert.equal(presentation.content, "value: 2\n");
  assert.equal(presentation.title, "Result YAML");
  assert.equal(presentation.highlightChanges, true);
});

test("validation shows the unchanged YAML stream on the output side", () => {
  assert.deepEqual(
    resultPresentation({ ok: true, output_yaml: "key: value\n" }, "validate", "key: value\n"),
    {
      content: "key: value\n",
      title: "Output YAML",
      highlightChanges: false,
      showMatchCount: false,
      showCopyResult: true,
    },
  );
  assert.deepEqual(
    resultPresentation({ ok: false, error_source: "document" }, "validate", "key: [\n"),
    {
      content: "",
      title: "Output YAML",
      highlightChanges: false,
      showMatchCount: false,
      showCopyResult: false,
    },
  );
});

test("schema generation shows a copyable JSON schema", () => {
  assert.deepEqual(resultPresentation({ ok: true, command_output: "{\"type\": \"object\"}\n" }, "schema", "name: api\n"), {
    content: "{\"type\": \"object\"}\n",
    title: "Generated JSON Schema",
    highlightChanges: false,
    showMatchCount: false,
    showCopyResult: true,
  });
  assert.deepEqual(resultPresentation({ ok: false }, "schema", "name: api\n"), {
    content: "",
    title: "Generated JSON Schema",
    highlightChanges: false,
    showMatchCount: false,
    showCopyResult: false,
  });
});

test("application failures show rollback while malformed inputs clear output", () => {
  const source = "value: 1\n";
  assert.equal(
    resultPresentation({ ok: false, error_source: "application" }, "patch", source).content,
    source,
  );
  assert.equal(
    resultPresentation({ ok: false, error_source: "patch" }, "patch", source).content,
    "",
  );
});

test("copy falls back when clipboard access is unavailable or rejected", async () => {
  let fallbackValue = "";
  const copied = await copyText("result", {
    clipboard: { writeText: async () => { throw new Error("insecure origin"); } },
    fallback(value) { fallbackValue = value; return true; },
  });
  assert.equal(copied, true);
  assert.equal(fallbackValue, "result");

  assert.equal(await copyText("result", { clipboard: undefined, fallback: () => false }), false);
});

test("line diff reports insertions and replacements as changed output lines", () => {
  assert.deepEqual(lineDiff("a\nb\nc", "a\nnew\nb\nc"), {
    changedLines: [2],
    deletions: [],
  });
  assert.deepEqual(lineDiff("a\nb\nc", "a\nnew\nc"), {
    changedLines: [2],
    deletions: [{ line: 2, removedLines: ["b"] }],
  });
});

test("line diff anchors deleted runs at the nearest output line", () => {
  assert.deepEqual(lineDiff("first\na\nb\nlast", "a\nb\nlast"), {
    changedLines: [],
    deletions: [{ line: 1, removedLines: ["first"] }],
  });
  assert.deepEqual(lineDiff("first\na\nb\nlast", "first\nlast"), {
    changedLines: [],
    deletions: [{ line: 2, removedLines: ["a", "b"] }],
  });
  assert.deepEqual(lineDiff("first\na\nb", "first"), {
    changedLines: [],
    deletions: [{ line: 1, removedLines: ["a", "b"] }],
  });
});

test("line diff handles unchanged and CRLF documents", () => {
  assert.deepEqual(lineDiff("a\r\nb\r\n", "a\r\nb\r\n"), { changedLines: [], deletions: [] });
  assert.deepEqual(lineDiff("a\r\nb\r\nc", "a\r\nc"), {
    changedLines: [],
    deletions: [{ line: 2, removedLines: ["b"] }],
  });
});
