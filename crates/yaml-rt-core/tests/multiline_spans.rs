mod common;

use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment};

fn pointer(path: &str) -> JsonPointer {
    JsonPointer::parse(path).unwrap()
}

fn scalar(doc: &YamlDoc, document: usize, path: &str) -> String {
    let node = doc.resolve_pointer(document, &pointer(path)).unwrap();
    doc.scalar_value(node).unwrap().into_owned()
}

#[test]
fn block_scalar_extraction_and_copy_preserve_content_and_chomping() {
    for header in ["|", "|-", "|+", ">", ">-", ">+", "|2+"] {
        for ending in ["\n", "\r\n"] {
            for final_break in ["", "\n", "\n\n"] {
                for suffix in ["", "tail: keep\n", "---\nother: document\n"] {
                    let body = format!(
                        "enabled: true\nmessage: {header}\n  First line\n  Second line\n  Third line"
                    );
                    let body = if final_break.is_empty() && !suffix.is_empty() {
                        format!("{body}\n")
                    } else {
                        format!("{body}{final_break}")
                    };
                    let input = format!(
                        "config:\n{}{}",
                        body.split_inclusive('\n')
                            .map(|line| format!("  {line}"))
                            .collect::<String>(),
                        suffix
                    )
                    .replace('\n', ending);
                    let expected = body.replace('\n', ending);
                    let mut doc = YamlDoc::parse(&input).unwrap();
                    assert_eq!(doc.to_string(), input);
                    common::check_span_ends(&doc).unwrap();
                    let value = scalar(&doc, 0, "/config/message");
                    let node = doc.resolve_pointer(0, &pointer("/config")).unwrap();
                    let extracted = doc.extract_node(node).unwrap();
                    assert_eq!(extracted, expected, "{input:?}");
                    let fragment = YamlDoc::parse(&extracted).unwrap();
                    assert_eq!(scalar(&fragment, 0, "/message"), value);
                    doc.copy_at(0, &pointer("/config"), &pointer("/backup"))
                        .unwrap();
                    let reparsed = YamlDoc::parse(doc.as_source()).unwrap();
                    assert_eq!(
                        scalar(&reparsed, 0, "/backup/message"),
                        value,
                        "{}",
                        doc.as_source()
                    );
                    assert_eq!(scalar(&reparsed, 0, "/config/message"), value);
                    if suffix.starts_with("tail") {
                        assert_eq!(scalar(&reparsed, 0, "/tail"), "keep");
                    }
                    if suffix.starts_with("---") {
                        assert_eq!(scalar(&reparsed, 1, "/other"), "document");
                    }
                }
            }
        }
    }
}

#[test]
fn copying_or_replacing_existing_collections_preserves_keep_chomping() {
    for ending in ["\n", "\r\n"] {
        let input =
            "config:\n  message: |+\n    first\n    second\n\nbackup:\n  old: true\nafter: keep\n"
                .replace('\n', ending);
        let original = YamlDoc::parse(&input).unwrap();
        let value = scalar(&original, 0, "/config/message");
        for operation in ["copy", "move", "replace"] {
            let mut doc = original.clone();
            match operation {
                "copy" => doc
                    .copy_at(0, &pointer("/config"), &pointer("/backup"))
                    .unwrap(),
                "move" => doc
                    .move_at(0, &pointer("/config"), &pointer("/backup"))
                    .unwrap(),
                _ => {
                    let node = doc.resolve_pointer(0, &pointer("/config")).unwrap();
                    let fragment = YamlFragment::parse(&doc.extract_node(node).unwrap()).unwrap();
                    doc.replace_at(0, &pointer("/backup"), &fragment).unwrap();
                }
            }
            assert_eq!(
                scalar(&doc, 0, "/backup/message"),
                value,
                "{operation}: {}",
                doc.as_source()
            );
            assert_eq!(scalar(&doc, 0, "/after"), "keep");
        }
    }
}

#[test]
fn terminal_block_scalar_anchors_still_enforce_copy_and_move_guards() {
    let input = "config:\n  message: &text |\n    first\n    second\nalias: *text\n";
    let mut doc = YamlDoc::parse(input).unwrap();
    assert!(
        doc.copy_at(0, &pointer("/config"), &pointer("/backup"))
            .is_err()
    );
    assert!(
        doc.move_at(0, &pointer("/config"), &pointer("/backup"))
            .is_err()
    );
    assert_eq!(doc.as_source(), input);
}

