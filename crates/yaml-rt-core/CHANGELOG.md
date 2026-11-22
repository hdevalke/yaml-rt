# yaml-rt-core changelog

## Unreleased

## 0.3.5 - 2026-11-22

### Fixes

* **editor:** allow replacement of omitted values (6ddf50a)
* **editor:** preserve live alias bindings during replacement (492dd74)
* **typed:** preserve exact values in block scalar writes (f91f5dd)
* **editor:** preserve separators when replacing block scalars (dfbd33d)
* **parser:** count leading blank lines in block scalars (a6ffa56)
* **parser:** enforce the implicit mapping key length limit (e87d61e)
* **core:** support bare carriage return line endings (b4ebb90)
* **parser:** normalize block scalar line breaks (e9a27b0)
* **parser:** exclude separator trivia from flow entry spans (9581d2b)
* **parser:** exclude trailing trivia from block scalar spans (64bab97)
* **parser:** allow omitted values in final flow mapping entries (330926a)
* **parser:** scan flow scalars in their grammar context (96c41db)
* **parser:** enforce collection depth before descending (b85a974)

### Other

* **parser:** skip flow key lookahead without a colon (bc0b8d5)
* **core:** reduce line facts and excess CST storage (10db46e)

## 0.3.4 - 2026-11-12

### Fixes

* **parser:** propagate multiline child source spans (9589e01), closes #5
* bound input and diagnostic resource usage (795da42)

### Other

* **parser:** remove unreachable compact mapping path (663353c)
* remove unreachable fallback branches (dc6e26c)

## 0.3.3 - 2026-10-17

### Features

* **release:** support independent package releases (6cc386c)

### Fixes

* **core:** index anchors for bounded resolution (5db337d)
* **core:** enforce YAML resource limits (de124c2)

### Documentation

* add badges to crate readmes (b0316e1)
* add crate-specific readmes and usage guidance (20fd67e)

## Historical releases

Version 0.3.2 is the migration baseline for independent releases.
Earlier synchronized release history is preserved in the
[root changelog](../../CHANGELOG.md). That archive includes workspace changes
and does not imply that every package changed in each release.
