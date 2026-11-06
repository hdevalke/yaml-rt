#![no_main]

mod support;

use std::str;

use libfuzzer_sys::fuzz_target;
use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment};

fuzz_target!(|data: &[u8]| {
    let mut fields = data.split(|byte| *byte == 0);
    let Some(control) = fields.next().filter(|field| !field.is_empty()) else {
        return;
    };
    let Ok(yaml) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };
    let Ok(path) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };
    let Ok(from) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };
    let Ok(value) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };
    let Ok(new_key) = str::from_utf8(fields.next().unwrap_or_default()) else {
        return;
    };

    let Ok(mut doc) = YamlDoc::parse_with_limits(yaml, support::limits()) else {
        return;
    };
    if doc.document_count() == 0 {
        return;
    }
    let Ok(path) = JsonPointer::parse(path) else {
        return;
    };
    let operation = control[0].wrapping_sub(b'0') % 7;
    let document =
        control.get(1).copied().unwrap_or(b'0').wrapping_sub(b'0') as usize % doc.document_count();
    let before = doc.to_string();

    let result = match operation {
        0 => {
            let Ok(value) = YamlFragment::parse(value) else {
                return;
            };
            doc.add_at(document, &path, &value)
        }
        1 => doc.remove_at(document, &path),
        2 => {
            let Ok(value) = YamlFragment::parse(value) else {
                return;
            };
            doc.replace_at(document, &path, &value)
        }
        3 => {
            let Ok(from) = JsonPointer::parse(from) else {
                return;
            };
            doc.move_at(document, &from, &path)
        }
        4 => {
            let Ok(from) = JsonPointer::parse(from) else {
                return;
            };
            doc.copy_at(document, &from, &path)
        }
        5 => {
            let Ok(value) = YamlFragment::parse(value) else {
                return;
            };
            doc.test_at(document, &path, &value).map(|_| ())
        }
        _ => doc.rename_key_at(document, &path, new_key),
    };

    if result.is_err() {
        assert_eq!(
            doc.to_string(),
            before,
            "failed edits must be transactional"
        );
        return;
    }
    support::assert_stable(&doc);
});
