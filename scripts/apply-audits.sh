#!/usr/bin/env bash
set -euo pipefail

# Apply audit findings one module at a time on a single branch, one commit per module.
#
# Usage:
#   ./scripts/apply-audits.sh [OPTIONS]
#
# Options:
#   -i, --input DIR       Audit results directory (default: audit-results/)
#   -m, --model MODEL     Model to use (default: sonnet)
#   -b, --branch NAME     Branch name (default: audit/apply-fixes)
#   --dry-run             Show what would be done without doing it
#   -h, --help            Show this help
#
# Examples:
#   ./scripts/apply-audits.sh
#   ./scripts/apply-audits.sh -i audit-results/ -m opus
#   ./scripts/apply-audits.sh -b audit/no-silent-failures --dry-run

MODEL="sonnet"
INPUT_DIR="audit-results"
BRANCH="audit/apply-fixes"
DRY_RUN=false

usage() {
    sed -n '/^# Usage:/,/^[^#]/p' "$0" | head -n -1 | sed 's/^# \?//'
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -i|--input)   INPUT_DIR="$2"; shift 2 ;;
        -m|--model)   MODEL="$2"; shift 2 ;;
        -b|--branch)  BRANCH="$2"; shift 2 ;;
        --dry-run)    DRY_RUN=true; shift ;;
        -h|--help)    usage ;;
        *)            echo "Unknown option: $1"; usage ;;
    esac
done

if [[ ! -d "$INPUT_DIR" ]]; then
    echo "Error: audit results directory '$INPUT_DIR' not found"
    exit 1
fi

# Collect audit files, skipping failed audits
AUDIT_FILES=()
for f in "$INPUT_DIR"/*.md; do
    [[ ! -f "$f" ]] && continue
    if head -1 "$f" | grep -q "^# Audit failed"; then
        echo "[skip] $(basename "$f" .md) — audit had failed"
        continue
    fi
    if [[ $(wc -c < "$f") -lt 50 ]]; then
        echo "[skip] $(basename "$f" .md) — audit file too small"
        continue
    fi
    AUDIT_FILES+=("$f")
done

if [[ ${#AUDIT_FILES[@]} -eq 0 ]]; then
    echo "No valid audit results found in $INPUT_DIR/"
    exit 1
fi

echo "Found ${#AUDIT_FILES[@]} audit results to apply"
echo "Model: $MODEL | Branch: $BRANCH"
echo "---"

if [[ -n $(git status --porcelain) ]]; then
    echo "Error: working tree is dirty. Commit or stash changes first."
    exit 1
fi

if ! $DRY_RUN; then
    # Create the branch from main (fail if it already exists)
    if ! git checkout -b "$BRANCH" main 2>/dev/null; then
        echo "Error: branch '$BRANCH' already exists. Delete it or choose a different name with -b."
        exit 1
    fi
    echo "Created branch: $BRANCH"
fi

APPLIED=0
SKIPPED=0
FAILED=0

apply_audit() {
    local audit_file="$1"
    local module_name
    module_name=$(basename "$audit_file" .md)

    # Map module name back to source path
    local module_path
    if [[ -f "src/${module_name}.rs" ]]; then
        module_path="src/${module_name}.rs"
    elif [[ -d "src/${module_name}" ]]; then
        module_path="src/${module_name}"
    else
        echo "[skip] $module_name — no matching source at src/${module_name}{,.rs}"
        ((SKIPPED++))
        return 0
    fi

    local audit_content
    audit_content=$(cat "$audit_file")

    echo ""
    echo "=== $module_name ==="
    echo "  Source: $module_path"

    if $DRY_RUN; then
        echo "  [dry-run] Would apply fixes and commit"
        return 0
    fi

    # Build the prompt into a temp file to avoid ARG_MAX
    local tmpfile
    tmpfile=$(mktemp)
    trap "rm -f '$tmpfile'" RETURN

    local source_files
    if [[ -d "$module_path" ]]; then
        source_files=$(find "$module_path" -name '*.rs' -type f | sort)
    else
        source_files="$module_path"
    fi

    {
        cat <<'INSTRUCTIONS'
You are fixing issues in a Rust module based on audit findings.

Rules:
- ONLY fix issues explicitly called out in the audit findings below
- Do NOT refactor, rename, or "improve" code beyond what the audit asks for
- Do NOT add comments explaining your fixes
- Do NOT touch files outside the module being audited
- If an audit finding is vague or you're unsure how to fix it, skip it

Your final output will be used directly as the git commit message.
First line: concise summary (<=72 chars). Then a blank line and a short body listing what was fixed.
No markdown fencing, no preamble, no Co-Authored-By lines.

INSTRUCTIONS
        echo "## Audit Findings"
        echo ""
        echo "$audit_content"
        echo ""
        echo "## Source Files"
        echo ""
        for f in $source_files; do
            echo "--- $f ---"
            cat "$f"
            echo ""
        done
    } > "$tmpfile"

    echo "  Applying fixes..."

    local commit_msg
    if commit_msg=$(claude -p --model "$MODEL" --no-session-persistence \
        --allowedTools "Edit Read Glob Grep" \
        < "$tmpfile" 2>/dev/null); then

        # Check if any changes were made
        if [[ -z $(git status --porcelain) ]]; then
            echo "  [no-op] No changes needed"
            ((SKIPPED++))
            return 0
        fi

        git add -A
        git commit -m "$commit_msg" > /dev/null 2>&1
        echo "  [done] Committed"
        ((APPLIED++))
    else
        echo "  [FAIL] Claude exited with an error"
        # Clean up any partial changes
        git checkout -- . 2>/dev/null
        git clean -fd 2>/dev/null
        ((FAILED++))
    fi
}

for audit_file in "${AUDIT_FILES[@]}"; do
    apply_audit "$audit_file"
done

echo ""
echo "---"
echo "Applied: $APPLIED | Skipped: $SKIPPED | Failed: $FAILED"

if ! $DRY_RUN && [[ $APPLIED -gt 0 ]]; then
    echo ""
    echo "Review the branch:"
    echo "  git log --oneline main..$BRANCH"
    echo "  git diff main...$BRANCH"
    echo ""
    echo "To merge:"
    echo "  git checkout main && git merge --no-ff $BRANCH"
fi
