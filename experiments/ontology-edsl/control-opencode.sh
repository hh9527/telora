#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
usage: control-opencode.sh [start|finalize]

Controls the active ontology eDSL experiment recorded under target/exp.

  start     Send A2-PROMPT.md exactly once to the prepared empty session.
  finalize  Record successful completion after the session becomes idle.
EOF
}

mode=${1:-}
if [[ $# -gt 0 ]]; then
    shift
fi
if [[ $# -ne 0 ]]; then
    usage >&2
    exit 64
fi

case $mode in
    start|finalize) ;;
    -h|--help|help)
        usage
        exit 0
        ;;
    *)
        [[ -z $mode ]] || printf 'unknown mode: %s\n' "$mode" >&2
        usage >&2
        exit 64
        ;;
esac

for command in curl jq flock sha256sum; do
    if ! command -v "$command" >/dev/null 2>&1; then
        printf '%s is not available on PATH\n' "$command" >&2
        exit 69
    fi
done

anchor=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$anchor/../.." && pwd -P)
state=$repo/target/exp
metadata=$state/SESSION.json

for file in dir session-id server-url SESSION.json; do
    if [[ ! -s $state/$file ]]; then
        printf 'missing experiment state: %s\n' "$state/$file" >&2
        exit 66
    fi
done

exec 9>"$state/lock"
flock -x 9

workspace=$(<"$state/dir")
session_id=$(<"$state/session-id")
server_url=$(<"$state/server-url")
if [[ ! -d $workspace ]]; then
    printf 'recorded workspace does not exist: %s\n' "$workspace" >&2
    exit 66
fi
if [[ ! $session_id =~ ^ses_[A-Za-z0-9]+$ ]]; then
    printf 'invalid session ID: %s\n' "$session_id" >&2
    exit 65
fi
if [[ ! $server_url =~ ^http://127\.0\.0\.1:[0-9]+$ ]]; then
    printf 'invalid server URL: %s\n' "$server_url" >&2
    exit 65
fi

metadata_workspace=$(jq -er '.workspace' "$metadata")
metadata_session_id=$(jq -er '.session_id' "$metadata")
metadata_server_url=$(jq -er '.server_url' "$metadata")
if [[ $metadata_workspace != "$workspace" || \
      $metadata_session_id != "$session_id" || \
      $metadata_server_url != "$server_url" ]]; then
    printf 'experiment identity does not match SESSION.json\n' >&2
    exit 65
fi

write_metadata() {
    local filter=$1
    local value=$2
    local temporary=$metadata.tmp.$$
    jq --arg value "$value" "$filter" "$metadata" >"$temporary"
    mv -f "$temporary" "$metadata"
}

if [[ $mode == start ]]; then
    if [[ $(jq -r '.task_started // false' "$metadata") == true ]]; then
        printf 'A2 task already started at %s\n' \
            "$(jq -r '.task_started_at // "unknown time"' "$metadata")"
        exit 0
    fi

    expected_prompt_hash=$(jq -er '.sha256["A2-PROMPT.md"]' "$metadata")
    actual_prompt_hash=$(sha256sum "$anchor/A2-PROMPT.md" | cut -d' ' -f1)
    if [[ $actual_prompt_hash != "$expected_prompt_hash" ]]; then
        printf 'A2-PROMPT.md differs from the prepared experiment input\n' >&2
        exit 65
    fi
else
    if [[ $(jq -r '.task_started // false' "$metadata") != true ]]; then
        printf 'A2 task has not been started\n' >&2
        exit 65
    fi
    if [[ $(jq -r '.task_completed // false' "$metadata") == true ]]; then
        printf 'A2 task already finalized at %s\n' \
            "$(jq -r '.task_completed_at // "unknown time"' "$metadata")"
        exit 0
    fi
fi

curl_common=(--silent --show-error --fail --noproxy '*')
workspace_query=$(jq -rn --arg value "$workspace" '$value | @uri')
curl "${curl_common[@]}" "$server_url/global/health" >/dev/null

if [[ $mode == start ]]; then
    messages=$(curl "${curl_common[@]}" \
        "$server_url/session/$session_id/message?directory=$workspace_query")
    if [[ $(jq -er 'length' <<<"$messages") -ne 0 ]]; then
        printf 'prepared session is not empty; refusing to start A2\n' >&2
        exit 65
    fi

    payload=$(jq -n --rawfile text "$anchor/A2-PROMPT.md" \
        '{parts: [{type: "text", text: $text}]}')
    curl "${curl_common[@]}" --request POST \
        --header 'Content-Type: application/json' \
        "$server_url/session/$session_id/prompt_async?directory=$workspace_query" \
        --data-raw "$payload" >/dev/null

    started_at=$(date --iso-8601=seconds)
    write_metadata \
        '.task_started = true | .task_started_at = $value | del(.task_completed, .task_completed_at)' \
        "$started_at"
    printf 'A2 task started at %s\n' "$started_at"
    exit 0
fi

statuses=$(curl "${curl_common[@]}" \
    "$server_url/session/status?directory=$workspace_query")
status=$(jq -r --arg id "$session_id" '.[$id].type // "idle"' \
    <<<"$statuses")
if [[ $status != idle ]]; then
    printf 'A2 session is still %s; refusing to finalize\n' "$status" >&2
    exit 75
fi

messages=$(curl "${curl_common[@]}" \
    "$server_url/session/$session_id/message?directory=$workspace_query")
completion=$(jq -er '
    [.[].info | select(.role == "assistant")][-1]
    | select(.finish == "stop" and .time.completed != null)
    | .time.completed
' <<<"$messages") || {
    printf 'last assistant message is not a completed stop\n' >&2
    exit 65
}

if [[ $completion =~ ^[0-9]+$ ]]; then
    completed_at=$(date --date="@$((completion / 1000))" --iso-8601=seconds)
else
    printf 'invalid assistant completion timestamp: %s\n' "$completion" >&2
    exit 65
fi
write_metadata \
    '.task_completed = true | .task_completed_at = $value' \
    "$completed_at"
printf 'A2 task finalized at %s\n' "$completed_at"
