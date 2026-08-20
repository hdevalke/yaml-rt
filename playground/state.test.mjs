import test from "node:test";
import assert from "node:assert/strict";

import { resultPresentation } from "./state.mjs";

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
