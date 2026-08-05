#!/usr/bin/env python3
"""Dependency and repository contract checks for the HashKey B20 release gate."""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import stat
import tarfile
import tempfile
import tomllib
import zipfile
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal, Protocol


# Workspace runtime test commands execute in debug test harness threads whose
# default 2 MiB stack overflows in deep clap argument-parsing recursion (Cast
# `opts::tests::parse_*`). The evidence plan owns this policy: it is applied to
# every workspace test command the module spawns and must not depend on ambient
# shell configuration.
TEST_RUST_MIN_STACK = "4194304"


def workspace_test_environment() -> dict[str, str]:
    """Returns the environment for workspace runtime test commands.

    Merges the current process environment with the module-owned test stack
    policy.
    """
    env = dict(os.environ)
    env["RUST_MIN_STACK"] = TEST_RUST_MIN_STACK
    return env


APPROVED_REPOSITORY = "https://github.com/HSKChain/optimism"
APPROVED_REVISION = "ab1d97f342299b62964eabcccde404e76481eb7a"
RELEASE_VERSION = "1.7.1"
HSK_RELEASE_TAG_PATTERN = rf"v{re.escape(RELEASE_VERSION)}-hsk-b20(?:[.-][0-9A-Za-z]+)*"
ORDINARY_RELEASE_TAG_PATTERN = r"v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][0-9A-Za-z]+)*"
RELEASE_BASELINE_REVISION = "4072e48705af9d93e3c0f6e29e93b5e9a40caed8"
RUST_VERSION = "1.95"
TEMPO_REPOSITORY = "https://github.com/tempoxyz/tempo"
TEMPO_REVISION = "e5c794e53c529ad15287f688cf8328ee985ccef5"
RETH_REPOSITORY = "https://github.com/paradigmxyz/reth"
RETH_REVISION = "0d303f75409c3b8a1b760bf275680b7c2deaa2a5"
OP_REVM_REPOSITORY = "https://github.com/foundry-rs/op-revm"
OP_REVM_REVISION = "c10fd76e66fd46cebc08ba370aa58bde72b94140"
OP_ALLOY_REPOSITORY = "https://github.com/foundry-rs/optimism"
OP_ALLOY_REVISION = "a305f5a34d20699d2301bdb57e379e62bc04937f"
RELEASE_BINARIES = ("forge", "cast", "anvil", "chisel")
RELEASE_FEATURES = (
    "aws-kms",
    "gcp-kms",
    "turnkey",
    "cli",
    "asm-keccak",
    "js-tracer",
    "hashkey",
)
B20_PACKAGES = ("hsk-b20-config", "hsk-b20-precompiles")
ALLOY_CORE_PACKAGES = (
    "alloy-primitives",
    "alloy-sol-types",
    "alloy-json-abi",
    "alloy-dyn-abi",
)
SINGLETON_PACKAGES = (*ALLOY_CORE_PACKAGES, "alloy-evm", "revm")

BUILD_EVIDENCE_IDS = ("standard-builds.workspace", "standard-builds.cli")
STATIC_EVIDENCE_IDS = ("static.fmt", "static.clippy-evm", "static.clippy-networks", "static.clippy-chisel")
GOLDEN_EVIDENCE_IDS = (
    "golden.asset",
    "golden.stablecoin",
    "golden.factory",
    "golden.policy",
)
FOCUSED_EVIDENCE_IDS = (
    "foundry-conformance",
    "cli.forge",
    "cli.anvil",
    "cli.cast",
    "cli.chisel",
)
SOURCE_EVIDENCE_IDS = (
    "gate-contract",
    "locked-dependency-graph",
    "documentation-contract",
    *BUILD_EVIDENCE_IDS,
    "no-default-build",
    *STATIC_EVIDENCE_IDS,
    *GOLDEN_EVIDENCE_IDS,
    *FOCUSED_EVIDENCE_IDS,
    "non-hashkey-regression",
    "full-workspace",
)


@dataclass(frozen=True)
class SourceGateInput:
    root: Path
    upstream_checkout: Path | None = None


@dataclass(frozen=True)
class EvidenceResult:
    evidence_id: str
    status: Literal["passed", "failed", "blocked"]
    summary: str


@dataclass(frozen=True)
class GateOutcome:
    phase: Literal["source", "artifact"]
    results: tuple[EvidenceResult, ...]
    success: bool


class _CommandExecutor(Protocol):
    def run(self, command: list[str], *, cwd: Path, env: dict[str, str]) -> int: ...


class _HostCommandExecutor:
    def run(self, command: list[str], *, cwd: Path, env: dict[str, str]) -> int:
        rendered = shlex.join(command)
        print(f"[hashkey-source] {rendered}", file=sys.stderr)
        try:
            return subprocess.run(command, cwd=cwd, env=env, check=False).returncode
        except OSError as error:
            print(f"[hashkey-source] {error}", file=sys.stderr)
            return 127


_executor_factory = _HostCommandExecutor


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as file:
        return tomllib.load(file)


def dependency_tables(value: Any):
    if not isinstance(value, dict):
        return
    for key, child in value.items():
        if key in {"dependencies", "dev-dependencies", "build-dependencies"} and isinstance(
            child, dict
        ):
            yield child
        yield from dependency_tables(child)


