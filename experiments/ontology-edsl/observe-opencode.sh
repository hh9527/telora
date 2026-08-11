#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
usage: observe-opencode.sh [snapshot|status|recent [COUNT]|files|events]

Reads the active ontology eDSL experiment recorded under target/exp. All
commands are observational; this script never sends prompts or aborts a session.
EOF
}

mode=${1:-snapshot}
if [[ $# -gt 0 ]]; then
    shift
fi

anchor=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$anchor/../.." && pwd -P)
state=$repo/target/exp

for command in curl jq; do
    if ! command -v "$command" >/dev/null 2>&1; then
        printf '%s is not available on PATH\n' "$command" >&2
        exit 69
    fi
done

for file in dir session-id server-url; do
    if [[ ! -s $state/$file ]]; then
        printf 'missing experiment state: %s\n' "$state/$file" >&2
        exit 66
    fi
done

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

workspace_query=$(jq -rn --arg value "$workspace" '$value | @uri')
curl_common=(--silent --show-error --fail --noproxy '*')

show_status() {
    local health statuses
    health=$(curl "${curl_common[@]}" "$server_url/global/health")
    statuses=$(curl "${curl_common[@]}" \
        "$server_url/session/status?directory=$workspace_query")
    jq -n \
        --arg workspace "$workspace" \
        --arg session_id "$session_id" \
        --argjson health "$health" \
        --argjson statuses "$statuses" \
        '{
          workspace: $workspace,
          session_id: $session_id,
          health: $health,
          status: ($statuses[$session_id] // {type: "idle"})
        }'
}

show_recent() {
    local count=${1:-3}
    if [[ ! $count =~ ^[1-9][0-9]*$ || $count -gt 20 ]]; then
        printf 'COUNT must be an integer from 1 through 20\n' >&2
        exit 64
    fi

    curl "${curl_common[@]}" \
        "$server_url/session/$session_id/message?directory=$workspace_query" \
        | jq --argjson count "$count" '
            [
              .
              | map(select(.info.role == "assistant"))
              | .[-$count:][]
              | {
                  message_id: .info.id,
                  completed: (.info.time.completed // null),
                  finish: (.info.finish // null),
                  parts: [
                    .parts[]
                    | if .type == "tool" then
                        {
                          type: "tool",
                          tool: .tool,
                          status: .state.status,
                          input: (.state.input // {}),
                          exit: (.state.metadata.exit // null),
                          output: ((.state.output // "") | .[0:1200])
                        }
                      elif .type == "reasoning" then
                        {
                          type: "reasoning",
                          text: ((.text // "") | .[-2400:])
                        }
                      elif .type == "text" then
                        {
                          type: "text",
                          text: ((.text // "") | .[-2400:])
                        }
                      else empty
                      end
                  ]
                }
            ]'
}

show_files() {
    find "$workspace/a2" -maxdepth 3 -type f \
        -printf '%TY-%Tm-%TdT%TH:%TM:%TS %s %P\n' | sort
}

show_events() {
    curl "${curl_common[@]}" --no-buffer \
        "$server_url/event?directory=$workspace_query" \
        | sed -u -n 's/^data: //p' \
        | jq --unbuffered --arg session_id "$session_id" '
            select(.properties.sessionID == $session_id)
            | if .type == "session.status" then
                {
                  type: .type,
                  status: .properties.status.type
                }
              elif .type == "session.error" then
                {
                  type: .type,
                  error: .properties.error
                }
              elif .type == "message.updated"
                   and (.properties.info.time.completed != null) then
                {
                  type: .type,
                  role: .properties.info.role,
                  finish: (.properties.info.finish // null),
                  completed: .properties.info.time.completed
                }
              elif .type == "message.part.updated"
                   and .properties.part.type == "tool"
                   and (.properties.part.state.status == "completed"
                        or .properties.part.state.status == "error") then
                {
                  type: "tool",
                  tool: .properties.part.tool,
                  status: .properties.part.state.status,
                  input: (.properties.part.state.input // {}),
                  exit: (.properties.part.state.metadata.exit // null),
                  output: ((.properties.part.state.output // "") | .[0:1200])
                }
              else empty
              end'
}

case $mode in
    snapshot)
        [[ $# -eq 0 ]] || { usage >&2; exit 64; }
        printf '%s\n' '--- status'
        show_status
        printf '%s\n' '--- recent'
        show_recent 3
        printf '%s\n' '--- files'
        show_files
        ;;
    status)
        [[ $# -eq 0 ]] || { usage >&2; exit 64; }
        show_status
        ;;
    recent)
        [[ $# -le 1 ]] || { usage >&2; exit 64; }
        show_recent "${1:-3}"
        ;;
    files)
        [[ $# -eq 0 ]] || { usage >&2; exit 64; }
        show_files
        ;;
    events)
        [[ $# -eq 0 ]] || { usage >&2; exit 64; }
        show_events
        ;;
    -h|--help|help)
        usage
        ;;
    *)
        printf 'unknown mode: %s\n' "$mode" >&2
        usage >&2
        exit 64
        ;;
esac
