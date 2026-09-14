#![no_main]

use libfuzzer_sys::fuzz_target;
use yaml_rt_core::YamlDoc;

fuzz_target!(|yaml: &str| {
    let doc = YamlDoc::parse(yaml);
    if let Ok(doc) = doc {
        let output = doc.to_string();
        assert_eq!(output, yaml);

        let reparsed = YamlDoc::parse(&output).expect("round-tripped YAML should reparse");
        assert_eq!(reparsed.to_string(), output);
        assert_eq!(
            reparsed.events_to_test_string(),
            doc.events_to_test_string()
        );
    }
});
