#![no_main]

use libfuzzer_sys::fuzz_target;
use yaml_rt::YamlDoc;

fuzz_target!(|data: &[u8]| {
    if let Ok(yaml_str) = std::str::from_utf8(data) {
        if yaml_str.len() > 1_000_000 {
            return;
        }

        let doc = YamlDoc::parse(yaml_str);
        if let Ok(doc) = doc {
            let output = doc.to_string();
            assert_eq!(output, yaml_str);

            let reparsed = YamlDoc::parse(&output).expect("round-tripped YAML should reparse");
            assert_eq!(reparsed.to_string(), output);
            assert_eq!(reparsed.events_to_test_string(), doc.events_to_test_string());
        }
    }
});
