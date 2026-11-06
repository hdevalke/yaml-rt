use yaml_rt_core::{ResourceLimits, YamlDoc};

pub(crate) fn limits() -> ResourceLimits {
    ResourceLimits {
        max_source_bytes: 64 * 1024,
        max_lines: 16 * 1024,
        max_nodes: 32 * 1024,
        max_collection_depth: 128,
        max_alias_chain: 32,
        max_expanded_nodes: 32 * 1024,
    }
}

pub(crate) fn assert_stable(doc: &YamlDoc) {
    let output = doc.to_string();
    let reparsed = YamlDoc::parse_with_limits(&output, limits())
        .expect("successfully edited YAML should reparse");
    assert_eq!(reparsed.to_string(), output);
    assert_eq!(
        reparsed.events_to_test_string(),
        doc.events_to_test_string()
    );
}
