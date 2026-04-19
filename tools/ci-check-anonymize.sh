#!/usr/bin/env bash
# ci-check-anonymize.sh — CI counterpart to the pre-commit hook.
#
# Grepping the committed tree catches commits that slipped past the hook
# (different dev machine, --no-verify, or a brand-new clone where
# install-hooks.sh hasn't been run yet).

set -euo pipefail

pattern='DU[0-9]{7}'
leaks=()

while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    case "$file" in
        *.png|*.jpg|*.jpeg|*.pdf|*.ico|*.woff|*.woff2) continue;;
    esac
    if matches=$(grep -Ea -o "$pattern" "$file" 2>/dev/null); then
        while IFS= read -r m; do
            [[ -z "$m" ]] && continue
            if [[ "$m" =~ ^DU0000[0-9]{3}$ ]]; then
                continue
            fi
            leaks+=("$file: $m")
        done <<< "$matches"
    fi
done < <(git ls-files)

if ((${#leaks[@]} > 0)); then
    echo "CI FAIL: un-anonymized account codes in tracked files:" >&2
    printf '  %s\n' "${leaks[@]}" >&2
    exit 1
fi
echo "CI ok: no un-anonymized IB account codes in tracked tree"
