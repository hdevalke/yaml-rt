# yaml-rt-core changelog

## Unreleased

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
