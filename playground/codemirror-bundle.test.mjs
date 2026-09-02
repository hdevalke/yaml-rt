import assert from "node:assert/strict";
import test from "node:test";

const requiredExports = [
  "basicSetup",
  "Decoration",
  "EditorState",
  "EditorView",
  "GutterMarker",
  "RangeSet",
  "StateEffect",
  "StateField",
  "gutter",
  "lineNumbers",
  "yaml",
];

test("the browser bundle exposes every CodeMirror API used by the playground", async () => {
  const codeMirror = await import("./generated/codemirror.js");

  for (const name of requiredExports) {
    assert.ok(name in codeMirror, `missing ${name} export`);
  }

  const field = codeMirror.StateField.define({
    create: () => codeMirror.RangeSet.empty,
    update: (value) => value,
    provide: (stateField) => codeMirror.EditorView.decorations.from(stateField),
  });
  assert.doesNotThrow(() => codeMirror.EditorState.create({
    doc: "key: value\n",
    extensions: [
      codeMirror.basicSetup,
      codeMirror.lineNumbers(),
      codeMirror.yaml(),
      codeMirror.EditorView.lineWrapping,
      field,
    ],
  }));
});
