#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fuzz_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
repo_root=$(CDPATH= cd -- "$fuzz_dir/.." && pwd)
suite_dir="$repo_root/third_party/yaml-test-suite"
seed_dir=${1:-"$fuzz_dir/corpus/seeds/parse_yaml"}

if [ ! -d "$suite_dir" ]; then
  echo "YAML Test Suite directory not found: $suite_dir" >&2
  exit 1
fi

# This directory contains reproducible source seeds, not libFuzzer discoveries.
# Clearing regular files prevents renamed or removed fixtures from accumulating.
mkdir -p "$seed_dir"
find "$seed_dir" -mindepth 1 -maxdepth 1 -type f -delete

find "$suite_dir" \( -name in.yaml -o -name out.yaml -o -name emit.yaml \) | while IFS= read -r fixture; do
  relative=${fixture#"$suite_dir"/}
  safe=$(printf '%s' "$relative" | tr '/ ' '__')
  cp "$fixture" "$seed_dir/$safe"
done

cat > "$seed_dir/edge-flow-nested.yaml" <<'EOF'
{outer: [a, {b: c}, [d, e]], trailing: value}
EOF

cat > "$seed_dir/edge-anchors-aliases.yaml" <<'EOF'
first: &anchor Foo
second: *anchor
third: &anchor [a, b, {c: d}]
fourth: *anchor
EOF

cat > "$seed_dir/edge-tags-directives.yaml" <<'EOF'
%YAML 1.2
%TAG !e! tag:example.com,2000:app/
---
- !e!tag%21 value
- !!str 12
- ! local
EOF

cat > "$seed_dir/edge-block-scalars.yaml" <<'EOF'
literal: |+
  a

    b
folded: >-
  c
  d

  e
EOF

cat > "$seed_dir/edge-tabs-comments.yaml" <<'EOF'
plain: text
 	lines
flow: [a, # comment
 b]
-	-1
EOF

cat > "$seed_dir/edge-explicit-keys.yaml" <<'EOF'
? [flow, key]
: {value: yes}
? >
  folded
: !!null
EOF

cat > "$seed_dir/edge-doc-markers.yaml" <<'EOF'
---
doc: one
...
---
- two
...
EOF

cat > "$seed_dir/edge-unicode-spans.yaml" <<'EOF'
--- ߅foo:
--- "߅":
--- [߅]
--- {߅: v}
߅foo: bar
key: ߅value
? ߅foo
: bar
- ߅foo: bar
--- !tag ߅foo
&a ߅foo
EOF

cat > "$seed_dir/regression-tag-directive-leading-space.yaml" <<'EOF'
 %TAG ! p-|er↓
!
EOF

cat > "$seed_dir/edge-malformed-flow.yaml" <<'EOF'
&fl
 { &fl
 { &- |-
  ab
...
e e: f },g: h }
]
EOF

count=$(find "$seed_dir" -type f | wc -l | tr -d ' ')
echo "Seeded $count parser corpus files in $seed_dir"
