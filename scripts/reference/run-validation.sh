#!/usr/bin/env sh
# Cross-platform wrapper for the F01 reference checks (macOS/Linux/CI).
# No installs, no configuration changes and no reference application launches.
set -eu

SCRIPT_DIR=$(CDPATH= cd "$(dirname "$0")" && pwd)
ROOT=$(CDPATH= cd "$SCRIPT_DIR/../.." && pwd)
PYTHON=${PYTHON:-python3}

# Optional explicit fixture root: run-validation.sh --root /path/to/tree [validator args...]
if [ "${1:-}" = "--root" ]; then
    if [ "$#" -lt 2 ] || [ -z "${2:-}" ]; then
        echo "--root requires a directory" >&2
        exit 2
    fi
    ROOT=$2
    shift 2
fi

if ! command -v "$PYTHON" >/dev/null 2>&1; then
    echo "Python 3 was not found. Set PYTHON to an interpreter path." >&2
    exit 2
fi

echo "Validating F01 reference artifacts under $ROOT"
"$PYTHON" "$SCRIPT_DIR/validate_reference.py" --root "$ROOT" "$@"

if [ "${SKIP_TESTS:-0}" != "1" ]; then
    echo "Running reference validator negative tests"
    "$PYTHON" "$SCRIPT_DIR/test_validate_reference.py" --root "$ROOT"
fi

echo "Reference checks passed. This validates the specification, not a reference runtime."
