import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

// CSS variables follow the selected theme, including changes while editors are open.
export const playgroundTheme = [
  EditorView.theme({
    "&": { color: "var(--ink)", backgroundColor: "var(--panel)" },
    ".cm-content": { caretColor: "var(--ink)" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--ink)" },
    ".cm-gutters": {
      color: "var(--muted)",
      backgroundColor: "var(--editor-gutter)",
      borderRightColor: "var(--line)",
    },
    ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--editor-active-line)" },
    ".cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-content ::selection": {
      backgroundColor: "var(--editor-selection)",
    },
    ".cm-selectionMatch": { backgroundColor: "var(--change-wash)" },
    "&.cm-focused .cm-matchingBracket": { backgroundColor: "var(--editor-selection)", color: "var(--ink)" },
    "&.cm-focused .cm-nonmatchingBracket": { backgroundColor: "var(--error-wash)", color: "var(--danger)" },
    ".cm-foldPlaceholder": {
      color: "var(--muted)",
      backgroundColor: "var(--editor-gutter)",
      borderColor: "var(--line)",
    },
    ".cm-panels, .cm-tooltip": {
      color: "var(--ink)",
      backgroundColor: "var(--panel)",
      borderColor: "var(--line)",
    },
    ".cm-searchMatch": { backgroundColor: "var(--accent-wash)", outline: "1px solid var(--accent)" },
    ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--editor-selection)" },
    ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
      color: "var(--ink)",
      backgroundColor: "var(--editor-selection)",
    },
    ".cm-button, .cm-textfield": {
      color: "var(--ink)",
      background: "var(--bg)",
      border: "1px solid var(--line)",
    },
  }),
  syntaxHighlighting(HighlightStyle.define([
    { tag: tags.comment, color: "var(--muted)" },
    { tag: [tags.propertyName, tags.attributeName], color: "var(--syntax-key)" },
    { tag: [tags.string, tags.content, tags.attributeValue], color: "var(--syntax-value)" },
    { tag: [tags.number, tags.bool, tags.null, tags.keyword], color: "var(--syntax-literal)" },
    { tag: [tags.labelName, tags.typeName, tags.meta], color: "var(--syntax-meta)" },
    { tag: tags.punctuation, color: "var(--ink)" },
  ])),
];
