#!/usr/bin/env python3

import importlib.util
import json
import os
import subprocess
import sys
import contextlib
import io
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock


sys.dont_write_bytecode = True
SCRIPT = Path(__file__).parents[1] / "hashkey_release_gate.py"
SPEC = importlib.util.spec_from_file_location("hashkey_release_gate", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = gate
SPEC.loader.exec_module(gate)


class HashKeyReleaseGateTests(unittest.TestCase):
    class RecordingExecutor:
        def __init__(self, fail_fragments=()):
            self.commands = []
            self.fail_fragments = tuple(fail_fragments)

        def run(self, command, *, cwd, env):
            self.commands.append((command, cwd, env))
            rendered = " ".join(command)
            return int(any(fragment in rendered for fragment in self.fail_fragments))

    def run_recorded_source(self, fail_fragments=()):
        executor = self.RecordingExecutor(fail_fragments)
        checkout = Path(__file__).parents[2]
        patches = [
            mock.patch.object(gate, "_executor_factory", return_value=executor),
            mock.patch.object(
                gate,
                "_upstream_checkout",
                return_value=contextlib.nullcontext((checkout, None)),
            ),
            mock.patch.object(gate, "validate_dependency_files", return_value=[]),
            mock.patch.object(gate, "validate_release_identity", return_value=[]),
            mock.patch.object(gate, "validate_release_files", return_value=[]),
        ]
        with contextlib.ExitStack() as stack:
            for patch in patches:
                stack.enter_context(patch)
            outcome = gate.run_source_gate(gate.SourceGateInput(checkout))
        return outcome, executor

    def test_source_plan_contains_exact_required_evidence_ids(self):
        self.assertEqual(
            gate.SOURCE_EVIDENCE_IDS,
            (
                "gate-contract",
                "locked-dependency-graph",
                "documentation-contract",
                "standard-builds.workspace",
                "standard-builds.cli",
                "no-default-build",
                "static.fmt",
                "static.clippy-evm",
                "static.clippy-networks",
                "static.clippy-chisel",
                "golden.asset",
                "golden.stablecoin",
                "golden.factory",
                "golden.policy",
                "foundry-conformance",
                "cli.forge",
                "cli.anvil",
                "cli.cast",
                "cli.chisel",
                "non-hashkey-regression",
                "full-workspace",
            ),
        )

    def test_source_gate_runs_full_workspace_and_non_hashkey_regression_once(self):
        outcome, executor = self.run_recorded_source()
        self.assertTrue(outcome.success)
        commands = [" ".join(command) for command, _, _ in executor.commands]
        self.assertEqual(
            sum("--no-default-features --locked --no-fail-fast" in command for command in commands),
            1,
        )
        self.assertEqual(
            sum("--all-features --locked --no-fail-fast" in command for command in commands),
            1,
        )

    def test_source_gate_preserves_early_failure_when_last_command_passes(self):
        outcome, executor = self.run_recorded_source(("test_hashkey_release_gate.py",))
        self.assertFalse(outcome.success)
        self.assertEqual(outcome.results[-1].status, "passed")
        self.assertEqual(outcome.results[0].status, "failed")
        self.assertGreater(len(executor.commands), 1)

    def test_source_gate_retains_multiple_independent_failures(self):
        outcome, _ = self.run_recorded_source(("cargo metadata", "cargo nextest"))
        self.assertFalse(outcome.success)
        failed = {result.evidence_id for result in outcome.results if result.status == "failed"}
        self.assertIn("locked-dependency-graph", failed)
        self.assertIn("non-hashkey-regression", failed)
        self.assertIn("full-workspace", failed)

    def test_source_commands_own_stack_and_required_cargo_flags(self):
        outcome, executor = self.run_recorded_source()
        self.assertTrue(outcome.success)
        for _, _, env in executor.commands:
            self.assertEqual(env["RUST_MIN_STACK"], gate.TEST_RUST_MIN_STACK)
        commands = [" ".join(command) for command, _, _ in executor.commands]
        self.assertTrue(any("--locked" in command for command in commands))
        self.assertTrue(any("--no-fail-fast" in command for command in commands))

    def test_provided_upstream_checkout_accepts_only_clean_root_and_uses_managed_target(self):
        checkout = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: checkout.rmdir())
        check_output = mock.patch.object(
            gate.subprocess,
            "check_output",
            side_effect=[
                f"{checkout}\n",
                f"{gate.APPROVED_REVISION}\n",
                f"{gate.APPROVED_REPOSITORY}\n",
                "100644 abc\\tREADME\\n",
            ],
        )
        status = mock.patch.object(
            gate.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 0, "", ""),
        )
        with check_output, status:
            with gate._upstream_checkout(Path("."), checkout) as (resolved, target):
                self.assertEqual(resolved, checkout.resolve())
                self.assertTrue(target.is_dir())
                self.assertNotEqual(target, checkout)
        self.assertTrue(checkout.exists())

    def test_provided_upstream_checkout_rejects_dirty_or_submodule_state(self):
        checkout = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: checkout.rmdir())
        common = [
            f"{checkout}\n",
            f"{gate.APPROVED_REVISION}\n",
            f"{gate.APPROVED_REPOSITORY}\n",
        ]
        with mock.patch.object(gate.subprocess, "check_output", side_effect=common + ["100644 abc\\tREADME\\n"]), mock.patch.object(
            gate.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 0, " M README\n", ""),
        ):
            with self.assertRaisesRegex(RuntimeError, "not clean"):
                gate._validate_provided_checkout(checkout)

        with mock.patch.object(gate.subprocess, "check_output", side_effect=common + ["160000 abc\\tlib\\n"]), mock.patch.object(
            gate.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 0, "", ""),
        ):
            with self.assertRaisesRegex(RuntimeError, "submodule"):
                gate._validate_provided_checkout(checkout)

    def make_tar(self, names=gate.RELEASE_BINARIES, modes=0o755):
        path = Path(tempfile.mkstemp(suffix=".tar.gz")[1])
        self.addCleanup(lambda: path.unlink(missing_ok=True))
        with tarfile.open(path, "w:gz") as archive:
            for name in names:
                member = tarfile.TarInfo(name)
                member.mode = modes
                payload = name.encode()
                member.size = len(payload)
                archive.addfile(member, io.BytesIO(payload))
        return path

    def make_zip(self, names=gate.RELEASE_BINARIES):
        path = Path(tempfile.mkstemp(suffix=".zip")[1])
        self.addCleanup(lambda: path.unlink(missing_ok=True))
        with zipfile.ZipFile(path, "w") as archive:
            for name in names:
                member = zipfile.ZipInfo(name)
                member.external_attr = 0o100755 << 16
                archive.writestr(member, name.encode())
        return path

    def test_tar_and_zip_adapters_extract_only_four_regular_binaries(self):
        tar_path = self.make_tar()
        with gate.TarGzArtifactAdapter(tar_path).extract() as extracted:
            self.assertEqual(tuple(path.name for path in extracted.binaries), gate.RELEASE_BINARIES)
            root = extracted.root
            self.assertTrue(all(path.is_file() for path in extracted.binaries))
            self.assertTrue(all(path.stat().st_mode & 0o111 for path in extracted.binaries))
        self.assertFalse(root.exists())

        zip_path = self.make_zip()
        with gate.ZipArtifactAdapter(zip_path).extract() as extracted:
            self.assertEqual(tuple(path.name for path in extracted.binaries), gate.RELEASE_BINARIES)
            self.assertTrue(all(path.is_file() for path in extracted.binaries))

    def test_archive_adapters_reject_namespace_and_membership_violations(self):
        for name in ("../forge", "/forge", "C:\\forge", "dir/forge", "forge ", "FORGE"):
            path = self.make_tar((name, "cast", "anvil", "chisel"))
            with self.subTest(name=name), self.assertRaises(gate.ArtifactEvidenceError):
                with gate.TarGzArtifactAdapter(path).extract():
                    pass

        for names in (
            ("forge", "cast", "anvil"),
            ("forge", "cast", "anvil", "chisel", "extra"),
            ("forge", "cast", "anvil", "chisel", "chisel"),
        ):
            path = self.make_tar(names)
            with self.subTest(names=names), self.assertRaises(gate.ArtifactEvidenceError):
                with gate.TarGzArtifactAdapter(path).extract():
                    pass

    def test_tar_adapter_rejects_links_and_non_executable_binaries(self):
        path = Path(tempfile.mkstemp(suffix=".tar.gz")[1])
        self.addCleanup(lambda: path.unlink(missing_ok=True))
        with tarfile.open(path, "w:gz") as archive:
            link = tarfile.TarInfo("forge")
            link.type = tarfile.SYMTYPE
            link.linkname = "cast"
            archive.addfile(link)
            for name in gate.RELEASE_BINARIES[1:]:
                member = tarfile.TarInfo(name)
                member.mode = 0o755
                member.size = 1
                archive.addfile(member, io.BytesIO(b"x"))
        with self.assertRaises(gate.ArtifactEvidenceError):
            with gate.TarGzArtifactAdapter(path).extract():
                pass

        non_executable = self.make_tar(modes=0o644)
        with self.assertRaises(gate.ArtifactEvidenceError):
            with gate.TarGzArtifactAdapter(non_executable).extract():
                pass

    def test_zip_adapter_rejects_symlink_entries(self):
        symlink = Path(tempfile.mkstemp(suffix=".zip")[1])
        self.addCleanup(lambda: symlink.unlink(missing_ok=True))
        with zipfile.ZipFile(symlink, "w") as archive:
            for name in gate.RELEASE_BINARIES:
                info = zipfile.ZipInfo(name)
                info.external_attr = (0o120777 << 16) if name == "forge" else (0o100755 << 16)
                archive.writestr(info, b"x")
        with self.assertRaises(gate.ArtifactEvidenceError):
            with gate.ZipArtifactAdapter(symlink).extract():
                pass

    def test_artifact_adapter_rejects_unknown_suffix_as_usage_error(self):
        with self.assertRaises(gate.ArtifactUsageError):
            gate.artifact_adapter(Path("release.bin"))

    def test_target_policy_is_closed_and_maps_archive_and_execution(self):
        self.assertEqual(
            set(gate.TARGET_POLICIES),
            {
                "x86_64-unknown-linux-gnu",
                "x86_64-unknown-linux-musl",
                "aarch64-unknown-linux-gnu",
                "aarch64-unknown-linux-musl",
                "x86_64-apple-darwin",
                "aarch64-apple-darwin",
                "x86_64-pc-windows-msvc",
            },
        )
        self.assertTrue(gate.TARGET_POLICIES["x86_64-unknown-linux-gnu"].standalone_execution)
        self.assertTrue(gate.TARGET_POLICIES["x86_64-pc-windows-msvc"].windows)
        with self.assertRaises(gate.ArtifactUsageError):
            gate._target_policy("x86_64-unknown-linux-gnuu")

    def test_stable_release_tag_must_resolve_to_checkout_head(self):
        head = "a" * 40
        with mock.patch.object(gate.subprocess, "check_output", return_value=f"{head}\n") as check:
            gate._validate_release_tag_head("v1.7.1-hsk-h20", head)
        check.assert_called_once_with(
            ["git", "rev-parse", "refs/tags/v1.7.1-hsk-h20^{commit}"], text=True
        )
        with mock.patch.object(gate.subprocess, "check_output", return_value=f"{'b' * 40}\n"):
            with self.assertRaises(gate.ArtifactEvidenceError):
                gate._validate_release_tag_head("v1.7.1-hsk-h20", head)

    def test_release_identity_projections_cover_hsk_stable_ordinary_and_nightly(self):
        self.assertEqual(
            gate._release_identity_projection("v1.7.1-hsk-h20"),
            ("hashkey-stable", "1.7.1-hsk-h20", None),
        )
        self.assertEqual(
            gate._release_identity_projection("v1.7.1"),
            ("stable", "1.7.1", None),
        )
        nightly_sha = "a" * 40
        self.assertEqual(
            gate._release_identity_projection(f"nightly-{nightly_sha}"),
            ("nightly", "nightly", nightly_sha),
        )
        with self.assertRaises(gate.ArtifactUsageError):
            gate._release_identity_projection("release-latest")

    def test_artifact_gate_checks_native_surfaces_and_commit_identity(self):
        archive = self.make_tar()
        head = "a" * 40
        identities = [gate.BinaryIdentity("1.7.1-hsk-h20", head)] * 4
        patches = [
            mock.patch.object(gate, "_host_matches_target", return_value=True),
            mock.patch.object(gate, "_probe_artifact_surfaces", return_value=(identities, "probed")),
            mock.patch.object(gate, "_checkout_head", return_value=head),
            mock.patch.object(gate, "_validate_release_tag_head"),
            mock.patch.object(gate, "_run_standalone_execution", return_value="execution"),
        ]
        with contextlib.ExitStack() as stack:
            for patch in patches:
                stack.enter_context(patch)
            outcome = gate.run_artifact_gate(
                gate.ArtifactGateInput(archive, "x86_64-unknown-linux-gnu", "v1.7.1-hsk-h20")
            )
        self.assertTrue(outcome.success)
        self.assertEqual(
            {result.evidence_id for result in outcome.results},
            set(gate.ARTIFACT_EVIDENCE_IDS),
        )

    def test_artifact_gate_fails_closed_on_host_mismatch_and_identity_mismatch(self):
        archive = self.make_tar()
        with mock.patch.object(gate, "_host_matches_target", return_value=False):
            outcome = gate.run_artifact_gate(
                gate.ArtifactGateInput(archive, "x86_64-unknown-linux-gnu", "v1.7.1-hsk-h20")
            )
        self.assertFalse(outcome.success)
        self.assertEqual(outcome.results[0].evidence_id, "artifact.host")

        head = "a" * 40
        identities = [gate.BinaryIdentity("1.7.1-hsk-h20", "b" * 40)] * 4
        with contextlib.ExitStack() as stack:
            stack.enter_context(mock.patch.object(gate, "_host_matches_target", return_value=True))
            stack.enter_context(mock.patch.object(gate, "_probe_artifact_surfaces", return_value=(identities, "probed")))
            stack.enter_context(mock.patch.object(gate, "_checkout_head", return_value=head))
            stack.enter_context(mock.patch.object(gate, "_validate_release_tag_head"))
            outcome = gate.run_artifact_gate(
                gate.ArtifactGateInput(archive, "x86_64-unknown-linux-gnu", "v1.7.1-hsk-h20")
            )
        self.assertFalse(outcome.success)
        self.assertEqual(
            next(result for result in outcome.results if result.evidence_id == "artifact.identity").status,
            "failed",
        )

    def test_cli_rejects_legacy_pin_print_flags(self):
        for flag in ("--print-approved-repository", "--print-approved-revision"):
            result = subprocess.run(
                [sys.executable, SCRIPT, "metadata", flag],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("unrecognized arguments", result.stderr)

    def test_approved_manifest_and_lock_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text(
                f'''[workspace]
members = ["crate"]

[workspace.package]
version = "{gate.RELEASE_VERSION}"

[workspace.dependencies]
hsk-h20-config = {{ git = "{gate.APPROVED_REPOSITORY}", branch = "{gate.H20_BRANCH}" }}
hsk-h20-precompiles = {{ git = "{gate.APPROVED_REPOSITORY}", branch = "{gate.H20_BRANCH}" }}
''',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text(
                f'''version = 4

[[package]]
name = "hsk-h20-config"
version = "0.1.0"
source = "{gate.H20_LOCK_SOURCE}"

[[package]]
name = "hsk-h20-precompiles"
version = "0.1.0"
source = "{gate.H20_LOCK_SOURCE}"

[[package]]
name = "h20-precompile-macros"
version = "0.1.0"
source = "{gate.H20_LOCK_SOURCE}"

[[package]]
name = "h20-precompile-storage"
version = "0.1.0"
source = "{gate.H20_LOCK_SOURCE}"
''',
                encoding="utf-8",
            )

            self.assertEqual(gate.validate_dependency_files(root), [])

    def test_rejects_wrong_h20_branch_and_base_dependency(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text(
                f'''[workspace]
members = []

[workspace.package]
version = "{gate.RELEASE_VERSION}"

[workspace.dependencies]
hsk-h20-config = {{ git = "{gate.APPROVED_REPOSITORY}", branch = "wrong" }}
hsk-h20-precompiles = {{ git = "https://github.com/base/base", rev = "deadbeef" }}
''',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")

            errors = gate.validate_dependency_files(root)

            self.assertTrue(any("hsk-h20-config" in error for error in errors))
            self.assertTrue(any("base/base" in error for error in errors))

    def test_rejects_external_path_dependency(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            root.mkdir()
            (root / "Cargo.toml").write_text(
                f'''[workspace]
members = []

[workspace.package]
version = "{gate.RELEASE_VERSION}"

[workspace.dependencies]
hsk-h20-config = {{ git = "{gate.APPROVED_REPOSITORY}", branch = "{gate.H20_BRANCH}" }}
hsk-h20-precompiles = {{ git = "{gate.APPROVED_REPOSITORY}", branch = "{gate.H20_BRANCH}" }}
tempo-revm = {{ path = "../tempo-spike/crates/revm" }}
''',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")

            errors = gate.validate_dependency_files(root)

            self.assertTrue(any("external path dependency" in error for error in errors))

    def test_single_core_dependency_universe_and_release_version_pass(self):
        metadata = self.metadata(
            ("alloy-primitives", "1.6.0", "registry+alloy-core"),
            ("alloy-sol-types", "1.6.0", "registry+alloy-core"),
            ("alloy-json-abi", "1.6.0", "registry+alloy-core"),
            ("alloy-dyn-abi", "1.6.0", "registry+alloy-core"),
            ("alloy-evm", "0.37.1", "registry+alloy-evm"),
            ("revm", "41.0.0", "registry+revm"),
            workspace_package=("forge", gate.RELEASE_VERSION, None),
        )

        self.assertEqual(gate.validate_metadata(metadata), [])

    def test_rejects_duplicate_core_dependency_universe(self):
        metadata = self.metadata(
            ("alloy-primitives", "1.6.0", "registry+alloy-core"),
            ("alloy-primitives", "1.7.0", "git+alloy-core"),
            ("alloy-sol-types", "1.6.0", "registry+alloy-core"),
            ("alloy-json-abi", "1.6.0", "registry+alloy-core"),
            ("alloy-dyn-abi", "1.6.0", "registry+alloy-core"),
            ("alloy-evm", "0.37.1", "registry+alloy-evm"),
            ("alloy-evm", "0.38.0", "git+alloy-evm"),
            ("revm", "41.0.0", "registry+revm"),
            ("revm", "42.0.0", "git+revm"),
        )

        errors = gate.validate_metadata(metadata)

        self.assertTrue(any("alloy-primitives" in error for error in errors))
        self.assertTrue(any("alloy-evm" in error for error in errors))
        self.assertTrue(any("revm" in error for error in errors))

    def test_rejects_non_release_workspace_package(self):
        metadata = self.metadata(
            ("alloy-primitives", "1.6.0", "registry+alloy-core"),
            ("alloy-sol-types", "1.6.0", "registry+alloy-core"),
            ("alloy-json-abi", "1.6.0", "registry+alloy-core"),
            ("alloy-dyn-abi", "1.6.0", "registry+alloy-core"),
            ("alloy-evm", "0.37.1", "registry+alloy-evm"),
            ("revm", "41.0.0", "registry+revm"),
            workspace_package=("forge", "1.7.2", None),
        )

        self.assertTrue(
            any("forge must be version 1.7.1" in error for error in gate.validate_metadata(metadata))
        )

    def test_release_metadata_records_exact_compatibility_revisions(self):
        commit = "1" * 40

        metadata = gate.build_release_metadata("v1.7.1-hsk-h20", commit)

        self.assertEqual(metadata["release"]["foundry_commit"], commit)
        self.assertEqual(metadata["release"]["binaries"], ["forge", "cast", "anvil", "chisel"])
        self.assertEqual(metadata["h20"]["semantic_revision"], gate.APPROVED_REVISION)
        self.assertEqual(metadata["h20"]["binding_revision"], gate.APPROVED_REVISION)
        self.assertEqual(
            metadata["compatibility"]["tempo"]["revision"], gate.TEMPO_REVISION
        )
        self.assertEqual(metadata["compatibility"]["reth"]["revision"], gate.RETH_REVISION)
        self.assertEqual(
            metadata["compatibility"]["op_revm"]["revision"], gate.OP_REVM_REVISION
        )
        self.assertEqual(
            metadata["compatibility"]["op_alloy"]["revision"], gate.OP_ALLOY_REVISION
        )
        self.assertIn("hashkey", metadata["build"]["features"])
        self.assertFalse(metadata["profile"]["production_fidelity"])

    def test_release_metadata_rejects_non_hsk_tag(self):
        with self.assertRaisesRegex(ValueError, "HSK release tag"):
            gate.build_release_metadata("v1.7.1", "1" * 40)

    def test_release_metadata_generation_and_gated_validation_round_trip(self):
        commit = "2" * 40
        metadata = gate.build_release_metadata("v1.7.1-hsk-h20", commit)
        self.assertEqual(
            gate.validate_release_metadata(
                metadata, expected_tag="v1.7.1-hsk-h20", expected_commit=commit
            ),
            [],
        )
        self.assertTrue(
            gate.validate_release_metadata(
                metadata, expected_tag="v1.7.1-hsk-h20", expected_commit="3" * 40
            )
        )

    def test_repository_release_contract_passes(self):
        root = SCRIPT.parents[2]

        self.assertEqual(gate.validate_release_identity(root), [])
        self.assertEqual(gate.validate_release_files(root), [])

    def test_workspace_test_environment_owns_required_stack_policy(self):
        environment = gate.workspace_test_environment()

        self.assertEqual(
            environment.get("RUST_MIN_STACK"),
            gate.TEST_RUST_MIN_STACK,
            "workspace runtime test commands must set RUST_MIN_STACK",
        )

    def test_cast_opts_parse_tests_require_module_owned_stack_policy(self):
        # Focused regression for the Cast `opts::tests::parse_*` stack overflows:
        # without the module-owned RUST_MIN_STACK the debug test harness thread
        # overflows in deep clap parsing; with it the tests pass. Runs against a
        # prebuilt cast test binary and skips when the workspace is not built.
        candidates = [
            path
            for path in sorted(
                (Path(__file__).parents[3] / "target/debug/deps").glob("cast-*")
            )
            if path.is_file() and os.access(path, os.X_OK)
        ]
        if not candidates:
            self.skipTest("cast test binary not built; run the workspace build first")

        base_env = dict(os.environ)
        base_env.pop("RUST_MIN_STACK", None)
        overflows = []
        for binary in candidates:
            without = subprocess.run(
                [binary, "opts::tests::parse_call_data", "--exact"],
                env=base_env,
                capture_output=True,
                text=True,
                timeout=300,
            )
            if without.returncode < 0:
                # Killed by a signal: the debug test thread aborts on stack
                # overflow. Plain usage errors (exit 2) are CLI binaries, not
                # test binaries, and must not be treated as overflows.
                overflows.append((binary, without))
        self.assertTrue(
            overflows,
            "no cast test binary overflowed without RUST_MIN_STACK; "
            "the stack policy is no longer required",
        )

        for binary, _ in overflows:
            with_env = subprocess.run(
                [binary, "opts::tests::parse_call_data", "--exact"],
                env=gate.workspace_test_environment(),
                capture_output=True,
                text=True,
                timeout=300,
            )
            self.assertEqual(
                with_env.returncode,
                0,
                f"{binary.name} parse_call_data failed with module-owned "
                f"RUST_MIN_STACK: {with_env.stderr}",
            )

    def test_cli_writes_release_metadata_without_running_cargo_metadata(self):
        root = SCRIPT.parents[2]
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "hashkey-release-metadata.json"

            subprocess.run(
                [
                    sys.executable,
                    SCRIPT,
                    "metadata",
                    "--root",
                    root,
                    "--output",
                    output,
                    "--tag",
                    "v1.7.1-hsk-h20",
                    "--commit",
                    "2" * 40,
                ],
                check=True,
            )

            metadata = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(metadata["release"]["tag"], "v1.7.1-hsk-h20")
            self.assertEqual(metadata["release"]["foundry_commit"], "2" * 40)

    @staticmethod
    def metadata(*packages, workspace_package=None):
        entries = [
            {
                "id": f"{name} {version} ({source})",
                "name": name,
                "version": version,
                "source": source,
                "manifest_path": f"/cargo/{name}-{version}/Cargo.toml",
                "dependencies": [],
            }
            for name, version, source in packages
        ]
        workspace_members = []
        if workspace_package is not None:
            name, version, source = workspace_package
            member_id = f"{name} {version} ({source})"
            entries.append(
                {
                    "id": member_id,
                    "name": name,
                    "version": version,
                    "source": source,
                    "manifest_path": f"/repo/crates/{name}/Cargo.toml",
                    "dependencies": [],
                }
            )
            workspace_members.append(member_id)
        return {
            "packages": entries,
            "workspace_members": workspace_members,
            "workspace_root": "/repo",
        }


if __name__ == "__main__":
    unittest.main()