def is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def validate_dependency_files(root: Path) -> list[str]:
    errors: list[str] = []
    manifest = load_toml(root / "Cargo.toml")
    workspace = manifest.get("workspace", {})
    dependencies = workspace.get("dependencies", {})

    if workspace.get("package", {}).get("version") != RELEASE_VERSION:
        errors.append(f"workspace package version must be {RELEASE_VERSION}")

    for package in B20_PACKAGES:
        dependency = dependencies.get(package)
        if not isinstance(dependency, dict):
            errors.append(f"{package} must be a workspace Git dependency")
            continue
        if dependency.get("git") != APPROVED_REPOSITORY:
            errors.append(f"{package} must use {APPROVED_REPOSITORY}")
        if dependency.get("rev") != APPROVED_REVISION:
            errors.append(f"{package} must pin rev {APPROVED_REVISION}")
        if "branch" in dependency or "tag" in dependency:
            errors.append(f"{package} must not use a moving branch or tag")
        if "path" in dependency:
            errors.append(f"{package} must not use a local path")

    for manifest_path in root.rglob("Cargo.toml"):
        if any(part in {".git", "target"} for part in manifest_path.parts):
            continue
        data = load_toml(manifest_path)
        for table in dependency_tables(data):
            for name, dependency in table.items():
                if not isinstance(dependency, dict):
                    continue
                source = dependency.get("git", "")
                if "github.com/base/base" in source:
                    errors.append(
                        f"direct base/base dependency found in {manifest_path.relative_to(root)}: {name}"
                    )
                path = dependency.get("path")
                if isinstance(path, str):
                    resolved = (manifest_path.parent / path).resolve()
                    if not is_within(resolved, root):
                        errors.append(
                            f"external path dependency found in {manifest_path.relative_to(root)}: {name} -> {path}"
                        )

    lock = load_toml(root / "Cargo.lock")
    packages = lock.get("package", [])
    expected_source = (
        f"git+{APPROVED_REPOSITORY}?rev={APPROVED_REVISION}#{APPROVED_REVISION}"
    )
    for package in B20_PACKAGES:
        matches = [entry for entry in packages if entry.get("name") == package]
        if len(matches) != 1:
            errors.append(f"Cargo.lock must contain exactly one {package} package")
        elif matches[0].get("source") != expected_source:
            errors.append(f"Cargo.lock {package} source must be {expected_source}")

    return errors


def locked_package_source(lock: dict[str, Any], package_name: str) -> str | None:
    matches = [entry for entry in lock.get("package", []) if entry.get("name") == package_name]
    if len(matches) != 1:
        return None
    source = matches[0].get("source")
    return source if isinstance(source, str) else None


def validate_release_identity(root: Path) -> list[str]:
    errors: list[str] = []
    manifest = load_toml(root / "Cargo.toml")
    workspace = manifest.get("workspace", {})
    workspace_package = workspace.get("package", {})
    dependencies = workspace.get("dependencies", {})

    if workspace_package.get("rust-version") != RUST_VERSION:
        errors.append(f"workspace rust-version must be {RUST_VERSION}")

    for package in (
        "tempo-chainspec",
        "tempo-primitives",
        "tempo-alloy",
        "tempo-evm",
        "tempo-revm",
        "tempo-contracts",
        "tempo-precompiles",
    ):
        dependency = dependencies.get(package)
        if not isinstance(dependency, dict):
            errors.append(f"{package} must be a workspace Git dependency")
            continue
        if dependency.get("git") != TEMPO_REPOSITORY or dependency.get("rev") != TEMPO_REVISION:
            errors.append(f"{package} must pin {TEMPO_REPOSITORY}@{TEMPO_REVISION}")

    lock = load_toml(root / "Cargo.lock")
    expected_sources = {
        "reth-ethereum-primitives": f"git+{RETH_REPOSITORY}?rev=0d303f7#{RETH_REVISION}",
        "op-revm": f"git+{OP_REVM_REPOSITORY}?rev={OP_REVM_REVISION}#{OP_REVM_REVISION}",
        "alloy-op-evm": (
            f"git+{OP_ALLOY_REPOSITORY}?branch=bump-alloy-evm-0-36-tempo#{OP_ALLOY_REVISION}"
        ),
    }
    for package, expected_source in expected_sources.items():
        source = locked_package_source(lock, package)
        if source != expected_source:
            errors.append(f"Cargo.lock {package} source must be {expected_source}")

    return errors


def parse_feature_line(line: str) -> set[str]:
    _, _, value = line.partition(":")
    if not value:
        _, _, value = line.partition("=")
    return {feature.strip().strip(",") for feature in value.replace(",", " ").split()}


