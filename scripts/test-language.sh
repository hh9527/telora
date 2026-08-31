#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_root="$repo_root/tests/language"
build_root="$repo_root/target/language-tests"
workspace="$build_root/workspace"
actual_root="$build_root/actual"
telora_bin=${TELORA_BIN:-"$repo_root/target/debug/telora"}

if [[ ! -x "$telora_bin" ]]; then
    echo "telora binary is not executable: $telora_bin" >&2
    exit 2
fi
if ! command -v jaq >/dev/null 2>&1; then
    echo "jaq is required" >&2
    exit 2
fi

rm -rf "$build_root"
mkdir -p "$workspace/src/generated" "$actual_root"
cp -R "$source_root/src/." "$workspace/src/"

mapfile -t testees < <(find "$workspace/src" -type f -name testee.telora | sort)
if [[ ${#testees[@]} -eq 0 ]]; then
    echo "no language testees found" >&2
    exit 2
fi

cases=()
for testee in "${testees[@]}"; do
    relative=${testee#"$workspace/src/"}
    case_id=${relative%/testee.telora}
    checker="$workspace/src/$case_id/check.telora"
    if [[ ! -f "$checker" ]]; then
        echo "missing checker for $case_id" >&2
        exit 2
    fi
    cases+=("$case_id")
done

generated="$workspace/src/generated/check-all.telora"
{
    echo 'import "std/dict" as dict;'
    echo 'import "std/entry" as entry;'
    echo 'import "std/value" { Value };'
    for index in "${!cases[@]}"; do
        printf 'import "@src/%s/check" as case_%s;\n' "${cases[$index]}" "$index"
    done
    echo 'def config: entry.ContextConfig = {sources: ["actual"], envs: [], args: '\''False};'
    echo 'def required: Fn(Dict(Value), String) -> Value = fn(values, name) {'
    echo '    match dict.get(values, name) {'
    echo '        '\''Some(value) => value,'
    echo '        '\''None => fail!("missing test observation", name),'
    echo '    }'
    echo '};'
    echo 'export def check = entry.main(config, fn(ctx) {'
    echo '    let actual = match dict.get(ctx.sources, "actual") {'
    echo '        '\''Some('\''Object(values)) => values,'
    echo '        _ => fail!("actual test observations must be an object"),'
    echo '    };'
    echo '    '\''Object({'
    for index in "${!cases[@]}"; do
        printf '        "%s": case_%s.check(required(actual, "%s")),\n' \
            "${cases[$index]}" "$index" "${cases[$index]}"
    done
    echo '    })'
    echo '});'
} >"$generated"

mapfile -t module_files < <(find "$workspace/src" -type f -name '*.telora' | sort)
modules_json=$(
    for module_file in "${module_files[@]}"; do
        relative=${module_file#"$workspace/src/"}
        printf '@src/%s\n' "${relative%.telora}"
    done | jaq -Rsc 'split("\n") | map(select(length > 0))'
)

printf '%s\n' '{"version":1,"members":["."]}' >"$workspace/telora-config.json"
jaq -n --argjson modules "$modules_json" \
    '{name:"language-tests",modules:$modules,dependencies:[]}' \
    >"$workspace/telora-crate.json"
jaq -n --argjson modules "$modules_json" \
    '{version:1,packages:{"language-tests":{source:{workspace:""},modules:$modules,dependencies:[]}}}' \
    >"$workspace/telora-lock.json"

entries="$actual_root/entries.jsonl"
: >"$entries"
for case_id in "${cases[@]}"; do
    raw_stdout="$actual_root/${case_id//\//__}.stdout.jsonl"
    raw_stderr="$actual_root/${case_id//\//__}.stderr.jsonl"
    mode=${case_id%%/*}

    set +e
    case "$mode" in
        eval)
            "$telora_bin" -C "$workspace" eval "@src/$case_id/testee:result" \
                >"$raw_stdout" 2>"$raw_stderr"
            ;;
        query)
            "$telora_bin" -C "$workspace" query exports "@src/$case_id/testee" \
                >"$raw_stdout" 2>"$raw_stderr"
            ;;
        check)
            "$telora_bin" -C "$workspace" check "@src/$case_id/testee" \
                >"$raw_stdout" 2>"$raw_stderr"
            ;;
        *)
            echo "unknown language test mode: $mode" >&2
            exit 2
            ;;
    esac
    exit_code=$?
    set -e

    jaq -n \
        --arg key "$case_id" \
        --argjson exit_code "$exit_code" \
        --slurpfile stdout "$raw_stdout" \
        --slurpfile stderr "$raw_stderr" \
        '{key:$key,value:{exit_code:$exit_code,stdout:$stdout,stderr:$stderr}}' \
        >>"$entries"
done

observations="$build_root/observations.json"
jaq -s 'from_entries' "$entries" >"$observations"

check_stdout="$build_root/check.stdout.json"
check_stderr="$build_root/check.stderr.jsonl"
set +e
"$telora_bin" -C "$workspace" eval-with "@src/generated/check-all:check" \
    --source "actual=$observations" >"$check_stdout" 2>"$check_stderr"
check_exit=$?
set -e

if [[ $check_exit -ne 0 ]]; then
    jaq -s '.' "$check_stderr" >&2
    exit "$check_exit"
fi

if ! jaq -e 'all(.[]; . == true)' "$check_stdout" >/dev/null; then
    jaq -n --slurpfile results "$check_stdout" \
        '{status:"error",results:$results[0]}'
    exit 1
fi

jaq -n --argjson total "${#cases[@]}" \
    '{status:"ok",total:$total}'
