#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# The Python module owns phase membership, policy, ordering, and aggregation.
# This launcher only resolves the repository root/interpreter and forwards args.
if [[ -n "${PYTHON:-}" ]]; then
  PYTHON_BIN="$PYTHON"
elif command -v python3 >/dev/null 2>&1; then
  PYTHON_BIN="$(command -v python3)"
else
  echo "error: python3 is required" >&2
  exit 127
fi

exec "$PYTHON_BIN" "$SCRIPT_DIR/hashkey_release_gate.py" --root "$REPO_ROOT" "$@"
