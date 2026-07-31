#!/usr/bin/env python3
"""Dependency and repository contract checks for the HashKey B20 release gate."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


APPROVED_REPOSITORY = "https://github.com/HSKChain/optimism"
APPROVED_REVISION = "efbccbcd344fd4b395032816c0bf5756b3995fb6"
RELEASE_VERSION = "1.7.1"
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
    args = parser.parse_args()

    if args.print_approved_repository:
        print(APPROVED_REPOSITORY)
        return 0
    if args.print_approved_revision:
        print(APPROVED_REVISION)
        return 0

    root = args.root.resolve()
    errors = validate_dependency_files(root)
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
