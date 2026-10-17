# Releasing yaml-rt

Developers choose versions and review changelogs in Git. CI validates source;
package-specific annotated tags publish immutable releases. There are no automated
release commits, shared version bumps, or CI-generated changelogs.

## Setup and conventions

The publishable packages are `yaml-rt-core`, `yaml-rt-rfc9535`, `yaml-rt-derive`,
`yaml-rt-serde`, `yaml-rt-schema`, `yaml-rt`, and `yaml-rt-cli`. Each has its own
SemVer version and `crates/<package>/CHANGELOG.md`. WASM and benchmark packages
remain unpublished. The root changelog archives the earlier synchronized releases;
0.3.2 is the migration baseline, not a new release.

Use Python 3.12+, Git, the workspace Rust toolchain, and authenticated GitHub CLI
(`gh auth login`, with read access to repository Actions). Install the pinned tool:

```sh
cargo install cargo-release --version 1.1.5 --locked
```

Configure the repository once:

1. Protect `main`, require ordinary CI, and review release changes through PRs.
2. Keep the protected GitHub environment `release`, with appropriate reviewers.
   Its deployment rules must permit package tags such as `yaml-rt-core/v0.3.3`,
   rather than allowing only the `main` branch.
3. Retain `CARGO_REGISTRY_TOKEN` in that environment, with publish permission for
   all seven allowed packages. Only the publication step receives it. This
   migration keeps token authentication; trusted publishing is not configured.
4. Allow release validation to read Actions runs, and the CLI artifact job to
   write repository releases. CI does not need permission to push commits or tags.
   Protect package tag creation/deletion with repository rules where available.

`release.toml` disables publication, tagging, and pushing by default. The helper
uses only cargo-release's `version` subcommand, with `dependent-version = "fix"`.
Do not use workspace-wide release commands. Publication is explicitly scoped to
the validated tag's package.

## Prepare one release

Start from a clean checkout, including untracked files, on a development branch.
The CLI-only example is:

```sh
./scripts/release prepare yaml-rt-cli patch
# Edit crates/yaml-rt-cli/CHANGELOG.md; review all changes.
```

Preparation updates only the selected package version, dependency requirements
that no longer accept it, and Cargo.lock. It prints the diff, creates no commit
or tag, and never publishes or pushes. A CLI patch does not bump library versions.
The version argument also accepts an explicit SemVer or cargo-release's
`minor`, `major`, `release`, `alpha`, `beta`, and `rc` levels.

Keep `## Unreleased` in every changelog. Before committing, add a nonempty section
for the selected version, with the actual release date and reviewed notes:

```markdown
## 0.3.3 - YYYY-MM-DD

- Describe the package's user-visible changes.
```

Replace `YYYY-MM-DD` with a valid date. Record deliberate dependency minimum
changes in the affected package's notes. Never infer a shared version from
Conventional Commits; Convco remains a commit validation tool.

Commit with a Conventional Commit message, then validate:

```sh
./scripts/release check
./scripts/release check yaml-rt-cli
```

The first command checks Cargo resolution, the locked dependency graph, package
contents and changelogs. It can inspect uncommitted changes. The package-specific
command also runs `cargo publish --dry-run --locked --all-features`; commit the
release changes first because that verification requires clean packaged source.
It needs read-only registry access, not a publication token.

Push the branch, merge the reviewed PR, and wait for successful `main` push CI.
From a clean local `main` checkout containing the merged changes:

```sh
./scripts/release tag yaml-rt-cli
```

This command fetches `origin/main` and tags, requires the selected commit to be
in main's history with successful CI on that exact SHA, rejects existing tags
and published versions, and reruns selected-package release checks. It creates
and pushes an annotated `yaml-rt-cli/v<CLI-version>` tag. It never pushes main.
A main ancestor is acceptable even if main has advanced. Do not manually bypass
these checks or move release tags.

## Release all changed packages

The batch commands select changed packages without giving the workspace a shared
version. Fetch current tags before planning a batch:

```sh
git fetch origin --tags
./scripts/release prepare-all patch --dry-run
./scripts/release prepare-all patch
```

`patch` is the default; `minor` and `major` are also available. Choose individual
levels or exact versions when needed, for example:

```sh
./scripts/release prepare-all patch --bump yaml-rt-core=minor --dry-run
./scripts/release prepare-all patch --bump yaml-rt-core=minor
```

