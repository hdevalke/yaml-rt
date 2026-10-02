#![no_main]

use libfuzzer_sys::fuzz_target;
use yaml_rt_core::{ResourceLimits, YamlDoc, tokens_to_string};

fuzz_target!(|yaml: &str| {
    let limits = ResourceLimits {
        max_source_bytes: 64 * 1024,
        max_lines: 16 * 1024,
        max_nodes: 32 * 1024,
        max_collection_depth: 128,
        max_alias_chain: 32,
        max_expanded_nodes: 32 * 1024,
    };
    let doc = YamlDoc::parse_with_limits(yaml, limits);
    if let Ok(doc) = doc {
        let output = doc.to_string();
        assert_eq!(output, yaml);

        if let Ok(tokens) = doc.tokens() {
            assert_eq!(tokens_to_string(&tokens, doc.source()), yaml);
        }

        let reparsed =
            YamlDoc::parse_with_limits(&output, limits).expect("round-tripped YAML should reparse");
        assert_eq!(reparsed.to_string(), output);
        assert_eq!(
            reparsed.events_to_test_string(),
            doc.events_to_test_string()
        );
    }
});