#[test]
fn multiline_children_extend_all_structural_ancestors() {
    let cases = [
        (
            "config:\n  nested:\n    message: |\n      first\n      second\n",
            "nested:\n  message: |\n    first\n    second\n",
        ),
        (
            "config:\n  - |\n    first\n    second\n",
            "- |\n  first\n  second\n",
        ),
        (
            "config:\n  - message: |\n      first\n      second\n",
            "- message: |\n    first\n    second\n",
        ),
        (
            "config:\n  message: first\n    second\n",
            "message: first\n  second",
        ),
        (
            "config:\n  message: 'first\n    second'\n",
            "message: 'first\n  second'",
        ),
        (
            "config:\n  message: \"first\n    second\"\n",
            "message: \"first\n  second\"",
        ),
        (
            "config:\n  message: [first,\n    {second: third}]\n",
            "message: [first,\n  {second: third}]",
        ),
        (
            "config:\n  ? message\n  : |\n    first\n    second\n",
            "? message\n: |\n  first\n  second\n",
        ),
        (
            "config:\n  message:\n    'first\n      second'\n",
            "message:\n  'first\n    second'",
        ),
        (
            "config:\n  message: |\n    first\n    second\n  after: keep # comment\n",
            "message: |\n  first\n  second\nafter: keep # comment",
        ),
        ("config:\n  - first\n    second\n", "- first\n  second"),
        ("config:\n  - 'first\n    second'\n", "- 'first\n  second'"),
        (
            "config:\n  - [first,\n    second]\n",
            "- [first,\n  second]",
        ),
        (
            "config:\n  ? |\n    first\n    second\n  : value\n",
            "? |\n  first\n  second\n: value",
        ),
        (
            "config:\n  message: !!str |+\n    first\n    second\n\n# outside\ntail: keep\n",
            "message: !!str |+\n  first\n  second\n\n",
        ),
        (
            "config:\n  - - |\n      first\n      second\n",
            "- - |\n    first\n    second\n",
        ),
    ];
    for (input, expected) in cases {
        let mut doc = YamlDoc::parse(input).unwrap();
        common::check_span_ends(&doc).unwrap();
        assert_eq!(doc.to_string(), input);
        let node = doc.resolve_pointer(0, &pointer("/config")).unwrap();
        assert_eq!(doc.extract_node(node).unwrap(), expected, "{input}");
        let expected_events = YamlDoc::parse(expected).unwrap().events_to_test_string();
        doc.copy_at(0, &pointer("/config"), &pointer("/backup"))
            .unwrap();
        let node = doc.resolve_pointer(0, &pointer("/backup")).unwrap();
        assert_eq!(
            YamlDoc::parse(&doc.extract_node(node).unwrap())
                .unwrap()
                .events_to_test_string(),
            expected_events
        );
    }
}

#[test]
fn multiline_collection_mutations_preserve_neighbors() {
    for ending in ["\n", "\r\n"] {
        for header in ["|", "|-", "|+", ">"] {
            let input = format!("before: keep\nconfig:\n  message: {header}\n    first\n    second\n\nafter: keep # comment\n---\nother: document\n").replace('\n', ending);
            let original = YamlDoc::parse(&input).unwrap();
            let value = scalar(&original, 0, "/config/message");
            for operation in ["move", "replace", "remove"] {
                let mut doc = original.clone();
                match operation {
                    "move" => doc
                        .move_at(0, &pointer("/config"), &pointer("/backup"))
                        .unwrap(),
                    "replace" => doc
                        .replace_at(
                            0,
                            &pointer("/config"),
                            &YamlFragment::parse("replacement: true").unwrap(),
                        )
                        .unwrap(),
                    _ => doc.remove_at(0, &pointer("/config")).unwrap(),
                }
                let output = doc.as_source();
                let parsed = YamlDoc::parse(output).unwrap();
                assert!(output.starts_with(&format!("before: keep{ending}")));
                assert!(
                    output.contains(&format!("after: keep # comment{ending}")),
                    "{operation}: {output:?}"
                );
                assert!(
                    output.ends_with(&format!("---{ending}other: document{ending}")),
                    "{operation}: {output:?}"
                );
                assert_eq!(scalar(&parsed, 1, "/other"), "document");
                if operation == "move" {
                    assert_eq!(scalar(&parsed, 0, "/backup/message"), value);
                } else {
                    assert!(!output.contains("first"));
                    assert!(!output.contains("second"));
                }
            }
        }
    }
}
