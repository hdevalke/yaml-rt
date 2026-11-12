use yaml_rt_core::{NodeKind, YamlDoc};

/// Block collections start after indentation, whereas entry wrappers can start
/// at the beginning of that line (including enclosing compact indicators).
/// A block scalar owns its last line break; its structural parent need not.
pub fn check_span_ends(doc: &YamlDoc) -> Result<(), String> {
    let mut pending = doc.root().into_iter().collect::<Vec<_>>();
    while let Some(parent) = pending.pop() {
        let parent_node = doc.node(parent).unwrap();
        for child in doc.children(parent) {
            let child_node = doc.node(child).unwrap();
            if child_node.span().start < parent_node.span().start {
                let prefix = &doc.as_source()
                    [child_node.span().start as usize..parent_node.span().start as usize];
                if !matches!(
                    child_node.kind(),
                    NodeKind::MappingEntry | NodeKind::SequenceEntry
                ) || prefix.contains(['\r', '\n'])
                {
                    return Err(format!(
                        "parent {parent_node:?} starts after child {child_node:?}"
                    ));
                }
            }
            let mut end = child_node.span().end as usize;
            if matches!(
                child_node.kind(),
                NodeKind::LiteralScalar | NodeKind::FoldedScalar
            ) {
                let source = doc.source().slice(child_node.span());
                end -= if source.ends_with("\r\n") {
                    2
                } else {
                    usize::from(source.ends_with(['\r', '\n']))
                };
            }
            if (parent_node.span().end as usize) < end {
                return Err(format!(
                    "parent {parent_node:?} does not cover child {child_node:?}"
                ));
            }
            pending.push(child);
        }
    }
    Ok(())
}
