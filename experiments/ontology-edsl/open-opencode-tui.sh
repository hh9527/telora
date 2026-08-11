#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
usage: open-opencode-tui.sh [--port PORT] [--telora PATH]

Creates or reuses the ontology eDSL experiment workspace and empty opencode
session, then opens a TUI whose daemon has the same lifecycle. It does not send
the A2 task.
EOF
}

port=4096
port_was_set=false
telora_binary=

while [[ $# -gt 0 ]]; do
    case $1 in
        --port)
            [[ $# -ge 2 ]] || { usage >&2; exit 64; }
            port=$2
            port_was_set=true
            shift 2
            ;;
        --telora)
            [[ $# -ge 2 ]] || { usage >&2; exit 64; }
            telora_binary=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'unknown argument: %s\n' "$1" >&2
            usage >&2
            exit 64
            ;;
    esac
done

if [[ ! $port =~ ^[0-9]+$ || $port -lt 1 || $port -gt 65535 ]]; then
    printf 'port must be an integer from 1 through 65535\n' >&2
    exit 64
fi

for command in opencode curl jq flock; do
    if ! command -v "$command" >/dev/null 2>&1; then
        printf '%s is not available on PATH\n' "$command" >&2
        exit 69
    fi
done

anchor=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$anchor/../.." && pwd -P)
template=$anchor/opencode-workspace
state=$repo/target/exp
mkdir -p "$state"

exec 9>"$state/lock"
flock -x 9

workspace_file=$state/dir
session_file=$state/session-id
server_file=$state/server-url
handshake_log=$state/HANDSHAKE.log
session_metadata=$state/SESSION.json

write_state() {
    local path=$1
    local value=$2
    local temporary=$path.tmp.$$
    printf '%s\n' "$value" >"$temporary"
    mv -f "$temporary" "$path"
}

install_workspace() {
    local destination=$1
    local binary=$2
    local config=$destination/opencode.json

    mkdir -p "$destination/a1" "$destination/a2/src" \
        "$destination/a2/bin-src" "$destination/bin"
    cp "$anchor/TASK-A2.md" "$destination/a1/TASK-A2.md"
    cp "$anchor/TELORA-TUTORIAL.md" "$destination/a1/TELORA-TUTORIAL.md"
    cp "$anchor/TELORA-CLI.md" "$destination/a1/TELORA-CLI.md"
    cp "$anchor/EDSL-DESIGN.md" "$destination/a1/EDSL-DESIGN.md"
    cp "$template/a2/telora-deps.json" "$destination/a2/telora-deps.json"
    cp "$template/a2/bin-src/main.telora" "$destination/a2/bin-src/main.telora"
    cp "$template/a2/bin-src/test.telora" "$destination/a2/bin-src/test.telora"
    cp "$template/bin/run" "$template/bin/run-test" "$template/bin/types" \
        "$template/bin/show" "$destination/bin/"
    cp "$binary" "$destination/bin/telora"
    chmod 0555 "$destination/bin/telora" "$destination/bin/run" \
        "$destination/bin/run-test" "$destination/bin/types" \
        "$destination/bin/show"
    chmod 0444 "$destination/a2/telora-deps.json"

    {
        cat <<'EOF'
{
  "$schema": "https://opencode.ai/config.json",
  "permission": {
    "read": {
      "*": "deny",
      "a1/**": "allow",
      "a2/**": "allow",
      "**/a1/**": "allow",
      "**/a2/**": "allow"
    },
    "list": {
      "*": "deny",
      "a1": "allow",
      "a1/**": "allow",
      "a2": "allow",
      "a2/**": "allow",
      "**/a1/**": "allow",
      "**/a2/**": "allow"
    },
    "glob": {
      "*": "deny",
      "a1/**": "allow",
      "a2/**": "allow",
      "**/a1/**": "allow",
      "**/a2/**": "allow"
    },
    "grep": {
      "*": "deny",
      "a1/**": "allow",
      "a2/**": "allow",
      "**/a1/**": "allow",
      "**/a2/**": "allow"
    },
    "edit": {
      "*": "deny",
      "a2/src/**": "allow",
      "a2/bin-src/**": "allow",
      "**/a2/src/**": "allow",
      "**/a2/bin-src/**": "allow"
    },
    "write": {
      "*": "deny",
      "a2/src/**": "allow",
      "a2/bin-src/**": "allow",
      "**/a2/src/**": "allow",
      "**/a2/bin-src/**": "allow"
    },
    "bash": {
      "*": "deny",
      "./bin/run": "allow",
      "./bin/run-test": "allow",
      "./bin/types": "allow",
      "./bin/show": "allow",
      "__no_more_commands__": "deny"
    },
    "task": "deny",
    "webfetch": "deny",
    "external_directory": "deny"
  }
}
EOF
    } >"$config"
}

if [[ -f $workspace_file ]]; then
    workspace=$(<"$workspace_file")
    if [[ -z $workspace || ! -d $workspace ]]; then
        printf 'recorded workspace does not exist: %s\n' "$workspace" >&2
        exit 66
    fi
else
    if [[ -z $telora_binary ]]; then
        if [[ -x $repo/target/debug/telora ]]; then
            telora_binary=$repo/target/debug/telora
        else
            cargo build --manifest-path "$repo/Cargo.toml" -p telora
            telora_binary=$repo/target/debug/telora
        fi
    fi
    telora_binary=$(cd -- "$(dirname -- "$telora_binary")" && \
        printf '%s/%s\n' "$PWD" "$(basename -- "$telora_binary")")
    if [[ ! -x $telora_binary ]]; then
        printf 'Telora executable is missing or not executable: %s\n' \
            "$telora_binary" >&2
        exit 66
    fi

    run_root=$(mktemp -d /tmp/test-XXXXXX)
    workspace=$run_root/ws
    install_workspace "$workspace" "$telora_binary"
    write_state "$workspace_file" "$workspace"
fi

server_file_is_new=false
if [[ -f $server_file ]]; then
    server_url=$(<"$server_file")
    if [[ ! $server_url =~ ^http://127\.0\.0\.1:([0-9]+)$ ]]; then
        printf 'invalid server URL in %s: %s\n' "$server_file" "$server_url" >&2
        exit 65
    fi
    saved_port=${BASH_REMATCH[1]}
    if [[ $port_was_set == true && $port -ne $saved_port ]]; then
        printf 'experiment already uses port %s; omit --port or use that port\n' \
            "$saved_port" >&2
        exit 64
    fi
    port=$saved_port
else
    server_url="http://127.0.0.1:$port"
    server_file_is_new=true
fi

server_was_running=false
if curl --silent --fail --max-time 1 --noproxy '*' \
    "$server_url/global/health" >/dev/null; then
    server_was_running=true
fi

if [[ $server_was_running == true && ! -f $session_file ]]; then
    printf 'port %s already hosts another opencode daemon\n' "$port" >&2
    exit 69
fi

temporary_daemon_pid=
stop_temporary_daemon() {
    if [[ -n $temporary_daemon_pid ]] && \
        kill -0 "$temporary_daemon_pid" 2>/dev/null; then
        kill "$temporary_daemon_pid" 2>/dev/null || true
        wait "$temporary_daemon_pid" 2>/dev/null || true
    fi
}
trap stop_temporary_daemon EXIT
trap 'exit 130' INT TERM

if [[ $server_was_running != true ]] && \
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    printf 'port %s is occupied by a service that is not this opencode daemon\n' \
        "$port" >&2
    exit 69
fi

if [[ $server_file_is_new == true ]]; then
    write_state "$server_file" "$server_url"
fi

if [[ $server_was_running != true && ! -f $session_file ]]; then
    (
        cd "$workspace"
        exec opencode serve --hostname 127.0.0.1 --port "$port" --pure
    ) </dev/null >"$handshake_log" 2>&1 &
    temporary_daemon_pid=$!

    daemon_healthy=false
    for _ in {1..100}; do
        if curl --silent --fail --max-time 1 --noproxy '*' \
            "$server_url/global/health" >/dev/null; then
            daemon_healthy=true
            break
        fi
        if ! kill -0 "$temporary_daemon_pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if [[ $daemon_healthy != true ]]; then
        printf 'temporary opencode daemon did not become healthy; see %s\n' \
            "$handshake_log" >&2
        exit 70
    fi
fi

workspace_query=$(jq -rn --arg value "$workspace" '$value | @uri')
if [[ -f $session_file ]]; then
    session_id=$(<"$session_file")
    if [[ ! $session_id =~ ^ses_[A-Za-z0-9]+$ ]]; then
        printf 'invalid session ID in %s\n' "$session_file" >&2
        exit 65
    fi
else
    session_response=$(curl --silent --show-error --fail --noproxy '*' \
        --request POST \
        --header 'Content-Type: application/json' \
        "$server_url/session?directory=$workspace_query" \
        --data-raw '{"title":"Ontology eDSL A2 (ready)"}')
    session_id=$(jq -er '.id | select(type == "string" and length > 0)' \
        <<<"$session_response")
    write_state "$session_file" "$session_id"
fi

printf -v tui_command '%q ' opencode "$workspace" --hostname 127.0.0.1 \
    --port "$port" --session "$session_id" --pure

if [[ ! -f $session_metadata ]]; then
    revision=$(git -C "$repo" rev-parse HEAD)
    if [[ -z $(git -C "$repo" status --porcelain) ]]; then
        dirty=false
    else
        dirty=true
    fi
    jq -n \
        --arg workspace "$workspace" \
        --arg server_url "$server_url" \
        --arg session_id "$session_id" \
        --arg tui_command "${tui_command% }" \
        --arg opencode_version "$(opencode --version)" \
        --arg revision "$revision" \
        --argjson dirty "$dirty" \
        --arg telora_hash "$(sha256sum "$workspace/bin/telora" | cut -d' ' -f1)" \
        --arg language_hash "$(sha256sum "$workspace/a1/TELORA-TUTORIAL.md" | cut -d' ' -f1)" \
        --arg cli_hash "$(sha256sum "$workspace/a1/TELORA-CLI.md" | cut -d' ' -f1)" \
        --arg design_hash "$(sha256sum "$workspace/a1/EDSL-DESIGN.md" | cut -d' ' -f1)" \
        --arg task_hash "$(sha256sum "$workspace/a1/TASK-A2.md" | cut -d' ' -f1)" \
        --arg prompt_hash "$(sha256sum "$anchor/A2-PROMPT.md" | cut -d' ' -f1)" \
        '{
          workspace: $workspace,
          server_url: $server_url,
          session_id: $session_id,
          event_url: ($server_url + "/event"),
          tui_command: $tui_command,
          task_started: false,
          opencode_version: $opencode_version,
          repository_revision: $revision,
          repository_dirty: $dirty,
          sha256: {
            telora: $telora_hash,
            "TELORA-TUTORIAL.md": $language_hash,
            "TELORA-CLI.md": $cli_hash,
            "EDSL-DESIGN.md": $design_hash,
            "TASK-A2.md": $task_hash,
            "A2-PROMPT.md": $prompt_hash
          }
        }' >"$session_metadata"
fi

stop_temporary_daemon
temporary_daemon_pid=
trap - EXIT INT TERM
flock -u 9
printf 'Workspace ready: %s\n' "$workspace"
printf 'Empty session ready: %s\n' "$session_id"
if [[ $server_was_running == true ]]; then
    exec opencode attach "$server_url" --dir "$workspace" \
        --session "$session_id" --pure
fi
exec opencode "$workspace" --hostname 127.0.0.1 --port "$port" \
    --session "$session_id" --pure
