#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
fuzz_dir=$(CDPATH='' cd -- "$script_dir/.." && pwd)
seed_dir=${1:-"$fuzz_dir/corpus/seeds/apply_patch"}

mkdir -p "$seed_dir"
find "$seed_dir" -mindepth 1 -maxdepth 1 -type f -delete

seed() {
  name=$1
  control=$2
  yaml=$3
  patch=$4
  printf '%s\000%b\000%b' "$control" "$yaml" "$patch" > "$seed_dir/$name"
}

seed add 0 'service: {name: api}\n' '- {op: add, path: /service/port, value: 8080}\n'
seed remove 0 'items: [a, b, c]\n' '- {op: remove, path: /items/1}\n'
seed replace 0 'name: api # keep\n' '- {op: replace, path: /name, value: web}\n'
seed move 0 'items: [a, b, c]\n' '- {op: move, from: /items/0, path: /items/2}\n'
seed copy 0 'source: {name: api}\ntarget: {}\n' '- {op: copy, from: /source, path: /target/copied}\n'
seed test 0 'enabled: true\n' '- {op: test, path: /enabled, value: true}\n'
seed rollback 0 'value: 1 # unchanged\n' '- {op: replace, path: /value, value: 2}\n- {op: test, path: /value, value: 3}\n'
seed multi-document 1 '%YAML 1.2\n---\nfirst: one\n---\nitems: [a]\n' '- {op: add, path: /items/-, value: b}\n'
seed anchors-and-aliases 0 'source: &item {value: 1}\ncopy: *item\ntarget: {}\n' '- {op: copy, from: /source, path: /target/copied}\n'

count=$(find "$seed_dir" -type f | wc -l | tr -d ' ')
echo "Seeded $count patch corpus files in $seed_dir"
