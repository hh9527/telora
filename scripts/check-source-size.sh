#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
baseline_file="$repo_root/scripts/source-size-baseline.txt"
soft_limit=1500
hard_limit=2500
failed=0

baseline_limit() {
    awk -v target="$1" '$1 == target { print $2 }' "$baseline_file"
}

while IFS= read -r file; do
    relative=${file#"$repo_root/"}
    lines=$(wc -l < "$file")
    allowed=$(baseline_limit "$relative")

    if (( lines > hard_limit )); then
        if [[ -z "$allowed" ]]; then
            echo "error: $relative has $lines lines (hard limit: $hard_limit)"
            failed=1
        elif (( lines > allowed )); then
            echo "error: $relative grew to $lines lines (migration baseline: $allowed)"
            failed=1
        else
            echo "baseline: $relative has $lines lines (temporary limit: $allowed)"
        fi
    elif (( lines > soft_limit )); then
        echo "review: $relative has $lines lines (target: $soft_limit)"
    fi
done < <(find "$repo_root/crates" -type f -name '*.rs' -print | sort)

exit "$failed"
