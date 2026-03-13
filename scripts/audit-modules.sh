#!/usr/bin/env bash
set -euo pipefail

# Audit each module in the codebase in parallel using the Claude CLI.
#
# Usage:
#   ./scripts/audit-modules.sh [OPTIONS]
#
# Options:
#   -p, --prompt PROMPT   The audit prompt/instruction (required)
#   -m, --model MODEL     Model to use (default: sonnet)
#   -j, --jobs N          Max parallel jobs (default: 4)
#   -o, --output DIR      Output directory (default: audit-results/)
#   -h, --help            Show this help

MODEL="sonnet"
JOBS=4
PROMPT=""
OUTPUT_DIR="audit-results"

usage() {
    sed -n '/^# Usage:/,/^$/p' "$0" | sed 's/^# \?//'
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -p|--prompt)  PROMPT="$2"; shift 2 ;;
        -m|--model)   MODEL="$2"; shift 2 ;;
        -j|--jobs)    JOBS="$2"; shift 2 ;;
        -o|--output)  OUTPUT_DIR="$2"; shift 2 ;;
        -h|--help)    usage ;;
        *)            echo "Unknown option: $1"; usage ;;
    esac
done

if [[ -z "$PROMPT" ]]; then
    echo "Error: --prompt is required"
    echo "Example: ./scripts/audit-modules.sh -p 'Review for error handling issues'"
    exit 1
fi

# Find all modules: directories under src/ plus standalone .rs files in src/
MODULES=()
for dir in src/*/; do
    MODULES+=("${dir%/}")
done
for file in src/*.rs; do
    [[ -f "$file" && "$(basename "$file")" != "lib.rs" ]] && MODULES+=("$file")
done

echo "Auditing ${#MODULES[@]} modules with model=$MODEL, jobs=$JOBS"
echo "Prompt: $PROMPT"
echo "Output: $OUTPUT_DIR/"
echo "---"

mkdir -p "$OUTPUT_DIR"

audit_module() {
    local module="$1"
    local name
    name=$(echo "$module" | sed 's|src/||; s|/|_|g; s|\.rs$||')
    local outfile="$OUTPUT_DIR/${name}.md"

    # Collect all .rs files in the module
    local files
    if [[ -d "$module" ]]; then
        files=$(find "$module" -name '*.rs' -type f | sort)
    else
        files="$module"
    fi

    # Build the file list for context
    local file_contents=""
    for f in $files; do
        file_contents+="
--- $f ---
$(cat "$f")
"
    done

    local full_prompt="You are auditing the Rust module \`$module\` from the Residuum project.

$PROMPT

Here are the files in this module:

$file_contents

Provide your audit findings in markdown. Be specific — reference file paths and line numbers. Focus on actionable findings, not praise."

    echo "[start] $module -> $outfile"

    if claude -p --model "$MODEL" --no-session-persistence "$full_prompt" > "$outfile" 2>/dev/null; then
        echo "[done]  $module"
    else
        echo "[ERROR] $module — claude exited with $?"
        echo "# Audit failed for $module" > "$outfile"
    fi
}

export -f audit_module
export MODEL OUTPUT_DIR PROMPT

# Run audits in parallel
printf '%s\n' "${MODULES[@]}" | xargs -P "$JOBS" -I {} bash -c 'audit_module "$@"' _ {}

echo "---"
echo "All audits complete. Results in $OUTPUT_DIR/"
