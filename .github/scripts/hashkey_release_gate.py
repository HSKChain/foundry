#!/usr/bin/env python3
"""Dependency and repository contract checks for the HashKey B20 release gate."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


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
APPROVED_REVISION = "efbccbcd344fd4b395032816c0bf5756b3995fb6"
RELEASE_VERSION = "1.7.1"
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
        ".github/scripts/hashkey-artifact-smoke.sh",
        ".github/scripts/hashkey-release-gate.sh all",
        "--write-release-metadata",
        "hashkey-release-metadata.json",
    ):
        if required not in release_workflow:
            errors.append(f"release workflow must include {required}")

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
    expected_tag = rf"v{re.escape(RELEASE_VERSION)}-hsk-b20(?:[.-][0-9A-Za-z]+)*"
    if re.fullmatch(expected_tag, tag) is None:
        raise ValueError(f"HSK release tag must match {expected_tag}")
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
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--metadata", type=Path)
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--print-approved-repository", action="store_true")
    output.add_argument("--print-approved-revision", action="store_true")
    output.add_argument("--write-release-metadata", type=Path)
    parser.add_argument("--tag")
    parser.add_argument("--commit")
    args = parser.parse_args()

    if args.print_approved_repository:
        print(APPROVED_REPOSITORY)
        return 0
    if args.print_approved_revision:
        print(APPROVED_REVISION)
        return 0

    root = args.root.resolve()
    errors = validate_dependency_files(root)
    errors.extend(validate_release_identity(root))
    errors.extend(validate_release_files(root))

    if args.write_release_metadata:
        if not args.tag or not args.commit:
            parser.error("--write-release-metadata requires --tag and --commit")
        try:
            metadata = build_release_metadata(args.tag, args.commit)
        except ValueError as error:
            errors.append(str(error))
        if errors:
            for error in errors:
                print(f"error: {error}", file=sys.stderr)
            return 1
        args.write_release_metadata.write_text(
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