def validate_release_files(root: Path) -> list[str]:
    errors: list[str] = []
    makefile = (root / "Makefile").read_text(encoding="utf-8")
    make_feature_lines = [line for line in makefile.splitlines() if "FEATURES ?=" in line]
    if len(make_feature_lines) != 2 or any(
        not set(RELEASE_FEATURES).issubset(parse_feature_line(line))
        for line in make_feature_lines
    ):
        errors.append("root Makefile default FEATURES must include the HSK release feature set")

    for relative in (".github/workflows/release.yml", ".github/workflows/docker-publish.yml"):
        workflow = (root / relative).read_text(encoding="utf-8")
        feature_lines = [line for line in workflow.splitlines() if "RUST_FEATURES:" in line]
        if len(feature_lines) != 1 or not set(RELEASE_FEATURES).issubset(
            parse_feature_line(feature_lines[0])
        ):
            errors.append(f"{relative} RUST_FEATURES must include the HSK release feature set")

    release_workflow = (root / ".github/workflows/release.yml").read_text(encoding="utf-8")
    for required in (
        ".github/scripts/hashkey-release-gate.sh source",
        ".github/scripts/hashkey-release-gate.sh artifact",
        "finalize-release:",
        "hashkey-release-metadata.json",
        "--validate-release-metadata",
        ".github/scripts/hashkey-release-gate.sh metadata",
        "--output hashkey-release-metadata.json",
    ):
        if required not in release_workflow:
            errors.append(f"release workflow must include {required}")
    for forbidden in (
        ".github/scripts/hashkey-release-gate.sh all",
        ".github/scripts/hashkey-artifact-smoke.sh",
        " basic",
        " execution",
    ):
        if forbidden in release_workflow:
            errors.append(f"release workflow must not include legacy artifact interface {forbidden}")

    finalize_index = release_workflow.find("  finalize-release:")
    if finalize_index < 0:
        finalize_index = len(release_workflow)
    pre_finalize = release_workflow[:finalize_index]
    if "gh release create" in pre_finalize or "gh release upload" in pre_finalize:
        errors.append("release publication must be owned by finalize-release")
    if "needs: [prepare, release, release-docker]" not in release_workflow:
        errors.append("finalize-release must depend on prepare, every release matrix job, and Docker")
    if "needs: [prepare, release]" not in release_workflow:
        errors.append("release-docker must depend on the complete release matrix")

    expected_targets = set(TARGET_POLICIES)
    workflow_targets = set(re.findall(r"target:\s*([^\s]+)", release_workflow))
    if workflow_targets != expected_targets:
        errors.append("release workflow target matrix must equal the closed artifact target map")
    expected_runners = {
        "x86_64-unknown-linux-gnu": "depot-ubuntu-22.04-16",
        "x86_64-unknown-linux-musl": "depot-ubuntu-22.04-16",
        "aarch64-unknown-linux-gnu": "depot-ubuntu-22.04-arm-16",
        "aarch64-unknown-linux-musl": "depot-ubuntu-22.04-arm-16",
        "x86_64-apple-darwin": "macos-14-large",
        "aarch64-apple-darwin": "macos-14",
        "x86_64-pc-windows-msvc": "depot-windows-latest-16",
    }
    for target, runner in expected_runners.items():
        if re.search(
            rf"- runner:\s*{re.escape(runner)}\s*\n\s+target:\s*{re.escape(target)}\b",
            release_workflow,
        ) is None:
            errors.append(f"release workflow must pair {target} with native runner {runner}")
    step_order = (
        "Archive binaries",
        "Validate final release archive",
        "Generate archive checksum",
        "Generate SBOM (SPDX)",
        "Sign archive with cosign (keyless)",
        "Upload complete release asset bundle",
    )
    step_positions = [release_workflow.find(f"- name: {name}") for name in step_order]
    if any(position < 0 for position in step_positions) or step_positions != sorted(step_positions):
        errors.append("release workflow must order archive, artifact, checksum, SBOM, sign, and upload steps")
    if "Verify native target host" not in release_workflow:
        errors.append("release workflow must prove native target host architecture")

    launcher = (root / ".github/scripts/hashkey-release-gate.sh").read_text(encoding="utf-8")
    if "exec " not in launcher or "hashkey_release_gate.py" not in launcher:
        errors.append("release gate shell must only resolve and exec the Python module")
    for forbidden in ("run_dependencies", "run_golden", "run_focused", "run_build_matrix", "run_static", "run_full", "case "):
        if forbidden in launcher:
            errors.append(f"release gate launcher must not own policy: {forbidden}")
    smoke_helper = (root / ".github/scripts/hashkey-artifact-smoke.sh").read_text(encoding="utf-8")
    if "MODE=" in smoke_helper or "basic|execution" in smoke_helper:
        errors.append("artifact smoke helper must not expose basic/execution modes")

    ci_workflow = (root / ".github/workflows/ci-hashkey.yml").read_text(encoding="utf-8")
    if ".github/scripts/hashkey-release-gate.sh source" not in ci_workflow:
        errors.append("HashKey CI must call the canonical source operation")
    if ".github/scripts/hashkey-release-gate.sh all" in ci_workflow:
        errors.append("HashKey CI must not call legacy all")
    makefile = (root / "Makefile").read_text(encoding="utf-8")
    if ".github/scripts/hashkey-release-gate.sh source" not in makefile:
        errors.append("Make must call the canonical source operation")

    required_docs = (
        root / "docs/hashkey-b20.md",
        root / "docs/hashkey-b20-config.md",
    )
    for path in required_docs:
        if not path.is_file():
            errors.append(f"missing HashKey release documentation: {path.relative_to(root)}")

    readme = (root / "README.md").read_text(encoding="utf-8")
    for link in ("./docs/hashkey-b20.md", "./docs/hashkey-b20-config.md"):
        if link not in readme:
            errors.append(f"README must link {link}")

    return errors


def build_release_metadata(tag: str, commit: str) -> dict[str, Any]:
    if re.fullmatch(HSK_RELEASE_TAG_PATTERN, tag) is None:
        raise ValueError(f"HSK release tag must match {HSK_RELEASE_TAG_PATTERN}")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("Foundry release commit must be a full lowercase Git SHA")

    return {
        "schema_version": 1,
        "release": {
            "tag": tag,
            "foundry_version": RELEASE_VERSION,
            "foundry_commit": commit,
            "foundry_baseline_revision": RELEASE_BASELINE_REVISION,
            "binaries": list(RELEASE_BINARIES),
        },
        "profile": {
            "selector": "hashkey",
            "execution_family": "optimism",
            "support_scope": "standalone-local",
            "production_fidelity": False,
        },
        "b20": {
            "semantics": "Beryl B20 v1",
            "repository": APPROVED_REPOSITORY,
            "semantic_revision": APPROVED_REVISION,
            "binding_revision": APPROVED_REVISION,
        },
        "compatibility": {
            "tempo": {"repository": TEMPO_REPOSITORY, "revision": TEMPO_REVISION},
            "reth": {"repository": RETH_REPOSITORY, "revision": RETH_REVISION},
            "op_revm": {"repository": OP_REVM_REPOSITORY, "revision": OP_REVM_REVISION},
            "op_alloy": {"repository": OP_ALLOY_REPOSITORY, "revision": OP_ALLOY_REVISION},
        },
        "build": {
            "rust_version": RUST_VERSION,
            "locked": True,
            "features": list(RELEASE_FEATURES),
        },
    }


