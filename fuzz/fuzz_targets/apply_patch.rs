#![no_main]

mod support;

use std::str;

use libfuzzer_sys::fuzz_target;
use yaml_rt_core::{YamlDoc, YamlPatch};

fuzz_target!(|data: &[u8]| {
    let mut fields = data.split(|byte| *byte == 0);
    let Some(control) = fields.next().filter(|field| !field.is_empty()) else {
        return;
    };
    let Ok(yaml) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };
    let Ok(patch_source) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };

    let Ok(mut doc) = YamlDoc::parse_with_limits(yaml, support::limits()) else {
        return;
    };
    if doc.document_count() == 0 {
        return;
    }
    let Ok(patch) = YamlPatch::parse(patch_source) else {
        return;
    };
    let document = control[0].wrapping_sub(b'0') as usize % doc.document_count();
    let before = doc.to_string();

    if doc.apply_patch(document, &patch).is_err() {
        assert_eq!(
            doc.to_string(),
            before,
            "failed patches must be transactional"
        );
        return;
    }
    support::assert_stable(&doc);
});
