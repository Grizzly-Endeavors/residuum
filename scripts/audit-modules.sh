#!/usr/bin/env bash
set -euo pipefail

# Audit each module in the codebase in parallel using the Claude CLI.
#
# Usage:
#   ./scripts/audit-modules.sh [OPTIONS]
#
# Options:
#   -p, --prompt PROMPT   Named prompt from scripts/prompts/ or inline text
#   -f, --file FILE       Read audit prompt from a file (use - for stdin)
#   -m, --model MODEL     Model to use (default: sonnet)
#   -j, --jobs N          Max parallel jobs (default: 4)
#   -o, --output DIR      Output directory (default: audit-results/)
#   -h, --help            Show this help
#
# Examples:
#   ./scripts/audit-modules.sh -p clean-audit
#   ./scripts/audit-modules.sh -p "Review for error handling issues"
#   ./scripts/audit-modules.sh -f audits/no-silent-failures.txt -m opus
#   cat prompt.txt | ./scripts/audit-modules.sh -f -

MODEL="sonnet"
JOBS=4
PROMPT=""
OUTPUT_DIR="audit-results"

usage() {
    sed -n '/^# Usage:/,/^# Examples:/p' "$0" | sed 's/^# \?//'
    sed -n '/^# Examples:/,/^$/p' "$0" | sed 's/^# \?//'
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -p|--prompt)
            SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
            if [[ -f "$SCRIPT_DIR/prompts/$2.md" ]]; then
                PROMPT="$(cat "$SCRIPT_DIR/prompts/$2.md")"
            else
                PROMPT="$2"
            fi
            shift 2 ;;
        -f|--file)
            if [[ "$2" == "-" ]]; then
                PROMPT="$(cat)"
            else
                PROMPT="$(cat "$2")"
            fi
            shift 2 ;;
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
    [[ -f "$file" && "$(basename "$file")" != "lib.rs" && "$(basename "$file")" != "error.rs" ]] && MODULES+=("$file")
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

    # Build the full prompt with file contents, write to a temp file to avoid ARG_MAX
    local tmpfile
    tmpfile=$(mktemp)
    trap "rm -f '$tmpfile'" RETURN

    {
        echo "You are auditing the Rust module \`$module\` from the Residuum project."
        echo ""
        echo "$PROMPT"
        echo ""
        echo "Here are the files in this module:"
        echo ""
        for f in $files; do
            echo "--- $f ---"
            cat "$f"
            echo ""
        done
        echo "Provide your audit findings in markdown. Be specific — reference file paths and line numbers. Focus on actionable findings, not praise."
    } > "$tmpfile"

    echo "[start] $module -> $outfile"

    if claude -p --model "$MODEL" --no-session-persistence < "$tmpfile" > "$outfile" 2>/dev/null; then
        echo "[done]  $module"
    else
        echo "[FAIL]  $module (exit $?)"
        echo "# Audit failed for $module" > "$outfile"
    fi
}

export -f audit_module
export MODEL OUTPUT_DIR PROMPT

# Run audits in parallel
printf '%s\n' "${MODULES[@]}" | xargs -P "$JOBS" -I {} bash -c 'audit_module "$@"' _ {}

echo "---"
echo "All module audits complete. Running review pass..."

# Build a prompt that points the model at the audit files and lets it edit them directly
tmpfile=$(mktemp)
trap "rm -f '$tmpfile'" EXIT

{
    cat <<'INSTRUCTIONS'
You have a directory of audit result files. For each .md file in the directory:

1. Read it
2. Remove any findings that require architectural changes, new types, API redesigns,
   or changes spanning multiple modules. These cannot be fixed by editing the module alone.
3. If a file has no remaining findings after filtering, delete the file entirely.
4. Otherwise, rewrite the file in place with only the actionable findings.

After editing all files, write a SUMMARY.md in the same directory containing:
- A table of modules with actionable findings and their severity
- A section listing everything you removed, organized by module, with brief reasoning
- A section on suggested architectural or cross-module changes (if any patterns emerged)

INSTRUCTIONS
    echo ""
    echo "The audit results directory is: $OUTPUT_DIR/"
} > "$tmpfile"

if claude -p --model "$MODEL" --no-session-persistence \
    --allowedTools "Read Glob Edit Write" \
    < "$tmpfile" > /dev/null 2>&1; then
    echo "Review complete."
else
    echo "Review pass failed."
fi

echo "---"
echo "Results in $OUTPUT_DIR/"
echo "  SUMMARY.md  — overview, removed items, architectural suggestions"
echo "  *.md        — cleaned per-module audits (ready for apply-audits.sh)"