def validate_release_metadata(
    metadata: dict[str, Any], *, expected_tag: str, expected_commit: str
) -> list[str]:
    errors: list[str] = []
    release = metadata.get("release")
    if not isinstance(release, dict):
        return ["release metadata must contain a release object"]
    if release.get("tag") != expected_tag:
        errors.append(f"release metadata tag must be {expected_tag}")
    if release.get("foundry_commit") != expected_commit:
        errors.append("release metadata commit does not match the gated checkout")
    if release.get("foundry_version") != RELEASE_VERSION:
        errors.append(f"release metadata version must be {RELEASE_VERSION}")
    if release.get("binaries") != list(RELEASE_BINARIES):
        errors.append("release metadata binary set is invalid")
    b20 = metadata.get("b20")
    if not isinstance(b20, dict) or b20.get("repository") != APPROVED_REPOSITORY:
        errors.append("release metadata B20 repository is invalid")
    if isinstance(b20, dict) and b20.get("semantic_revision") != APPROVED_REVISION:
        errors.append("release metadata B20 semantic revision is invalid")
    if isinstance(b20, dict) and b20.get("binding_revision") != APPROVED_REVISION:
        errors.append("release metadata B20 binding revision is invalid")
    return errors


def validate_metadata(metadata: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    packages = metadata.get("packages", [])

    for name in SINGLETON_PACKAGES:
        identities = {
            (package.get("version"), package.get("source"))
            for package in packages
            if package.get("name") == name
        }
        if len(identities) != 1:
            rendered = ", ".join(
                f"{version} ({source})" for version, source in sorted(identities)
            ) or "missing"
            errors.append(f"{name} must resolve to one package identity; found {rendered}")

    alloy_core_identities = {
        (package.get("version"), package.get("source"))
        for package in packages
        if package.get("name") in ALLOY_CORE_PACKAGES
    }
    if len(alloy_core_identities) != 1:
        rendered = ", ".join(
            f"{version} ({source})" for version, source in sorted(alloy_core_identities)
        )
        errors.append(f"Alloy core packages must share one universe; found {rendered}")

    package_by_id = {package.get("id"): package for package in packages}
    for member_id in metadata.get("workspace_members", []):
        member = package_by_id.get(member_id)
        if member is None:
            errors.append(f"workspace member missing from metadata: {member_id}")
            continue
        if member.get("version") != RELEASE_VERSION:
            errors.append(
                f"workspace package {member.get('name')} must be version {RELEASE_VERSION}"
            )
        for dependency in member.get("dependencies", []):
            source = dependency.get("source") or ""
            path = dependency.get("path")
            if "github.com/base/base" in source:
                errors.append(
                    f"workspace package {member.get('name')} directly depends on base/base"
                )
            if path and not is_within(Path(path).resolve(), Path(metadata["workspace_root"]).resolve()):
                errors.append(
                    f"workspace package {member.get('name')} uses external path dependency {path}"
                )

    return errors


def _command(
    executor: _CommandExecutor,
    root: Path,
    command: list[str],
    *,
    env: dict[str, str] | None = None,
) -> tuple[bool, str]:
    result = executor.run(
        command,
        cwd=root,
        env=workspace_test_environment() if env is None else env,
    )
    return result == 0, shlex.join(command)


def _command_group(
    executor: _CommandExecutor,
    root: Path,
    commands: list[list[str]],
    *,
    env: dict[str, str] | None = None,
) -> tuple[bool, str]:
    rendered: list[str] = []
    passed = True
    for command in commands:
        command_passed, command_rendered = _command(executor, root, command, env=env)
        rendered.append(command_rendered)
        passed = command_passed and passed
    return passed, "; ".join(rendered)


def _validate_provided_checkout(checkout: Path) -> None:
    try:
        top_level = Path(
            subprocess.check_output(
                ["git", "-C", str(checkout), "rev-parse", "--show-toplevel"],
                text=True,
            ).strip()
        ).resolve()
        revision = subprocess.check_output(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True
        ).strip()
        remote = subprocess.check_output(
            ["git", "-C", str(checkout), "remote", "get-url", "origin"],
            text=True,
        ).strip()
        status = subprocess.run(
            [
                "git",
                "-C",
                str(checkout),
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignored=matching",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        index = subprocess.check_output(
            ["git", "-C", str(checkout), "ls-files", "--stage"], text=True
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError(f"invalid upstream checkout: {error}") from error

    if top_level != checkout:
        raise RuntimeError("provided upstream path must be the checkout root")
    if remote != APPROVED_REPOSITORY:
        raise RuntimeError(f"upstream remote is {remote}, expected {APPROVED_REPOSITORY}")
    if revision != APPROVED_REVISION:
        raise RuntimeError(f"upstream checkout is {revision}, expected {APPROVED_REVISION}")
    if status.stdout.strip():
        raise RuntimeError("provided upstream checkout is not clean")
    if any(line.split(maxsplit=1)[0] == "160000" for line in index.splitlines() if line):
        raise RuntimeError("provided upstream checkout contains a submodule entry")
    if (checkout / ".gitmodules").exists():
        raise RuntimeError("provided upstream checkout contains .gitmodules")


@contextmanager
def _upstream_checkout(root: Path, provided: Path | None):
    if provided is not None:
        checkout = provided.resolve()
        _validate_provided_checkout(checkout)
        with tempfile.TemporaryDirectory(prefix="hashkey-optimism-target-") as target:
            yield checkout, Path(target)
        return

    with tempfile.TemporaryDirectory(prefix="hashkey-optimism-") as temporary:
        checkout = Path(temporary) / "optimism"
        try:
            subprocess.run(["git", "init", "--quiet", str(checkout)], check=True)
            subprocess.run(
                ["git", "-C", str(checkout), "remote", "add", "origin", APPROVED_REPOSITORY],
                check=True,
            )
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(checkout),
                    "fetch",
                    "--quiet",
                    "--depth",
                    "1",
                    "origin",
                    APPROVED_REVISION,
                ],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(checkout), "checkout", "--quiet", "--detach", "FETCH_HEAD"],
                check=True,
            )
            _validate_provided_checkout(checkout)
        except (OSError, subprocess.CalledProcessError, RuntimeError) as error:
            raise RuntimeError(f"managed upstream acquisition failed: {error}") from error
        yield checkout, Path(temporary) / "target"


