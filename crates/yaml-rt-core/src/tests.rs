use super::*;

#[test]
fn bootstrap_parser_preserves_source() {
    let source = "---\nkey: value\n# comment\n";
    let doc = YamlDoc::parse(source).expect("placeholder parser should accept text");

    assert_eq!(doc.as_source(), source);
    assert_eq!(doc.to_string(), source);
}

#[test]
fn target_version_is_yaml_1_2_2() {
    assert_eq!(TARGET_YAML_VERSION, "1.2.2");
}

#[test]
fn source_tracks_line_columns() {
    let source = Source::new("a\nbc\n".to_owned()).expect("valid YAML characters");

    assert_eq!(source.line_col(0), LineCol { line: 1, column: 1 });
    assert_eq!(source.line_col(2), LineCol { line: 2, column: 1 });
    assert_eq!(source.slice(Span::new(2, 4)), "bc");
    assert_eq!(source.line_starts(), &[0, 2, 5]);
}

#[test]
fn source_rejects_invalid_yaml_characters() {
    let error = Source::new("valid\0invalid".to_owned()).expect_err("NUL is not YAML text");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Source);
    assert_eq!(error.diagnostic.span, Span::new(5, 6));
    assert!(error.to_string().contains("U+0000"));
}

#[test]
fn try_slice_reports_invalid_spans() {
    let source = Source::new("é".to_owned()).expect("valid YAML characters");
    let error = source
        .try_slice(Span::new(0, 1))
        .expect_err("span splits UTF-8 code point");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Source);
    assert_eq!(
        source.diagnostic_position(&error.diagnostic),
        LineCol { line: 1, column: 1 }
    );
}

#[test]
fn lexer_preserves_mvp_yaml_source() {
    let input = "# comment
---
key: value
list:
  - item
quoted: \"hello\"
single: 'hello'
...
";
    let doc = YamlDoc::parse(input).expect("lexer MVP should accept fixture");

    let tokens = doc.tokens().expect("document tokenization succeeds");
    assert_eq!(tokens_to_string(&tokens, &doc.source), input);
    assert_eq!(doc.to_string(), input);
    assert_eq!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Comment)
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::DocumentStart)
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::DocumentEnd)
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::DoubleQuotedScalar)
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::SingleQuotedScalar)
    );
}

#[test]
fn lexer_emits_flow_marker_tokens() {
    let source = Source::new(
        "flow: [a, {b: c}]
"
        .to_owned(),
    )
    .expect("valid YAML characters");
    let tokens = lex(&source).expect("lexer MVP should accept flow markers");
    let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();

    assert_eq!(tokens_to_string(&tokens, &source), source.as_str());
    assert!(kinds.contains(&TokenKind::FlowSequenceStart));
    assert!(kinds.contains(&TokenKind::FlowSequenceEnd));
    assert!(kinds.contains(&TokenKind::FlowMappingStart));
    assert!(kinds.contains(&TokenKind::FlowMappingEnd));
    assert!(kinds.contains(&TokenKind::Comma));
}

#[test]
fn lexer_reports_unterminated_quoted_scalars() {
    let error = YamlDoc::parse("quoted: \"oops").expect_err("quote is unterminated");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Lexer);
    assert!(
        error
            .to_string()
            .contains("unterminated double-quoted scalar")
    );
    assert_eq!(error.diagnostic.expected, ["closing \"".to_owned()]);
}

#[test]
fn lexer_keeps_embedded_quotes_inside_plain_scalars() {
    let input = "a!\"#$%&'()*+,-./09:;<=>?@AZ[\\]^_`az{|}~: safe\n?foo: safe question mark\n:foo: safe colon\n-foo: safe dash\nthis is#not: a comment\n";
    let doc = YamlDoc::parse(input).expect("embedded quotes are plain mapping key content");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :a!\"#$%&'()*+,-./09:;<=>?@AZ[\\\\]^_`az{|}~\n=VAL :safe\n=VAL :?foo\n=VAL :safe question mark\n=VAL ::foo\n=VAL :safe colon\n=VAL :-foo\n=VAL :safe dash\n=VAL :this is#not\n=VAL :a comment\n-MAP\n-DOC\n-STR\n"
    );

    let input = "- bla\"keks: foo\n- bla]keks: foo\n";
    let doc = YamlDoc::parse(input).expect("embedded quotes are plain sequence key content");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :bla\"keks\n=VAL :foo\n-MAP\n+MAP\n=VAL :bla]keks\n=VAL :foo\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_root_scalar() {
    let doc = YamlDoc::parse("value\n").expect("valid scalar");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n=VAL :value\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_explicit_document_block_mapping() {
    let doc = YamlDoc::parse("---\nhost: localhost\n").expect("valid mapping");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n+MAP\n=VAL :host\n=VAL :localhost\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_nested_block_sequence() {
    let doc = YamlDoc::parse("ports:\n  - 8080\n  - 9090\n").expect("valid sequence");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :ports\n+SEQ\n=VAL :8080\n=VAL :9090\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_flow_collections() {
    let doc = YamlDoc::parse("settings: {a: [b, c]}\n").expect("valid flow collections");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :settings\n+MAP {}\n=VAL :a\n+SEQ []\n=VAL :b\n=VAL :c\n-SEQ\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_scalar_styles_and_decoded_values() {
    let doc = YamlDoc::parse(
            "plain: value\nsingle: 'Bob''s'\ndouble: \"line\\nnext\"\nliteral: |\n  one\n  two\nfolded: >\n  one\n  two\n",
        )
        .expect("valid scalars");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :plain\n=VAL :value\n=VAL :single\n=VAL 'Bob's\n=VAL :double\n=VAL \"line\\nnext\n=VAL :literal\n=VAL |one\\ntwo\\n\n=VAL :folded\n=VAL >one two\\n\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_decode_double_quoted_hex_escapes() {
    let doc = YamlDoc::parse(
            "unicode: \"Sosa did fine.\\u263A\"\nhex esc: \"\\x0d\\x0a is \\r\\n\"\nwide: \"\\U0001F600\"\n",
        )
        .expect("valid double-quoted hex escapes");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :unicode\n=VAL \"Sosa did fine.☺\n=VAL :hex esc\n=VAL \"\\r\\n is \\r\\n\n=VAL :wide\n=VAL \"😀\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(
        doc.to_string(),
        "unicode: \"Sosa did fine.\\u263A\"\nhex esc: \"\\x0d\\x0a is \\r\\n\"\nwide: \"\\U0001F600\"\n"
    );
}

#[test]
fn events_decode_double_quoted_escaped_line_continuation() {
    let input = concat!("quoted: \"folded \\", "\n  non-content\"\n");
    let doc = YamlDoc::parse(input).expect("valid escaped line continuation");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :quoted\n=VAL \"folded non-content\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), input);
}

#[test]
fn events_decode_double_quoted_escaped_tab_continuation() {
    for (input, expected) in [
        ("\"3 trailing\\\t\n    tab\"\n", "3 trailing\\t tab"),
        ("\"4 trailing\\\t  \n    tab\"\n", "4 trailing\\t tab"),
    ] {
        let doc = YamlDoc::parse(input).expect("valid escaped tab continuation");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            format!("+STR\n+DOC\n=VAL \"{expected}\n-DOC\n-STR\n")
        );
    }
}

#[test]
fn double_quoted_hex_escape_errors_are_typed() {
    for input in ["\"\\u12\"", "\"\\xZZ\"", "\"\\U00110000\""] {
        let error = YamlDoc::parse(input).expect_err("invalid escape should fail");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Typed);
        assert!(
            error.diagnostic.message.contains("double-quoted"),
            "{input:?} should report a double-quoted escape error"
        );
    }
}

#[test]
fn parser_rejects_invalid_double_quoted_scalars() {
    for input in [
        "---\n\"\\.\"\n",
        "---\ndouble: \"quoted \\' scalar\"\n",
        "---\n\"\n---\n\"\n",
        "--- \"a\n... x\nb\"\n",
        "foo: \"bar\n\tbaz\"\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("invalid double-quoted scalar should fail");

        assert!(
            matches!(
                error.diagnostic.kind,
                DiagnosticKind::Parser | DiagnosticKind::Typed
            ),
            "{input:?} should report parser or typed validation"
        );
        assert!(
            error.diagnostic.position.is_some(),
            "{input:?} should include source position"
        );
    }
}

#[test]
fn events_fold_multiline_double_quoted_scalar() {
    let doc = YamlDoc::parse("quoted: \"So does this\n  quoted scalar.\\n\"\n")
        .expect("valid multiline double-quoted scalar");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :quoted\n=VAL \"So does this quoted scalar.\\n\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(
        doc.to_string(),
        "quoted: \"So does this\n  quoted scalar.\\n\"\n"
    );
}

#[test]
fn events_fold_multiline_single_quoted_blank_values() {
    let doc = YamlDoc::parse("a: '\n  '\ne: '\n\n  '\ng: '\n\n\n  '\n")
        .expect("valid multiline single-quoted blanks");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :a\n=VAL ' \n=VAL :e\n=VAL '\\n\n=VAL :g\n=VAL '\\n\\n\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), "a: '\n  '\ne: '\n\n  '\ng: '\n\n\n  '\n");
}

#[test]
fn events_fold_multiline_quoted_flow_sequence_values() {
    let doc = YamlDoc::parse("[\"double\n quoted\", 'single\n quoted']\n")
        .expect("valid multiline quoted flow scalars");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n=VAL \"double quoted\n=VAL 'single quoted\n-SEQ\n-DOC\n-STR\n"
    );
    assert_eq!(
        doc.to_string(),
        "[\"double\n quoted\", 'single\n quoted']\n"
    );
}

#[test]
fn events_render_implicit_flow_mapping_sequence_entry() {
    let doc = YamlDoc::parse("[foo: bar]\n").expect("valid implicit flow mapping item");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n+MAP {}\n=VAL :foo\n=VAL :bar\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), "[foo: bar]\n");
}

#[test]
fn events_render_yaml_test_8udb_flow_sequence_shape() {
    let doc = YamlDoc::parse(
            "[\n\"double\n quoted\", 'single\n           quoted',\nplain\n text, [ nested ],\nsingle: pair,\n]\n",
        )
        .expect("valid flow sequence with implicit mapping");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n=VAL \"double quoted\n=VAL 'single quoted\n=VAL :plain text\n+SEQ []\n=VAL :nested\n-SEQ\n+MAP {}\n=VAL :single\n=VAL :pair\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_explicit_block_mapping_key_value_pair() {
    let doc = YamlDoc::parse("? key\n: value\n").expect("valid explicit mapping key");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), "? key\n: value\n");
}

#[test]
fn events_render_explicit_key_with_comment_before_value() {
    let doc = YamlDoc::parse("? key\n# comment\n: value\n").expect("valid explicit key comment");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), "? key\n# comment\n: value\n");
}

#[test]
fn events_fold_explicit_plain_scalar_key_continuations() {
    let input = "? a\n  true\n: null\n  d\n? e\n  42\n";
    let doc = YamlDoc::parse(input).expect("valid explicit key continuations");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :a true\n=VAL :null d\n=VAL :e 42\n=VAL :\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_pair_continued_explicit_plain_key_with_value() {
    let input = "? key\n  continued\n: value\n";
    let doc = YamlDoc::parse(input).expect("valid continued explicit key with value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key continued\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_explicit_set_keys_as_empty_values() {
    let doc = YamlDoc::parse("--- !!set\n? Mark McGwire\n? Sammy Sosa\n")
        .expect("valid explicit set keys");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n+MAP <tag:yaml.org,2002:set>\n=VAL :Mark McGwire\n=VAL :\n=VAL :Sammy Sosa\n=VAL :\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_explicit_sequence_key() {
    let doc = YamlDoc::parse("complex:\n  ? - a\n  : b\n").expect("valid explicit sequence key");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :complex\n+MAP\n+SEQ\n=VAL :a\n-SEQ\n=VAL :b\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), "complex:\n  ? - a\n  : b\n");
}

#[test]
fn events_render_explicit_folded_scalar_key_with_empty_value() {
    let doc = YamlDoc::parse("complex:\n  ? >\n    a\n  :\n").expect("valid explicit scalar key");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :complex\n+MAP\n=VAL >a\\n\n=VAL :\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), "complex:\n  ? >\n    a\n  :\n");
}

