# yaml-rt-schema changelog

## Unreleased

## 0.3.4 - 2026-11-12

### Fixes

* bound input and diagnostic resource usage (795da42)

### Other

* remove unreachable fallback branches (dc6e26c)

## 0.3.3 - 2026-10-17

### Features

* **release:** support independent package releases (6cc386c)

### Fixes

* rust 1.99 clippy warnings in schema validation (c1017bb)
* **schema:** bound YAML graph expansion (0aa2b00)

### Other

* **schema:** harden conformance checks and expand coverage (2fc6c61)
* **schema:** cache meta-schemas and streamline conformance tests (4cb9c65)

### Documentation

* add badges to crate readmes (b0316e1)
* add crate-specific readmes and usage guidance (20fd67e)

### Dependencies

- Update `yaml-rt-core` (normal) requirement from `^0.3.2` to `^0.3.3`.

## Historical releases

Version 0.3.2 is the migration baseline for independent releases.
Earlier synchronized release history is preserved in the
[root changelog](../../CHANGELOG.md). That archive includes workspace changes
and does not imply that every package changed in each release.