def _golden_command(upstream: Path, suite: str) -> list[str]:
    return [
        "cargo",
        "test",
        "--locked",
        "--manifest-path",
        str(upstream / "rust/Cargo.toml"),
        "-p",
        "hsk-b20-precompiles",
        "--features",
        "test-utils",
        "--test",
        suite,
    ]


def _source_result(
    evidence_id: str,
    executor: _CommandExecutor,
    root: Path,
    commands: list[list[str]],
    *,
    env: dict[str, str] | None = None,
) -> EvidenceResult:
    try:
        passed, rendered = _command_group(executor, root, commands, env=env)
    except Exception as error:
        return EvidenceResult(evidence_id, "failed", f"{type(error).__name__}: {error}")
    return EvidenceResult(
        evidence_id,
        "passed" if passed else "failed",
        rendered if passed else f"command failed: {rendered}",
    )


def _source_validation_result(
    evidence_id: str, root: Path, validators: list[Any]
) -> EvidenceResult:
    errors: list[str] = []
    for validator in validators:
        errors.extend(validator(root))
    return EvidenceResult(
        evidence_id,
        "passed" if not errors else "failed",
        "validation passed" if not errors else "; ".join(errors),
    )


def run_source_gate(input: SourceGateInput) -> GateOutcome:
    root = input.root.resolve()
    executor = _executor_factory()
    results: list[EvidenceResult] = []

    results.append(
        _source_result(
            "gate-contract",
            executor,
            root,
            [[sys.executable, str(Path(__file__).resolve().parent / "tests/test_hashkey_release_gate.py")]],
        )
    )
    results.append(
        _source_result(
            "locked-dependency-graph",
            executor,
            root,
            [["cargo", "metadata", "--locked", "--all-features", "--format-version", "1"]],
        )
    )
    results.append(
        _source_validation_result(
            "documentation-contract",
            root,
            [validate_dependency_files, validate_release_identity, validate_release_files],
        )
    )
    for evidence_id, command in (
        (
            "standard-builds.workspace",
            ["cargo", "build", "--workspace", "--locked"],
        ),
        (
            "standard-builds.cli",
            [
                "cargo",
                "build",
                "--locked",
                "-p",
                "forge@1.7.1",
                "-p",
                "cast@1.7.1",
                "-p",
                "anvil@1.7.1",
                "-p",
                "chisel@1.7.1",
                "--features",
                "hashkey",
            ],
        ),
    ):
        results.append(_source_result(evidence_id, executor, root, [command]))
    results.append(
        _source_result(
            "no-default-build",
            executor,
            root,
            [["cargo", "build", "--workspace", "--no-default-features", "--locked"]],
        )
    )
    for evidence_id, command in (
        (
            "static.fmt",
            ["cargo", "+nightly", "fmt", "--all", "--", "--check"],
        ),
        (
            "static.clippy-evm",
            [
                "cargo",
                "+nightly",
                "clippy",
                "-p",
                "foundry-evm-core@1.7.1",
                "--all-targets",
                "--features",
                "hashkey",
                "--locked",
            ],
        ),
        (
            "static.clippy-networks",
            [
                "cargo",
                "+nightly",
                "clippy",
                "-p",
                "foundry-evm-networks@1.7.1",
                "--all-targets",
                "--all-features",
                "--locked",
            ],
        ),
        (
            "static.clippy-chisel",
            [
                "cargo",
                "+nightly",
                "clippy",
                "-p",
                "chisel@1.7.1",
                "--all-targets",
                "--features",
                "hashkey",
                "--locked",
            ],
        ),
    ):
        results.append(_source_result(evidence_id, executor, root, [command]))

    try:
        with _upstream_checkout(root, input.upstream_checkout) as (upstream, target_dir):
            golden_env = workspace_test_environment()
            if target_dir is not None:
                golden_env["CARGO_TARGET_DIR"] = str(target_dir / "target")
            for evidence_id, suite in zip(
                GOLDEN_EVIDENCE_IDS,
                (
                    "b20_asset_v1_golden",
                    "b20_stablecoin_v1_golden",
                    "b20_factory_v1_golden",
                    "b20_policy_v1_golden",
                ),
            ):
                results.append(
                    _source_result(
                        evidence_id,
                        executor,
                        root,
                        [_golden_command(upstream, suite)],
                        env=golden_env,
                    )
                )
    except RuntimeError as error:
        for evidence_id in GOLDEN_EVIDENCE_IDS:
            results.append(EvidenceResult(evidence_id, "blocked", str(error)))

    focused = {
        "foundry-conformance": [["cargo", "test", "--locked", "-p", "foundry-evm-core@1.7.1", "--features", "hashkey", "--test", "hashkey"]],
        "cli.forge": [["cargo", "test", "--locked", "-p", "forge@1.7.1", "--test", "cli", "--features", "hashkey", "hashkey::"]],
        "cli.anvil": [["cargo", "test", "--locked", "-p", "anvil@1.7.1", "--test", "it", "--features", "hashkey", "hashkey::"]],
        "cli.cast": [["cargo", "test", "--locked", "-p", "cast@1.7.1", "--test", "cli", "--features", "hashkey", "hashkey::hashkey_b20_anvil_cast_workflow", "--", "--exact"]],
        "cli.chisel": [["cargo", "test", "--locked", "-p", "chisel@1.7.1", "--test", "it", "--features", "hashkey", "repl::hashkey_b20_stateful_session", "--", "--exact"]],
    }
    for evidence_id in FOCUSED_EVIDENCE_IDS:
        results.append(_source_result(evidence_id, executor, root, focused[evidence_id]))

    results.append(
        _source_result(
            "non-hashkey-regression",
            executor,
            root,
            [["cargo", "nextest", "run", "--workspace", "--no-default-features", "--locked", "--no-fail-fast"]],
        )
    )
    results.append(
        _source_result(
            "full-workspace",
            executor,
            root,
            [["cargo", "nextest", "run", "--workspace", "--all-features", "--locked", "--no-fail-fast"]],
        )
    )
    return GateOutcome(
        "source",
        tuple(results),
        all(result.status == "passed" for result in results),
    )