The helper compares each crate's Cargo package file list and file contents, plus
Cargo-resolved manifest metadata, with its nearest reachable package release tag.
Until a crate has a package tag, it uses the synchronized `v0.3.2` tag. A package
absent from that baseline retains its initial version. Changes to inherited
workspace metadata or dependency requirements count as package changes even when
the crate's own directory was untouched. Documentation inside a package counts;
repository-only documentation does not. Generated lockfiles and VCS metadata do
not select packages. Root licenses also count for CLI binary archives.

The migration itself changes manifests and adds package changelogs, so the first
batch preview may select all seven packages. Subsequent previews use each
package's independent tag. Detection uses local tags and includes tracked and
untracked, non-ignored working files. Actual preparation requires a clean tree;
its dry run can preview uncommitted development changes.

Both modes simulate cargo-release edits in disposable local checkouts. The dry
run prints the selected packages, resulting versions, exact manifest/lockfile
diff, and publication order without modifying your working files, commits, or
tags. Actual preparation applies that same kind of diff in one operation; no
intermediate commits are created in your checkout. Existing prepared versions
are retained, so repeating preparation does not increment them again. Overrides
choose the level for packages still at their baseline version; use single-package
preparation to deliberately increment an already prepared version again.

Cargo-release fixes incompatible dependency requirements. If those fixes change
other packages' published inputs, the batch includes and bumps those dependents
as well. A compatible core patch alone leaves unchanged dependents out. Version
levels are deliberate maintainer choices, not inferred from commits or APIs;
review the default patch bumps and any breaking API changes. Raise compatible
minimum dependency versions manually when a consumer needs a newly introduced
API, as described below. Package selection cannot infer that requirement from
Rust source.

After preparation, review the diff, write dated changelog entries for every
selected package, and commit all release changes together. Run
`./scripts/release check`, merge the reviewed PR, and wait for successful main CI.
Then, from a clean `main` checkout at the merged commit:

```sh
./scripts/release tag-all --dry-run
./scripts/release tag-all
```

The tagging dry run validates local selection, prepared versions and changelogs,
and prints the sequential tag/wait actions. It performs no fetch, registry check,
GitHub API request, tag creation, or push. Remote main/CI status, registry checks,
and workflow waits happen only in actual execution. No publication token is
needed locally; the real command needs GitHub CLI authentication.

Actual execution fetches tags, verifies main ancestry and exact-commit CI, and
uses the same package checks as `tag PACKAGE`. It pushes one annotated tag,
watches that exact tag's workflow with `gh run watch`, confirms registry
provenance/indexing, then proceeds to the next dependency-ordered package. It
stops on any failure and never pushes the remaining tags. Environment approvals
still apply. The default workflow wait is two hours per package; change it with
`./scripts/release tag-all --timeout 10800` (seconds).

To resume, stay on the same merged commit, rerun failed workflow jobs if necessary,
and repeat `tag-all`. Existing same-commit tags are validated and their workflows
watched without creating replacement tags or republishing packages. A local tag
left by a failed push can be pushed unchanged if its version remains unpublished.
Completed releases are checked and skipped before continuing. Resume from the
original commit even if main has advanced; existing tags pointing elsewhere are
never moved. Keep one batch active at a time, and do not manually push additional
release tags while it is waiting.

## Core and dependent releases

For a compatible core patch:

```sh
./scripts/release prepare yaml-rt-core patch
# Edit crates/yaml-rt-core/CHANGELOG.md, review, and commit.
./scripts/release check yaml-rt-core
# Merge and wait for main CI; switch to the merged main commit.
./scripts/release tag yaml-rt-core
```

A requirement `0.3.2` means `>=0.3.2, <0.4.0`; a core patch to 0.3.3 does not
require dependent publication. Cargo-release retains compatible requirements.
If a dependent starts using an API introduced in 0.3.3, manually raise its
minimum requirement to 0.3.3 and deliberately prepare that dependent's release.
Workspace-inherited requirements affect every package inheriting that entry;
use package-local requirements when different minimums are necessary.

A core change to 0.4.0 is incompatible with `0.3.x`. Prepare core first:

```sh
./scripts/release prepare yaml-rt-core minor
# Inspect updated workspace dependency requirements, fix dependent code,
# update core notes, and commit before another prepare operation.
./scripts/release prepare yaml-rt-rfc9535 minor
# Review notes and commit each deliberately selected release.
```

