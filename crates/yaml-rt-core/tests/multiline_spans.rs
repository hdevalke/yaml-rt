mod common;

use yaml_rt_core::{JsonPointer, MappingEntryStyle, Span, YamlDoc, YamlFragment};

fn pointer(path: &str) -> JsonPointer {
    JsonPointer::parse(path).unwrap()
}

fn scalar(doc: &YamlDoc, document: usize, path: &str) -> String {
    let node = doc.resolve_pointer(document, &pointer(path)).unwrap();
    doc.scalar_value(node).unwrap().into_owned()
}

#[test]
fn block_scalar_values_normalize_physical_line_breaks() {
    let input = "literal: |+\n  first\n  second\n\nfolded: >+\n\n  first\n  second\n\nquoted: \"first\\r\\nsecond\"\n";
    for ending in ["\n", "\r\n", "\r"] {
        let input = input.replace('\n', ending);
        let doc = YamlDoc::parse(&input).unwrap();
        assert_eq!(scalar(&doc, 0, "/literal"), "first\nsecond\n\n");
        assert_eq!(scalar(&doc, 0, "/folded"), "\nfirst second\n\n");
        assert_eq!(scalar(&doc, 0, "/quoted"), "first\r\nsecond");
        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn multiline_replacement_preserves_indentation_and_line_endings() {
    for ending in ["\n", "\r\n", "\r"] {
        let input = "root:\n  target: old\n  sibling: kept\n".replace('\n', ending);
        let replacement = "|\n  first\n  second\n".replace('\n', ending);
        let expected =
            "root:\n  target: |\n     first\n     second\n  sibling: kept\n".replace('\n', ending);
        let mut doc = YamlDoc::parse(&input).unwrap();
        doc.replace_at(
            0,
            &pointer("/root/target"),
            &YamlFragment::parse(&replacement).unwrap(),
        )
        .unwrap();
        assert_eq!(doc.as_source(), expected);
        assert_eq!(scalar(&doc, 0, "/root/target"), "first\nsecond\n");
        assert_eq!(scalar(&doc, 0, "/root/sibling"), "kept");
    }
}

#[test]
fn inserted_comment_lines_each_receive_a_comment_indicator() {
    let mut doc = YamlDoc::parse("existing: kept\r").unwrap();
    let root = doc.root_mapping().unwrap();
    doc.insert_mapping_entry_with_comment(
        root,
        "new",
        "value",
        MappingEntryStyle::Inherit,
        Some("first\rnext: still a comment\r\nlast"),
    )
    .unwrap();
    doc.commit_edits().unwrap();
    assert_eq!(
        doc.as_source(),
        "existing: kept\r# first\r# next: still a comment\r# last\rnew: value\r"
    );
    assert_eq!(doc.mapping_entries(doc.root_mapping().unwrap()).count(), 2);
}

#[test]
fn block_scalar_syntax_ends_before_unowned_trivia() {
    for (input, path, syntax, value) in [
        ("a: one  # note\n", "/a", "one", "one"),
        (
            "a: one\r\n  café#part \t# note\r\nb: three\r\n",
            "/a",
            "one\r\n  café#part",
            "one café#part",
        ),
        (
            "a: &ref one's value  # note\nb: *ref\n",
            "/a",
            "&ref one's value",
            "one's value",
        ),
        ("- ' a # b '  # note\n", "/0", "' a # b '", " a # b "),
        (
            "a: &ref \"a # b\"  # note\n",
            "/a",
            "&ref \"a # b\"",
            "a # b",
        ),
        ("? key\n: child: one  # note\n", "/key/child", "one", "one"),
    ] {
        let mut doc = YamlDoc::parse(input).unwrap();
        let node = doc.resolve_pointer(0, &pointer(path)).unwrap();
        let span = doc.node(node).unwrap().span();
        assert_eq!(doc.source().slice(span), syntax, "{input:?}");
        assert_eq!(doc.scalar_value(node).unwrap(), value, "{input:?}");
        assert_eq!(doc.to_string(), input);
        common::check_span_ends(&doc).unwrap();
        if !syntax.starts_with('&') {
            doc.replace_node_text(node, "new").unwrap();
            let mut expected = input.to_owned();
            expected.replace_range(span.start as usize..span.end as usize, "new");
            assert_eq!(doc.to_string(), expected);
            assert_eq!(scalar(&YamlDoc::parse(&expected).unwrap(), 0, path), "new");
        }
    }
}

#[test]
fn block_scalar_edits_preserve_the_comment_after_the_value() {
    let input = "a: one\n  two  # note\nb: three\n";
    let mut scalar_edit = YamlDoc::parse(input).unwrap();
    scalar_edit.set_scalar(&["a"], "new").unwrap();
    assert_eq!(scalar_edit.to_string(), "a: new  # note\nb: three\n");

    let mut whole_edit = YamlDoc::parse(input).unwrap();
    let replacement = YamlFragment::parse("[new]").unwrap();
    whole_edit
        .replace_at(0, &pointer("/a"), &replacement)
        .unwrap();
    assert_eq!(whole_edit.to_string(), "a: [new]  # note\nb: three\n");
    assert_eq!(scalar(&whole_edit, 0, "/a/0"), "new");
    assert_eq!(scalar(&whole_edit, 0, "/b"), "three");
}

#[test]
fn block_quoted_scalar_spans_require_valid_trailing_syntax() {
    for (input, suffix) in [
        ("- 'one' 'two'\n", "'two'"),
        ("a: &ref \"one\" é\n", "é"),
        ("\"one\"suffix\n", "suffix"),
    ] {
        let error = YamlDoc::parse(input).unwrap_err();
        let start = input.find(suffix).unwrap();
        assert_eq!(error.diagnostic.span.start as usize, start);
        assert_eq!(
            error.diagnostic.span.end as usize,
            start + suffix.chars().next().unwrap().len_utf8()
        );
    }
}

#[test]
fn flow_entry_spans_cover_syntax_without_trailing_trivia() {
    let input = "items: [v  # first\r\n  , k: 'x'  # pair\r\n  ] # tail\r\nmap: {a:  # empty\r\n  , b: &ref  # property only\r\n  , c  # omitted\r\n  }\r\n";
    let doc = YamlDoc::parse(input).unwrap();
    let slice = |node| doc.source().slice(doc.node(node).unwrap().span());
    let sequence = doc.resolve_pointer(0, &pointer("/items")).unwrap();
    let sequence_entries = doc.children(sequence).collect::<Vec<_>>();
    assert_eq!(slice(sequence_entries[0]), "v");
    assert_eq!(slice(sequence_entries[1]), "k: 'x'");
    let pair = doc.resolve_pointer(0, &pointer("/items/1")).unwrap();
    assert_eq!(slice(pair), "k: 'x'");
    assert_eq!(doc.extract_node(pair).unwrap(), "{k: 'x'}");
    assert_eq!(slice(doc.children(pair).next().unwrap()), "k: 'x'");
    assert!(slice(sequence).ends_with("\r\n  ]"));

    let mapping = doc.resolve_pointer(0, &pointer("/map")).unwrap();
    let mapping_entries = doc.children(mapping).collect::<Vec<_>>();
    assert_eq!(slice(mapping_entries[0]), "a:");
    assert_eq!(slice(mapping_entries[1]), "b: &ref");
    assert_eq!(slice(mapping_entries[2]), "c");
    let empty = doc.resolve_pointer(0, &pointer("/map/a")).unwrap();
    let after_colon = input.find("a:").unwrap() + 2;
    assert_eq!(
        doc.node(empty).unwrap().span(),
        Span::empty_from_usize(after_colon)
    );
    let decorated = doc.resolve_pointer(0, &pointer("/map/b")).unwrap();
    assert_eq!(slice(decorated), "&ref");
    assert_eq!(doc.anchor(decorated), Some("ref"));
    assert_eq!(doc.scalar_value(decorated).unwrap(), "");
    common::check_span_ends(&doc).unwrap();
    assert_eq!(doc.to_string(), input);

    let mut replaced = doc.clone();
    replaced
        .replace_at(
            0,
            &pointer("/items/1/k"),
            &YamlFragment::parse("new").unwrap(),
        )
        .unwrap();
    assert_eq!(replaced.to_string(), input.replace("'x'", "'new'"));

    let mut copied = doc;
    copied
        .copy_at(0, &pointer("/items/1"), &pointer("/copy"))
        .unwrap();
    assert_eq!(scalar(&copied, 0, "/copy/k"), "x");
    assert!(copied.as_source().starts_with(input));
}

#[test]
fn empty_explicit_flow_entries_own_their_indicators() {
    for input in ["{?  # note\n}", "[?  # note\n]"] {
        let doc = YamlDoc::parse(input).unwrap();
        let root = doc.document_root(0).unwrap().unwrap();
        let root_entry = doc.children(root).next().unwrap();
        let mapping_entry = if input.starts_with('[') {
            let mapping = doc.children(root_entry).next().unwrap();
            doc.children(mapping).next().unwrap()
        } else {
            root_entry
        };
        assert_eq!(doc.node(root_entry).unwrap().span(), Span::new(1, 2));
        assert_eq!(doc.node(mapping_entry).unwrap().span(), Span::new(1, 2));
        for child in doc.children(mapping_entry) {
            assert_eq!(doc.node(child).unwrap().span(), Span::empty(2));
        }
        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn flow_edits_find_separators_outside_comments_and_reuse_trailing_commas() {
    let mut doc =
        YamlDoc::parse("items: [\n  a # keep, literally\n  ,\n  b\n]\ntail: kept\n").unwrap();
    doc.remove_at(0, &pointer("/items/1")).unwrap();
    assert_eq!(
        doc.to_string(),
        "items: [\n  a # keep, literally\n  \n]\ntail: kept\n"
    );
    assert_eq!(scalar(&doc, 0, "/items/0"), "a");
    assert_eq!(scalar(&doc, 0, "/tail"), "kept");

    let mut doc = YamlDoc::parse("items: [\n  a # kept\n  , b\n]\n").unwrap();
    doc.remove_at(0, &pointer("/items/1")).unwrap();
    assert_eq!(doc.to_string(), "items: [\n  a # kept\n]\n");

    let mut doc = YamlDoc::parse("map: {\n  a: 1, # kept\n}\n").unwrap();
    doc.add_at(0, &pointer("/map/b"), &YamlFragment::parse("2").unwrap())
        .unwrap();
    assert_eq!(doc.to_string(), "map: {\n  a: 1, # kept\n  b: 2\n}\n");
    assert_eq!(scalar(&doc, 0, "/map/b"), "2");
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

#[test]
fn block_scalar_replacement_keeps_the_separator_and_header_comment() {
    for ending in ["\n", "\r\n", "\r"] {
        for head in ["|", ">-", "|+"] {
            let input =
                format!("root:\n  a: {head} # keep\n    old\n  b: next\n").replace('\n', ending);
            let mut doc = YamlDoc::parse(&input).unwrap();
            doc.replace_at(0, &pointer("/root/a"), &YamlFragment::parse("ZZ").unwrap())
                .unwrap();
            assert_eq!(
                doc.as_source(),
                "root:\n  a: ZZ # keep\n  b: next\n".replace('\n', ending)
            );
            assert_eq!(scalar(&doc, 0, "/root/a"), "ZZ");
            assert_eq!(scalar(&doc, 0, "/root/b"), "next");
        }
    }
}

#[test]
fn block_scalar_replacement_preserves_the_incoming_fragment_and_neighboring_trivia() {
    for (fragment, expected) in [
        ("a: one", "value:\n  a: one\n# original note\ntail: kept\n"),
        (
            "|- # incoming note\n  new",
            "value: |- # incoming note\n  new\n# original note\ntail: kept\n",
        ),
    ] {
        let mut doc = YamlDoc::parse("value: | # original note\n  old\ntail: kept\n").unwrap();
        doc.replace_at(
            0,
            &pointer("/value"),
            &YamlFragment::parse(fragment).unwrap(),
        )
        .unwrap();
        assert_eq!(doc.as_source(), expected);
        assert_eq!(scalar(&doc, 0, "/tail"), "kept");
    }
    let mut doc = YamlDoc::parse("value: |\n  old\n# between\ntail: kept\n").unwrap();
    let fragment = YamlFragment::parse("|+\n  new\n\n").unwrap();
    doc.replace_at(0, &pointer("/value"), &fragment).unwrap();
    assert_eq!(scalar(&doc, 0, "/value"), "new\n\n");
    assert_eq!(scalar(&doc, 0, "/tail"), "kept");
    assert_eq!(
        doc.as_source(),
        "value: |+\n  new\n\n# between\ntail: kept\n"
    );
}

#[test]
fn typed_block_scalar_write_reparses_to_the_exact_requested_string() {
    use yaml_rt_core::YamlValue;
    for ending in ["\n", "\r\n", "\r"] {
        for head in ["|", "|-", "|+", ">", ">-", ">+", ">2-"] {
            for requested in [
                "",
                "new",
                "new\n",
                "new\n\n",
                "one\ntwo",
                "one\n\ntwo",
                "\nlead\n",
                "one\n  indent\ntwo\n",
                "  preserve",
                "a\nb\n \nc",
                "\n\n",
                "a\rb",
            ] {
                for suffix in ["\nb: next\n", ""] {
                    let input = format!("a: {head} # keep\n  old{suffix}").replace('\n', ending);
                    let mut doc = YamlDoc::parse(&input).unwrap();
                    let a = doc.resolve_pointer(0, &pointer("/a")).unwrap();
                    requested.to_owned().write_yaml(&mut doc, Some(a)).unwrap();
                    let output = doc.to_string();
                    let doc = YamlDoc::parse(&output)
                        .unwrap_or_else(|error| panic!("{output:?}: {error}"));
                    assert_eq!(scalar(&doc, 0, "/a"), requested, "{output:?}");
                    assert!(output.contains(" # keep"), "{output:?}");
                    if !suffix.is_empty() {
                        assert_eq!(scalar(&doc, 0, "/b"), "next", "{output:?}");
                        assert!(
                            output.ends_with(&suffix.replace('\n', ending)),
                            "{output:?}"
                        );
                    }
                }
            }
        }
    }
}

fn alias_value(doc: &YamlDoc, path: &str) -> String {
    let alias = doc.resolve_pointer(0, &pointer(path)).unwrap();
    let target = doc.resolve_alias(alias).expect("alias must still resolve");
    doc.scalar_value(target).unwrap().into_owned()
}

#[test]
fn replacements_keep_external_alias_bindings_and_reject_removing_live_nested_anchors() {
    let input = "first: &id [one]\nref1: *id\nsecond: &id {old: two}\nref2: *id\n";
    let mut doc = YamlDoc::parse(input).unwrap();
    doc.replace_at(0, &pointer("/second"), &YamlFragment::parse("ZZ").unwrap())
        .unwrap();
    assert_eq!(
        doc.as_source(),
        "first: &id [one]\nref1: *id\nsecond: &id ZZ\nref2: *id\n"
    );
    assert_eq!(scalar(&doc, 0, "/ref1/0"), "one");
    assert_eq!(alias_value(&doc, "/ref2"), "ZZ");

    let failed = YamlDoc::parse("a: &outer [&inner old]\nb: *inner\nc: *outer\n").unwrap();
    for (source, replacement) in [
        (failed.as_source(), "&outer ZZ"),
        ("first: &id old\nsecond: &id [old]\nref: *id\n", "&other ZZ"),
    ] {
        let mut doc = YamlDoc::parse(source).unwrap();
        let path = if source.starts_with("a:") {
            "/a"
        } else {
            "/second"
        };
        let error = doc
            .replace_at(
                0,
                &pointer(path),
                &YamlFragment::parse(replacement).unwrap(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("anchor used by an alias"));
        assert_eq!(doc.as_source(), source);
    }
}

#[test]
fn fragment_anchor_renames_follow_alias_bindings() {
    let mut doc = YamlDoc::parse("a: &id [old]\nb: *id\n").unwrap();
    let fragment = YamlFragment::parse("&id [*id, &id leaf, *id]").unwrap();
    doc.replace_at(0, &pointer("/a"), &fragment).unwrap();
    assert_eq!(doc.as_source(), "a: &id [*id, &id_1 leaf, *id_1]\nb: *id\n");
    assert_eq!(scalar(&doc, 0, "/b/1"), "leaf");
    assert_eq!(alias_value(&doc, "/b/2"), "leaf");
    assert!(doc.resolve_pointer(0, &pointer("/a/0/1")).is_ok());
}

#[test]
fn replacement_tags_follow_scalar_preservation_and_whole_fragment_syntax() {
    let mut doc = YamlDoc::parse("a: !!str old\nb: !Old &id [old]\nc: *id\n").unwrap();
    doc.replace_at(0, &pointer("/a"), &YamlFragment::parse("new").unwrap())
        .unwrap();
    doc.replace_at(0, &pointer("/b"), &YamlFragment::parse("!New new").unwrap())
        .unwrap();
    let a = doc.resolve_pointer(0, &pointer("/a")).unwrap();
    let b = doc.resolve_pointer(0, &pointer("/b")).unwrap();
    assert_eq!(doc.raw_tag(a), Some("!!str"));
    assert_eq!(doc.raw_tag(b), Some("!New"));
    assert_eq!(doc.anchor(b), Some("id"));
    assert_eq!(alias_value(&doc, "/c"), "new");
}

#[test]
fn inline_block_scalar_consumes_deferred_leading_blank_lines() {
    for ending in ["\n", "\r\n", "\r"] {
        for (header, body) in [("|", "one\ntwo"), (">", "one two")] {
            let source =
                format!("a: {header} # note\n\n\n  one\n  two\nb: next\n").replace('\n', ending);
            let doc = YamlDoc::parse(&source).unwrap();
            assert_eq!(scalar(&doc, 0, "/a"), format!("\n\n{body}\n"));
            assert_eq!(scalar(&doc, 0, "/b"), "next");
            assert_eq!(doc.to_string(), source);
            common::check_span_ends(&doc).unwrap();
        }
        let over_indented = "a: |\n   \n  one\nb: next\n".replace('\n', ending);
        let error = YamlDoc::parse(&over_indented).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid block scalar content indentation")
        );
    }
}
