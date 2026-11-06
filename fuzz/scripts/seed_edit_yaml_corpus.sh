#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
fuzz_dir=$(CDPATH='' cd -- "$script_dir/.." && pwd)
seed_dir=${1:-"$fuzz_dir/corpus/seeds/edit_yaml"}

mkdir -p "$seed_dir"
find "$seed_dir" -mindepth 1 -maxdepth 1 -type f -delete

seed() {
  name=$1
  control=$2
  yaml=$3
  path=$4
  from=$5
  value=$6
  new_key=$7
  printf '%s\000%b\000%s\000%s\000%b\000%s' \
    "$control" "$yaml" "$path" "$from" "$value" "$new_key" > "$seed_dir/$name"
}

seed add-sequence 0 'items: [a, b]\n' '/items/-' '' 'c\n' ''
seed remove-mapping 1 'service: {name: api, port: 8080}\n' '/service/port' '' '' ''
seed replace-block 2 'service:\n  name: api # keep\n' '/service/name' '' 'web\n' ''
seed move-sequence 3 'items: [a, b, c]\n' '/items/2' '/items/0' '' ''
seed copy-anchored-neighbor 4 'source: {name: api}\ntarget: {}\n' '/target/copied' '/source' '' ''
seed semantic-test 5 'enabled: true\n' '/enabled' '' 'true\n' ''
seed rename-key 6 'service:\n  old: value # keep\n' '/service/old' '' '' 'new'
seed multi-document 01 '%YAML 1.2\n---\nfirst: one\n---\nitems: [a]\n' '/items/-' '' 'b\n' ''
seed anchors-and-aliases 4 'source: &item {value: 1}\ncopy: *item\ntarget: {}\n' '/target/copied' '/source' '' ''

count=$(find "$seed_dir" -type f | wc -l | tr -d ' ')
echo "Seeded $count editor corpus files in $seed_dir"
