#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)
source_root=${TELORA_LANGUAGE_ROOT:-"$repo_root/tests/language"}
build_root="$repo_root/target/language_tests"
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
cp "$workspace/src/test-support.telora" "$workspace/src/test_support.telora"
cp "$workspace/src/runtime-support.telora" "$workspace/src/runtime_support.telora"
mapfile -t testees < <(find "$workspace/src" -type f -name testee.telora | sort)
if [[ ${#testees[@]} -eq 0 ]]; then
    echo "no language testees found" >&2
    exit 2
fi

cases=()
case_checks=()
for testee in "${testees[@]}"; do
    relative=${testee#"$workspace/src/"}
    case_id=${relative%/testee.telora}
    checker="$workspace/src/$case_id/check.telora"
    expected="$workspace/src/$case_id/expected.txt"
    cases+=("$case_id")
    child_root=${testee%.telora}
    declarations=()
    while IFS= read -r companion; do
        if [[ $case_id == check/instance-convergence && $(basename "$companion") == large-template.telora ]]; then
            continue
        fi
        base=$(basename "$companion" .telora)
        module=${base//-/_}
        mkdir -p "$child_root"
        cp "$companion" "$child_root/$module.telora"
        declarations+=("mod $module;")
    done < <(find "$(dirname "$testee")" -maxdepth 1 -type f -name '*.telora' \
        ! -name testee.telora ! -name check.telora | sort)
    while IFS= read -r data; do
        mkdir -p "$child_root"
        cp "$data" "$child_root/$(basename "$data")"
    done < <(find "$(dirname "$testee")" -maxdepth 1 -type f \
        \( -name '*.json' -o -name '*.yaml' -o -name '*.yml' -o -name '*.toml' \) | sort)
    if [[ -d "$(dirname "$testee")/helpers" ]]; then
        mkdir -p "$child_root/helpers"
        cp -R "$(dirname "$testee")/helpers/." "$child_root/helpers/"
        printf '%s\n' 'pub mod group;' >"$child_root/helpers.telora"
        declarations+=("mod helpers;")
    fi
    mkdir -p "$child_root"
    case "$case_id" in
        test/check-result-provenance)
            cp "$workspace/src/test/check-result/provider.telora" "$child_root/provider.telora"
            declarations+=("mod provider;")
            ;;
        check/diag-check-result-tool-stage)
            cp "$workspace/src/check/check-result-tool-stage/provider.telora" "$child_root/provider.telora"
            declarations+=("mod provider;")
            ;;
        test/properties)
            cp "$workspace/src/eval/properties/model.telora" "$child_root/model.telora"
            declarations+=("mod model;")
            ;;
        test/module-interfaces)
            while IFS= read -r companion; do
                base=$(basename "$companion" .telora)
                module=${base//-/_}
                cp "$companion" "$child_root/$module.telora"
                declarations+=("mod $module;")
            done < <(find "$workspace/src/check/module-interfaces" -maxdepth 1 -type f -name '*.telora' \
                ! -name testee.telora ! -name check.telora | sort)
            ;;
        test/source-base)
            cp "$workspace/src/fixture-helper/module.telora" "$child_root/module.telora"
            cp "$workspace/src/fixture-helper/input.json" "$child_root/input.json"
            declarations+=("mod module;")
            ;;
    esac
    if [[ ${#declarations[@]} -gt 0 ]]; then
        temporary="$testee.rfc0308"
        sed -n 'p' "$testee" >"$temporary"
        printf '\n%s\n' "${declarations[@]}" >>"$temporary"
        mv "$temporary" "$testee"
    fi
    if [[ -f "$checker" ]]; then
        case_checks+=(1)
    elif [[ -f "$expected" ]]; then
        case_checks+=(2)
    else
        case_checks+=(0)
    fi
done

if [[ -d "$workspace/src/test" ]]; then
    mkdir -p "$workspace/tests"
    cp -R "$workspace/src/test/." "$workspace/tests/"
fi

printf '%s\n' \
    'mod support;' \
    'mod test_support;' \
    'mod runtime_support;' \
    'mod unknown_support;' \
    'pub type LanguageTests = struct {};' \
    >"$workspace/src/lib.telora"

generated="$build_root/check-all.telora"
{
    echo 'mod support;'
    echo 'mod unknown_support;'
    for index in "${!cases[@]}"; do
        if [[ ${case_checks[$index]} -eq 1 ]]; then
            cp "$workspace/src/${cases[$index]}/check.telora" "$workspace/src/case_${index}.telora"
            printf 'mod case_%s;\n' "$index"
        fi
    done
    echo 'use std::dict as dict;'
    echo 'use std::transform_service as entry;'
    echo 'use std::value::{ Value };'
    echo 'def required: Fn(Dict(Value), String) -> Value = fn(values, name) {'
    echo '    match dict::get(values, name) {'
    echo '        Some(value) => value,'
    echo '        None => fail!("missing test observation", name),'
    echo '    }'
    echo '};'
    echo 'type CheckerService = struct {};'
    echo 'impl entry::TransformService for CheckerService {'
    echo '    init: fn(ctx) { {}.ty!(Self) },'
    echo '    transform: fn(self, input) {'
    echo '    let actual = match input {'
    echo '        Value::Object(values) => values,'
    echo '        _ => fail!("actual test observations must be an object"),'
    echo '    };'
    echo '    Value::Object({'
    for index in "${!cases[@]}"; do
        if [[ ${case_checks[$index]} -eq 1 ]]; then
            printf '        "%s": case_%s::check(required(actual, "%s")),\n' \
                "${cases[$index]}" "$index" "${cases[$index]}"
        elif [[ ${case_checks[$index]} -eq 2 ]]; then
            expected=$(jaq -Rs 'split("\r\n") | join("\n") | split("\r") | join("\n") | rtrimstr("\n")' "$workspace/src/${cases[$index]}/expected.txt")
            printf '        "%s": if support::failed_with(required(actual, "%s"), %s) { Value::True } else { Value::False },\n' \
                "${cases[$index]}" "${cases[$index]}" "$expected"
        else
            printf '        "%s": if support::succeeded(required(actual, "%s")) { Value::True } else { Value::False },\n' \
                "${cases[$index]}" "${cases[$index]}"
        fi
    done
    echo '    })'
    echo '    },'
    echo '};'
    echo '@entry::collection'
    echo 'pub type MainService = struct { @entry::slot("transform") checker: CheckerService };'
} >"$generated"

printf '%s\n' '{"version":1,"members":["."]}' >"$workspace/telora-config.json"
jaq -n \
    '{name:"language_tests",dependencies:[]}' \
    >"$workspace/telora-crate.json"
jaq -n \
    '{version:1,packages:{"language_tests":{source:{workspace:""},dependencies:[]}}}' \
    >"$workspace/telora-lock.json"

entries="$actual_root/entries.jsonl"
: >"$entries"

# Error fixtures need separate sessions: a static error prevents that session
# from entering tool/runtime execution. Combining them would suppress unrelated
# runtime diagnostics and share an exit status between independent assertions.
for case_id in "${cases[@]}"; do
    mode=${case_id%%/*}

    raw_stdout="$actual_root/${case_id//\//__}.stdout.jsonl"
    raw_stderr="$actual_root/${case_id//\//__}.stderr.jsonl"

    set +e
    case "$mode" in
        test)
            "$telora_bin" -C "$workspace" test "${case_id#test/}/testee" \
                >"$raw_stdout" 2>"$raw_stderr"
            ;;
        eval)
            "$telora_bin" -C "$workspace" eval "@src/$case_id/testee:result" \
                >"$raw_stdout" 2>"$raw_stderr"
            ;;
        query)
            "$telora_bin" -C "$workspace" query exports "@src/$case_id/testee" \
                >"$raw_stdout" 2>"$raw_stderr"
            ;;
        query-at)
            "$telora_bin" -C "$workspace" query at "@src/$case_id/testee" \
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
request="$build_root/request.json"
jaq -n --slurpfile observations "$observations" '{method:"transform",input:$observations[0]}' >"$request"
cp "$generated" "$workspace/src/lib.telora"

check_stdout="$build_root/check.stdout.json"
check_stderr="$build_root/check.stderr.jsonl"
# The aggregate checker parses every observation inside Guest (about 1.25 MB).
# This is a test-runner budget, independent of the cases and product defaults.
set +e
"$telora_bin" --request-fuel 5000 -C "$workspace" run "@src/lib" \
    <"$request" >"$check_stdout" 2>"$check_stderr"
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