MAX_ARCHIVE_MEMBERS = 4
MAX_ARCHIVE_BYTES = 256 * 1024 * 1024


class ArtifactUsageError(ValueError):
    """The artifact invocation is malformed or unsupported."""


class ArtifactEvidenceError(ValueError):
    """The archive is readable but does not satisfy the artifact contract."""


@dataclass(frozen=True)
class TargetPolicy:
    target: str
    archive_suffix: str
    windows: bool
    standalone_execution: bool
    host_architecture: str


TARGET_POLICIES = {
    policy.target: policy
    for policy in (
        TargetPolicy("x86_64-unknown-linux-gnu", ".tar.gz", False, True, "x86_64"),
        TargetPolicy("x86_64-unknown-linux-musl", ".tar.gz", False, False, "x86_64"),
        TargetPolicy("aarch64-unknown-linux-gnu", ".tar.gz", False, False, "aarch64"),
        TargetPolicy("aarch64-unknown-linux-musl", ".tar.gz", False, False, "aarch64"),
        TargetPolicy("x86_64-apple-darwin", ".tar.gz", False, False, "x86_64"),
        TargetPolicy("aarch64-apple-darwin", ".tar.gz", False, False, "aarch64"),
        TargetPolicy("x86_64-pc-windows-msvc", ".zip", True, False, "x86_64"),
    )
}

ARTIFACT_EVIDENCE_IDS = ("artifact.archive", "artifact.host", "artifact.surfaces", "artifact.identity", "artifact.execution")


@dataclass(frozen=True)
class ArtifactGateInput:
    archive: Path
    target: str
    release_tag: str


@dataclass(frozen=True)
class BinaryIdentity:
    version: str
    commit: str


@dataclass(frozen=True)
class ExtractedArtifact:
    root: Path
    binaries: tuple[Path, ...]


class _ArtifactAdapter(Protocol):
    @contextmanager
    def extract(self) -> ExtractedArtifact: ...


def _canonical_archive_name(name: str) -> str:
    normalized = name.replace("\\", "/")
    if not normalized or "\x00" in normalized:
        raise ArtifactEvidenceError("archive member has an invalid name")
    if normalized.startswith("/") or normalized.startswith("//"):
        raise ArtifactEvidenceError(f"archive member is absolute: {name}")
    if re.match(r"^[A-Za-z]:/", normalized):
        raise ArtifactEvidenceError(f"archive member is drive-qualified: {name}")
    components = normalized.split("/")
    if len(components) != 1 or components[0] in {".", ".."}:
        raise ArtifactEvidenceError(f"archive member is not root-level: {name}")
    if components[0].rstrip(" .") != components[0]:
        raise ArtifactEvidenceError(f"archive member has an aliasing suffix: {name}")
    return components[0]


def _check_archive_names(names: list[str], expected: tuple[str, ...]) -> list[str]:
    if len(names) != MAX_ARCHIVE_MEMBERS:
        raise ArtifactEvidenceError("archive must contain exactly four members")
    canonical = [_canonical_archive_name(name) for name in names]
    if len({name.casefold() for name in canonical}) != len(canonical):
        raise ArtifactEvidenceError("archive contains a case-fold name collision")
    if set(canonical) != set(expected):
        raise ArtifactEvidenceError(
            f"archive members must be exactly {', '.join(expected)}"
        )
    return canonical


@dataclass(frozen=True)
class TarGzArtifactAdapter:
    archive: Path
    expected_names: tuple[str, ...] = RELEASE_BINARIES
    require_executable: bool = True

    @contextmanager
    def extract(self) -> ExtractedArtifact:
        try:
            with tarfile.open(self.archive, mode="r:gz") as bundle:
                members = bundle.getmembers()
                canonical = _check_archive_names(
                    [member.name for member in members], self.expected_names
                )
                total_size = 0
                for member in members:
                    if not member.isfile() or member.issym() or member.islnk():
                        raise ArtifactEvidenceError(
                            f"archive member is not a regular file: {member.name}"
                        )
                    if member.size < 0:
                        raise ArtifactEvidenceError("archive member has a negative size")
                    total_size += member.size
                    if total_size > MAX_ARCHIVE_BYTES:
                        raise ArtifactEvidenceError("archive exceeds the size limit")
                    if self.require_executable and not member.mode & 0o111:
                        raise ArtifactEvidenceError(
                            f"Unix binary is not executable: {member.name}"
                        )
                with tempfile.TemporaryDirectory(prefix="hashkey-artifact-") as temporary:
                    root = Path(temporary)
                    binaries: list[Path] = []
                    for member, name in zip(members, canonical):
                        destination = root / name
                        source = bundle.extractfile(member)
                        if source is None:
                            raise ArtifactEvidenceError(
                                f"archive member cannot be read: {member.name}"
                            )
                        with source, destination.open("wb") as output:
                            while chunk := source.read(1024 * 1024):
                                output.write(chunk)
                        destination.chmod(member.mode & 0o777)
                        binaries.append(destination)
                    yield ExtractedArtifact(root, tuple(binaries))
        except ArtifactEvidenceError:
            raise
        except (OSError, EOFError, tarfile.TarError) as error:
            raise ArtifactEvidenceError(f"invalid tar.gz archive: {error}") from error


