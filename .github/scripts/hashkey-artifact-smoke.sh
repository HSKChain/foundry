#!/usr/bin/env bash
set -euo pipefail

BIN_DIR="${1:?usage: hashkey-artifact-smoke.sh <bin-dir> [basic|execution]}"
MODE="${2:-basic}"

if [[ "$MODE" != "basic" && "$MODE" != "execution" ]]; then
  echo "error: smoke mode must be basic or execution" >&2
  exit 2
fi

resolve_binary() {
  local name="$1"
  if [[ -x "$BIN_DIR/$name" ]]; then
    printf '%s\n' "$BIN_DIR/$name"
  elif [[ -x "$BIN_DIR/$name.exe" ]]; then
    printf '%s\n' "$BIN_DIR/$name.exe"
  else
    echo "error: missing executable $name in $BIN_DIR" >&2
    return 1
  fi
}

declare -A bins
for name in forge cast anvil chisel; do
  bins[$name]="$(resolve_binary "$name")"
  version="$(${bins[$name]} --version)"
  ${bins[$name]} --help >/dev/null
  if [[ "${HASHKEY_EXPECTED_TAG:-}" == *-hsk-b20* ]]; then
    expected_version="${HASHKEY_EXPECTED_TAG#v}"
    if [[ "$version" != *"$expected_version"* ]]; then
      echo "error: $name version does not identify HSK release $expected_version: $version" >&2
      exit 1
    fi
  fi
done

if [[ "$MODE" == "basic" ]]; then
  exit 0
fi

SMOKE_DIR="$(mktemp -d)"
ANVIL_PID=""
cleanup() {
  if [[ -n "$ANVIL_PID" ]]; then
    kill "$ANVIL_PID" 2>/dev/null || true
    wait "$ANVIL_PID" 2>/dev/null || true
  fi
  rm -rf -- "$SMOKE_DIR"
}
trap cleanup EXIT

mkdir -p "$SMOKE_DIR/src"
cat > "$SMOKE_DIR/foundry.toml" <<'EOF'
[profile.default]
src = "src"
out = "out"
network = "hashkey"
EOF
cat > "$SMOKE_DIR/src/B20Smoke.sol" <<'EOF'
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface IActivationRegistry {
    function isActivated(bytes32 feature) external view returns (bool);
}

contract B20Smoke {
    IActivationRegistry constant REGISTRY =
        IActivationRegistry(0x8453000000000000000000000000000000000001);

    function assetActive() external view returns (bool) {
        return REGISTRY.isActivated(keccak256("base.b20_asset"));
    }
}
EOF

${bins[forge]} build --root "$SMOKE_DIR"

PORT="${HASHKEY_SMOKE_PORT:-18545}"
RPC="http://127.0.0.1:$PORT"
${bins[anvil]} --network hashkey --host 127.0.0.1 --port "$PORT" --silent \
  >"$SMOKE_DIR/anvil.log" 2>&1 &
ANVIL_PID="$!"

ready=0
for _ in $(seq 1 60); do
  if ${bins[cast]} block-number --rpc-url "$RPC" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.25
done
if [[ "$ready" -ne 1 ]]; then
  cat "$SMOKE_DIR/anvil.log" >&2
  echo "error: HashKey Anvil did not become ready" >&2
  exit 1
fi

asset_feature="$(${bins[cast]} keccak "base.b20_asset")"
active="$(${bins[cast]} call \
  --rpc-url "$RPC" \
  0x8453000000000000000000000000000000000001 \
  "isActivated(bytes32)(bool)" \
  "$asset_feature")"
if [[ "$active" != "true" ]]; then
  echo "error: standalone B20Asset feature is not active: $active" >&2
  exit 1
fi

factory_code="$(${bins[cast]} code \
  --rpc-url "$RPC" \
  0xB20F000000000000000000000000000000000000)"
if [[ "${factory_code,,}" != "0xef" ]]; then
  echo "error: standalone B20 Factory marker is $factory_code, expected 0xef" >&2
  exit 1
fi
