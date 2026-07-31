#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
APPROVED_REPOSITORY="$(
  PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/hashkey_release_gate.py" \
    --print-approved-repository
)"
APPROVED_REVISION="$(
  PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/hashkey_release_gate.py" \
    --print-approved-revision
)"
UPSTREAM_DIR="${HASHKEY_OPTIMISM_DIR:-}"
TEMP_UPSTREAM_DIR=""
export RUST_MIN_STACK="${RUST_MIN_STACK:-4194304}"

cleanup() {
  if [[ -n "$TEMP_UPSTREAM_DIR" && -d "$TEMP_UPSTREAM_DIR" ]]; then
    rm -rf -- "$TEMP_UPSTREAM_DIR"
  fi
}
trap cleanup EXIT

run_dependencies() {
  PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/tests/test_hashkey_release_gate.py"
  PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/hashkey_release_gate.py" --root "$REPO_ROOT"
}

prepare_upstream() {
  if [[ -z "$UPSTREAM_DIR" ]]; then
    TEMP_UPSTREAM_DIR="$(mktemp -d)"
    UPSTREAM_DIR="$TEMP_UPSTREAM_DIR/optimism"
    git init --quiet "$UPSTREAM_DIR"
    git -C "$UPSTREAM_DIR" remote add origin "$APPROVED_REPOSITORY"
    git -C "$UPSTREAM_DIR" fetch --quiet --depth 1 origin "$APPROVED_REVISION"
    git -C "$UPSTREAM_DIR" checkout --quiet --detach FETCH_HEAD
  fi

  local resolved_revision
  resolved_revision="$(git -C "$UPSTREAM_DIR" rev-parse HEAD)"
  if [[ "$resolved_revision" != "$APPROVED_REVISION" ]]; then
    echo "error: HashKey optimism checkout is $resolved_revision, expected $APPROVED_REVISION" >&2
    return 1
  fi
}

run_golden() {
  prepare_upstream
  local manifest="$UPSTREAM_DIR/rust/Cargo.toml"
  local suites=(
    b20_asset_v1_golden
    b20_stablecoin_v1_golden
    b20_factory_v1_golden
    b20_policy_v1_golden
  )
  local suite
  for suite in "${suites[@]}"; do
    cargo test \
      --locked \
      --manifest-path "$manifest" \
      -p hsk-b20-precompiles \
      --features test-utils \
      --test "$suite"
  done
}

run_focused() {
  cargo test --locked -p foundry-evm-core@1.7.1 --features hashkey --test hashkey
  cargo test --locked -p forge@1.7.1 --test cli --features hashkey hashkey::
  cargo test --locked -p anvil@1.7.1 --test it --features hashkey hashkey::
  cargo test \
    --locked \
    -p cast@1.7.1 \
    --test cli \
    --features hashkey \
    hashkey::hashkey_b20_anvil_cast_workflow \
    -- --exact
  cargo test \
    --locked \
    -p chisel@1.7.1 \
    --test it \
    --features hashkey \
    repl::hashkey_b20_stateful_session \
    -- --exact
}

run_build_matrix() {
  local cli_packages=(
    -p forge@1.7.1
    -p cast@1.7.1
    -p anvil@1.7.1
    -p chisel@1.7.1
  )
  cargo build --workspace --locked
  # Optimism support is unconditional in the v1.7.1 CLI packages.
  cargo build --locked "${cli_packages[@]}"
  cargo build --locked "${cli_packages[@]}" --features hashkey
  cargo build --workspace --no-default-features --locked
}

run_static() {
  cargo +nightly fmt --all -- --check
  cargo +nightly clippy \
    -p foundry-evm-core@1.7.1 \
    --all-targets \
    --features hashkey \
    --locked
  cargo +nightly clippy \
    -p foundry-evm-networks@1.7.1 \
    --all-targets \
    --all-features \
    --locked
  cargo +nightly clippy \
    -p chisel@1.7.1 \
    --all-targets \
    --features hashkey \
    --locked
}

run_full() {
  cargo nextest run --workspace --all-features --locked --no-fail-fast
}

case "${1:-all}" in
  dependencies)
    run_dependencies
    ;;
  golden)
    run_golden
    ;;
  focused)
    run_focused
    ;;
  build-matrix)
    run_build_matrix
    ;;
  static)
    run_static
    ;;
  full)
    run_full
    ;;
  all)
    failed=0
    run_dependencies || failed=1
    run_golden || failed=1
    run_focused || failed=1
    run_build_matrix || failed=1
    run_static || failed=1
    exit "$failed"
    ;;
  *)
    echo "usage: $0 [dependencies|golden|focused|build-matrix|static|full|all]" >&2
    exit 2
    ;;
esac