#[test]
fn parser_events_render_explicit_following_sequence_key() {
    let doc = YamlDoc::parse("---\n?\n- a\n- b\n:\n- c\n- d\n").expect("valid parser");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n+MAP\n+SEQ\n=VAL :a\n=VAL :b\n-SEQ\n+SEQ\n=VAL :c\n=VAL :d\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_scalar_anchors_and_tags() {
    let doc =
        YamlDoc::parse("plain: &anchor !<tag:example.com,2026:x> value\nquoted: !!str \"123\"\n")
            .expect("valid scalar node properties");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :plain\n=VAL &anchor <tag:example.com,2026:x> :value\n=VAL :quoted\n=VAL <tag:yaml.org,2002:str> \"123\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_bare_non_specific_tags() {
    for (input, expected) in [
        ("! a\n", "+STR\n+DOC\n=VAL <!> :a\n-DOC\n-STR\n"),
        (
            "- ! 12\n",
            "+STR\n+DOC\n+SEQ\n=VAL <!> :12\n-SEQ\n-DOC\n-STR\n",
        ),
        ("!\n", "+STR\n+DOC\n=VAL <!> :\n-DOC\n-STR\n"),
    ] {
        let doc = YamlDoc::parse(input).expect("valid bare non-specific tag");

        assert_eq!(doc.events_to_test_string(), expected);
        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn events_render_plain_alias_before_inline_comment() {
    let doc = YamlDoc::parse("rbi:\n  - *SS # Subsequent occurrence\n")
        .expect("valid alias sequence entry");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :rbi\n+SEQ\n=ALI *SS\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn events_only_classify_plain_scalars_as_aliases() {
    let input = "plain: *alias\nsingle: '*alias'\ndouble: \"*alias\"\nliteral: |\n  *alias\nfolded: >\n  *alias\n";
    let doc = YamlDoc::parse(input).expect("valid scalar styles beginning with an asterisk");

    let events = doc.events().collect::<Vec<_>>();
    let aliases = events
        .iter()
        .filter(|event| matches!(event.kind, YamlEventKind::Alias { .. }))
        .count();
    let scalar_values: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            YamlEventKind::Scalar { value, .. } if value.starts_with('*') => Some(value.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(aliases, 1);
    assert_eq!(scalar_values, ["*alias", "*alias", "*alias\n", "*alias\n"]);
}

#[test]
fn semantics_preserve_scalar_anchors_and_tags() {
    let doc =
        YamlDoc::parse("plain: &anchor !local value\n").expect("valid scalar node properties");
    let value = doc
        .get_path(&["plain"])
        .expect("path succeeds")
        .expect("value exists");

    assert_eq!(
        doc.semantic_kind(value),
        Some(SemanticKind::Scalar {
            style: YamlScalarStyle::Plain
        })
    );
    assert_eq!(doc.raw_tag(value), Some("!local"));
    assert_eq!(
        doc.resolved_tag(value).expect("tag resolves").as_deref(),
        Some("!local")
    );
    assert_eq!(doc.anchor(value), Some("anchor"));
}

#[test]
fn alias_resolution_uses_the_latest_document_local_anchor() {
    let doc = YamlDoc::parse(
        "first: &value one\nfirst_alias: *value\nsecond: &value two\nsecond_alias: *value\n",
    )
    .expect("valid shadowed anchors");
    let first = doc.get_path(&["first"]).unwrap().unwrap();
    let first_alias = doc.get_path(&["first_alias"]).unwrap().unwrap();
    let second = doc.get_path(&["second"]).unwrap().unwrap();
    let second_alias = doc.get_path(&["second_alias"]).unwrap().unwrap();

    assert_eq!(doc.alias_name(first_alias), Some("value"));
    assert_eq!(doc.resolve_alias(first_alias), Some(first));
    assert_eq!(doc.resolve_alias(second_alias), Some(second));
}

#[test]
fn custom_tag_resolution_is_document_local() {
    let doc = YamlDoc::parse(
        "%TAG !e! tag:first/\n--- !e!value one\n...\n%TAG !e! tag:second/\n--- !e!value two\n",
    )
    .expect("valid per-document tag directives");
    let tagged = doc
        .documents()
        .filter_map(|document| {
            doc.children(document)
                .find(|child| doc.semantic_kind(*child).is_some())
        })
        .collect::<Vec<_>>();

    assert_eq!(doc.raw_tag(tagged[0]), Some("!e!value"));
    assert_eq!(
        doc.resolved_tag(tagged[0]).unwrap().as_deref(),
        Some("tag:first/value")
    );
    assert_eq!(
        doc.resolved_tag(tagged[1]).unwrap().as_deref(),
        Some("tag:second/value")
    );
}

#[test]
fn parser_builds_anchored_and_tagged_flow_collection_values() {
    let doc =
        YamlDoc::parse("items: &seq !!seq [one, two]\nsettings: !<tag:yaml.org,2002:map> {a: b}\n")
            .expect("valid flow collection node properties");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :items\n+SEQ [] &seq <tag:yaml.org,2002:seq>\n=VAL :one\n=VAL :two\n-SEQ\n=VAL :settings\n+MAP {} <tag:yaml.org,2002:map>\n=VAL :a\n=VAL :b\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
    let items = doc
        .get_path(&["items"])
        .expect("path succeeds")
        .expect("items exists");
    assert_eq!(
        doc.semantic_kind(items),
        Some(SemanticKind::Sequence {
            style: CollectionStyle::Flow
        })
    );
    assert_eq!(doc.raw_tag(items), Some("!!seq"));
    assert_eq!(
        doc.resolved_tag(items).expect("tag resolves").as_deref(),
        Some("tag:yaml.org,2002:seq")
    );
    assert_eq!(doc.anchor(items), Some("seq"));
}

#[test]
fn directives_accept_yaml_version_before_explicit_document() {
    let doc = YamlDoc::parse("%YAML 1.2 # comment\n--- value\n").expect("valid directive");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL :value\n-DOC\n-STR\n"
    );
    assert_eq!(count_nodes(&doc, NodeKind::Directive), 1);
}

#[test]
fn directives_tolerate_reserved_and_unsupported_versions_before_document() {
    for (input, expected) in [
        (
            "%FOO  bar baz # ignored\n---\n\"foo\"\n",
            "+STR\n+DOC ---\n=VAL \"foo\n-DOC\n-STR\n",
        ),
        (
            "%YAML 1.3 # Attempt parsing\n---\n\"foo\"\n",
            "+STR\n+DOC ---\n=VAL \"foo\n-DOC\n-STR\n",
        ),
        ("%YAM 1.1\n---\n", "+STR\n+DOC ---\n=VAL :\n-DOC\n-STR\n"),
        ("%YAMLL 1.1\n---\n", "+STR\n+DOC ---\n=VAL :\n-DOC\n-STR\n"),
    ] {
        let doc = YamlDoc::parse(input).expect("reserved directive should be tolerated");

        assert_eq!(doc.events_to_test_string(), expected);
        assert_eq!(doc.to_string(), input);
        assert_eq!(count_nodes(&doc, NodeKind::Directive), 1);
    }
}

#[test]
fn directives_expose_metadata_with_cst_nodes() {
    let doc =
        YamlDoc::parse("%YAML 1.2\n%TAG !e! tag:example.com,2000:app/\n%FOO bar baz\n---\nvalue\n")
            .expect("valid directives");

    let yaml = doc.yaml_directive().expect("YAML directive exists");
    assert_eq!(yaml.version, "1.2");
    assert_eq!(
        doc.node(yaml.node).expect("yaml node exists").kind,
        NodeKind::Directive
    );

    let tags = doc.tag_directives();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].handle, "!e!");
    assert_eq!(tags[0].prefix, "tag:example.com,2000:app/");
    assert_eq!(
        doc.node(tags[0].node).expect("tag node exists").kind,
        NodeKind::Directive
    );

    let reserved = doc.reserved_directives();
    assert_eq!(reserved.len(), 1);
    assert_eq!(reserved[0].name, "%FOO");
    assert_eq!(reserved[0].parameters, ["bar", "baz"]);
}

#[test]
fn directive_editor_updates_existing_directives_preserving_comments() {
    let mut doc =
        YamlDoc::parse("%YAML 1.2 # keep yaml\n%TAG !e! tag:old/ # keep tag\n---\n!e!foo value\n")
            .expect("valid directives");

    doc.set_yaml_directive("1.3").expect("YAML directive edits");
    doc.set_tag_directive("!e!", "tag:new/")
        .expect("TAG directive edits");

    assert_eq!(
        doc.to_string(),
        "%YAML 1.3 # keep yaml\n%TAG !e! tag:new/ # keep tag\n---\n!e!foo value\n"
    );
}

#[test]
fn directive_editor_inserts_before_explicit_and_implicit_documents() {
    let mut explicit = YamlDoc::parse("---\nvalue\n").expect("valid explicit document");
    explicit
        .set_yaml_directive("1.2")
        .expect("YAML directive inserts");
    explicit
        .set_tag_directive("!e!", "tag:example.com,2000:app/")
        .expect("TAG directive inserts");
    assert_eq!(
        explicit.to_string(),
        "%YAML 1.2\n%TAG !e! tag:example.com,2000:app/\n---\nvalue\n"
    );

    let mut implicit = YamlDoc::parse("value\n").expect("valid implicit document");
    implicit
        .set_tag_directive("!e!", "tag:example.com,2000:app/")
        .expect("TAG directive inserts");
    assert_eq!(
        implicit.to_string(),
        "%TAG !e! tag:example.com,2000:app/\nvalue\n"
    );
}

#[test]
fn directive_editor_removes_directive_lines() {
    let mut doc = YamlDoc::parse(
        "%YAML 1.2\n%TAG !e! tag:example.com,2000:app/\n%TAG !f! tag:foo/\n---\nvalue\n",
    )
    .expect("valid directives");

    doc.remove_yaml_directive().expect("YAML directive removes");
    doc.remove_tag_directive("!e!")
        .expect("selected TAG directive removes");

    assert_eq!(doc.to_string(), "%TAG !f! tag:foo/\n---\nvalue\n");
}

#[test]
fn directive_editor_commit_reparses_tag_resolution() {
    let mut doc =
        YamlDoc::parse("%TAG !e! tag:old/\n---\n!e!foo value\n").expect("valid directives");

    doc.set_tag_directive("!e!", "tag:new/")
        .expect("TAG directive edits");
    doc.commit_edits().expect("edited directive reparses");

    let semantic = doc
        .nodes
        .iter()
        .enumerate()
        .find_map(|(index, _)| {
            let node = NodeId::from_usize(index);
            (matches!(doc.semantic_kind(node), Some(SemanticKind::Scalar { .. }))
                && doc.scalar_value(node).is_ok_and(|value| value == "value")
                && doc
                    .resolved_tag(node)
                    .is_ok_and(|tag| tag.as_deref() == Some("tag:new/foo")))
            .then_some(node)
        })
        .expect("tag resolution updates after commit");
    assert!(doc.node(semantic).is_some());
}

#[test]
fn directive_editor_rejects_invalid_inputs_without_editing() {
    let mut doc = YamlDoc::parse("---\nvalue\n").expect("valid document");

    let error = doc
        .set_yaml_directive("1.x")
        .expect_err("invalid YAML version rejects");
    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert!(doc.edits.is_empty());

    let error = doc
        .set_tag_directive("bad", "tag:example.com/")
        .expect_err("invalid TAG handle rejects");
    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert!(doc.edits.is_empty());

    let error = doc
        .set_tag_directive("!e!", "tag:bad prefix/")
        .expect_err("invalid TAG prefix rejects");
    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert_eq!(doc.to_string(), "---\nvalue\n");
}

#[test]
fn tag_directive_resolves_secondary_handle() {
    let doc = YamlDoc::parse("%TAG !! tag:example.com,2000:app/\n---\n!!int 1 - 3\n")
        .expect("valid tag directive");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL <tag:example.com,2000:app/int> :1 - 3\n-DOC\n-STR\n"
    );
}

#[test]
fn tag_directive_resolves_named_handle() {
    let doc = YamlDoc::parse("%TAG !e! tag:example.com,2000:app/\n---\n!e!foo \"bar\"\n")
        .expect("valid named tag directive");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL <tag:example.com,2000:app/foo> \"bar\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_rejects_directives_after_tagged_implicit_content() {
    for input in [
        "!foo \"bar\"\n%TAG ! tag:example.com,2000:app/\n---\n!foo \"bar\"\n",
        "!foo \"bar\"\n%YAML 1.2\n---\n",
        "!foo \"bar\"\n%FOO ignored\n---\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("directive after content should reject");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(
            error.diagnostic.message,
            "directives must appear before document content"
        );
    }
}

