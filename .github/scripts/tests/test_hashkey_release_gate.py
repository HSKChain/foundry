#!/usr/bin/env python3

import importlib.util
import json
import os
import subprocess
import sys
import contextlib
import tempfile
import unittest
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
                "standard-builds",
                "no-default-build",
                "static",
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

    def test_cli_exposes_the_approved_upstream_source(self):
        repository = subprocess.run(
            [sys.executable, SCRIPT, "--print-approved-repository"],
            check=True,
            capture_output=True,
            text=True,
        )
        revision = subprocess.run(
            [sys.executable, SCRIPT, "--print-approved-revision"],
            check=True,
            capture_output=True,
            text=True,
        )

        self.assertEqual(repository.stdout.strip(), gate.APPROVED_REPOSITORY)
        self.assertEqual(revision.stdout.strip(), gate.APPROVED_REVISION)

    def test_approved_manifest_and_lock_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text(
                f'''[workspace]
members = ["crate"]

[workspace.package]
version = "{gate.RELEASE_VERSION}"

[workspace.dependencies]
hsk-b20-config = {{ git = "{gate.APPROVED_REPOSITORY}", rev = "{gate.APPROVED_REVISION}" }}
hsk-b20-precompiles = {{ git = "{gate.APPROVED_REPOSITORY}", rev = "{gate.APPROVED_REVISION}" }}
''',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text(
                f'''version = 4

[[package]]
name = "hsk-b20-config"
version = "0.1.0"
source = "git+{gate.APPROVED_REPOSITORY}?rev={gate.APPROVED_REVISION}#{gate.APPROVED_REVISION}"

[[package]]
name = "hsk-b20-precompiles"
version = "0.1.0"
source = "git+{gate.APPROVED_REPOSITORY}?rev={gate.APPROVED_REVISION}#{gate.APPROVED_REVISION}"
''',
                encoding="utf-8",
            )

            self.assertEqual(gate.validate_dependency_files(root), [])

    def test_rejects_moving_b20_revision_and_base_dependency(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text(
                f'''[workspace]
members = []

[workspace.package]
version = "{gate.RELEASE_VERSION}"

[workspace.dependencies]
hsk-b20-config = {{ git = "{gate.APPROVED_REPOSITORY}", branch = "main" }}
hsk-b20-precompiles = {{ git = "https://github.com/base/base", rev = "deadbeef" }}
''',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")

            errors = gate.validate_dependency_files(root)

            self.assertTrue(any("hsk-b20-config" in error for error in errors))
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
hsk-b20-config = {{ git = "{gate.APPROVED_REPOSITORY}", rev = "{gate.APPROVED_REVISION}" }}
hsk-b20-precompiles = {{ git = "{gate.APPROVED_REPOSITORY}", rev = "{gate.APPROVED_REVISION}" }}
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

        metadata = gate.build_release_metadata("v1.7.1-hsk-b20", commit)

        self.assertEqual(metadata["release"]["foundry_commit"], commit)
        self.assertEqual(metadata["release"]["binaries"], ["forge", "cast", "anvil", "chisel"])
        self.assertEqual(metadata["b20"]["semantic_revision"], gate.APPROVED_REVISION)
        self.assertEqual(metadata["b20"]["binding_revision"], gate.APPROVED_REVISION)
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
                    "--root",
                    root,
                    "--write-release-metadata",
                    output,
                    "--tag",
                    "v1.7.1-hsk-b20",
                    "--commit",
                    "2" * 40,
                ],
                check=True,
            )

            metadata = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(metadata["release"]["tag"], "v1.7.1-hsk-b20")
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