@dataclass(frozen=True)
class ZipArtifactAdapter:
    archive: Path
    expected_names: tuple[str, ...] = RELEASE_BINARIES
    require_executable: bool = False

    @contextmanager
    def extract(self) -> ExtractedArtifact:
        try:
            with zipfile.ZipFile(self.archive) as bundle:
                members = bundle.infolist()
                canonical = _check_archive_names(
                    [member.filename for member in members], self.expected_names
                )
                total_size = 0
                for member in members:
                    if member.flag_bits & 0x1:
                        raise ArtifactEvidenceError(
                            f"encrypted archive member: {member.filename}"
                        )
                    mode = member.external_attr >> 16
                    if member.is_dir() or (mode and not stat.S_ISREG(mode)):
                        raise ArtifactEvidenceError(
                            f"archive member is not a regular file: {member.filename}"
                        )
                    total_size += member.file_size
                    if total_size > MAX_ARCHIVE_BYTES:
                        raise ArtifactEvidenceError("archive exceeds the size limit")
                    if self.require_executable and not mode & 0o111:
                        raise ArtifactEvidenceError(
                            f"Windows binary is not executable: {member.filename}"
                        )
                with tempfile.TemporaryDirectory(prefix="hashkey-artifact-") as temporary:
                    root = Path(temporary)
                    binaries: list[Path] = []
                    for member, name in zip(members, canonical):
                        destination = root / name
                        with bundle.open(member) as source, destination.open("wb") as output:
                            while chunk := source.read(1024 * 1024):
                                output.write(chunk)
                        if self.require_executable:
                            destination.chmod((member.external_attr >> 16) & 0o777)
                        binaries.append(destination)
                    yield ExtractedArtifact(root, tuple(binaries))
        except ArtifactEvidenceError:
            raise
        except (OSError, EOFError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
            raise ArtifactEvidenceError(f"invalid zip archive: {error}") from error


def artifact_adapter(
    archive: Path, *, expected_names: tuple[str, ...] = RELEASE_BINARIES
) -> _ArtifactAdapter:
    name = archive.name.lower()
    if name.endswith(".tar.gz"):
        return TarGzArtifactAdapter(archive, expected_names)
    if name.endswith(".zip"):
        return ZipArtifactAdapter(archive, expected_names)
    raise ArtifactUsageError("archive must use .tar.gz or .zip suffix")


def _target_policy(target: str) -> TargetPolicy:
    try:
        return TARGET_POLICIES[target]
    except KeyError as error:
        raise ArtifactUsageError(f"unsupported release target: {target}") from error


def _host_architecture() -> str:
    machine = platform.machine().lower()
    if machine in {"amd64", "x86_64"}:
        return "x86_64"
    if machine in {"arm64", "aarch64"}:
        return "aarch64"
    return machine


def _host_matches_target(policy: TargetPolicy) -> bool:
    system = platform.system().lower()
    if policy.host_architecture != _host_architecture():
        return False
    if "windows" in policy.target:
        return system == "windows"
    if "apple-darwin" in policy.target:
        return system == "darwin"
    return system == "linux"


def _expected_binary_names(policy: TargetPolicy) -> tuple[str, ...]:
    suffix = ".exe" if policy.windows else ""
    return tuple(f"{name}{suffix}" for name in RELEASE_BINARIES)


def _release_identity_projection(tag: str) -> tuple[str, str, str | None]:
    if re.fullmatch(HSK_RELEASE_TAG_PATTERN, tag):
        return "hashkey-stable", tag[1:], None
    nightly = re.fullmatch(r"nightly-([0-9a-f]{40})", tag)
    if nightly:
        return "nightly", "nightly", nightly.group(1)
    if re.fullmatch(ORDINARY_RELEASE_TAG_PATTERN, tag):
        return "stable", tag[1:], None
    raise ArtifactUsageError(f"unsupported release tag: {tag}")


def _parse_binary_identity(output: str) -> BinaryIdentity:
    version_match = re.search(r"^Version:\s*(\S+)\s*$", output, re.MULTILINE)
    commit_match = re.search(r"^Commit SHA:\s*([0-9a-f]{40})\s*$", output, re.MULTILINE)
    if version_match is None or commit_match is None:
        raise ArtifactEvidenceError("binary --version output has no release identity")
    return BinaryIdentity(version_match.group(1), commit_match.group(1))


def _checkout_head() -> str:
    try:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise ArtifactEvidenceError(f"cannot resolve release checkout HEAD: {error}") from error


def _validate_release_tag_head(tag: str, head: str) -> None:
    release_class, _, nightly_sha = _release_identity_projection(tag)
    if nightly_sha is not None:
        if nightly_sha != head:
            raise ArtifactEvidenceError("nightly release tag does not match checkout HEAD")
        return
    try:
        tagged_head = subprocess.check_output(
            ["git", "rev-parse", f"refs/tags/{tag}^{{commit}}"], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise ArtifactEvidenceError(f"cannot resolve stable release tag {tag}: {error}") from error
    if tagged_head != head:
        raise ArtifactEvidenceError(
            f"{release_class} release tag {tag} does not point to checkout HEAD"
        )


def _run_binary(binary: Path, argument: str) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            [str(binary), argument],
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ArtifactEvidenceError(f"binary probe failed for {binary.name}: {error}") from error


def _probe_artifact_surfaces(
    binaries: tuple[Path, ...], policy: TargetPolicy
) -> tuple[list[BinaryIdentity], str]:
    identities: list[BinaryIdentity] = []
    for binary in binaries:
        version = _run_binary(binary, "--version")
        if version.returncode != 0:
            raise ArtifactEvidenceError(f"{binary.name} --version failed")
        help_output = _run_binary(binary, "--help")
        if help_output.returncode != 0:
            raise ArtifactEvidenceError(f"{binary.name} --help failed")
        identities.append(_parse_binary_identity(version.stdout))
    return identities, f"probed {len(binaries)} native {policy.target} binaries"


def _validate_binary_identities(identities: list[BinaryIdentity], tag: str, head: str) -> str:
    release_class, expected_version, _ = _release_identity_projection(tag)
    _validate_release_tag_head(tag, head)
    for identity in identities:
        if identity.version != expected_version:
            raise ArtifactEvidenceError(
                f"binary version {identity.version} does not match {expected_version}"
            )
        if not head.startswith(identity.commit):
            raise ArtifactEvidenceError(
                f"binary commit {identity.commit} does not match checkout HEAD"
            )
    if len({identity.commit for identity in identities}) != 1:
        raise ArtifactEvidenceError("binary commit identities do not agree")
    return f"{release_class} identity matches {head}"


def _run_standalone_execution(extracted: ExtractedArtifact) -> str:
    helper = Path(__file__).with_name("hashkey-artifact-smoke.sh")
    try:
        result = subprocess.run(
            ["bash", str(helper), str(extracted.root)],
            check=False,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ArtifactEvidenceError(f"standalone execution failed: {error}") from error
    if result.returncode != 0:
        raise ArtifactEvidenceError(result.stderr.strip() or "standalone execution failed")
    return "standalone HashKey execution passed"


def run_artifact_gate(input: ArtifactGateInput) -> GateOutcome:
    policy = _target_policy(input.target)
    archive = input.archive.resolve()
    if not archive.is_file():
        raise ArtifactUsageError(f"archive does not exist: {archive}")
    if not archive.name.lower().endswith(policy.archive_suffix):
        raise ArtifactUsageError(
            f"target {policy.target} requires a {policy.archive_suffix} archive"
        )
    if not _host_matches_target(policy):
        return GateOutcome(
            "artifact",
            (EvidenceResult("artifact.host", "failed", f"host does not match {policy.target}"),),
            False,
        )

    expected_names = _expected_binary_names(policy)
    try:
        adapter = artifact_adapter(archive, expected_names=expected_names)
        with adapter.extract() as extracted:
            results = [EvidenceResult("artifact.archive", "passed", "archive preflight and extraction passed")]
            results.append(EvidenceResult("artifact.host", "passed", f"native host matches {policy.target}"))
            try:
                identities, summary = _probe_artifact_surfaces(extracted.binaries, policy)
                results.append(EvidenceResult("artifact.surfaces", "passed", summary))
            except ArtifactEvidenceError as error:
                results.append(EvidenceResult("artifact.surfaces", "failed", str(error)))
                return GateOutcome("artifact", tuple(results), False)
            try:
                results.append(EvidenceResult("artifact.identity", "passed", _validate_binary_identities(identities, input.release_tag, _checkout_head())))
            except ArtifactEvidenceError as error:
                results.append(EvidenceResult("artifact.identity", "failed", str(error)))
            if policy.standalone_execution:
                try:
                    results.append(EvidenceResult("artifact.execution", "passed", _run_standalone_execution(extracted)))
                except ArtifactEvidenceError as error:
                    results.append(EvidenceResult("artifact.execution", "failed", str(error)))
            else:
                results.append(EvidenceResult("artifact.execution", "passed", "not required for this target"))
            return GateOutcome("artifact", tuple(results), all(result.status == "passed" for result in results))
    except ArtifactEvidenceError as error:
        return GateOutcome(
            "artifact",
            (EvidenceResult("artifact.archive", "failed", str(error)),),
            False,
        )


def cargo_metadata(root: Path) -> dict[str, Any]:
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--all-features", "--format-version", "1"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("source", "artifact", "metadata"))
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--upstream-checkout", type=Path)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--release-tag")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--validate-release-metadata", type=Path)
    parser.add_argument("--tag")
    parser.add_argument("--commit")
    args = parser.parse_args()

    root = args.root.resolve()
    if args.operation == "source":
        outcome = run_source_gate(SourceGateInput(root, args.upstream_checkout))
        for result in outcome.results:
            print(
                f"{result.status}: {result.evidence_id}: {result.summary}",
                file=sys.stderr,
            )
        return 0 if outcome.success else 1
    if args.operation == "artifact":
        if args.archive is None or args.target is None or args.release_tag is None:
            print("error: artifact requires --archive, --target, and --release-tag", file=sys.stderr)
            return 2
        try:
            outcome = run_artifact_gate(
                ArtifactGateInput(args.archive, args.target, args.release_tag)
            )
        except ArtifactUsageError as error:
            print(f"error: {error}", file=sys.stderr)
            return 2
        for result in outcome.results:
            print(
                f"{result.status}: {result.evidence_id}: {result.summary}",
                file=sys.stderr,
            )
        return 0 if outcome.success else 1
    if args.validate_release_metadata:
        if not args.tag or not args.commit:
            parser.error("--validate-release-metadata requires --tag and --commit")
        try:
            metadata = json.loads(args.validate_release_metadata.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            print(f"error: cannot read release metadata: {error}", file=sys.stderr)
            return 1
        errors = validate_release_metadata(
            metadata, expected_tag=args.tag, expected_commit=args.commit
        )
        if errors:
            for error in errors:
                print(f"error: {error}", file=sys.stderr)
            return 1
        return 0

    errors = validate_dependency_files(root)
    errors.extend(validate_release_identity(root))
    errors.extend(validate_release_files(root))

    if args.output:
        if args.operation != "metadata":
            parser.error("--output is only valid for the metadata operation")
        if not args.tag or not args.commit:
            parser.error("metadata --output requires --tag and --commit")
        try:
            metadata = build_release_metadata(args.tag, args.commit)
        except ValueError as error:
            errors.append(str(error))
        if errors:
            for error in errors:
                print(f"error: {error}", file=sys.stderr)
            return 1
        args.output.write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        return 0

    metadata = (
        json.loads(args.metadata.read_text(encoding="utf-8"))
        if args.metadata
        else cargo_metadata(root)
    )
    errors.extend(validate_metadata(metadata))

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    print(
        "HashKey dependency contract passed: "
        f"HSKChain/optimism@{APPROVED_REVISION[:10]}, Foundry {RELEASE_VERSION}, "
        "one Alloy/REVM universe, no base/base or external paths."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