#[test]
fn parser_preserves_directive_and_tagged_scalar_neighbors() {
    for (input, expected) in [
        (
            "%TAG ! tag:example.com,2000:app/\n---\n!foo \"bar\"\n",
            "+STR\n+DOC ---\n=VAL <tag:example.com,2000:app/foo> \"bar\n-DOC\n-STR\n",
        ),
        (
            "%FOO  bar baz # ignored\n---\n\"foo\"\n",
            "+STR\n+DOC ---\n=VAL \"foo\n-DOC\n-STR\n",
        ),
        (
            "!foo \"bar\"\n",
            "+STR\n+DOC\n=VAL <!foo> \"bar\n-DOC\n-STR\n",
        ),
    ] {
        let doc = YamlDoc::parse(input).expect("valid directive or tagged scalar neighbor");

        assert_eq!(doc.events_to_test_string(), expected);
        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn tag_directive_percent_decodes_suffix() {
    let doc = YamlDoc::parse("%TAG !e! tag:example.com,2000:app/\n---\n- !e!tag%21 baz\n")
        .expect("valid tag directive with URI escape");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n+SEQ\n=VAL <tag:example.com,2000:app/tag!> :baz\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn events_render_multi_document_stream_with_explicit_end() {
    let doc =
        YamlDoc::parse("%YAML 1.2\n--- |\n%!PS-Adobe-2.0\n...\n%YAML 1.2\n---\n# Empty\n...\n")
            .expect("valid multi-document stream");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL |%!PS-Adobe-2.0\\n\n-DOC ...\n+DOC ---\n=VAL :\n-DOC ...\n-STR\n"
    );
    assert_eq!(doc.documents().count(), 2);
}

#[test]
fn parser_builds_empty_documents_in_stream() {
    let doc = YamlDoc::parse("---\n...\n---\n...\n").expect("valid empty documents");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL :\n-DOC ...\n+DOC ---\n=VAL :\n-DOC ...\n-STR\n"
    );
    assert_eq!(doc.documents().count(), 2);
}

#[test]
fn parser_keeps_contentless_streams_empty() {
    for input in [
        "",
        "# Comment only.\n",
        "  # Comment\n   \n\n",
        "...\n",
        "# comment\n...\n",
    ] {
        let doc = YamlDoc::parse(input).expect("contentless stream is valid");

        assert_eq!(doc.events_to_test_string(), "+STR\n-STR\n");
        assert_eq!(doc.to_string(), input);
        assert_eq!(doc.documents().count(), 0);
    }
}

#[test]
fn parser_preserves_explicit_empty_documents() {
    for (input, expected) in [
        ("---\n", "+STR\n+DOC ---\n=VAL :\n-DOC\n-STR\n"),
        ("---\n...\n", "+STR\n+DOC ---\n=VAL :\n-DOC ...\n-STR\n"),
    ] {
        let doc = YamlDoc::parse(input).expect("explicit empty document is valid");

        assert_eq!(doc.events_to_test_string(), expected);
        assert_eq!(doc.to_string(), input);
        assert_eq!(doc.documents().count(), 1);
    }
}

#[test]
fn parser_reports_malformed_and_duplicate_directives() {
    for (input, message) in [
        ("%YAML\n---\n", "missing YAML directive version"),
        ("%YAML 1.2\n%YAML 1.2\n---\n", "duplicate YAML directive"),
        (
            "key: value\n%YAML 1.2\n",
            "directives must appear before document content",
        ),
        (
            "%YAML 1.2\n",
            "directives must be followed by document content",
        ),
        ("%YAML 1.1#...\n---\n", "invalid YAML directive version"),
        (
            "%TAG !bad tag:example.com,2000:app/\n---\n",
            "invalid TAG directive handle",
        ),
    ] {
        let error = YamlDoc::parse(input).expect_err("directive should be rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.message, message);
    }
}

#[test]
fn yaml_value_writes_anchored_plain_scalar_preserving_anchor() {
    let mut doc = YamlDoc::parse("plain: &anchor value\n").expect("valid anchor");
    let plain = doc
        .get_path(&["plain"])
        .expect("path lookup succeeds")
        .expect("plain exists");

    String::from("updated")
        .write_yaml(&mut doc, Some(plain))
        .expect("anchored scalar writes");

    assert_eq!(doc.to_string(), "plain: &anchor updated\n");
}

#[test]
fn yaml_value_writes_tagged_double_quoted_scalar_preserving_tag() {
    let mut doc = YamlDoc::parse("plain: !!str \"value\"\n").expect("valid tag");
    let plain = doc
        .get_path(&["plain"])
        .expect("path lookup succeeds")
        .expect("plain exists");

    String::from("new \"value\"")
        .write_yaml(&mut doc, Some(plain))
        .expect("tagged scalar writes");

    assert_eq!(doc.to_string(), "plain: !!str \"new \\\"value\\\"\"\n");
}

#[test]
fn yaml_value_writes_scalar_with_tag_and_anchor_preserving_prefix() {
    let mut doc = YamlDoc::parse("plain: !local  &anchor value\n").expect("valid properties");
    let plain = doc
        .get_path(&["plain"])
        .expect("path lookup succeeds")
        .expect("plain exists");

    String::from("updated")
        .write_yaml(&mut doc, Some(plain))
        .expect("decorated scalar writes");

    assert_eq!(doc.to_string(), "plain: !local  &anchor updated\n");
}

#[test]
fn yaml_value_writes_decorated_block_scalars() {
    let mut literal = YamlDoc::parse("&msg\n|-\nold\n").expect("valid literal");
    let message = literal_scalar(&literal).expect("literal exists");

    String::from("new")
        .write_yaml(&mut literal, Some(message))
        .expect("anchored literal writes");

    assert_eq!(literal.to_string(), "&msg\n|-\nnew");

    let mut folded = YamlDoc::parse("folded:\n   !foo\n  >1\n old\n").expect("valid folded");
    let message = folded_scalar(&folded).expect("folded exists");

    String::from("new text")
        .write_yaml(&mut folded, Some(message))
        .expect("tagged folded writes");

    assert_eq!(folded.to_string(), "folded:\n   !foo\n  >1\n new text\n");
}

#[test]
fn yaml_value_writes_decorated_flow_collections_preserving_properties() {
    let mut sequence = YamlDoc::parse("items: &items [one, two]\n").expect("valid sequence");
    let items = sequence
        .get_path(&["items"])
        .expect("path lookup succeeds")
        .expect("items exists");

    vec!["three".to_owned()]
        .write_yaml(&mut sequence, Some(items))
        .expect("anchored flow sequence writes");

    assert_eq!(sequence.to_string(), "items: &items [three]\n");

    let mut mapping = YamlDoc::parse("settings: !!map {a: b}\n").expect("valid tagged mapping");
    let settings = mapping
        .get_path(&["settings"])
        .expect("path lookup succeeds")
        .expect("settings exists");
    let replacement = std::collections::BTreeMap::from([("a".to_owned(), "updated".to_owned())]);

    replacement
        .write_yaml(&mut mapping, Some(settings))
        .expect("tagged flow mapping writes");

    assert_eq!(mapping.to_string(), "settings: !!map {a: updated}\n");
}

#[test]
fn yaml_value_writes_decorated_collections_preserving_property_spacing() {
    let mut doc =
        YamlDoc::parse("items: !seq  &items [one, two]\n").expect("valid decorated sequence");
    let items = doc
        .get_path(&["items"])
        .expect("path lookup succeeds")
        .expect("items exists");

    vec!["three".to_owned()]
        .write_yaml(&mut doc, Some(items))
        .expect("decorated flow sequence writes");

    assert_eq!(doc.to_string(), "items: !seq  &items [three]\n");
}

#[test]
fn yaml_value_replaces_property_only_block_sequence_body() {
    let mut doc =
        YamlDoc::parse("items: &items\n  - one\n  - two\n").expect("valid block sequence");
    let items = doc
        .get_path(&["items"])
        .expect("path lookup succeeds")
        .expect("items exists");

    vec!["three".to_owned()]
        .write_yaml(&mut doc, Some(items))
        .expect("decorated block sequence writes");

    assert_eq!(doc.to_string(), "items: &items\n  - three\n");
}

#[test]
fn yaml_value_writes_empty_decorated_mapping_and_preserves_semantic_metadata_after_commit() {
    let mut doc =
        YamlDoc::parse("settings: &settings !!map {a: b}\n").expect("valid decorated mapping");
    let settings = doc
        .get_path(&["settings"])
        .expect("path lookup succeeds")
        .expect("settings exists");
    let replacement = std::collections::BTreeMap::<String, String>::new();

    replacement
        .write_yaml(&mut doc, Some(settings))
        .expect("empty decorated mapping writes");

    assert_eq!(doc.to_string(), "settings: &settings !!map {}\n");

    doc.commit_edits().expect("decorated mapping reparses");
    let settings = doc
        .get_path(&["settings"])
        .expect("path lookup succeeds after commit")
        .expect("settings exists after commit");
    assert!(matches!(
        doc.semantic_kind(settings),
        Some(SemanticKind::Mapping { .. })
    ));
    assert_eq!(
        doc.resolved_tag(settings)
            .expect("mapping tag resolves")
            .as_deref(),
        Some("tag:yaml.org,2002:map")
    );
    assert_eq!(doc.anchor(settings), Some("settings"));
}

#[test]
fn yaml_value_rejects_invalid_decorated_collection_fragments_without_editing() {
    let mut doc = YamlDoc::parse("items: &items [one]\n").expect("valid decorated sequence");
    let items = doc
        .get_path(&["items"])
        .expect("path lookup succeeds")
        .expect("items exists");

    let error = vec!["bad\nvalue".to_owned()]
        .write_yaml(&mut doc, Some(items))
        .expect_err("invalid plain fragment rejects");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert_eq!(doc.to_string(), "items: &items [one]\n");
    assert!(doc.edits.is_empty());
}

#[test]
fn parser_events_carry_source_spans() {
    let doc = YamlDoc::parse("host: localhost\n").expect("valid mapping");
    let scalar_events: Vec<_> = doc
        .events()
        .filter_map(|event| match event.kind {
            YamlEventKind::Scalar { value, .. } => Some((value, event.span)),
            _ => None,
        })
        .collect();

    assert_eq!(
        scalar_events,
        [
            ("host".to_owned(), Span::new(0, 4)),
            ("localhost".to_owned(), Span::new(6, 15))
        ]
    );
}

#[test]
fn semantic_events_link_directly_to_originating_cst_nodes() {
    let doc = YamlDoc::parse("---\nroot:\n  - {key: &anchor value}\n  - *anchor\n")
        .expect("valid nested block and flow collections");

    for event in doc.events() {
        if matches!(
            event.kind,
            YamlEventKind::DocumentStart { .. }
                | YamlEventKind::MappingStart { .. }
                | YamlEventKind::SequenceStart { .. }
                | YamlEventKind::Scalar { .. }
                | YamlEventKind::Alias { .. }
        ) {
            let cst = event.cst.expect("semantic node event has a CST link");
            assert!(doc.node(cst).is_some(), "CST link points into the arena");
        }
    }
}

#[test]
fn semantic_metadata_is_keyed_by_cst_node() {
    let doc = YamlDoc::parse("value\n").expect("valid scalar");
    let document = doc.documents().next().expect("document exists");
    let scalar = doc.children(document).next().expect("scalar exists");

    assert_eq!(
        doc.node(scalar).map(|node| node.span),
        Some(Span::new(0, 5))
    );
    assert_eq!(
        doc.semantic_kind(scalar),
        Some(SemanticKind::Scalar {
            style: YamlScalarStyle::Plain
        })
    );
}

#[test]
fn semantic_sequence_items_preserve_path_lookup() {
    let doc = YamlDoc::parse("ports:\n  - 8080\n  - 9090\n").expect("valid sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("path lookup succeeds")
        .expect("ports exists");
    let items = doc.sequence_items(ports).collect::<Vec<_>>();

    assert_eq!(
        doc.node(ports).map(|node| node.kind),
        Some(NodeKind::BlockSequence)
    );
    assert_eq!(
        items
            .iter()
            .map(|item| doc.scalar_value(*item).expect("scalar value"))
            .collect::<Vec<_>>(),
        ["8080".to_owned(), "9090".to_owned()]
    );
}

#[test]
fn path_lookup_returns_semantic_cst_node() {
    let doc = YamlDoc::parse("server:\n  host: localhost\n").expect("valid nested mapping");
    let host = doc
        .get_path(&["server", "host"])
        .expect("path lookup succeeds")
        .expect("host exists");

    assert_eq!(
        doc.semantic_kind(host),
        Some(SemanticKind::Scalar {
            style: YamlScalarStyle::Plain
        })
    );
    assert_eq!(
        doc.get_path(&["server", "host"])
            .expect("CST path lookup succeeds"),
        Some(host)
    );
}

#[test]
fn semantic_mapping_entries_build_flow_mapping_and_sequence() {
    let doc = YamlDoc::parse("settings: {a: [b, c]}\n").expect("valid flow collections");
    let settings = doc
        .get_path(&["settings"])
        .expect("path lookup succeeds")
        .expect("settings exists");
    let entries = doc.mapping_entries(settings).collect::<Vec<_>>();

    assert_eq!(
        doc.node(settings).map(|node| node.kind),
        Some(NodeKind::FlowMapping)
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(doc.scalar_value(entries[0].0).expect("key reads"), "a");
    assert_eq!(
        doc.node(entries[0].1).map(|node| node.kind),
        Some(NodeKind::FlowSequence)
    );
}

#[test]
fn collection_values_have_logical_cst_wrapper_parents() {
    let doc = YamlDoc::parse(
        "same:\n- item\nnested:\n  key: value\ncompact:\n  - child: value\nflow: [{a: b}, [c]]\nprops: &node\n  value: yes\n",
    )
    .expect("nested collection forms are valid");

    for path in [
        &["same"][..],
        &["nested"][..],
        &["compact"][..],
        &["flow"][..],
        &["props"][..],
    ] {
        let value = doc
            .get_path(path)
            .expect("path lookup succeeds")
            .expect("value exists");
        let parent = doc
            .node(value)
            .and_then(Node::parent)
            .expect("value parent");
        assert_eq!(
            doc.node(parent).map(Node::kind),
            Some(NodeKind::MappingEntry)
        );
    }

    let compact = doc
        .get_path(&["compact"])
        .expect("path lookup succeeds")
        .expect("compact sequence exists");
    let compact_mapping = doc.sequence_items(compact).next().expect("compact item");
    let compact_parent = doc
        .node(compact_mapping)
        .and_then(Node::parent)
        .expect("compact mapping parent");
    assert_eq!(
        doc.node(compact_parent).map(Node::kind),
        Some(NodeKind::SequenceEntry)
    );

    let flow = doc
        .get_path(&["flow"])
        .expect("path lookup succeeds")
        .expect("flow sequence exists");
    for item in doc.sequence_items(flow) {
        let parent = doc.node(item).and_then(Node::parent).expect("flow parent");
        assert_eq!(
            doc.node(parent).map(Node::kind),
            Some(NodeKind::SequenceEntry)
        );
    }
}

#[test]
fn semantic_values_build_literal_and_folded_scalars() {
    let doc = YamlDoc::parse("literal: |\n  one\nfolded: >\n  one\n  two\n")
        .expect("valid block scalars");
    let literal = doc
        .get_path(&["literal"])
        .expect("path lookup succeeds")
        .expect("literal exists");
    let folded = doc
        .get_path(&["folded"])
        .expect("path lookup succeeds")
        .expect("folded exists");

    assert_eq!(doc.scalar_value(literal).expect("literal reads"), "one\n");
    assert_eq!(doc.scalar_value(folded).expect("folded reads"), "one two\n");
}

#[test]
fn parser_builds_block_mapping_and_sequence_cst() {
    let input = "host: localhost
ports:
  - 8080
  - 9090
";
    let doc = YamlDoc::parse(input).expect("parser MVP should accept block collections");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.nodes.first().map(|node| node.kind),
        Some(NodeKind::Stream)
    );
    assert_eq!(count_nodes(&doc, NodeKind::BlockMapping), 1);
    assert_eq!(count_nodes(&doc, NodeKind::MappingEntry), 2);
    assert_eq!(count_nodes(&doc, NodeKind::BlockSequence), 1);
    assert_eq!(count_nodes(&doc, NodeKind::SequenceEntry), 2);
    assert!(scalar_texts(&doc).contains(&"host"));
    assert!(scalar_texts(&doc).contains(&"localhost"));
    assert!(scalar_texts(&doc).contains(&"8080"));
    assert!(scalar_texts(&doc).contains(&"9090"));
}

#[test]
fn parser_attaches_same_indent_sequence_after_empty_mapping_value() {
    let input = "one:\n- 2\n- 3\nfour: 5\n";
    let doc = YamlDoc::parse(input).expect("valid same-indent sequence value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :one\n+SEQ\n=VAL :2\n=VAL :3\n-SEQ\n=VAL :four\n=VAL :5\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_closes_same_indent_sequence_before_next_mapping_entry() {
    let input = "foo:\n- 42\nbar:\n  - 44\n";
    let doc = YamlDoc::parse(input).expect("valid sibling sequence values");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :foo\n+SEQ\n=VAL :42\n-SEQ\n=VAL :bar\n+SEQ\n=VAL :44\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_attaches_sequence_before_nested_mapping_value() {
    let input = "sequence:\n- one\n- two\nmapping:\n  ? sky\n  : blue\n  sea : green\n";
    let doc = YamlDoc::parse(input).expect("valid sequence then mapping values");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :sequence\n+SEQ\n=VAL :one\n=VAL :two\n-SEQ\n=VAL :mapping\n+MAP\n=VAL :sky\n=VAL :blue\n=VAL :sea\n=VAL :green\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_preserves_empty_mapping_value_before_sibling_mapping() {
    let input = "key:\nnext: value\n";
    let doc = YamlDoc::parse(input).expect("valid empty value before sibling mapping");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :\n=VAL :next\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_composes_empty_anchored_mapping_value_before_alias() {
    let input = "---\na: &anchor\nb: *anchor\n";
    let doc = YamlDoc::parse(input).expect("valid empty anchored scalar value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n+MAP\n=VAL :a\n=VAL &anchor :\n=VAL :b\n=ALI *anchor\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_rejects_invalid_node_property_placement() {
    for input in [
        "key1: &a value\nkey2: &b *a\n",
        "key1: &alias value1\n&b *alias : value2\n",
        "&anchor - sequence entry\n",
        "- !!str, xxx\n",
        "top1: &node1\n  &k1 key1: val1\ntop2: &node2\n  &v2 val2\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("invalid node property should reject");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }
}

#[test]
fn parser_preserves_valid_node_property_neighbors() {
    for input in [
        "a: &anchor\nb: *anchor\n",
        "&a4 !!map\n&a5 !!str key5: value4\n",
        "top6: \n  &anchor6 'key6' : scalar6\n",
    ] {
        let doc = YamlDoc::parse(input).expect("valid node property placement");

        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn parser_composes_empty_tagged_scalars_in_sequence_mappings() {
    let input = "- !!str\n-\n  !!null : a\n  b: !!str\n- !!str : !!null\n";
    let doc = YamlDoc::parse(input).expect("valid empty tagged scalar positions");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL <tag:yaml.org,2002:str> :\n+MAP\n=VAL <tag:yaml.org,2002:null> :\n=VAL :a\n=VAL :b\n=VAL <tag:yaml.org,2002:str> :\n-MAP\n+MAP\n=VAL <tag:yaml.org,2002:str> :\n=VAL <tag:yaml.org,2002:null> :\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_composes_empty_anchored_scalars_in_explicit_entries() {
    let input = "- &a\n- a\n-\n  &a : a\n  b: &b\n-\n  &c : &a\n-\n  ? &d\n-\n  ? &e\n  : &a\n";
    let doc = YamlDoc::parse(input).expect("valid empty anchored scalar positions");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL &a :\n=VAL :a\n+MAP\n=VAL &a :\n=VAL :a\n=VAL :b\n=VAL &b :\n-MAP\n+MAP\n=VAL &c :\n=VAL &a :\n-MAP\n+MAP\n=VAL &d :\n=VAL :\n-MAP\n+MAP\n=VAL &e :\n=VAL &a :\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_tag_to_same_indent_mapping_sequence_value() {
    let input = "sequence: !!seq\n- entry\n- !!seq\n - nested\nmapping: !!map\n foo: bar\n";
    let doc = YamlDoc::parse(input).expect("valid tagged same-indent collection values");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :sequence\n+SEQ <tag:yaml.org,2002:seq>\n=VAL :entry\n+SEQ <tag:yaml.org,2002:seq>\n=VAL :nested\n-SEQ\n-SEQ\n=VAL :mapping\n+MAP <tag:yaml.org,2002:map>\n=VAL :foo\n=VAL :bar\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_alias_and_anchored_block_mapping_keys() {
    let input = "\"top1\" : \n  \"key1\" : &alias1 scalar1\n'top2' : \n  'key2' : &alias2 scalar2\ntop3: &node3 \n  *alias1 : scalar3\ntop4: \n  *alias2 : scalar4\ntop5   :    \n  scalar5\ntop6: \n  &anchor6 'key6' : scalar6\n";
    let doc = YamlDoc::parse(input).expect("valid anchored and alias block keys");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL \"top1\n+MAP\n=VAL \"key1\n=VAL &alias1 :scalar1\n-MAP\n=VAL 'top2\n+MAP\n=VAL 'key2\n=VAL &alias2 :scalar2\n-MAP\n=VAL :top3\n+MAP &node3\n=ALI *alias1\n=VAL :scalar3\n-MAP\n=VAL :top4\n+MAP\n=ALI *alias2\n=VAL :scalar4\n-MAP\n=VAL :top5\n=VAL :scalar5\n=VAL :top6\n+MAP\n=VAL &anchor6 'key6\n=VAL :scalar6\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_flow_sequence_value_with_implicit_mapping_item() {
    let input = "\"implicit block key\" : [\n  \"implicit flow key\" : value,\n ]\n";
    let doc = YamlDoc::parse(input).expect("valid flow sequence mapping value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL \"implicit block key\n+SEQ []\n+MAP {}\n=VAL \"implicit flow key\n=VAL :value\n-MAP\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_explicit_block_sequence_and_flow_keys() {
    let input = "? - Detroit Tigers\n  - Chicago cubs\n:\n  - 2001-07-23\n\n? [ New York Yankees,\n    Atlanta Braves ]\n: [ 2001-07-02, 2001-08-12,\n    2001-08-14 ]\n";
    let doc = YamlDoc::parse(input).expect("valid explicit sequence and flow keys");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n+SEQ\n=VAL :Detroit Tigers\n=VAL :Chicago cubs\n-SEQ\n+SEQ\n=VAL :2001-07-23\n-SEQ\n+SEQ []\n=VAL :New York Yankees\n=VAL :Atlanta Braves\n-SEQ\n+SEQ []\n=VAL :2001-07-02\n=VAL :2001-08-12\n=VAL :2001-08-14\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_flow_collection_key_after_explicit_indicator() {
    let input = "? []: x\n";
    let doc = YamlDoc::parse(input).expect("valid explicit flow collection key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n+MAP\n+SEQ []\n-SEQ\n=VAL :x\n-MAP\n=VAL :\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_anchor_colon_and_alias_colon_keys() {
    let input = "&a: key: &a value\nfoo:\n  *a:\n";
    let doc = YamlDoc::parse(input).expect("valid colon-suffixed anchor and alias keys");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL &a: :key\n=VAL &a :value\n=VAL :foo\n=ALI *a:\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_explicit_compact_mapping_key_and_value() {
    let input = "- sun: yellow\n- ? earth: blue\n  : moon: white\n";
    let doc = YamlDoc::parse(input).expect("valid explicit compact mapping key and value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :sun\n=VAL :yellow\n-MAP\n+MAP\n+MAP\n=VAL :earth\n=VAL :blue\n-MAP\n+MAP\n=VAL :moon\n=VAL :white\n-MAP\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_mapping_value_for_bare_sequence_entry() {
    let input = "-\n  name: Mark\n";
    let doc = YamlDoc::parse(input).expect("parser should accept nested mapping entry value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :name\n=VAL :Mark\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_nested_compact_block_sequence_entry() {
    let input = "- - s1_i1\n  - s1_i2\n- s2\n";
    let doc = YamlDoc::parse(input).expect("parser should accept nested sequence entry value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+SEQ\n=VAL :s1_i1\n=VAL :s1_i2\n-SEQ\n=VAL :s2\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_compact_mapping_sequence_entry_value() {
    let input = "block sequence:\n  - one\n  - two : three\n";
    let doc = YamlDoc::parse(input).expect("parser should accept compact mapping entry value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :block sequence\n+SEQ\n=VAL :one\n+MAP\n=VAL :two\n=VAL :three\n-MAP\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_nested_mapping_and_sequence_for_bare_sequence_entries() {
    let input = "-\n foo: bar\n-\n - 42\n";
    let doc =
        YamlDoc::parse(input).expect("parser should accept nested bare sequence entry values");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :foo\n=VAL :bar\n-MAP\n+SEQ\n=VAL :42\n-SEQ\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_preserves_true_empty_block_sequence_entry() {
    let input = "-\n- value\n";
    let doc = YamlDoc::parse(input).expect("parser should accept empty sequence entry");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL :\n=VAL :value\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_treats_comment_only_sequence_entry_as_empty() {
    let input = "- # Empty\n- |\n block node\n- - one # Compact\n  - two # sequence\n- one: two # Compact mapping\n";
    let doc = YamlDoc::parse(input).expect("parser should accept comment-only entry");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL :\n=VAL |block node\\n\n+SEQ\n=VAL :one\n=VAL :two\n-SEQ\n+MAP\n=VAL :one\n=VAL :two\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_folds_same_line_plain_scalar_continuations() {
    let input = "plain: a\n b\n\n c\n";
    let doc = YamlDoc::parse(input).expect("parser should accept plain scalar continuations");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :plain\n=VAL :a b\\nc\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_folds_next_line_plain_scalar_mapping_values() {
    let input = "key:\n  value\n  with\n  \t\n  tabs\n";
    let doc = YamlDoc::parse(input).expect("parser should accept next-line plain scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :value with\\ntabs\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_folds_log_message_plain_scalar_mapping_value() {
    let input = "Warning:\n  This is an error message\n  for the log file\n";
    let doc = YamlDoc::parse(input).expect("parser should accept log message scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :Warning\n=VAL :This is an error message for the log file\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_preserves_tab_prefixed_plain_scalar_continuation() {
    let input = "plain: text\n \tlines\n";
    let doc = YamlDoc::parse(input).expect("parser should accept tab-prefixed content");
    let plain = doc
        .get_path(&["plain"])
        .expect("lookup succeeds")
        .expect("plain exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(doc.scalar_value(plain).expect("plain reads"), "text lines");
}

#[test]
fn parser_accepts_tab_prefixed_quoted_scalar_continuation() {
    let input = "quoted: \"text\n  \tlines\"\n";
    let doc = YamlDoc::parse(input).expect("parser should accept quoted tab content");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :quoted\n=VAL \"text lines\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_tab_prefixed_sequence_entry_continuation() {
    let input = "x:\n - x\n  \tx\n";
    let doc = YamlDoc::parse(input).expect("parser should accept sequence continuation");
    let items = doc
        .get_path(&["x"])
        .expect("lookup succeeds")
        .expect("x exists");
    let sequence = doc.sequence_items(items).collect::<Vec<_>>();

    assert_eq!(doc.to_string(), input);
    assert_eq!(sequence.len(), 1);
    assert_eq!(
        doc.scalar_value(sequence[0]).expect("sequence item reads"),
        "x x"
    );
}

#[test]
fn parser_accepts_root_flow_collection_with_leading_tab() {
    let input = "\t[\n\t]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept tab-prefixed root flow");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_treats_inline_comment_mapping_value_as_empty_for_nested_block_value() {
    let input = "hr: # 1998 hr ranking\n  - Mark McGwire\n  - Sammy Sosa\n";
    let doc = YamlDoc::parse(input).expect("parser should accept commented nested value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :hr\n+SEQ\n=VAL :Mark McGwire\n=VAL :Sammy Sosa\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_anchor_to_root_block_sequence() {
    let input = "&sequence\n- a\n";
    let doc = YamlDoc::parse(input).expect("parser should accept anchored root sequence");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ &sequence\n=VAL :a\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_anchor_to_nested_block_mapping() {
    let input = "top1: &node1\n  key1: one\n";
    let doc = YamlDoc::parse(input).expect("parser should accept anchored nested mapping");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :top1\n+MAP &node1\n=VAL :key1\n=VAL :one\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_tag_to_nested_block_sequence() {
    let input = "foo: !!seq\n  - !!str a\n";
    let doc = YamlDoc::parse(input).expect("parser should accept tagged nested sequence");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :foo\n+SEQ <tag:yaml.org,2002:seq>\n=VAL <tag:yaml.org,2002:str> :a\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_anchor_to_compact_nested_mapping_key() {
    let input = "top3:\n  &k3 key3: three\n";
    let doc = YamlDoc::parse(input).expect("parser should accept anchored nested key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :top3\n+MAP\n=VAL &k3 :key3\n=VAL :three\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_keeps_property_only_value_as_scalar_when_nested_value_is_plain() {
    let input = "top6: &val6\n  six\n";
    let doc = YamlDoc::parse(input).expect("parser should accept anchored scalar value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :top6\n=VAL &val6 :six\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_split_root_scalar_properties() {
    let input = "---\n&a1\n!!str\nscalar1\n";
    let doc = YamlDoc::parse(input).expect("parser should accept split scalar properties");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL &a1 <tag:yaml.org,2002:str> :scalar1\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_split_root_scalar_properties_in_reversed_order() {
    let input = "---\n!!str\n&a2\nscalar2\n";
    let doc = YamlDoc::parse(input).expect("parser should accept reversed split scalar properties");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL &a2 <tag:yaml.org,2002:str> :scalar2\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_split_properties_to_nested_block_mapping() {
    let input = "key: &anchor\n !!map\n  a: b\n";
    let doc = YamlDoc::parse(input).expect("parser should accept split nested mapping properties");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key\n+MAP &anchor <tag:yaml.org,2002:map>\n=VAL :a\n=VAL :b\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_applies_split_property_to_block_scalar() {
    let input = "folded:\n   !foo\n  >1\n value\n";
    let doc = YamlDoc::parse(input).expect("parser should accept tagged block scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :folded\n=VAL <!foo> >value\\n\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_keeps_same_line_split_property_cases() {
    let input = "&a4 !!map\n&a5 !!str key5: value4\n";
    let doc = YamlDoc::parse(input).expect("parser should accept same-line properties");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP &a4 <tag:yaml.org,2002:map>\n=VAL &a5 <tag:yaml.org,2002:str> :key5\n=VAL :value4\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_root_literal_scalar_cst() {
    let input = "|\n  hello\n  world\n";
    let doc = YamlDoc::parse(input).expect("parser should accept root literal scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(count_nodes(&doc, NodeKind::LiteralScalar), 1);
    let literal = literal_scalar(&doc).expect("literal scalar exists");
    assert_eq!(
        doc.source
            .slice(doc.node(literal).expect("node exists").span),
        input
    );
}

#[test]
fn parser_builds_literal_scalar_mapping_value_cst() {
    let input = "message: |\n  hello\n  world\nnext: value\n";
    let doc = YamlDoc::parse(input).expect("parser should accept literal mapping value");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.node(message).map(|node| node.kind),
        Some(NodeKind::LiteralScalar)
    );
    assert_eq!(
        String::read_yaml(&doc, message).expect("literal reads"),
        "hello\nworld\n"
    );
    assert_eq!(
        doc.get_path(&["next"])
            .expect("lookup succeeds")
            .map(|node| doc.scalar_text(node).expect("scalar")),
        Some("value")
    );
}

#[test]
fn yaml_value_reads_literal_scalar_chomping() {
    let strip = YamlDoc::parse("message: |-\n  hello\n\n").expect("valid strip literal");
    let keep = YamlDoc::parse("message: |+\n  hello\n\n").expect("valid keep literal");
    let strip_node = strip
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");
    let keep_node = keep
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    assert_eq!(
        String::read_yaml(&strip, strip_node).expect("strip reads"),
        "hello"
    );
    assert_eq!(
        String::read_yaml(&keep, keep_node).expect("keep reads"),
        "hello\n\n"
    );
}

#[test]
fn parser_preserves_tab_prefixed_literal_content() {
    let input = "foo: |\n \t\nbar: 1\n";
    let doc = YamlDoc::parse(input).expect("valid literal scalar with tab content");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :foo\n=VAL |\\t\\n\n=VAL :bar\n=VAL :1\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_rejects_invalid_block_scalar_forms() {
    for input in [
        "block: ># comment\n  scalar\n",
        "empty block scalar: >\n \n  \n   \n # comment\n",
        "foo: |\n\t\nbar: 1\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("invalid block scalar should reject");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }
}

#[test]
fn parser_keeps_blank_only_literal_keep_content() {
    let input = "- |+\n\n\n";
    let doc = YamlDoc::parse(input).expect("valid empty keep literal scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL |\\n\\n\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn yaml_value_preserves_literal_whitespace_only_lines() {
    let input = "text: |\n  a\n    \n  b\n";
    let doc = YamlDoc::parse(input).expect("valid literal scalar with whitespace line");
    let text = doc
        .get_path(&["text"])
        .expect("lookup succeeds")
        .expect("text exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        String::read_yaml(&doc, text).expect("literal reads"),
        "a\n  \nb\n"
    );
}

#[test]
fn parser_preserves_trailing_literal_whitespace_only_line() {
    let input = "foo: |\n  x\n   ";
    let doc = YamlDoc::parse(input).expect("valid literal scalar with trailing whitespace line");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :foo\n=VAL |x\\n \\n\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_root_folded_scalar_cst() {
    let input = ">\n  folded\n  line\n";
    let doc = YamlDoc::parse(input).expect("parser should accept root folded scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(count_nodes(&doc, NodeKind::FoldedScalar), 1);
    let folded = folded_scalar(&doc).expect("folded scalar exists");
    assert_eq!(
        doc.source
            .slice(doc.node(folded).expect("node exists").span),
        input
    );
}

#[test]
fn parser_builds_folded_scalar_mapping_value_cst() {
    let input = "message: >\n  folded\n  line\nnext: value\n";
    let doc = YamlDoc::parse(input).expect("parser should accept folded mapping value");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.node(message).map(|node| node.kind),
        Some(NodeKind::FoldedScalar)
    );
    assert_eq!(
        String::read_yaml(&doc, message).expect("folded reads"),
        "folded line\n"
    );
    assert_eq!(
        doc.get_path(&["next"])
            .expect("lookup succeeds")
            .map(|node| doc.scalar_text(node).expect("scalar")),
        Some("value")
    );
}

#[test]
fn yaml_value_reads_folded_scalar_paragraphs_and_more_indented_lines() {
    let doc = YamlDoc::parse("message: >\n  folded\n  line\n\n    literal\n  tail\n")
        .expect("valid folded scalar");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    assert_eq!(
        String::read_yaml(&doc, message).expect("folded reads"),
        "folded line\n\n  literal\ntail\n"
    );
}

#[test]
fn yaml_value_reads_folded_scalar_blank_line_paragraphs() {
    let doc = YamlDoc::parse(">\n  ab\n  cd\n\n  ef\n\n\n  gh\n").expect("valid folded scalar");
    let folded = folded_scalar(&doc).expect("folded scalar exists");

    assert_eq!(
        String::read_yaml(&doc, folded).expect("folded reads"),
        "ab cd\nef\n\ngh\n"
    );
}

#[test]
fn yaml_value_reads_folded_scalar_more_indented_lines() {
    let doc = YamlDoc::parse(">\n  folded\n    * bullet\n\n    * list\n  tail\n")
        .expect("valid folded scalar");
    let folded = folded_scalar(&doc).expect("folded scalar exists");

    assert_eq!(
        String::read_yaml(&doc, folded).expect("folded reads"),
        "folded\n  * bullet\n\n  * list\ntail\n"
    );
}

#[test]
fn parser_keeps_apostrophe_inside_folded_block_scalar_content() {
    let input = "--- >\n  Mark McGwire's\n  year was crippled\n  by a knee injury.\n";
    let doc = YamlDoc::parse(input).expect("valid apostrophe in folded scalar content");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL >Mark McGwire's year was crippled by a knee injury.\\n\n-DOC\n-STR\n"
    );
}

#[test]
fn yaml_value_reads_folded_scalar_chomping() {
    let strip = YamlDoc::parse("message: >-\n  folded\n  line\n\n").expect("valid strip folded");
    let keep = YamlDoc::parse("message: >+\n  folded\n  line\n\n").expect("valid keep folded");
    let strip_node = strip
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");
    let keep_node = keep
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    assert_eq!(
        String::read_yaml(&strip, strip_node).expect("strip reads"),
        "folded line"
    );
    assert_eq!(
        String::read_yaml(&keep, keep_node).expect("keep reads"),
        "folded line\n\n"
    );
}

#[test]
fn parser_respects_explicit_block_scalar_indentation() {
    let input = "- aaa: |2\n    xxx\n  bbb: |\n    xxx\n";
    let doc = YamlDoc::parse(input).expect("valid explicit indentation scalar");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :aaa\n=VAL |xxx\\n\n=VAL :bbb\n=VAL |xxx\\n\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_tab_prefixed_block_scalar_content() {
    let input = "block: |\n  text\n   \tlines\n";
    let doc = YamlDoc::parse(input).expect("valid tab content in block scalar");
    let block = doc
        .get_path(&["block"])
        .expect("lookup succeeds")
        .expect("block exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        String::read_yaml(&doc, block).expect("literal reads"),
        "text\n \tlines\n"
    );
}

#[test]
fn parser_keeps_empty_block_scalars_from_consuming_siblings() {
    let input = "strip: >-\n\nclip: >\n\nkeep: |+\n";
    let doc = YamlDoc::parse(input).expect("valid empty block scalars");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :strip\n=VAL >\n=VAL :clip\n=VAL >\n=VAL :keep\n=VAL |\\n\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_literal_scalar_inside_block_sequence_entry() {
    let input = "- |\n  hello\n  # content comment\n- next\n";
    let doc = YamlDoc::parse(input).expect("parser should accept literal sequence value");
    let literal = literal_scalar(&doc).expect("literal scalar exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        String::read_yaml(&doc, literal).expect("literal reads"),
        "hello\n# content comment\n"
    );
    assert_eq!(count_nodes(&doc, NodeKind::SequenceEntry), 2);
}

#[test]
fn parser_builds_folded_scalar_inside_block_sequence_entry() {
    let input = "- >\n  folded\n  line\n- next\n";
    let doc = YamlDoc::parse(input).expect("parser should accept folded sequence value");
    let folded = folded_scalar(&doc).expect("folded scalar exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        String::read_yaml(&doc, folded).expect("folded reads"),
        "folded line\n"
    );
    assert_eq!(count_nodes(&doc, NodeKind::SequenceEntry), 2);
}

#[test]
fn parser_reports_invalid_literal_scalar_headers() {
    for input in ["message: |bad\n", "message: |0\n", "message: |--\n"] {
        let error = YamlDoc::parse(input).expect_err("literal header should be rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert!(
            error
                .diagnostic
                .message
                .starts_with("invalid block scalar header before")
        );
        assert!(!error.diagnostic.expected.is_empty());
        assert!(error.diagnostic.position.is_some());
    }
}

#[test]
fn parser_reports_invalid_folded_scalar_headers() {
    for input in ["message: >bad\n", "message: >0\n", "message: >--\n"] {
        let error = YamlDoc::parse(input).expect_err("folded header should be rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert!(
            error
                .diagnostic
                .message
                .starts_with("invalid block scalar header before")
        );
        assert!(!error.diagnostic.expected.is_empty());
        assert!(error.diagnostic.position.is_some());
    }
}

#[test]
fn yaml_value_writes_literal_scalar_values() {
    let mut doc = YamlDoc::parse("message: |\n  hello\n").expect("valid literal scalar");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    "updated"
        .to_owned()
        .write_yaml(&mut doc, Some(message))
        .expect("literal scalar writes");

    assert_eq!(doc.to_string(), "message: |\n  updated\n");
}

#[test]
fn yaml_value_writes_folded_scalar_values() {
    let mut doc = YamlDoc::parse("message: >\n  hello\n").expect("valid folded scalar");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    "updated"
        .to_owned()
        .write_yaml(&mut doc, Some(message))
        .expect("folded scalar writes");

    assert_eq!(doc.to_string(), "message: >\n  updated\n");
}

#[test]
fn parser_builds_flow_sequence_mapping_value_cst() {
    let input = "items: [a, b, c]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow sequence mapping value");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");
    let sequence = doc.node(items).expect("items node exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(sequence.kind, NodeKind::FlowSequence);
    assert_eq!(sequence.span, Span::new(7, 16));
    assert_eq!(flow_sequence_scalar_texts(&doc, items), ["a", "b", "c"]);
}

#[test]
fn parser_builds_flow_sequence_inside_block_sequence_entry() {
    let input = "- [one, two,]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow sequence entry value");
    let flow = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowSequence)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow sequence exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(flow_sequence_scalar_texts(&doc, flow), ["one", "two"]);
}

#[test]
fn parser_builds_nested_root_flow_sequences() {
    let input = "[a, [b, c]]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept nested flow sequences");
    let flow_sequences: Vec<NodeId> = doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == NodeKind::FlowSequence)
        .map(|(index, _)| NodeId::from_usize(index))
        .collect();

    assert_eq!(doc.to_string(), input);
    assert_eq!(flow_sequences.len(), 2);
    assert_eq!(flow_sequence_scalar_texts(&doc, flow_sequences[0]), ["a"]);
    assert_eq!(
        flow_sequence_scalar_texts(&doc, flow_sequences[1]),
        ["b", "c"]
    );
}

#[test]
fn yaml_value_reads_flow_sequence_values() {
    let doc = YamlDoc::parse("items: [one, two]\n").expect("valid flow sequence mapping");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");
    assert_eq!(
        Vec::<String>::read_yaml(&doc, items).expect("flow sequence reads"),
        ["one".to_owned(), "two".to_owned()]
    );
}

#[test]
fn yaml_value_writes_flow_sequence_values() {
    let mut doc = YamlDoc::parse("items: [one, two]\n").expect("valid flow sequence mapping");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");

    vec!["three".to_owned()]
        .write_yaml(&mut doc, Some(items))
        .expect("flow sequence writes");

    assert_eq!(doc.to_string(), "items: [three]\n");
}

#[test]
fn parser_builds_flow_mapping_mapping_value_cst() {
    let input = "settings: {a: b, c: d}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow mapping value");
    let settings = doc
        .get_path(&["settings"])
        .expect("lookup succeeds")
        .expect("settings exists");
    let mapping = doc.node(settings).expect("settings node exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(mapping.kind, NodeKind::FlowMapping);
    assert_eq!(doc.children(settings).count(), 2);
    assert_eq!(
        flow_mapping_scalar_pairs(&doc, settings),
        [("a", "b"), ("c", "d")]
    );
}

#[test]
fn parser_keeps_non_separating_colons_in_flow_plain_scalars() {
    let input = "{url: http://foo.com, empty:, key: value:with:colons}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow plain colons");
    let mapping = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow mapping exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_mapping_scalar_pairs(&doc, mapping),
        [
            ("url", "http://foo.com"),
            ("empty", ""),
            ("key", "value:with:colons")
        ]
    );
}

#[test]
fn parser_keeps_non_separating_colons_in_block_plain_scalars() {
    let input =
        "items:\n  - ::vector\n  - http://example.com/foo#bar\nkey ends with two colons::: value\n";
    let doc = YamlDoc::parse(input).expect("parser should accept block plain colons");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :items\n+SEQ\n=VAL :::vector\n=VAL :http://example.com/foo#bar\n-SEQ\n=VAL :key ends with two colons::\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_key_only_flow_mapping_entry_with_colon_text() {
    let input = "{http://foo.com, omitted value:}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept key-only flow entry");
    let mapping = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow mapping exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_mapping_scalar_pairs(&doc, mapping),
        [("http://foo.com", ""), ("omitted value", "")]
    );
}

#[test]
fn parser_accepts_quoted_flow_keys_without_separator_space() {
    let input = "{ \"key\":value, \"key\"::value }\n";
    let doc = YamlDoc::parse(input).expect("parser should accept quoted flow keys");
    let mapping = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow mapping exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_mapping_scalar_pairs(&doc, mapping),
        [("\"key\"", "value"), ("\"key\"", ":value")]
    );
}

#[test]
fn parser_builds_flow_mapping_inside_block_sequence_entry() {
    let input = "- {a: b}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow mapping entry value");
    let flow = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow mapping exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(flow_mapping_scalar_pairs(&doc, flow), [("a", "b")]);
}

#[test]
fn parser_builds_nested_flow_mapping_collections() {
    let input = "{a: [b, c], nested: {d: e}}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept nested flow collections");
    let flow_mappings = count_nodes(&doc, NodeKind::FlowMapping);
    let flow_sequences = count_nodes(&doc, NodeKind::FlowSequence);

    assert_eq!(doc.to_string(), input);
    assert_eq!(flow_mappings, 2);
    assert_eq!(flow_sequences, 1);
    assert!(scalar_texts(&doc).contains(&"a"));
    assert!(scalar_texts(&doc).contains(&"nested"));
    assert!(scalar_texts(&doc).contains(&"d"));
    assert!(scalar_texts(&doc).contains(&"e"));
}

#[test]
fn parser_accepts_flow_sequence_comments_between_items() {
    let input = "---\n[ word1\n# comment\n, word2]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow sequence comments");
    let sequence = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowSequence)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow sequence exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_sequence_scalar_texts(&doc, sequence),
        ["word1", "word2"]
    );
}

#[test]
fn parser_accepts_flow_mapping_comment_before_colon() {
    let input = "---\n{ \"foo\" # comment\n  :bar }\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow mapping comment");
    let mapping = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow mapping exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_mapping_scalar_pairs(&doc, mapping),
        [("\"foo\"", "bar")]
    );
}

#[test]
fn parser_accepts_flow_sequence_end_of_line_comments() {
    let input = "flow: [    # Leading spaces\n   By two,        # in flow style\n  Also by two,    # are neither\n  \tStill by two   # content nor\n    ]             # indentation.\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow item comments");
    let flow = doc
        .get_path(&["flow"])
        .expect("lookup succeeds")
        .expect("flow exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_sequence_scalar_texts(&doc, flow),
        ["By two", "Also by two", "Still by two"]
    );
}

#[test]
fn parser_keeps_non_comment_hash_in_flow_plain_scalar() {
    let input = "[http://example.com/foo#bar]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept hash in flow scalar");
    let sequence = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowSequence)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow sequence exists");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        flow_sequence_scalar_texts(&doc, sequence),
        ["http://example.com/foo#bar"]
    );
}

#[test]
fn parser_accepts_multiline_flow_mapping_entries() {
    let sequence_input = "- { multi\n  line, a: b}\n";
    let sequence_doc =
        YamlDoc::parse(sequence_input).expect("parser should accept multiline flow entry");
    let sequence_mapping = sequence_doc
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .expect("flow mapping exists");

    assert_eq!(sequence_doc.to_string(), sequence_input);
    assert_eq!(
        flow_mapping_scalar_pairs(&sequence_doc, sequence_mapping),
        [("multi\n  line", ""), ("a", "b")]
    );

    let mapping_input = "Sammy Sosa: {\n    hr: 63,\n    avg: 0.288\n  }\n";
    let mapping_doc =
        YamlDoc::parse(mapping_input).expect("parser should accept multiline flow mapping");
    let nested_mapping = mapping_doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == NodeKind::FlowMapping)
        .map(|(index, _)| NodeId::from_usize(index))
        .next()
        .expect("flow mapping exists");

    assert_eq!(mapping_doc.to_string(), mapping_input);
    assert_eq!(
        flow_mapping_scalar_pairs(&mapping_doc, nested_mapping),
        [("hr", "63"), ("avg", "0.288")]
    );
}

#[test]
fn parser_accepts_split_flow_mapping_separator_lines() {
    let nested_input = "k: {\n k\n :\n v\n }\n";
    let nested_doc =
        YamlDoc::parse(nested_input).expect("parser should accept split flow separator");

    assert_eq!(nested_doc.to_string(), nested_input);
    assert_eq!(
        nested_doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :k\n+MAP {}\n=VAL :k\n=VAL :v\n-MAP\n-MAP\n-DOC\n-STR\n"
    );

    let root_input = "{ key\n :\n value }\n";
    let root_doc = YamlDoc::parse(root_input).expect("parser should accept root split separator");

    assert_eq!(root_doc.to_string(), root_input);
    assert_eq!(
        root_doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP {}\n=VAL :key\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_flow_collection_key_in_block_mapping() {
    let input = "[flow]: block\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow sequence key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n+SEQ []\n=VAL :flow\n-SEQ\n=VAL :block\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_flow_mapping_key_with_nested_block_value() {
    let input = "{ first: Sammy, last: Sosa }:\n  hr: 65\n  avg: 0.278\n";
    let doc = YamlDoc::parse(input).expect("parser should accept flow mapping key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n+MAP {}\n=VAL :first\n=VAL :Sammy\n=VAL :last\n=VAL :Sosa\n-MAP\n+MAP\n=VAL :hr\n=VAL :65\n=VAL :avg\n=VAL :0.278\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_anchored_flow_sequence_key() {
    let input = "{ &a [a, &b b]: *b, *a : [c, *b, d]}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept anchored flow key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP {}\n+SEQ [] &a\n=VAL :a\n=VAL &b :b\n-SEQ\n=ALI *b\n=ALI *a\n+SEQ []\n=VAL :c\n=ALI *b\n=VAL :d\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_explicit_flow_mapping_entries() {
    let input = "{\n? explicit: entry,\nimplicit: entry,\n?\n}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept explicit flow entries");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP {}\n=VAL :explicit\n=VAL :entry\n=VAL :implicit\n=VAL :entry\n=VAL :\n=VAL :\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_explicit_flow_sequence_mapping_entries() {
    let flow_input = "[\n? foo\n bar : baz\n]\n";
    let flow_doc =
        YamlDoc::parse(flow_input).expect("parser should accept explicit flow seq entry");
    assert_eq!(flow_doc.to_string(), flow_input);
    assert_eq!(
        flow_doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n+MAP {}\n=VAL :foo bar\n=VAL :baz\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );

    let block_input = "- ? : x\n";
    let block_doc =
        YamlDoc::parse(block_input).expect("parser should accept compact explicit entry");
    assert_eq!(block_doc.to_string(), block_input);
    assert_eq!(
        block_doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n+MAP\n=VAL :\n=VAL :x\n-MAP\n=VAL :\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_property_prefixed_root_flow_sequence() {
    let input = "&flowseq [\n a: b,\n &c c: d\n]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept anchored flow sequence");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ [] &flowseq\n+MAP {}\n=VAL :a\n=VAL :b\n-MAP\n+MAP {}\n=VAL &c :c\n=VAL :d\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_split_tag_before_flow_value() {
    let input = "!!map {\n  k: !!seq\n  [ a, !!str b]\n}\n";
    let doc = YamlDoc::parse(input).expect("parser should accept tagged flow value");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP {} <tag:yaml.org,2002:map>\n=VAL :k\n+SEQ [] <tag:yaml.org,2002:seq>\n=VAL :a\n=VAL <tag:yaml.org,2002:str> :b\n-SEQ\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_implicit_flow_mapping_collection_key() {
    let input = "[ {JSON: like}:adjacent ]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept collection key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n+MAP {}\n+MAP {}\n=VAL :JSON\n=VAL :like\n-MAP\n=VAL :adjacent\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_nested_flow_collection_key_in_implicit_mapping() {
    let input = "[[[b,c]]: d, e]\n";
    let doc = YamlDoc::parse(input).expect("parser should accept nested collection key");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n+MAP {}\n+SEQ []\n+SEQ []\n=VAL :b\n=VAL :c\n-SEQ\n-SEQ\n=VAL :d\n-MAP\n=VAL :e\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_empty_implicit_flow_mapping_keys() {
    for (input, value) in [
        ("[ : empty key ]\n", "empty key"),
        ("[: another empty key]\n", "another empty key"),
    ] {
        let doc = YamlDoc::parse(input).expect("parser should accept empty key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            format!(
                "+STR\n+DOC\n+SEQ []\n+MAP {{}}\n=VAL :\n=VAL :{value}\n-MAP\n-SEQ\n-DOC\n-STR\n"
            )
        );
    }
}

#[test]
fn yaml_value_reads_flow_mapping_values() {
    let doc = YamlDoc::parse("settings: {a: b, c: d}\n").expect("valid flow mapping");
    let settings = doc
        .get_path(&["settings"])
        .expect("lookup succeeds")
        .expect("settings exists");
    let values = std::collections::BTreeMap::<String, String>::read_yaml(&doc, settings)
        .expect("flow mapping reads");

    assert_eq!(values.get("a").map(String::as_str), Some("b"));
    assert_eq!(values.get("c").map(String::as_str), Some("d"));
}

#[test]
fn yaml_value_writes_flow_mapping_values() {
    let mut doc = YamlDoc::parse("settings: {a: b}\n").expect("valid flow mapping");
    let settings = doc
        .get_path(&["settings"])
        .expect("lookup succeeds")
        .expect("settings exists");
    let values = std::collections::BTreeMap::from([("a".to_owned(), "updated".to_owned())]);

    values
        .write_yaml(&mut doc, Some(settings))
        .expect("flow mapping writes");

    assert_eq!(doc.to_string(), "settings: {a: updated}\n");
}

#[test]
fn parser_reports_malformed_flow_mappings() {
    for (input, message) in [
        ("settings: {a: b\n", "missing flow mapping closing brace"),
        ("settings: {, a: b}\n", "unexpected comma in flow mapping"),
        (
            "settings: {a: b, , c: d}\n",
            "unexpected comma in flow mapping",
        ),
        (
            "settings: {a b}\n",
            "missing colon after flow mapping key before `}`",
        ),
        ("{a: b} }\n", "unexpected token `}` after flow collection"),
    ] {
        let error = YamlDoc::parse(input).expect_err("input should be rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.message, message);
        assert!(
            !error.diagnostic.expected.is_empty(),
            "{input:?} should report expected items"
        );
        assert!(
            error.diagnostic.position.is_some(),
            "{input:?} should include source position"
        );
    }
}

#[test]
fn parser_rejects_fuzzed_flow_mapping_key_indicator_without_panicking() {
    let input = "&fl\n { &fl\n { &- |-\n  ab\n...\ne e: f },g: h }\n]\n";
    let error = YamlDoc::parse(input).expect_err("malformed flow key should be rejected");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    assert_eq!(
        error.diagnostic.message,
        "block indicator `...` is not allowed as a flow scalar"
    );
    assert!(error.diagnostic.position.is_some());
}

#[test]
fn parser_rejects_invalid_flow_collection_forms() {
    for input in [
        "---\n[ a, b, c, ]#invalid\n",
        "---\n- [-, -]\n",
        "[\n--- ,\n...\n]\n",
        "---\n[\nsequence item\n]\ninvalid item\n",
        "---\n{\n foo: 1\n bar: 2 }\n",
        "k: {\nk\n:\nv\n}\n",
        "[-]\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("invalid flow form should be rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert!(
            error.diagnostic.position.is_some(),
            "{input:?} should include source position"
        );
    }
}

#[test]
fn parser_preserves_valid_nearby_flow_collection_forms() {
    for input in [
        "[http://example.com/foo#bar]\n",
        "[ word1\n# comment\n, word2]\n",
        "{ \"foo\" # comment\n  :bar }\n",
        "Sammy Sosa: {\n    hr: 63,\n    avg: 0.288\n  }\n",
    ] {
        let doc = YamlDoc::parse(input).expect("valid flow form should still parse");

        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn parser_reports_malformed_flow_sequences() {
    for (input, message) in [
        ("items: [a, b\n", "missing flow sequence closing bracket"),
        ("items: [a, , b]\n", "unexpected comma in flow sequence"),
        ("items: [, a]\n", "unexpected comma in flow sequence"),
        ("[a] ]\n", "unexpected token `]` after flow collection"),
    ] {
        let error = YamlDoc::parse(input).expect_err("input should be rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.message, message);
        assert!(
            !error.diagnostic.expected.is_empty(),
            "{input:?} should report expected items"
        );
        assert!(
            error.diagnostic.position.is_some(),
            "{input:?} should include source position"
        );
    }
}

#[test]
fn parser_rejects_tabs_used_as_block_indicator_separation() {
    for input in ["-\t-\n", "?\t-\n", "?\tkey:\n", "? key:\n:\tkey:\n"] {
        let error = YamlDoc::parse(input).expect_err("tab must not enable block structure");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }
}

#[test]
fn parser_rejects_tabs_enabling_nested_block_structure() {
    for input in ["- \t-\n", "- [\n\tfoo,\n foo\n ]\n"] {
        let error = YamlDoc::parse(input).expect_err("tab must not enable nested structure");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }
}

#[test]
fn parser_preserves_tab_after_sequence_indicator_as_scalar_content() {
    let input = "-\t-1\n";
    let doc = YamlDoc::parse(input).expect("tab before plain scalar should be accepted");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL :-1\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_indented_root_block_sequence() {
    let input = " - !!str a\n - b\n - !!int 42\n - d\n";
    let doc = YamlDoc::parse(input).expect("valid indented root sequence");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n=VAL <tag:yaml.org,2002:str> :a\n=VAL :b\n=VAL <tag:yaml.org,2002:int> :42\n=VAL :d\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_indented_root_flow_sequence() {
    let input = "  [1, 2, 3]\n";
    let doc = YamlDoc::parse(input).expect("valid indented root flow sequence");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ []\n=VAL :1\n=VAL :2\n=VAL :3\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_accepts_nested_mappings_in_indented_root_sequence() {
    let input = " - key: value\n   key2: value2\n -\n   key3: value3\n";
    let doc = YamlDoc::parse(input).expect("valid indented root sequence mappings");

    assert_eq!(doc.to_string(), input);
    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :key\n=VAL :value\n=VAL :key2\n=VAL :value2\n-MAP\n+MAP\n=VAL :key3\n=VAL :value3\n-MAP\n-SEQ\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_folds_root_plain_scalar_continuations() {
    for (input, expected) in [
        (
            "---\nk:#foo\n &a !t s\n",
            "+STR\n+DOC ---\n=VAL :k:#foo &a !t s\n-DOC\n-STR\n",
        ),
        (
            "---\nscalar\n%YAML 1.2\n",
            "+STR\n+DOC ---\n=VAL :scalar %YAML 1.2\n-DOC\n-STR\n",
        ),
        (
            "Bare\ndocument\n...\n|\n%!PS-Adobe-2.0 # Not the first line\n",
            "+STR\n+DOC\n=VAL :Bare document\n-DOC ...\n+DOC\n=VAL |%!PS-Adobe-2.0 # Not the first line\\n\n-DOC\n-STR\n",
        ),
    ] {
        let doc = YamlDoc::parse(input).expect("valid root plain scalar continuation");

        assert_eq!(doc.to_string(), input);
        assert_eq!(doc.events_to_test_string(), expected);
    }
}

#[test]
fn parser_rejects_invalid_compact_block_collection_values() {
    for input in [
        "key: - a\n     - b\n",
        "--- &anchor a: b\n",
        "key:\n - bar\n - baz\n invalid\n",
        "---\nflow: [a,\nb,\nc]\n",
        "---\n[ key\n  : value ]\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("compact block syntax is invalid");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }
}

#[test]
fn parser_rejects_invalid_scalar_termination_and_orphaned_block_content() {
    for input in [
        "this\n is\n  invalid: x\n",
        "- item1\n- item2\ninvalid\n",
        "k1: v1\n k2: v2\n",
        "word1  # comment\nword2\n",
        "key:\n  word1 word2\n  no: key\n",
        "key2: \"quoted2\" trailing content\n",
        "key: \"value\"# invalid comment\n",
        "a: b: c: d\n",
        "a: 'b': c\n",
    ] {
        let error = YamlDoc::parse(input).expect_err("invalid scalar syntax is rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser, "{input:?}");
    }
}

#[test]
fn parser_preserves_valid_scalar_termination_neighbors() {
    for input in [
        "---\nscalar\n%YAML 1.2\n",
        "key: \"value\" # separated comment\n",
        "url: http://foo.com\n",
        "{key: value:with:colons}\n",
    ] {
        let doc = YamlDoc::parse(input).unwrap_or_else(|error| {
            panic!("nearby valid scalar syntax remains valid for {input:?}: {error:?}")
        });

        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn parser_rejects_directive_followed_by_document_end_without_document() {
    let error =
        YamlDoc::parse("%YAML 1.2\n...\n").expect_err("document end cannot start a document");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
}

#[test]
fn parser_reports_tabs_in_indentation() {
    let error =
        YamlDoc::parse("root:\n\tchild: value\n").expect_err("tabs are invalid indentation");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    assert_eq!(error.diagnostic.span, Span::new(6, 7));
    assert_eq!(
        error.diagnostic.expected,
        ["spaces for indentation".to_owned()]
    );
}

#[test]
fn parser_reports_invalid_indentation_without_parent() {
    let error = YamlDoc::parse(
        "  key: value
",
    )
    .expect_err("indented root line has no parent");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    assert_eq!(error.diagnostic.span, Span::new(0, 2));
    assert_eq!(
        error.diagnostic.position,
        Some(LineCol { line: 1, column: 1 })
    );
    assert!(error.to_string().contains("invalid indentation"));
}

#[test]
fn parser_accepts_empty_block_mapping_key() {
    let doc = YamlDoc::parse(
        ": value
",
    )
    .expect("empty mapping keys are valid YAML");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :\n=VAL :value\n-MAP\n-DOC\n-STR\n"
    );
}

#[test]
fn parser_builds_empty_scalar_values() {
    let doc = YamlDoc::parse(
        "key:
items:
  -
flow: {empty:}
",
    )
    .expect("empty nodes are valid YAML scalars in the accepted subset");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :\n=VAL :items\n+SEQ\n=VAL :\n-SEQ\n=VAL :flow\n+MAP {}\n=VAL :empty\n=VAL :\n-MAP\n-MAP\n-DOC\n-STR\n"
    );
    assert_eq!(
        doc.scalar_value(doc.get_path(&["key"]).expect("lookup").expect("key"))
            .expect("empty mapping scalar reads"),
        ""
    );
    let items = doc
        .get_path(&["items"])
        .expect("lookup")
        .expect("items exists");
    assert_eq!(
        Vec::<String>::read_yaml(&doc, items).expect("empty sequence scalar reads"),
        [String::new()]
    );
    let flow_empty = doc
        .get_path(&["flow", "empty"])
        .expect("lookup")
        .expect("flow empty exists");
    assert_eq!(
        doc.scalar_value(flow_empty)
            .expect("empty flow scalar reads"),
        ""
    );
}

#[test]
fn parser_treats_marker_like_text_as_plain_scalar() {
    for (input, expected) in [
        (
            "---word1\nword2\n",
            "+STR\n+DOC\n=VAL :---word1 word2\n-DOC\n-STR\n",
        ),
        (
            "---\n---word1\nword2\n",
            "+STR\n+DOC ---\n=VAL :---word1 word2\n-DOC\n-STR\n",
        ),
    ] {
        let doc = YamlDoc::parse(input).expect("marker-like text is plain scalar content");

        assert_eq!(doc.events_to_test_string(), expected);
        assert_eq!(doc.to_string(), input);
    }
}

#[test]
fn parser_rejects_compact_document_mapping_with_utf8_key_without_panic() {
    let error =
        YamlDoc::parse("--- ߅foo:").expect_err("malformed compact document mapping is rejected");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
}

#[test]
fn parser_accepts_non_ascii_plain_scalar_after_explicit_document_start() {
    let input = "--- ߅foo\n";
    let doc = YamlDoc::parse(input).expect("non-ASCII plain scalar is valid");

    assert_eq!(
        doc.events_to_test_string(),
        "+STR\n+DOC ---\n=VAL :߅foo\n-DOC\n-STR\n"
    );
    assert_eq!(doc.to_string(), input);
}

#[test]
fn parser_handles_utf8_in_offset_sensitive_positions_without_panicking() {
    for input in [
        "߅foo: bar\n",
        "key: ߅value\n",
        "? ߅foo\n: bar\n",
        "- ߅foo: bar\n",
        "--- [߅]\n",
        "--- {߅: v}\n",
        "--- !tag ߅foo\n",
        "&a ߅foo\n",
    ] {
        let doc = YamlDoc::parse(input).expect("valid UTF-8 scalar content parses");
        let output = doc.to_string();
        let reparsed = YamlDoc::parse(&output).expect("round-tripped UTF-8 YAML reparses");

        assert_eq!(output, input);
        assert_eq!(
            reparsed.events_to_test_string(),
            doc.events_to_test_string()
        );
    }
}

#[test]
fn parser_indexes_unicode_lines_by_byte_offset() {
    let input = "clé: un\r\n次:\n  - 値\n";
    let doc = YamlDoc::parse(input).expect("Unicode keys and mixed line endings parse");
    let first = doc.get_path(&["clé"]).unwrap().unwrap();
    let sequence = doc.get_path(&["次"]).unwrap().unwrap();
    let item = doc.sequence_items(sequence).next().expect("sequence item");

    assert_eq!(doc.scalar_text(first).unwrap(), "un");
    assert_eq!(doc.scalar_text(item).unwrap(), "値");
    assert_eq!(doc.to_string(), input);
}

#[test]
fn parser_rejects_malformed_compact_utf8_document_content_without_panicking() {
    for input in ["--- ߅foo:", "--- \"߅\":", "!%sҦ"] {
        let error = YamlDoc::parse(input)
            .expect_err("malformed compact UTF-8 document mapping is rejected");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }
}

#[test]
fn semantic_lookup_reads_root_mapping_values() {
    let input = "host: localhost
port: 8080
";
    let doc = YamlDoc::parse(input).expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");
    let host = doc
        .get_mapping_value(root, "host")
        .expect("lookup succeeds")
        .expect("host exists");
    let port = doc
        .get_path(&["port"])
        .expect("lookup succeeds")
        .expect("port exists");

    assert_eq!(doc.scalar_text(host).expect("host is scalar"), "localhost");
    assert_eq!(doc.scalar_text(port).expect("port is scalar"), "8080");
    assert_eq!(doc.get_mapping_value(root, "missing"), Ok(None));
}

#[test]
fn semantic_lookup_follows_nested_block_mappings() {
    let input = "server:
  host: localhost
  port: 8080
";
    let doc = YamlDoc::parse(input).expect("valid nested MVP mapping");
    let host = doc
        .get_path(&["server", "host"])
        .expect("lookup succeeds")
        .expect("nested host exists");
    let port = doc
        .get_path(&["server", "port"])
        .expect("lookup succeeds")
        .expect("nested port exists");

    assert_eq!(doc.scalar_text(host).expect("host is scalar"), "localhost");
    assert_eq!(doc.scalar_text(port).expect("port is scalar"), "8080");
    assert_eq!(doc.get_path(&["server", "missing"]), Ok(None));
}

#[test]
fn semantic_lookup_can_return_nested_sequences() {
    let input = "ports:
  - 8080
  - 9090
";
    let doc = YamlDoc::parse(input).expect("valid nested MVP sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("lookup succeeds")
        .expect("ports exists");

    assert_eq!(
        doc.node(ports).map(|node| node.kind),
        Some(NodeKind::BlockSequence)
    );
}

#[test]
fn document_selection_counts_stream_documents() {
    assert_eq!(
        YamlDoc::parse("").expect("empty stream").document_count(),
        0
    );
    assert_eq!(
        YamlDoc::parse("host: localhost\n")
            .expect("single document")
            .document_count(),
        1
    );
    assert_eq!(
        YamlDoc::parse("---\nname: first\n---\nname: second\n")
            .expect("multi document")
            .document_count(),
        2
    );
}

#[test]
fn document_selection_reads_second_document_mapping() {
    let doc = YamlDoc::parse("---\nname: first\n---\nname: second\n")
        .expect("valid multi-document stream");
    let second_root = doc
        .document_root_mapping(1)
        .expect("second document root mapping exists");
    let second_name = doc
        .get_mapping_value(second_root, "name")
        .expect("mapping lookup succeeds")
        .expect("second name exists");
    let path_name = doc
        .get_path_in_document(1, &["name"])
        .expect("path lookup succeeds")
        .expect("path name exists");

    assert_eq!(doc.scalar_text(second_name).expect("name scalar"), "second");
    assert_eq!(doc.scalar_text(path_name).expect("name scalar"), "second");
    assert_eq!(
        doc.scalar_text(
            doc.get_path(&["name"])
                .expect("first path lookup succeeds")
                .expect("first name exists")
        )
        .expect("first name scalar"),
        "first"
    );
}

#[test]
fn document_selection_reports_out_of_range_indexes() {
    let doc = YamlDoc::parse("---\nname: first\n").expect("valid document");
    let error = doc
        .document_root_mapping(1)
        .expect_err("second document does not exist");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Semantic);
    assert_eq!(error.diagnostic.expected, ["an existing document index"]);
}

#[test]
fn document_selection_edit_survives_commit() {
    let mut doc = YamlDoc::parse("---\nname: first\n---\nname: second\n")
        .expect("valid multi-document stream");
    let second_name = doc
        .get_path_in_document(1, &["name"])
        .expect("second path lookup succeeds")
        .expect("second name exists");

    "updated"
        .to_owned()
        .write_yaml(&mut doc, Some(second_name))
        .expect("second document edit queues");
    doc.commit_edits().expect("edited stream reparses");

    let first = doc
        .get_path_in_document(0, &["name"])
        .expect("first path lookup succeeds")
        .expect("first name exists");
    let second = doc
        .get_path_in_document(1, &["name"])
        .expect("second path lookup succeeds")
        .expect("second name exists");

    assert_eq!(doc.scalar_text(first).expect("first name"), "first");
    assert_eq!(doc.scalar_text(second).expect("second name"), "updated");
}

#[test]
fn document_append_empty_mapping_to_empty_stream() {
    let mut doc = YamlDoc::parse("").expect("empty stream parses");

    doc.append_empty_mapping_document()
        .expect("empty mapping document append queues");

    assert_eq!(doc.to_string(), "---\n{}\n");
    assert_eq!(doc.document_count(), 0);

    doc.commit_edits().expect("appended stream reparses");
    assert_eq!(doc.document_count(), 1);
}

#[test]
fn document_append_mapping_is_visible_after_commit() {
    let mut doc = YamlDoc::parse("name: first\n").expect("valid document");
    let second = std::collections::BTreeMap::from([("name".to_owned(), "second".to_owned())]);

    doc.append_document(&second)
        .expect("mapping document append queues");

    assert_eq!(doc.document_count(), 1);
    assert_eq!(doc.to_string(), "name: first\n---\nname: second\n");

    doc.commit_edits().expect("appended mapping reparses");
    assert_eq!(doc.document_count(), 2);
    let name = doc
        .get_path_in_document(1, &["name"])
        .expect("second document lookup succeeds")
        .expect("second name exists");
    assert_eq!(doc.scalar_text(name).expect("name scalar"), "second");
}

#[test]
fn document_append_after_explicit_end_marker() {
    let mut doc = YamlDoc::parse("---\nname: first\n...\n").expect("valid ended stream");
    let second = std::collections::BTreeMap::from([("name".to_owned(), "second".to_owned())]);

    doc.append_document(&second)
        .expect("append after end marker queues");

    assert_eq!(
        doc.to_string(),
        "---\nname: first\n...\n---\nname: second\n"
    );
    doc.commit_edits().expect("stream reparses");
    assert_eq!(doc.document_count(), 2);
}

#[test]
fn document_append_adds_separator_after_missing_final_newline() {
    let mut doc = YamlDoc::parse("name: first").expect("valid document");
    let second = std::collections::BTreeMap::from([("name".to_owned(), "second".to_owned())]);

    doc.append_document(&second)
        .expect("append after no trailing newline queues");

    assert_eq!(doc.to_string(), "name: first\n---\nname: second\n");
    YamlDoc::parse(&doc.to_string()).expect("preview reparses");
}

#[test]
fn document_append_nested_collection_document() {
    let mut doc = YamlDoc::parse("name: first\n").expect("valid document");
    let matrix =
        std::collections::BTreeMap::from([("matrix".to_owned(), vec![vec![1_u16, 2], vec![3]])]);

    doc.append_document(&matrix)
        .expect("nested collection document append queues");
    doc.commit_edits().expect("nested document reparses");

    let matrix_node = doc
        .get_path_in_document(1, &["matrix"])
        .expect("second document lookup succeeds")
        .expect("matrix exists");
    assert_eq!(
        Vec::<Vec<u16>>::read_yaml(&doc, matrix_node).expect("matrix reads"),
        vec![vec![1, 2], vec![3]]
    );
}

#[test]
fn patch_writer_replaces_scalar_node_text() {
    let mut doc = YamlDoc::parse(
        "host: localhost
port: 8080
",
    )
    .expect("valid MVP mapping");
    let port = doc
        .get_path(&["port"])
        .expect("lookup succeeds")
        .expect("port exists");

    doc.replace_node_text(port, "9090")
        .expect("replacement edit queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost
port: 9090
"
    );
    assert_eq!(doc.scalar_text(port).expect("CST is unchanged"), "8080");
}

#[test]
fn commit_edits_reparses_and_clears_pending_edits() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");

    doc.insert_mapping_entry(root, "port", "8080", MappingEntryStyle::default())
        .expect("insert queues");
    assert!(doc.get_path(&["port"]).expect("old graph lookup").is_none());

    doc.commit_edits().expect("edited YAML reparses");

    assert!(doc.edits.is_empty());
    let port = doc
        .get_path(&["port"])
        .expect("new graph lookup")
        .expect("port exists after commit");
    assert_eq!(doc.scalar_text(port).expect("port scalar"), "8080");
    assert_eq!(doc.to_string(), "host: localhost\nport: 8080\n");
}

#[test]
fn commit_edits_returns_parse_error_without_replacing_document() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");

    doc.replace_node_text(root, "host: [\n")
        .expect("invalid replacement still queues");
    let error = doc
        .commit_edits()
        .expect_err("invalid edited YAML should not commit");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    assert_eq!(doc.as_source(), "host: localhost\n");
    assert_eq!(doc.to_string(), "host: [\n\n");
    assert_eq!(doc.edits.len(), 1);
    assert!(
        doc.get_path(&["host"])
            .expect("old graph still works")
            .is_some()
    );
}

#[test]
fn patch_writer_inserts_mapping_entry_with_inherited_style() {
    let mut doc = YamlDoc::parse(
        "server:
  host: localhost
other: keep
",
    )
    .expect("valid nested MVP mapping");
    let server = doc
        .get_path(&["server"])
        .expect("lookup succeeds")
        .expect("server mapping exists");

    doc.insert_mapping_entry(server, "port", "8080", MappingEntryStyle::Inherit)
        .expect("insert edit queues");

    assert_eq!(
        doc.to_string(),
        "server:
  host: localhost
  port: 8080
other: keep
"
    );
}

#[test]
fn patch_writer_inserts_after_final_line_without_line_break() {
    let mut doc = YamlDoc::parse("host: localhost").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");

    doc.insert_mapping_entry(root, "port", "8080", MappingEntryStyle::default())
        .expect("insert edit queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost
port: 8080
"
    );
}

#[test]
fn patch_writer_inserts_block_sequence_value() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");

    doc.insert_mapping_value_with_comment(
        root,
        "ports",
        &vec![8080_u16, 9090],
        MappingEntryStyle::default(),
        None,
    )
    .expect("sequence insert queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nports:\n  - 8080\n  - 9090\n"
    );
    YamlDoc::parse(&doc.to_string()).expect("inserted sequence reparses");
}

#[test]
fn patch_writer_inserts_nested_block_sequence_values() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");
    let matrix = vec![vec![1_u16, 2], vec![3]];

    doc.insert_mapping_value_with_comment(
        root,
        "matrix",
        &matrix,
        MappingEntryStyle::default(),
        None,
    )
    .expect("nested sequence insert queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nmatrix:\n  -\n    - 1\n    - 2\n  -\n    - 3\n"
    );
    let reparsed = YamlDoc::parse(&doc.to_string()).expect("nested sequence reparses");
    let matrix_node = reparsed
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");
    assert_eq!(
        Vec::<Vec<u16>>::read_yaml(&reparsed, matrix_node).expect("matrix reads"),
        matrix
    );
}

#[test]
fn patch_writer_inserts_sequence_of_block_mappings() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");
    let limits = vec![
        std::collections::BTreeMap::from([("high".to_owned(), 5_u16)]),
        std::collections::BTreeMap::from([("low".to_owned(), 1_u16)]),
    ];

    doc.insert_mapping_value_with_comment(
        root,
        "limits",
        &limits,
        MappingEntryStyle::default(),
        None,
    )
    .expect("sequence of mappings insert queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nlimits:\n  -\n    high: 5\n  -\n    low: 1\n"
    );
    let reparsed = YamlDoc::parse(&doc.to_string()).expect("nested mappings reparse");
    let limits_node = reparsed
        .get_path(&["limits"])
        .expect("lookup succeeds")
        .expect("limits exists");
    assert_eq!(
        Vec::<std::collections::BTreeMap<String, u16>>::read_yaml(&reparsed, limits_node)
            .expect("limits reads"),
        limits
    );
}

#[test]
fn patch_writer_inserts_block_mapping_value() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");
    let limits =
        std::collections::BTreeMap::from([("high".to_owned(), 5_u16), ("low".to_owned(), 1_u16)]);

    doc.insert_mapping_value_with_comment(
        root,
        "limits",
        &limits,
        MappingEntryStyle::default(),
        None,
    )
    .expect("mapping insert queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nlimits:\n  high: 5\n  low: 1\n"
    );
    YamlDoc::parse(&doc.to_string()).expect("inserted mapping reparses");
}

#[test]
fn patch_writer_inserts_mapping_with_nested_sequence_values() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");
    let routes = std::collections::BTreeMap::from([
        ("primary".to_owned(), vec![80_u16, 443]),
        ("secondary".to_owned(), vec![8080_u16]),
    ]);

    doc.insert_mapping_value_with_comment(
        root,
        "routes",
        &routes,
        MappingEntryStyle::default(),
        None,
    )
    .expect("mapping with nested sequences insert queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nroutes:\n  primary:\n    - 80\n    - 443\n  secondary:\n    - 8080\n"
    );
    let reparsed = YamlDoc::parse(&doc.to_string()).expect("nested map reparses");
    let routes_node = reparsed
        .get_path(&["routes"])
        .expect("lookup succeeds")
        .expect("routes exists");
    assert_eq!(
        std::collections::BTreeMap::<String, Vec<u16>>::read_yaml(&reparsed, routes_node)
            .expect("routes reads"),
        routes
    );
}

#[test]
fn patch_writer_inserts_empty_collection_values() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");
    let empty_map = std::collections::BTreeMap::<String, u16>::new();

    doc.insert_mapping_value_with_comment(
        root,
        "ports",
        &Vec::<u16>::new(),
        MappingEntryStyle::default(),
        None,
    )
    .expect("empty sequence insert queues");
    doc.insert_mapping_value_with_comment(
        root,
        "limits",
        &empty_map,
        MappingEntryStyle::default(),
        None,
    )
    .expect("empty mapping insert queues");

    assert_eq!(doc.to_string(), "host: localhost\nports: []\nlimits: {}\n");
    YamlDoc::parse(&doc.to_string()).expect("empty collections reparse");
}

#[test]
fn patch_writer_rejects_invalid_plain_collection_fragments() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");

    let error = doc
        .insert_mapping_value_with_comment(
            root,
            "names",
            &vec!["bad\nvalue".to_owned()],
            MappingEntryStyle::default(),
            None,
        )
        .expect_err("multi-line plain sequence value should reject");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert!(error.diagnostic.message.contains("single-line plain text"));
    assert_eq!(doc.to_string(), "host: localhost\n");
}

#[test]
fn patch_writer_removes_mapping_entry_line() {
    let mut doc = YamlDoc::parse(
        "host: localhost
port: 8080
extra: keep
",
    )
    .expect("valid MVP mapping");
    let port_entry = mapping_entry_by_key(&doc, "port").expect("port entry exists");

    doc.remove_node(port_entry).expect("remove edit queues");

    assert_eq!(
        doc.to_string(),
        "host: localhost
extra: keep
"
    );
}

#[test]
fn patch_writer_retains_only_allowed_mapping_entries() {
    let mut doc = YamlDoc::parse(
        "host: localhost
port: 8080
extra: remove
debug: false
",
    )
    .expect("valid MVP mapping");
    let root = doc.root_mapping().expect("root mapping exists");

    doc.retain_mapping_entries(root, &["host", "debug"])
        .expect("retain edits queue");

    assert_eq!(
        doc.to_string(),
        "host: localhost
debug: false
"
    );
}

#[test]
fn set_scalar_preserves_double_quoted_style_and_inline_comment() {
    let mut doc =
        YamlDoc::parse("# leading comment\nname: \"old\" # keep me\n").expect("valid MVP mapping");

    doc.set_scalar(&["name"], "new \"value\"")
        .expect("scalar replacement queues");

    assert_eq!(
        doc.to_string(),
        "# leading comment\nname: \"new \\\"value\\\"\" # keep me\n"
    );
}

#[test]
fn set_scalar_preserves_single_quoted_style() {
    let mut doc = YamlDoc::parse("name: 'old'\n").expect("valid MVP mapping");

    doc.set_scalar(&["name"], "Bob's")
        .expect("scalar replacement queues");

    assert_eq!(doc.to_string(), "name: 'Bob''s'\n");
}

#[test]
fn set_scalar_preserves_plain_style() {
    let mut doc = YamlDoc::parse("port: 8080\n").expect("valid MVP mapping");

    doc.set_scalar(&["port"], "9090")
        .expect("scalar replacement queues");

    assert_eq!(doc.to_string(), "port: 9090\n");
}

#[test]
fn set_scalar_preserves_plain_inline_comment() {
    let mut doc = YamlDoc::parse("port: 8080 # chosen port\n").expect("valid MVP mapping");

    doc.set_scalar(&["port"], "9090")
        .expect("scalar replacement queues");

    assert_eq!(doc.to_string(), "port: 9090 # chosen port\n");
}

#[test]
fn string_write_rewrites_literal_block_scalar() {
    let mut doc = YamlDoc::parse("message: |\n  old\n").expect("valid literal scalar");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    "new\ntext"
        .to_owned()
        .write_yaml(&mut doc, Some(message))
        .expect("literal scalar rewrite queues");

    assert_eq!(doc.to_string(), "message: |\n  new\n  text\n");
}

#[test]
fn string_write_rewrites_folded_block_scalar() {
    let mut doc = YamlDoc::parse("message: >\n  old\n").expect("valid folded scalar");
    let message = doc
        .get_path(&["message"])
        .expect("lookup succeeds")
        .expect("message exists");

    "new text"
        .to_owned()
        .write_yaml(&mut doc, Some(message))
        .expect("folded scalar rewrite queues");

    assert_eq!(doc.to_string(), "message: >\n  new text\n");
}

#[test]
fn set_scalar_rejects_plain_replacement_that_would_change_style() {
    let mut doc = YamlDoc::parse("name: old\n").expect("valid MVP mapping");

    let error = doc
        .set_scalar(&["name"], "new value # comment-like")
        .expect_err("plain style cannot safely preserve this value");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert_eq!(
        error.diagnostic.message,
        "plain scalar replacement cannot preserve plain style"
    );
}

#[test]
fn patch_writer_rejects_overlapping_edits() {
    let mut doc = YamlDoc::parse(
        "host: localhost
",
    )
    .expect("valid MVP mapping");
    let host = doc
        .get_path(&["host"])
        .expect("lookup succeeds")
        .expect("host exists");

    doc.replace_node_text(host, "example.com")
        .expect("first replacement queues");
    let error = doc
        .replace_node_text(host, "localhost.local")
        .expect_err("same span overlaps pending edit");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert_eq!(
        error.diagnostic.message,
        "edit overlaps an existing pending edit"
    );
}

fn mapping_entry_by_key(doc: &YamlDoc, key: &str) -> Option<NodeId> {
    let root = doc.root_mapping().ok()?;
    doc.children(root).find(|entry| {
        let Some(_) = doc.node(*entry) else {
            return false;
        };
        let Some(key_node) = doc.children(*entry).next() else {
            return false;
        };
        doc.scalar_text(key_node) == Ok(key)
    })
}

fn count_nodes(doc: &YamlDoc, kind: NodeKind) -> usize {
    doc.nodes.iter().filter(|node| node.kind == kind).count()
}

fn scalar_texts(doc: &YamlDoc) -> Vec<&str> {
    doc.nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Scalar)
        .map(|node| doc.source.slice(node.span))
        .collect()
}

fn literal_scalar(doc: &YamlDoc) -> Option<NodeId> {
    doc.nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::LiteralScalar)
        .map(|(index, _)| NodeId::from_usize(index))
}

fn folded_scalar(doc: &YamlDoc) -> Option<NodeId> {
    doc.nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.kind == NodeKind::FoldedScalar)
        .map(|(index, _)| NodeId::from_usize(index))
}

fn flow_sequence_scalar_texts(doc: &YamlDoc, sequence: NodeId) -> Vec<&str> {
    doc.sequence_items(sequence)
        .filter_map(|item| {
            let item = doc.node(item)?;
            (item.kind == NodeKind::Scalar).then(|| doc.source.slice(item.span))
        })
        .collect()
}

fn flow_mapping_scalar_pairs(doc: &YamlDoc, mapping: NodeId) -> Vec<(&str, &str)> {
    doc.children(mapping)
        .filter_map(|entry| {
            let mut children = doc.children(entry);
            let key = children.next()?;
            let value = children.next()?;
            Some((doc.scalar_text(key).ok()?, doc.scalar_text(value).ok()?))
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct Config {
    host: String,
    port: u16,
    debug: bool,
}

impl FromYamlDoc for Config {
    fn from_yaml_doc(doc: &YamlDoc) -> Result<Self, YamlError> {
        let host = doc.get_path(&["host"])?.ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    "missing required field `host`",
                    Span::empty(0),
                )
                .with_expected("host"),
            )
        })?;
        let port = doc.get_path(&["port"])?;
        let debug = doc.get_path(&["debug"])?;

        Ok(Self {
            host: String::read_yaml(doc, host)?,
            port: match port {
                Some(node) => u16::read_yaml(doc, node)?,
                None => 8080,
            },
            debug: match debug {
                Some(node) => bool::read_yaml(doc, node)?,
                None => false,
            },
        })
    }
}

impl ToYamlDoc for Config {
    fn apply_to_yaml_doc(&self, doc: &mut YamlDoc) -> Result<(), YamlError> {
        let root = doc.root_mapping()?;

        if let Some(host) = doc.get_path(&["host"])? {
            self.host.write_yaml(doc, Some(host))?;
        } else {
            doc.insert_mapping_entry(root, "host", &self.host, MappingEntryStyle::default())?;
        }

        if let Some(port) = doc.get_path(&["port"])? {
            self.port.write_yaml(doc, Some(port))?;
        } else {
            doc.insert_mapping_entry(
                root,
                "port",
                &self.port.to_string(),
                MappingEntryStyle::default(),
            )?;
        }

        if let Some(debug) = doc.get_path(&["debug"])? {
            self.debug.write_yaml(doc, Some(debug))?;
        } else {
            doc.insert_mapping_entry(
                root,
                "debug",
                if self.debug { "true" } else { "false" },
                MappingEntryStyle::default(),
            )?;
        }

        Ok(())
    }
}

#[test]
fn yaml_value_reads_and_writes_scalar_values() {
    let mut doc =
        YamlDoc::parse("name: \"old\"\nenabled: false\nport: 3000\n").expect("valid MVP mapping");
    let name = doc
        .get_path(&["name"])
        .expect("lookup succeeds")
        .expect("name exists");
    let enabled = doc
        .get_path(&["enabled"])
        .expect("lookup succeeds")
        .expect("enabled exists");
    let port = doc
        .get_path(&["port"])
        .expect("lookup succeeds")
        .expect("port exists");

    assert_eq!(String::read_yaml(&doc, name).expect("string reads"), "old");
    assert!(!bool::read_yaml(&doc, enabled).expect("bool reads"));
    assert_eq!(u16::read_yaml(&doc, port).expect("u16 reads"), 3000);

    true.write_yaml(&mut doc, Some(enabled))
        .expect("bool writes");
    9090_u16
        .write_yaml(&mut doc, Some(port))
        .expect("u16 writes");

    assert_eq!(
        doc.to_string(),
        "name: \"old\"\nenabled: true\nport: 9090\n"
    );
}

#[test]
fn yaml_value_reads_and_writes_option_values() {
    let mut doc = YamlDoc::parse(
        "name: old
maybe: value
keep: yes
",
    )
    .expect("valid MVP mapping");
    let name = doc
        .get_path(&["name"])
        .expect("lookup succeeds")
        .expect("name exists");
    let maybe = doc
        .get_path(&["maybe"])
        .expect("lookup succeeds")
        .expect("maybe exists");

    assert_eq!(
        Option::<String>::read_yaml(&doc, name).expect("option reads"),
        Some("old".to_owned())
    );

    Option::<String>::None
        .write_yaml(&mut doc, Some(maybe))
        .expect("none removes containing entry");

    assert_eq!(
        doc.to_string(),
        "name: old
keep: yes
"
    );
}

#[test]
fn yaml_value_reads_and_writes_vec_values() {
    let mut doc = YamlDoc::parse(
        "ports:
  - 8080
  - 9090
",
    )
    .expect("valid MVP sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("lookup succeeds")
        .expect("ports exists");

    assert_eq!(
        Vec::<u16>::read_yaml(&doc, ports).expect("vec reads"),
        vec![8080, 9090]
    );

    vec![3000_u16, 3001]
        .write_yaml(&mut doc, Some(ports))
        .expect("vec writes existing sequence");

    assert_eq!(
        doc.to_string(),
        "ports:
  - 3000
  - 3001
"
    );
}

#[test]
fn yaml_value_patches_same_length_block_sequence_items() {
    let mut doc = YamlDoc::parse(
        "ports:
  - 8080 # first
  - 9090 # second
",
    )
    .expect("valid sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("lookup succeeds")
        .expect("ports exists");

    vec![3000_u16, 3001]
        .write_yaml(&mut doc, Some(ports))
        .expect("vec patches existing sequence items");

    assert_eq!(
        doc.to_string(),
        "ports:
  - 3000 # first
  - 3001 # second
"
    );
}

#[test]
fn yaml_value_shrinks_block_sequences_by_removing_tail_entries() {
    let mut doc = YamlDoc::parse(
        "ports:
  - 8080 # first
  - 9090 # second
",
    )
    .expect("valid sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("lookup succeeds")
        .expect("ports exists");

    vec![3000_u16]
        .write_yaml(&mut doc, Some(ports))
        .expect("vec patches prefix and removes tail");

    assert_eq!(doc.to_string(), "ports:\n  - 3000 # first\n");
}

#[test]
fn yaml_value_grows_block_sequences_by_appending_tail_entries() {
    let mut doc = YamlDoc::parse(
        "ports:
  - 8080 # first
  - 9090 # second
",
    )
    .expect("valid sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("lookup succeeds")
        .expect("ports exists");

    vec![3000_u16, 3001, 3002]
        .write_yaml(&mut doc, Some(ports))
        .expect("vec patches prefix and appends tail");

    assert_eq!(
        doc.to_string(),
        "ports:\n  - 3000 # first\n  - 3001 # second\n  - 3002\n"
    );
}

#[test]
fn yaml_value_preserves_crlf_when_appending_block_sequence_entries() {
    let mut doc = YamlDoc::parse("ports:\r\n  - 8080 # first\r\n").expect("valid CRLF sequence");
    let ports = doc
        .get_path(&["ports"])
        .expect("lookup succeeds")
        .expect("ports exists");

    vec![3000_u16, 3001]
        .write_yaml(&mut doc, Some(ports))
        .expect("vec appends with inherited line endings");

    assert_eq!(
        doc.to_string(),
        "ports:\r\n  - 3000 # first\r\n  - 3001\r\n"
    );
}

#[test]
fn yaml_value_grows_block_sequence_with_nested_sequence_items() {
    let mut doc = YamlDoc::parse(
        "matrix:
  # first row
  -
    - \"0\" # keep style
",
    )
    .expect("valid sequence");
    let matrix = doc
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");
    let replacement = vec![vec![1_u16, 2], vec![3]];

    replacement
        .write_yaml(&mut doc, Some(matrix))
        .expect("nested block sequence appends tail");

    assert_eq!(
        doc.to_string(),
        "matrix:\n  # first row\n  -\n    - \"1\" # keep style\n    - 2\n  -\n    - 3\n"
    );
    doc.commit_edits().expect("nested sequence append commits");
    let matrix = doc
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");
    assert_eq!(
        Vec::<Vec<u16>>::read_yaml(&doc, matrix).expect("matrix reads"),
        replacement
    );
}

#[test]
fn yaml_value_writes_nested_flow_sequence_values() {
    let mut doc = YamlDoc::parse("matrix: [[0]]\n").expect("valid flow sequence");
    let matrix = doc
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");
    let replacement = vec![vec![1_u16, 2], vec![3]];

    replacement
        .write_yaml(&mut doc, Some(matrix))
        .expect("nested flow sequence replacement writes");

    assert_eq!(doc.to_string(), "matrix: [[1, 2], [3]]\n");
    doc.commit_edits().expect("nested flow sequence commits");
    let matrix = doc
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");
    assert_eq!(
        Vec::<Vec<u16>>::read_yaml(&doc, matrix).expect("matrix reads"),
        replacement
    );
}

#[test]
fn yaml_value_patches_same_length_nested_block_sequences_in_place() {
    let mut doc = YamlDoc::parse(
        "matrix:\n  # first row\n  -\n    - \"1\" # keep style\n    - 2\n  # second row\n  -\n    - '3'\n",
    )
    .expect("valid nested block sequence");
    let matrix = doc
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");

    vec![vec![10_u16, 20], vec![30]]
        .write_yaml(&mut doc, Some(matrix))
        .expect("same-length nested sequence patches in place");

    assert_eq!(
        doc.to_string(),
        "matrix:\n  # first row\n  -\n    - \"10\" # keep style\n    - 20\n  # second row\n  -\n    - '30'\n"
    );
}

#[test]
fn yaml_value_patches_nested_block_mapping_sequence_items_in_place() {
    let mut doc = YamlDoc::parse(
        "items:\n  -\n    name: \"old\" # keep name comment\n    extra: keep\n  -\n    name: 'older'\n    extra: keep-too\n",
    )
    .expect("valid sequence of mappings");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");
    let replacement = vec![
        std::collections::BTreeMap::from([("name".to_owned(), "new".to_owned())]),
        std::collections::BTreeMap::from([("name".to_owned(), "newer".to_owned())]),
    ];

    replacement
        .write_yaml(&mut doc, Some(items))
        .expect("nested mapping items patch in place");

    assert_eq!(
        doc.to_string(),
        "items:\n  -\n    name: \"new\" # keep name comment\n    extra: keep\n  -\n    name: 'newer'\n    extra: keep-too\n"
    );
}

#[test]
fn yaml_value_grows_nested_block_mapping_sequence_items_by_appending_tail() {
    let mut doc =
        YamlDoc::parse("items:\n  -\n    name: \"old\" # keep name comment\n    extra: keep\n")
            .expect("valid sequence of mappings");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");
    let replacement = vec![
        std::collections::BTreeMap::from([("name".to_owned(), "new".to_owned())]),
        std::collections::BTreeMap::from([
            ("name".to_owned(), "newer".to_owned()),
            ("port".to_owned(), "9090".to_owned()),
        ]),
    ];

    replacement
        .write_yaml(&mut doc, Some(items))
        .expect("nested mapping tail appends");

    assert_eq!(
        doc.to_string(),
        "items:\n  -\n    name: \"new\" # keep name comment\n    extra: keep\n  -\n    name: newer\n    port: 9090\n"
    );
}

#[test]
fn yaml_value_writes_nested_flow_mapping_values() {
    let mut doc = YamlDoc::parse("settings: {old: value}\n").expect("valid flow mapping");
    let settings = doc
        .get_path(&["settings"])
        .expect("lookup succeeds")
        .expect("settings exists");
    let replacement = std::collections::BTreeMap::from([
        ("a".to_owned(), vec![1_u16, 2]),
        ("b".to_owned(), vec![3]),
    ]);

    replacement
        .write_yaml(&mut doc, Some(settings))
        .expect("nested flow mapping values write");

    assert_eq!(doc.to_string(), "settings: {a: [1, 2], b: [3]}\n");
}

#[test]
fn yaml_value_writes_flow_sequence_of_mappings() {
    let mut doc = YamlDoc::parse("items: [{old: 0}]\n").expect("valid flow sequence");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");
    let replacement = vec![
        std::collections::BTreeMap::from([("a".to_owned(), 1_u16)]),
        std::collections::BTreeMap::from([("b".to_owned(), 2_u16)]),
    ];

    replacement
        .write_yaml(&mut doc, Some(items))
        .expect("flow sequence of mappings writes");

    assert_eq!(doc.to_string(), "items: [{a: 1}, {b: 2}]\n");
}

#[test]
fn yaml_value_writes_decorated_nested_flow_collections() {
    let mut doc = YamlDoc::parse("matrix: !seq  &matrix [[0]]\n").expect("valid decorated flow");
    let matrix = doc
        .get_path(&["matrix"])
        .expect("lookup succeeds")
        .expect("matrix exists");

    vec![vec![1_u16, 2], vec![3]]
        .write_yaml(&mut doc, Some(matrix))
        .expect("decorated nested flow writes");

    assert_eq!(doc.to_string(), "matrix: !seq  &matrix [[1, 2], [3]]\n");
}

#[test]
fn yaml_value_rejects_invalid_flow_fragments_without_editing() {
    let mut doc = YamlDoc::parse("items: [ok]\n").expect("valid flow sequence");
    let items = doc
        .get_path(&["items"])
        .expect("lookup succeeds")
        .expect("items exists");

    let error = vec!["bad,item".to_owned()]
        .write_yaml(&mut doc, Some(items))
        .expect_err("invalid flow scalar rejects");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
    assert_eq!(doc.to_string(), "items: [ok]\n");
    assert!(doc.edits.is_empty());
}

#[test]
fn yaml_value_reads_and_writes_btree_map_values() {
    let mut doc = YamlDoc::parse(
        "limits:
  low: 1
  high: 5
",
    )
    .expect("valid MVP mapping");
    let limits = doc
        .get_path(&["limits"])
        .expect("lookup succeeds")
        .expect("limits exists");

    let values =
        std::collections::BTreeMap::<String, u16>::read_yaml(&doc, limits).expect("map reads");
    assert_eq!(values.get("low"), Some(&1));
    assert_eq!(values.get("high"), Some(&5));

    let mut replacement = std::collections::BTreeMap::new();
    replacement.insert("high".to_owned(), 7_u16);
    replacement.insert("low".to_owned(), 2_u16);
    replacement
        .write_yaml(&mut doc, Some(limits))
        .expect("map writes existing mapping");

    assert_eq!(
        doc.to_string(),
        "limits:
  low: 2
  high: 7
"
    );
}

#[test]
fn yaml_value_updates_block_mapping_keys_and_preserves_unknown_entries() {
    let mut doc = YamlDoc::parse(
        "limits:
  low: 1 # keep low
  extra: keep
",
    )
    .expect("valid mapping");
    let limits = doc
        .get_path(&["limits"])
        .expect("lookup succeeds")
        .expect("limits exists");
    let replacement =
        std::collections::BTreeMap::from([("high".to_owned(), 7_u16), ("low".to_owned(), 2_u16)]);

    replacement
        .write_yaml(&mut doc, Some(limits))
        .expect("map patches and inserts");

    assert_eq!(
        doc.to_string(),
        "limits:
  low: 2 # keep low
  extra: keep
  high: 7
"
    );
}

#[test]
fn yaml_value_updates_block_mapping_keys_preserving_order_and_comments() {
    let mut doc = YamlDoc::parse(
        "limits:
  # low limit
  low: 1
  mid: keep
  high: 5 # upper
",
    )
    .expect("valid mapping");
    let limits = doc
        .get_path(&["limits"])
        .expect("lookup succeeds")
        .expect("limits exists");
    let replacement =
        std::collections::BTreeMap::from([("high".to_owned(), 7_u16), ("low".to_owned(), 2_u16)]);

    replacement
        .write_yaml(&mut doc, Some(limits))
        .expect("map patches existing keys only");

    assert_eq!(
        doc.to_string(),
        "limits:
  # low limit
  low: 2
  mid: keep
  high: 7 # upper
"
    );
}

#[test]
fn manual_typed_config_overlay_preserves_unknown_fields_and_style() {
    let mut doc = YamlDoc::parse(
        "# main server\nhost: \"localhost\"\n\n# chosen port\nport: 3000\n\nextra: keep-me\n",
    )
    .expect("valid MVP mapping");
    let mut config = Config::from_yaml_doc(&doc).expect("manual overlay reads");

    assert_eq!(
        config,
        Config {
            host: "localhost".to_owned(),
            port: 3000,
            debug: false,
        }
    );

    config.port = 9090;
    config.debug = true;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("manual overlay writes");

    assert_eq!(
        doc.to_string(),
        "# main server\nhost: \"localhost\"\n\n# chosen port\nport: 9090\n\nextra: keep-me\ndebug: true\n"
    );
}

#[test]
fn yaml_value_reports_typed_parse_errors() {
    let doc = YamlDoc::parse("port: nope\n").expect("valid MVP mapping");
    let port = doc
        .get_path(&["port"])
        .expect("lookup succeeds")
        .expect("port exists");

    let error = u16::read_yaml(&doc, port).expect_err("not a u16");

    assert_eq!(error.diagnostic.kind, DiagnosticKind::Typed);
    assert_eq!(
        error.diagnostic.position,
        Some(LineCol { line: 1, column: 7 })
    );
}

#[test]
fn diagnostics_render_expected_items_and_notes() {
    let diagnostic = Diagnostic::new(DiagnosticKind::Parser, "unexpected token", Span::empty(0))
        .with_expected("mapping value")
        .with_expected("sequence entry")
        .with_note("while parsing a block collection");

    assert_eq!(
        diagnostic.to_string(),
        "Parser: unexpected token (expected: mapping value, sequence entry)\nnote: while parsing a block collection"
    );
}
