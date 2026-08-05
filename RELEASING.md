# Releasing yaml-rt

Releases are prepared and published by the manual GitHub Actions workflow in
`.github/workflows/release.yml`. The workflow calculates or validates one
shared workspace version, creates the release commit and tag, builds CLI
archives, publishes the crates, smoke-tests crates.io, and creates the GitHub
release last.

Do not create the release tag manually.

## One-time repository setup

1. Create a protected GitHub environment named `release`.
2. Add `CARGO_REGISTRY_TOKEN` as an environment secret. The token needs publish
   access to `yaml-rt-core`, `yaml-rt-rfc9535`, `yaml-rt-derive`,
   `yaml-rt-serde`, `yaml-rt-cli`, and `yaml-rt`, including permission to create
   the initially unpublished `yaml-rt-rfc9535` package.
3. Require any desired reviewers or branch protections on that environment.
4. Ensure GitHub Actions may create repository contents so the workflow can
   push the release commit and tag and create a release.

The registry token is exposed only to the publication job.

## Before dispatch

Start from a clean `main` branch with all intended changes pushed. Confirm that
CI is green and that the release notes implied by the Conventional Commits are
correct.

For a local release-readiness run:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy \
  -p yaml-rt-core -p yaml-rt-rfc9535 -p yaml-rt-derive -p yaml-rt-serde \
  -p yaml-rt -p yaml-rt-cli \
  --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" \
  cargo doc --workspace --all-features --no-deps
YAML_TEST_SUITE_RUN_ALL=1 \
YAML_TEST_SUITE_CHECK_JSON=1 \
  cargo test -p yaml-rt-core --test yaml_test_suite
cargo audit
convco check
```

`cargo-release` requires a clean working tree. Untracked files also make the
release preparation fail, so keep local-only files outside the checkout used
for a dry run.

## Dispatch

Open the `Release` workflow and choose **Run workflow** on `main`.

The version input accepts:

- `auto`, calculated by Convco;
- `major`, `minor`, or `patch`;
- a supported prerelease label;
- an exact SemVer version.

Automatic calculation starts at 0.1.0 for an unpublished repository. The
workflow rejects invalid versions and existing tags before changing Git.

## Pipeline order

1. Verify formatting, tests, feature combinations, Clippy, rustdoc,
   conformance, package contents, benchmarks, security audit, and commit
   messages.
2. Run `cargo release` without publication. Its hook regenerates
   `CHANGELOG.md`, then it creates and pushes the consolidated release commit
   and annotated tag.
3. Check out the tag and build Linux musl, macOS, and Windows CLI archives with
   SHA-256 files.
4. Enter the protected `release` environment and publish all six crates in
   dependency order.
5. Wait for crates.io, compile external consumers across facade feature
   combinations, install the published CLI, and perform a lossless edit.
6. Create the GitHub release and attach every archive and checksum.

The GitHub release is deliberately last: its presence means both crate
publication and smoke tests succeeded.

## Recovery

- If verification or preparation fails before a tag is pushed, fix the branch
  and dispatch again.
- If a build fails after the tag exists, rerun the failed workflow jobs or the
  workflow for that tag without changing tagged source.
- If the release commit and tag exist but no crate was published and the source
  must change, remove them only as a deliberate maintainer operation, then
  prepare a new release from the corrected branch.
- Published crate versions are immutable. After any package is published, fix
  forward with a new patch release. Yank a broken version only when consumers
  should stop selecting it.
- Never reuse a version or move a tag for an already published release.

## Post-release checks

Confirm that all six crates show the same version on crates.io, the four target
archives and their checksums appear on GitHub, installation from crates.io
reports the expected `yaml-rt --version`, and `main` contains the generated
release commit.