Also consider Serde, schema, facade, CLI, and unpublished WASM/benchmark users.
Cargo-release updates incompatible requirements but does not choose which
packages deserve new releases. Review the displayed requirement changes, bump
packages whose distributable contents change, and keep each preparation committed
before running the next one. `0.0.x` patches can also change compatibility lines;
Cargo's rules, rather than a custom release resolver, govern this behavior.

Merge the related changes together if useful. Publish prerequisites first:

1. Core and derive have no internal publication prerequisites.
2. RFC 9535, Serde, and schema depend on core.
3. Facade depends on core, derive, and Serde; CLI depends on core, RFC 9535, and schema.

Skip unchanged packages. Wait for each prerequisite release workflow to finish
and for its version to be indexed before checking/tagging a dependent. Run
`./scripts/release check <package>` and `./scripts/release tag <package>` for
each selected package, or use `tag-all` to do that sequentially. Do not push all
dependent tags together: GitHub concurrency retains only one pending run and
does not guarantee dependency order.

## What Actions does

Ordinary branch/PR CI retains formatting, tests on three OSes, facade feature
combinations, Clippy, rustdoc, YAML and JSON Schema conformance, package checks,
benchmarks, audit, fuzz regressions, Conventional Commits, and workflow linting.
It also tests the release helper with disposable Git repositories. Core and
derive archives are always verified against the registry. For packages with
internal dependencies, ordinary CI dry-runs unpublished release candidates;
already published version numbers may contain ongoing development changes and
are checked through workspace CI and package-content inspection instead.
Candidate dry runs may report a missing internal registry dependency during
coordinated preparation; selected release checks must pass without that exception.
In particular, the migration retains 0.3.2 even where current development source
already uses APIs absent from the published core 0.3.2. Before releasing those
consumers, release the new core and raise their minimum core requirement.

A package tag workflow checks out the exact pushed SHA and validates the package
allowlist, strict SemVer, annotated tag object, unchanged remote tag, manifest,
changelog, main ancestry, and successful main push CI on that SHA. This avoids
repeating the full CI suite on tags. Publication reruns these guards inside the
protected environment and uses Cargo package verification/dry run to establish
registry dependency availability. Only the selected crate is published, then
its registry record, archive provenance, and index availability are checked.
All publication workflows share a non-cancelling concurrency group.

Only CLI tags build Linux x86_64 musl, Linux aarch64 musl, macOS and Windows
binaries. Versioned archives `yaml-rt-v<CLI-version>-<target>.tar.gz` and convenient
`yaml-rt-<target>.tar.gz` aliases retain SHA-256 files and include the CLI README and licenses. Native
builds also verify the executable version before packaging. The executable remains
`yaml-rt`. Download binaries from the **yaml-rt-cli** GitHub Release, whose title
and tag identify the CLI version independently of the facade version.

After publication, external consumer checks compile the selected library; facade
releases exercise its feature combinations. CLI releases install the exact
published CLI version and verify a lossless YAML edit. CLI GitHub Releases use
committed package notes, and become public after assets are complete. Library
releases use crates.io and package tags without separate GitHub Releases.

## Recovery and immutable versions

- Before a tag: correct the branch and review again. A failed tag push may leave
  a local annotated tag. Inspect the remote and workflow state; if the remote
  tag is absent and the version unpublished, push that same reviewed tag
  explicitly. Never force-push or silently replace a remote tag.
- Before publication: fix transient registry, dependency, permission, or
  environment problems and rerun the original workflow. If source must change,
  use a new version and tag; do not move the old tag.
- After publication: rerun the original workflow using the same tag. The helper
  skips publication only if the unyanked registry version's checksum and packaged
  clean VCS SHA/subdirectory identify this exact source. A different or
  unverifiable version fails closed. It never republishes unrelated crates.
- If publication succeeds but indexing, smoke tests, binary builds, or release
  creation fail, rerun failed jobs or the original run. Successful registry state
  remains intact. Wait for transient archive/index propagation before retrying.
- CLI asset retries retain existing archives and checksum files, verify their
  pairing, and upload only missing assets. A draft release can be resumed. A
  checksum mismatch requires investigation; the helper never overwrites an
  existing uploaded asset. Builds may rerun, but existing assets take precedence.
- Published versions are immutable, including yanked versions. Fix source with
  a new version. Never reuse a published version or force-move a release tag.

No live publication or GitHub environment change is needed to test the migration.
Local safety tests use mocked registry/publication operations and temporary Git
repositories. Real environment approvals, registry uploads/indexing, cross-OS
artifacts, and GitHub asset uploads require an actual deliberate release.
