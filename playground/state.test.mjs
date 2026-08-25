import test from "node:test";
import assert from "node:assert/strict";

import { copyText, lineDiff, resultPresentation } from "./state.mjs";

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
