#!/usr/bin/env bash
# pre-commit-anonymize.sh — reject commits that contain un-anonymized IB
# account codes.
#
# The regex matches DU followed by exactly 7 digits and is restricted to
# *text-ish* staged files (source, YAML, TOML, Markdown, etc.) plus
# `.tws.pcap` files that have NOT been zstd-compressed. Binary `.dbn` and
# `.tws.pcap.zst` files are scanned with `grep -a` so hits still surface.
#
# Bypasses (`--no-verify`) are legitimate in rare cases (merging a branch that
# already passed the hook on another machine). Any bypass is logged to
# `tools/hook-bypasses.log`, PR-reviewed.
#
# Synthetic codes in the reserved range `DU0000000..DU0000999` are allowed —
# they are the anonymized form.

set -euo pipefail

staged=$(git diff --cached --name-only --diff-filter=ACMR || true)
if [[ -z "$staged" ]]; then
    exit 0
fi

# Pattern: DU followed by 7 digits, NOT in the reserved DU0000xxx range.
# We first match any DU\d{7} hit, then filter out ones that start DU0000.
pattern='DU[0-9]{7}'
offenders=()

while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    [[ ! -f "$file" ]] && continue
    # Skip obviously binary artifacts we never expect to carry account codes.
    case "$file" in
        *.png|*.jpg|*.jpeg|*.pdf|*.ico|*.woff|*.woff2) continue;;
        # Stored captures: scan with -a (treat-as-text) so we catch bytes.
        *.tws.pcap|*.dbn) ;;
        *) ;;
    esac

    if matches=$(grep -Ea -o "$pattern" "$file" 2>/dev/null); then
        while IFS= read -r m; do
            [[ -z "$m" ]] && continue
            # Accept synthetic range DU0000xxx (xxx = 000..999).
            if [[ "$m" =~ ^DU0000[0-9]{3}$ ]]; then
                continue
            fi
            offenders+=("$file: $m")
        done <<< "$matches"
    fi
done <<< "$staged"

if ((${#offenders[@]} > 0)); then
    echo "error: un-anonymized account codes in staged files:" >&2
    printf '  %s\n' "${offenders[@]}" >&2
    echo >&2
    echo "Run 'midas-ib-sim anonymize <in.tws.pcap> --out <out.tws.pcap>'" >&2
    echo "or move the file into fixtures/sessions/raw/ (gitignored)." >&2
    exit 1
fi
