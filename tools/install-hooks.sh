#!/usr/bin/env bash
# install-hooks.sh — install the ib-sim pre-commit hook into .git/hooks/.
#
# Run once after cloning:
#   tools/install-hooks.sh
#
# The hook scans staged files for un-anonymized IB account codes (DU\d{7}
# outside the reserved DU0000000..DU0000999 synthetic range). See
# plan/ib-sim/07-session-recording.md §"Data Lifecycle" for the policy.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_DIR="$REPO_ROOT/.git/hooks"

if [[ ! -d "$HOOKS_DIR" ]]; then
    echo "error: $HOOKS_DIR does not exist — run inside a git clone"
    exit 1
fi

cp "$SCRIPT_DIR/pre-commit-anonymize.sh" "$HOOKS_DIR/pre-commit"
chmod +x "$HOOKS_DIR/pre-commit"
echo "installed pre-commit hook → $HOOKS_DIR/pre-commit"
