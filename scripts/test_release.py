"""Release safety tests. Network calls and publication are always mocked.

Cargo-release integration runs against disposable local Git repositories with
empty crates, so the actual dependency policy is tested without registry writes.
"""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader("release", str(Path(__file__).with_name("release")))
spec = importlib.util.spec_from_loader(loader.name, loader)
release = importlib.util.module_from_spec(spec)
loader.exec_module(release)


class Guards(unittest.TestCase):
    @patch.object(release, "package", return_value="0.3.2")
    def test_invalid_tags(self, package):
        for tag in ("v0.3.2", "evil/v0.3.2", "yaml-rt-wasm/v0.3.2", "yaml-rt-bench/v0.3.2",
                    "yaml-rt-core/v00.3.2", "yaml-rt-core/v0.3", "yaml-rt-core/v0.3.2-01",
                    "yaml-rt-core/v0.3.2-", "yaml-rt-core/v0.3.2+", "yaml-rt-core/v0.3.2\n",
                    "yaml-rt-cli/v$(touch bad)", "yaml-rt-cli/v0.3.3", "../yaml-rt-core/v0.3.2"):
            with self.subTest(tag=tag), self.assertRaises(RuntimeError):
                release.parse_tag(tag)

    def test_semver(self):
        for version in ("0.0.0", "0.3.3", "1.0.0-rc.1", "1.0.0-0A.0+build.01"):
            self.assertIsNotNone(release.SEMVER.fullmatch(version))

    def test_tags_select_each_package_at_its_own_version(self):
        for name in release.PACKAGES:
            version = release.package(name)
            self.assertEqual(release.parse_tag(f"{name}/v{version}"), (name, version))

    def test_changelog_requires_reviewed_version(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(release, "ROOT", Path(tmp)):
            directory = Path(tmp) / "crates/yaml-rt-cli"
            directory.mkdir(parents=True)
            changelog = directory / "CHANGELOG.md"
            changelog.write_text("# Changelog\n\n## Unreleased\n")
            with self.assertRaises(RuntimeError):
                release.notes("yaml-rt-cli", "0.3.3")
            changelog.write_text("# Changelog\n\n## Unreleased\n\n## 0.3.3 - 2026-10-17\n\n- Fix editing.\n")
            self.assertEqual(release.notes("yaml-rt-cli", "0.3.3"), "- Fix editing.\n")

    def test_absent_and_registry_failure(self):
        with patch.object(release, "request", return_value=None):
            self.assertFalse(release.registry("yaml-rt-cli", "0.3.3"))
        with patch.object(release, "request", side_effect=OSError("network failure")):
            with self.assertRaises(OSError):
                release.registry("yaml-rt-cli", "0.3.3")

    def test_retry_checks_archive_provenance(self):
        import hashlib
        buffer = io.BytesIO()
        vcs = json.dumps({"git": {"sha1": "a" * 40}, "path_in_vcs": "crates/yaml-rt-cli"}).encode()
        with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
            info = tarfile.TarInfo("yaml-rt-cli-0.3.3/.cargo_vcs_info.json")
            info.size = len(vcs)
            tar.addfile(info, io.BytesIO(vcs))
        archive = buffer.getvalue()
        record = json.dumps({"version": {"num": "0.3.3", "crate": "yaml-rt-cli", "yanked": False,
                                        "checksum": hashlib.sha256(archive).hexdigest()}}).encode()
        with patch.object(release, "request", side_effect=[record, archive]):
            self.assertTrue(release.registry("yaml-rt-cli", "0.3.3", "a" * 40))
        with patch.object(release, "request", side_effect=[record, archive]):
            with self.assertRaisesRegex(RuntimeError, "does not belong"):
                release.registry("yaml-rt-cli", "0.3.3", "b" * 40)

    def test_successful_publication_is_not_repeated(self):
        with patch.object(release, "validate_tag", return_value=("yaml-rt-cli", "0.3.3", "abc")), \
             patch.object(release, "check"), patch.object(release, "registry", return_value=True), \
             patch.object(release, "wait_index") as wait, patch.object(release, "run") as run:
            release.publish("yaml-rt-cli/v0.3.3")
            run.assert_not_called()
            wait.assert_called_once_with("yaml-rt-cli", "0.3.3", "abc")

    def test_index_probe_cannot_resolve_a_local_workspace_package(self):
        with patch.object(release, "registry", return_value=True), \
             patch.object(release.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as cargo:
            release.wait_index("yaml-rt-core", "0.3.3", "abc")
            self.assertNotEqual(Path(cargo.call_args.kwargs["cwd"]), release.ROOT)
            self.assertIn("--registry", cargo.call_args.args[0])

    def test_first_publication_selects_exactly_one_package(self):
        with patch.object(release, "validate_tag", return_value=("yaml-rt-rfc9535", "0.3.3", "abc")), \
             patch.object(release, "check"), patch.object(release, "registry", return_value=False), \
             patch.object(release, "wait_index"), patch.object(release, "run") as run:
            release.publish("yaml-rt-rfc9535/v0.3.3")
            run.assert_called_once_with("cargo", "publish", "--locked", "--all-features", "--registry", "crates-io", "-p", "yaml-rt-rfc9535")

    def test_duplicate_tag_fails_before_creating_or_publishing(self):
        def fake_run(*args, **kwargs):
            if args[:3] == ("git", "branch", "--show-current"):
                return "main"
            if args[:2] == ("git", "rev-parse"):
                return "abc"
            return ""
        with patch.object(release, "package", return_value="0.3.3"), patch.object(release, "clean"), \
             patch.object(release, "fetch"), patch.object(release, "main_commit"), patch.object(release, "ci_success"), \
             patch.object(release, "run", side_effect=fake_run) as run, \
             patch.object(release.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
             patch.object(release, "check") as check:
            with self.assertRaisesRegex(RuntimeError, "already exists"):
                release.tag("yaml-rt-cli")
            check.assert_not_called()
            self.assertFalse(any(call.args[:2] in (("git", "tag"), ("git", "push")) for call in run.call_args_list))

    def check_fixtures(self):
        return {"workspace_members": list(release.PACKAGES), "packages": [
            {"name": name, "id": name, "publish": None, "dependencies":
             [] if name in ("yaml-rt-core", "yaml-rt-derive") else
             [{"name": "yaml-rt-core", "path": "crates/yaml-rt-core", "kind": None, "req": "^0.3.2"}]}
            for name in release.PACKAGES]}

    def test_ci_checks_standalone_packages_and_defers_published_development(self):
        contents = "Cargo.toml\nCargo.toml.orig\nREADME.md\nCHANGELOG.md"
        with patch.object(release, "metadata", return_value=self.check_fixtures()), \
             patch.object(release, "run", return_value=contents), \
             patch.object(release, "registry", return_value=True), \
             patch.object(release.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "verified")) as dry_run, \
             contextlib.redirect_stdout(io.StringIO()):
            release.check(ci=True)
            self.assertEqual([call.args[0][-1] for call in dry_run.call_args_list], ["yaml-rt-core", "yaml-rt-derive"])

    def test_missing_dependencies_block_selected_release_checks(self):
        contents = "Cargo.toml\nCargo.toml.orig\nREADME.md\nCHANGELOG.md"
        error = subprocess.CompletedProcess([], 101, "error: no matching package named `yaml-rt-core` found")
        with patch.object(release, "metadata", return_value=self.check_fixtures()), \
             patch.object(release, "notes"), patch.object(release, "run", return_value=contents), \
             patch.object(release.subprocess, "run", return_value=error), \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "Publish dry run failed"):
                release.check("yaml-rt-cli")

    def test_http_errors_fail_closed(self):
        import urllib.error
        unavailable = urllib.error.HTTPError("url", 503, "unavailable", {}, None)
        self.addCleanup(unavailable.close)
        with patch.object(release.urllib.request, "urlopen", side_effect=unavailable):
            with self.assertRaises(urllib.error.HTTPError):
                release.request("https://example.invalid")
        missing = urllib.error.HTTPError("url", 404, "missing", {}, None)
        self.addCleanup(missing.close)
        with patch.object(release.urllib.request, "urlopen", side_effect=missing):
            self.assertIsNone(release.request("https://example.invalid"))

    def test_failed_ci_prevents_tagging(self):
        with patch.object(release, "run", return_value=""):
            with self.assertRaisesRegex(RuntimeError, "No successful main"):
                release.ci_success("a" * 40)

    @patch.object(release, "package", return_value="0.3.2")
    def test_asset_retry_keeps_uploaded_archives(self, package):
        import hashlib
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            names = [f"yaml-rt-{prefix}{target}.tar.gz" for target in ("linux-x86", "linux-arm", "macos", "windows")
                     for prefix in ("v0.3.2-", "")]
            for name in names:
                (directory / name).write_bytes(b"rebuilt archive")
                (directory / (name + ".sha256")).write_text("new checksum")
            existing = {"tag_name": "yaml-rt-cli/v0.3.2", "draft": True,
                        "assets": [{"name": name} for name in names]}
            uploaded = []
            def fake_run(*args, **kwargs):
                if args[:3] == ("gh", "repo", "view"):
                    return "owner/repository"
                if args[:2] == ("gh", "api"):
                    return json.dumps([[existing]])
                if args[:3] == ("gh", "release", "download"):
                    (directory / args[args.index("--pattern") + 1]).write_bytes(b"original published archive")
                if args[:3] == ("gh", "release", "upload"):
                    uploaded.append(Path(args[4]).name)
            with patch.object(release, "run", side_effect=fake_run):
                release.assets("yaml-rt-cli/v0.3.2", directory)
            self.assertEqual(set(uploaded), {name + ".sha256" for name in names})
            for name in names:
                self.assertEqual((directory / name).read_bytes(), b"original published archive")
                self.assertEqual((directory / (name + ".sha256")).read_text().split()[0],
                                 hashlib.sha256(b"original published archive").hexdigest())

    def test_batch_wait_selects_exact_tag_when_multiple_releases_share_a_commit(self):
        sha = "a" * 40
        records = [
            {"id": 10, "head_sha": sha, "event": "push", "display_title": "Release yaml-rt-core/v0.3.3", "html_url": "core-run"},
            {"id": 20, "head_sha": sha, "event": "push", "display_title": "Release yaml-rt-cli/v0.3.3", "html_url": "cli-run"},
        ]
        with patch.object(release, "run", return_value=json.dumps([{"workflow_runs": records}])), \
             patch.object(release.subprocess, "run") as watch, contextlib.redirect_stdout(io.StringIO()):
            release.wait_release("yaml-rt-core/v0.3.3", sha, 60)
            self.assertEqual(watch.call_args.args[0][:4], ["gh", "run", "watch", "10"])
            self.assertIn("--exit-status", watch.call_args.args[0])

    def test_batch_wait_reports_failed_workflow_without_retrying_or_dispatching(self):
        record = {"id": 10, "head_sha": "abc", "event": "push", "display_title": "Release yaml-rt-cli/v0.3.3", "html_url": "failed-run"}
        with patch.object(release, "run", return_value=json.dumps([{"workflow_runs": [record]}])), \
             patch.object(release.subprocess, "run", side_effect=subprocess.CalledProcessError(1, ["gh", "run", "watch"])), \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "failed-run.*No further tags"):
                release.wait_release("yaml-rt-cli/v0.3.3", "abc", 60)

    def test_prepare_cli_defaults_to_auto(self):
        with patch.object(release.sys, "argv", ["release", "prepare-all", "--dry-run"]), \
             patch.object(release, "prepare_all") as prepare:
            release.main()
            prepare.assert_called_once_with("auto", True, [])
        with patch.object(release.sys, "argv", ["release", "prepare", "yaml-rt-cli"]), \
             patch.object(release, "prepare") as prepare:
            release.main()
            prepare.assert_called_once_with("yaml-rt-cli", "auto")

    def test_workflow_limits_cli_artifacts(self):
        workflow = (release.ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("if: needs.validate.outputs.package == 'yaml-rt-cli'", workflow)
        self.assertIn("run-name: Release ${{ github.ref_name }}", workflow)
        self.assertNotIn("--workspace", workflow)
        self.assertNotIn("workflow_dispatch:", workflow)
        self.assertNotIn("cargo release", workflow)


@unittest.skipUnless(shutil.which("cargo"), "Cargo unavailable")
class CargoReleaseIntegration(unittest.TestCase):
    def setUp(self):
        result = subprocess.run(["cargo", "release", "--version"], capture_output=True, text=True)
        if result.stdout.strip() != f"cargo-release {release.TOOL_VERSION}":
            self.skipTest(f"cargo-release {release.TOOL_VERSION} required")
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        deps = {"yaml-rt-rfc9535": ("yaml-rt-core",), "yaml-rt-serde": ("yaml-rt-core",),
                "yaml-rt-schema": ("yaml-rt-core",),
                "yaml-rt": ("yaml-rt-core", "yaml-rt-derive", "yaml-rt-serde"),
                "yaml-rt-cli": ("yaml-rt-core", "yaml-rt-rfc9535", "yaml-rt-schema")}
        manifest = '[workspace]\nmembers = ["crates/*"]\nresolver = "3"\n[workspace.dependencies]\n'
        for name in release.PACKAGES:
            manifest += f'{name} = {{ path = "crates/{name}", version = "0.3.2" }}\n'
            directory = self.root / "crates" / name
            (directory / "src").mkdir(parents=True)
            (directory / "src/lib.rs").write_text("")
            (directory / "README.md").write_text(name)
            (directory / "CHANGELOG.md").write_text("# Changelog\n\n## Unreleased\n\n## 0.3.2 - 2026-10-17\n\n- Initial fixture.\n")
            (directory / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion = "0.3.2"\nedition = "2024"\n'
                'description = "release fixture"\nlicense = "MIT"\n[dependencies]\n' +
                ''.join(f'{dep}.workspace = true\n' for dep in deps.get(name, ())))
        (self.root / "Cargo.toml").write_text(manifest)
        (self.root / ".gitignore").write_text("/target/\n")
        shutil.copy(release.ROOT / "release.toml", self.root)
        (self.root / "scripts").mkdir()
        shutil.copy(release.ROOT / "scripts/convco-release.yml", self.root / "scripts/convco-release.yml")
        self.execute("git", "init", "-b", "main")
        self.execute("git", "config", "user.email", "fixture@example.invalid")
        self.execute("git", "config", "user.name", "Release fixture")
        self.execute("cargo", "generate-lockfile", "--offline")
        self.execute("git", "add", ".")
        self.execute("git", "commit", "-m", "chore: initialize release fixture")

    def execute(self, *args):
        result = subprocess.run(args, cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def bump(self, name, level):
        with patch.object(release, "ROOT", self.root), contextlib.redirect_stdout(io.StringIO()):
            release.prepare(name, level)
        return {name: tomllib.loads((self.root / "crates" / name / "Cargo.toml").read_text())["package"]["version"]
                for name in release.PACKAGES}

    def test_cli_only(self):
        versions = self.bump("yaml-rt-cli", "patch")
        self.assertEqual(versions.pop("yaml-rt-cli"), "0.3.3")
        self.assertEqual(set(versions.values()), {"0.3.2"})
        self.assertNotIn("Cargo.toml", self.execute("git", "diff", "--name-only").splitlines())

    def test_jsonpath_only(self):
        versions = self.bump("yaml-rt-rfc9535", "patch")
        self.assertEqual(versions.pop("yaml-rt-rfc9535"), "0.3.3")
        self.assertEqual(set(versions.values()), {"0.3.2"})

    def test_core_compatible_patch(self):
        before = (self.root / "Cargo.toml").read_text()
        versions = self.bump("yaml-rt-core", "patch")
        self.assertEqual(versions.pop("yaml-rt-core"), "0.3.3")
        self.assertEqual(set(versions.values()), {"0.3.2"})
        self.assertEqual((self.root / "Cargo.toml").read_text(), before)

    def test_core_incompatible_minor(self):
        versions = self.bump("yaml-rt-core", "minor")
        self.assertEqual(versions.pop("yaml-rt-core"), "0.4.0")
        self.assertEqual(set(versions.values()), {"0.3.2"})
        deps = tomllib.loads((self.root / "Cargo.toml").read_text())["workspace"]["dependencies"]
        self.assertEqual(deps["yaml-rt-core"]["version"], "0.4.0")
        data = json.loads(self.execute("cargo", "metadata", "--offline", "--locked", "--format-version", "1"))
        dependents = {p["name"] for p in data["packages"] if any(d["name"] == "yaml-rt-core" and d["req"] == "^0.4.0" for d in p["dependencies"])}
        self.assertEqual(dependents, {"yaml-rt", "yaml-rt-rfc9535", "yaml-rt-schema", "yaml-rt-serde", "yaml-rt-cli"})
        # Actual Cargo graph confirms prerequisite-first publication is possible.
        completed = set()
        remaining = {p["id"]: p for p in data["packages"]}
        while remaining:
            ready = [p for p in remaining.values() if all(d["name"] in completed for d in p["dependencies"])]
            self.assertTrue(ready, "dependency cycle")
            for p in ready:
                completed.add(p["name"])
                del remaining[p["id"]]

    def baseline(self):
        self.execute("git", "tag", "-a", "v0.3.2", "-m", "Synchronized fixture release")

    def commit(self):
        self.execute("git", "add", ".")
        self.execute("git", "commit", "--allow-empty", "-m", "fix: change fixture source")

    def change(self, *names):
        for name in names:
            (self.root / "crates" / name / "src/lib.rs").write_text("pub fn changed() {}\n")
        self.commit()

    def batch(self, dry_run=False, overrides=(), bump="patch"):
        with patch.object(release, "ROOT", self.root), contextlib.redirect_stdout(io.StringIO()):
            return release.prepare_all(bump=bump, dry_run=dry_run, overrides=overrides)

    def versions(self):
        return {name: tomllib.loads((self.root / "crates" / name / "Cargo.toml").read_text())["package"]["version"]
                for name in release.PACKAGES}

    def release_notes(self, names):
        for name in names:
            path = self.root / "crates" / name / "CHANGELOG.md"
            version = self.versions()[name]
            path.write_text(path.read_text().replace("## Unreleased\n", f"## Unreleased\n\n## {version} - 2026-10-17\n\n- Fixture changes.\n"))
        self.commit()

    def test_batch_unchanged_and_root_docs_are_skipped(self):
        self.baseline()
        (self.root / "README.md").write_text("Repository documentation only\n")
        self.commit()
        self.assertEqual(self.batch(), [])
        self.assertFalse(self.execute("git", "status", "--porcelain").strip())

    def test_batch_cli_dry_run_does_not_change_files_or_refs(self):
        self.baseline()
        self.change("yaml-rt-cli")
        before = {p: (self.root / p).read_bytes() for p in self.execute("git", "ls-files").splitlines()}
        refs = self.execute("git", "show-ref")
        self.assertEqual(self.batch(dry_run=True), ["yaml-rt-cli"])
        self.assertEqual(before, {p: (self.root / p).read_bytes() for p in before})
        self.assertEqual(refs, self.execute("git", "show-ref"))
        self.assertFalse(self.execute("git", "status", "--porcelain").strip())

    def test_batch_cli_only_and_repeated_preparation(self):
        self.baseline()
        self.change("yaml-rt-cli")
        self.assertEqual(self.batch(), ["yaml-rt-cli"])
        versions = self.versions()
        self.assertEqual(versions.pop("yaml-rt-cli"), "0.3.3")
        self.assertEqual(set(versions.values()), {"0.3.2"})
        self.commit()
        self.assertEqual(self.batch(), ["yaml-rt-cli"])
        self.assertEqual(self.versions()["yaml-rt-cli"], "0.3.3")
        self.assertFalse(self.execute("git", "status", "--porcelain").strip())

    def test_batch_core_patch_does_not_select_unchanged_dependents(self):
        self.baseline()
        self.change("yaml-rt-core")
        self.assertEqual(self.batch(), ["yaml-rt-core"])
        versions = self.versions()
        self.assertEqual(versions.pop("yaml-rt-core"), "0.3.3")
        self.assertEqual(set(versions.values()), {"0.3.2"})

    def test_batch_incompatible_core_adds_affected_dependents(self):
        self.baseline()
        self.change("yaml-rt-core")
        order = self.batch(overrides=("yaml-rt-core=minor",))
        self.assertEqual(order, ["yaml-rt-core", "yaml-rt-rfc9535", "yaml-rt-serde", "yaml-rt-schema", "yaml-rt", "yaml-rt-cli"])
        versions = self.versions()
        self.assertEqual(versions.pop("yaml-rt-core"), "0.4.0")
        self.assertEqual(versions.pop("yaml-rt-derive"), "0.3.2")
        self.assertEqual(set(versions.values()), {"0.3.3"})

    def test_batch_reads_workspace_dependency_requirements(self):
        self.baseline()
        manifest = self.root / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace('version = "0.3.2" }', 'version = "~0.3.2" }', 1))
        self.commit()
        self.assertEqual(set(self.batch()), {"yaml-rt-rfc9535", "yaml-rt-serde", "yaml-rt-schema", "yaml-rt", "yaml-rt-cli"})
        self.assertEqual(self.versions()["yaml-rt-core"], "0.3.2")

    def test_batch_uses_package_specific_baseline_after_migration(self):
        self.baseline()
        self.change("yaml-rt-cli")
        self.batch()
        self.release_notes(("yaml-rt-cli",))
        self.execute("git", "tag", "-a", "yaml-rt-cli/v0.3.3", "-m", "CLI fixture")
        self.change("yaml-rt-core")
        self.assertEqual(self.batch(), ["yaml-rt-core"])
        self.assertEqual(self.versions()["yaml-rt-cli"], "0.3.3")

    def test_batch_respects_tracked_files_matching_gitignore(self):
        (self.root / ".gitignore").write_text("/target/\n*.data\n")
        path = self.root / "crates/yaml-rt-core/fixture.data"
        path.write_text("before")
        self.execute("git", "add", "--force", str(path))
        self.commit()
        self.baseline()
        path.write_text("after")
        self.commit()
        self.assertEqual(self.batch(), ["yaml-rt-core"])

    def test_batch_tag_dry_run_has_no_remote_or_mutating_actions(self):
        self.baseline()
        self.change("yaml-rt-cli")
        self.batch()
        self.release_notes(("yaml-rt-cli",))
        refs = self.execute("git", "show-ref")
        with patch.object(release, "ROOT", self.root), patch.object(release, "fetch") as fetch, \
             patch.object(release, "tag") as tag, patch.object(release, "wait_release") as wait, \
             patch.object(release, "registry") as registry, contextlib.redirect_stdout(io.StringIO()):
            release.tag_all(dry_run=True)
            fetch.assert_not_called()
            tag.assert_not_called()
            wait.assert_not_called()
            registry.assert_not_called()
        self.assertEqual(refs, self.execute("git", "show-ref"))
        self.assertFalse(self.execute("git", "status", "--porcelain").strip())

    def test_batch_tag_rejects_unprepared_changes(self):
        self.baseline()
        self.change("yaml-rt-cli")
        with patch.object(release, "ROOT", self.root), patch.object(release, "tag") as tag, \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "version is unprepared"):
                release.tag_all(dry_run=True)
            tag.assert_not_called()

    def test_batch_tag_requires_notes_for_every_candidate_before_pushing(self):
        self.baseline()
        self.change("yaml-rt-core", "yaml-rt-derive")
        self.batch()
        self.release_notes(("yaml-rt-core",))
        with patch.object(release, "ROOT", self.root), patch.object(release, "tag") as tag, \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "dated, reviewed changelog"):
                release.tag_all(dry_run=True)
            tag.assert_not_called()

    def test_batch_tag_stops_at_first_failed_workflow(self):
        self.baseline()
        self.change("yaml-rt-core", "yaml-rt-derive")
        self.release_notes(self.batch())
        with patch.object(release, "ROOT", self.root), patch.object(release, "fetch"), \
             patch.object(release, "main_commit"), patch.object(release, "ci_success"), \
             patch.object(release, "tag") as tag, patch.object(release, "wait_index") as indexed, \
             patch.object(release, "wait_release", side_effect=RuntimeError("workflow failed")), \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "workflow failed"):
                release.tag_all()
            tag.assert_called_once_with("yaml-rt-core")
            indexed.assert_not_called()

    def test_batch_tag_waits_for_each_workflow_and_registry_before_next_tag(self):
        self.baseline()
        self.change("yaml-rt-core", "yaml-rt-derive")
        self.release_notes(self.batch())
        events = []
        with patch.object(release, "ROOT", self.root), patch.object(release, "fetch"), \
             patch.object(release, "main_commit"), patch.object(release, "ci_success"), \
             patch.object(release, "tag", side_effect=lambda name: events.append(("tag", name))), \
             patch.object(release, "wait_release", side_effect=lambda tag, *args: events.append(("wait", tag.split("/")[0]))), \
             patch.object(release, "wait_index", side_effect=lambda name, *args: events.append(("indexed", name))), \
             contextlib.redirect_stdout(io.StringIO()):
            release.tag_all()
        self.assertEqual(events, [(step, name) for name in ("yaml-rt-core", "yaml-rt-derive") for step in ("tag", "wait", "indexed")])

    def test_batch_tag_resumes_existing_same_commit_tag_without_republication(self):
        self.baseline()
        self.change("yaml-rt-cli")
        self.release_notes(self.batch())
        release_tag = "yaml-rt-cli/v0.3.3"
        self.execute("git", "tag", "-a", release_tag, "-m", "CLI fixture")
        original_run = release.run
        def remote_or_local(*args, **kwargs):
            if args[:2] == ("git", "ls-remote"):
                return "existing-annotated-tag-object\trefs/tags/" + release_tag
            return original_run(*args, **kwargs)
        with patch.object(release, "ROOT", self.root), patch.object(release, "fetch"), \
             patch.object(release, "main_commit"), patch.object(release, "ci_success"), \
             patch.object(release, "run", side_effect=remote_or_local), \
             patch.object(release, "tag") as tag, patch.object(release, "validate_tag") as validate, \
             patch.object(release, "registry", return_value=True), patch.object(release, "wait_release") as wait, \
             patch.object(release, "wait_index"), contextlib.redirect_stdout(io.StringIO()):
            release.tag_all()
            tag.assert_not_called()
            validate.assert_called_once_with(release_tag)
            self.assertEqual(wait.call_args.args[0], release_tag)


    def auto_available(self):
        if not shutil.which("convco"):
            self.skipTest("Convco required for automatic release tests")
        self.assertEqual(self.execute("convco", "--version").strip(), f"convco {release.CONVCO_VERSION}")

    def conventional_change(self, name, message):
        path = self.root / "crates" / name / "src/lib.rs"
        text = path.read_text()
        path.write_text(text + f"\npub fn new_behavior_{text.count('pub fn')}() {{}}\n")
        self.execute("git", "add", ".")
        self.execute("git", "commit", "-m", message)

    def test_auto_uses_paths_and_does_not_leak_another_packages_breaking_change(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-core", "fix(core): correct package behavior")
        self.conventional_change("yaml-rt-cli", "feat(cli)!: change command behavior")
        self.assertEqual(self.batch(bump="auto"), ["yaml-rt-core", "yaml-rt-cli"])
        self.assertEqual(self.versions()["yaml-rt-core"], "0.3.3")
        self.assertEqual(self.versions()["yaml-rt-cli"], "0.4.0")
        self.assertEqual(self.versions()["yaml-rt"], "0.3.2")

    def test_auto_zero_major_feature_uses_convco_patch_and_ignores_local_force_flags(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-core", "feat(core): add compatible behavior")
        with patch.dict(release.os.environ, {"CONVCO_FORCE_MAJOR_BUMP": "true", "CONVCO_TREAT_MAJOR_ZERO_AS_STABLE": "true"}):
            self.assertEqual(self.batch(bump="auto"), ["yaml-rt-core"])
        self.assertEqual(self.versions()["yaml-rt-core"], "0.3.3")

    def test_auto_breaking_core_minor_updates_requirements_and_patches_dependents(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-core", "refactor(core): change public behavior\n\nBREAKING CHANGE: remove the prior API.")
        order = self.batch(bump="auto")
        self.assertEqual(order, ["yaml-rt-core", "yaml-rt-rfc9535", "yaml-rt-serde", "yaml-rt-schema", "yaml-rt", "yaml-rt-cli"])
        versions = self.versions()
        self.assertEqual(versions.pop("yaml-rt-core"), "0.4.0")
        self.assertEqual(versions.pop("yaml-rt-derive"), "0.3.2")
        self.assertEqual(set(versions.values()), {"0.3.3"})

    def test_auto_package_prefix_excludes_already_released_breaking_history(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-core", "feat(core)!: previous released behavior")
        self.execute("git", "tag", "-a", "yaml-rt-core/v0.3.2", "-m", "Independent fixture baseline")
        self.conventional_change("yaml-rt-core", "fix(core): correct subsequent behavior")
        self.assertEqual(self.batch(bump="auto"), ["yaml-rt-core"])
        self.assertEqual(self.versions()["yaml-rt-core"], "0.3.3")

    def test_auto_docs_only_change_gets_fallback_patch_and_retains_prepared_version(self):
        self.auto_available()
        self.baseline()
        (self.root / "crates/yaml-rt-cli/README.md").write_text("Improved command documentation\n")
        self.execute("git", "add", ".")
        self.execute("git", "commit", "-m", "docs(cli): document command usage")
        self.assertEqual(self.batch(bump="auto"), ["yaml-rt-cli"])
        self.assertEqual(self.versions()["yaml-rt-cli"], "0.3.3")
        self.commit()
        with patch.object(release, "automatic_bump", side_effect=AssertionError("already prepared")):
            self.assertEqual(self.batch(bump="auto"), ["yaml-rt-cli"])
        self.assertEqual(self.versions()["yaml-rt-cli"], "0.3.3")

    def test_auto_override_can_replace_convco_recommendation(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-cli", "feat(cli)!: change command behavior")
        self.assertEqual(self.batch(bump="auto", overrides=("yaml-rt-cli=patch",)), ["yaml-rt-cli"])
        self.assertEqual(self.versions()["yaml-rt-cli"], "0.3.3")

    def test_auto_dry_run_leaves_files_versions_and_refs_unchanged(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-cli", "feat(cli)!: change command behavior")
        before = {p: (self.root / p).read_bytes() for p in self.execute("git", "ls-files").splitlines()}
        refs = self.execute("git", "show-ref")
        self.assertEqual(self.batch(bump="auto", dry_run=True), ["yaml-rt-cli"])
        self.assertEqual(before, {p: (self.root / p).read_bytes() for p in before})
        self.assertEqual(refs, self.execute("git", "show-ref"))

    def test_auto_keeps_initial_versions_without_any_release_baseline(self):
        with patch.object(release, "automatic_bump", side_effect=AssertionError("no release history")):
            order = self.batch(bump="auto")
        self.assertEqual(set(order), set(release.PACKAGES))
        self.assertEqual(set(self.versions().values()), {"0.3.2"})
        self.assertFalse(self.execute("git", "status", "--porcelain").strip())

    def test_auto_single_package_preparation(self):
        self.auto_available()
        self.baseline()
        self.conventional_change("yaml-rt-cli", "feat(cli)!: change command behavior")
        versions = self.bump("yaml-rt-cli", "auto")
        self.assertEqual(versions.pop("yaml-rt-cli"), "0.4.0")
        self.assertEqual(set(versions.values()), {"0.3.2"})

    def stable_core_baseline(self):
        manifest = self.root / "crates/yaml-rt-core/Cargo.toml"
        manifest.write_text(manifest.read_text().replace('version = "0.3.2"', 'version = "1.2.3"'))
        workspace = self.root / "Cargo.toml"
        workspace.write_text(workspace.read_text().replace('version = "0.3.2" }', 'version = "1.2.3" }', 1))
        self.execute("cargo", "generate-lockfile", "--offline")
        self.commit()
        self.baseline()
        self.execute("git", "tag", "-a", "yaml-rt-core/v1.2.3", "-m", "Stable independent core fixture")

    def test_auto_stable_feature_bumps_minor(self):
        self.auto_available()
        self.stable_core_baseline()
        self.conventional_change("yaml-rt-core", "feat(core): add compatible behavior")
        self.assertEqual(self.batch(bump="auto"), ["yaml-rt-core"])
        self.assertEqual(self.versions()["yaml-rt-core"], "1.3.0")

    def test_auto_stable_breaking_change_bumps_major(self):
        self.auto_available()
        self.stable_core_baseline()
        self.conventional_change("yaml-rt-core", "feat(core)!: change public behavior")
        order = self.batch(bump="auto")
        self.assertEqual(order[0], "yaml-rt-core")
        self.assertEqual(self.versions()["yaml-rt-core"], "2.0.0")
        self.assertNotIn("yaml-rt-derive", order)

    def test_auto_rejects_prerelease_promotion_and_wrong_convco_baseline(self):
        self.auto_available()
        self.baseline()
        with patch.object(release, "ROOT", self.root):
            with self.assertRaisesRegex(RuntimeError, "prerelease"):
                release.automatic_bump("yaml-rt-core", "v0.3.2", "0.4.0-rc.1")
            with self.assertRaisesRegex(RuntimeError, "baseline differs"):
                release.automatic_bump("yaml-rt-core", "v0.3.2", "0.3.3")


if __name__ == "__main__":
    unittest.main()
