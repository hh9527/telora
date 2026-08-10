#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage:" >&2
    echo "  $0 ai2 DEST TELORA" >&2
    echo "  $0 ai2-correction DEST TELORA PRIOR_CANDIDATE FEEDBACK" >&2
    echo "  $0 ai3 DEST TELORA ACCEPTED_EDSL" >&2
    echo "  $0 ai4 DEST PUBLIC_INTENT PUBLIC_API REQUEST" >&2
    exit 2
}

[[ $# -ge 1 ]] || usage

role=$1
shift
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd -- "$script_dir/../.." && pwd)

init_workspace() {
    local dest=$1
    [[ ! -e "$dest" ]] || {
        echo "destination already exists: $dest" >&2
        exit 1
    }
    mkdir -p "$dest/requirement" "$dest/crates"
}

init_git() {
    local dest=$1
    git -C "$dest" init -q
    git -C "$dest" config user.name "Telora Experiment Host"
    git -C "$dest" config user.email "experiment@localhost"
    git -C "$dest" add .
    git -C "$dest" commit -q -m "${role} isolated baseline"
    git -C "$dest" rev-parse HEAD
}

case "$role" in
    ai2)
        [[ $# -eq 2 ]] || usage
        dest=$1
        telora=$2
        [[ -x "$telora" ]] || { echo "Telora is not executable: $telora" >&2; exit 1; }
        init_workspace "$dest"
        mkdir -p "$dest/bin" "$dest/crates/ontology-edsl"
        cp "$telora" "$dest/bin/telora"
        cp "$script_dir/roles/ai2.md" "$dest/requirement/ROLE.md"
        cp "$repo_dir/tutorial.md" "$dest/requirement/tutorial.md"
        cp "$script_dir/edsl-design.md" "$dest/requirement/edsl-design.md"
        ;;
    ai2-correction)
        [[ $# -eq 4 ]] || usage
        dest=$1
        telora=$2
        prior=$3
        feedback=$4
        [[ -x "$telora" ]] || { echo "Telora is not executable: $telora" >&2; exit 1; }
        [[ -d "$prior/src" ]] || { echo "prior candidate source is missing: $prior/src" >&2; exit 1; }
        [[ -f "$feedback" ]] || { echo "feedback packet is missing: $feedback" >&2; exit 1; }
        for file in telora-deps.json EDSL_TUTORIAL.md AI3_CONTRACT.md STAGE2_DESIGN.md STAGE2_NOTES.md; do
            [[ -f "$prior/$file" ]] || { echo "prior candidate file is missing: $prior/$file" >&2; exit 1; }
        done
        init_workspace "$dest"
        mkdir -p "$dest/bin" "$dest/crates/ontology-edsl"
        cp "$telora" "$dest/bin/telora"
        cp "$script_dir/roles/ai2.md" "$dest/requirement/ROLE.md"
        cp "$repo_dir/tutorial.md" "$dest/requirement/tutorial.md"
        cp "$script_dir/edsl-design.md" "$dest/requirement/edsl-design.md"
        cp "$feedback" "$dest/requirement/FEEDBACK.md"
        cp -R "$prior/." "$dest/crates/ontology-edsl/"
        ;;
    ai3)
        [[ $# -eq 3 ]] || usage
        dest=$1
        telora=$2
        edsl=$3
        [[ -x "$telora" ]] || { echo "Telora is not executable: $telora" >&2; exit 1; }
        [[ -d "$edsl/src" ]] || { echo "accepted eDSL source is missing: $edsl/src" >&2; exit 1; }
        for file in telora-deps.json EDSL_TUTORIAL.md AI3_CONTRACT.md; do
            [[ -f "$edsl/$file" ]] || { echo "accepted eDSL file is missing: $edsl/$file" >&2; exit 1; }
        done
        init_workspace "$dest"
        mkdir -p "$dest/bin" "$dest/crates/ontology-edsl" "$dest/crates/enterprise-model"
        cp "$telora" "$dest/bin/telora"
        cp "$script_dir/roles/ai3.md" "$dest/requirement/ROLE.md"
        cp "$repo_dir/tutorial.md" "$dest/requirement/tutorial.md"
        cp "$script_dir/domain.md" "$dest/requirement/domain.md"
        cp "$edsl/EDSL_TUTORIAL.md" "$dest/requirement/EDSL_TUTORIAL.md"
        cp "$edsl/AI3_CONTRACT.md" "$dest/requirement/AI3_CONTRACT.md"
        cp "$edsl/telora-deps.json" "$dest/crates/ontology-edsl/telora-deps.json"
        cp -R "$edsl/src" "$dest/crates/ontology-edsl/src"
        ;;
    ai4)
        [[ $# -eq 4 ]] || usage
        dest=$1
        public_intent=$2
        public_api=$3
        request=$4
        for file in "$public_intent" "$public_api" "$request"; do
            [[ -f "$file" ]] || { echo "Stage 4 input is missing: $file" >&2; exit 1; }
        done
        init_workspace "$dest"
        mkdir -p "$dest/crates/intent"
        cp "$script_dir/roles/ai4.md" "$dest/requirement/ROLE.md"
        cp "$script_dir/intent-tutorial.md" "$dest/requirement/INTENT_TUTORIAL.md"
        cp "$public_intent" "$dest/requirement/PUBLIC_INTENT.md"
        cp "$public_api" "$dest/requirement/PUBLIC_API.md"
        cp "$request" "$dest/requirement/REQUEST.md"
        ;;
    *)
        usage
        ;;
esac

init_git "$dest"
