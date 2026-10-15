#!/usr/bin/env bash
# Files the failures of a nightly stress run (.github/workflows/nightly-stress.yml)
# as GitHub issues labelled `investigate`, one per failing test. When an open
# issue with the same title exists, the run is added to it as a comment instead,
# so a test that keeps failing collects its runs in one place. An issue's title
# is "Nightly stress: <test>", and it is matched exactly.
#
# Usage: scripts/stress-issues.sh <suite> <job-result> [log...]
#   suite       Rust or Web
#   job-result  the stress job's result (success, failure, cancelled, skipped);
#               anything but failure files nothing
#   log         the output of a step that failed. The failures are read from it
#               by scripts/ci-failures-parse.awk, the parser behind `just ci-failures`.
#
# A failed job whose logs name no failing test (the build broke, a step failed
# before any test ran, the runner went away) is filed as one issue for the suite,
# with the logs' last lines when there are any.
#
# Environment:
#   GH_TOKEN, GITHUB_REPOSITORY, GITHUB_SERVER_URL, GITHUB_RUN_ID, GITHUB_SHA
#                           as GitHub Actions sets them
#   STRESS_ISSUES_DRY_RUN   when 1, print the issues and comments instead of filing
#                           them; GitHub is not queried
#
# Exit status: 0 when everything was filed (or nothing needed filing), 1 when an
# issue or comment could not be filed or the open issues could not be listed.
set -euo pipefail

usage="usage: scripts/stress-issues.sh <Rust|Web> <job-result> [log...]"
suite="${1:?$usage}"
result="${2:?$usage}"
shift 2

case "$suite" in
Rust)
    what="the Rust test suite as concurrent copies with every CPU busy (\`scripts/stress-tests.sh\`)"
    ;;
Web)
    what="every end-to-end test repeatedly with the browser's CPU throttled (\`scripts/web-stress.sh\`)"
    ;;
*)
    echo "error: the suite must be Rust or Web, got \"$suite\". $usage" >&2
    exit 1
    ;;
esac

if [ "$result" != failure ]; then
    echo "$suite stress: the job's result is $result, so there is nothing to file."
    exit 0
fi

dry_run="${STRESS_ISSUES_DRY_RUN:-}"
repo="${GITHUB_REPOSITORY:-}"
if [ "$dry_run" != 1 ]; then
    : "${repo:?GITHUB_REPOSITORY must be set to file issues}"
    command -v gh >/dev/null || { echo "error: gh is required to file issues" >&2; exit 1; }
fi
run_url="${GITHUB_SERVER_URL:-https://github.com}/${repo:-<repository>}/actions/runs/${GITHUB_RUN_ID:-<run>}"
commit="${GITHUB_SHA:-$(git rev-parse HEAD 2>/dev/null || echo unknown)}"
today="$(date -u +%Y-%m-%d)"

parser="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/ci-failures-parse.awk"
[ -f "$parser" ] || { echo "error: the log parser is missing at $parser" >&2; exit 1; }

US=$'\037'
RS=$'\036'
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# One entry per failing test, in the order first seen. A test named in two logs
# (the unloaded Tests step and the stress run) is one entry with both sets of
# messages.
titles=()
declare -A files=() projects=() messages=() counts=()
unparsed=""
logs_read=0

for log in "$@"; do
    if [ ! -f "$log" ]; then
        echo "note: $log does not exist; skipping it." >&2
        continue
    fi
    logs_read=$((logs_read + 1))
    while IFS="$US" read -r kind _key title file_list count project_list message_list; do
        message_lines="${message_list//$RS/$'\n'}"
        if [ "$kind" = unparsed ]; then
            unparsed+="$message_lines"$'\n'
            continue
        fi
        # A test binary that crashed is named by its file, which carries a build
        # hash; without the hash the title stays the same from one night to the next.
        if [ "$kind" = rust ] && [[ "$title" =~ ^(.+)-[0-9a-f]{16}$ ]]; then
            title="${BASH_REMATCH[1]} test binary"
        fi
        if [ -z "${counts[$title]+set}" ]; then
            titles+=("$title")
            counts[$title]=0
            files[$title]=""
            projects[$title]=""
            messages[$title]=""
        fi
        seen="${counts[$title]}"
        counts[$title]=$((seen + count))
        [ -z "$file_list" ] || files[$title]="$file_list"
        [ -z "$project_list" ] || projects[$title]="${projects[$title]:+${projects[$title]}, }$project_list"
        [ -z "$message_lines" ] || messages[$title]+="$message_lines"$'\n'
    done < <(awk -f "$parser" "$log")
done

if [ "$suite" = Rust ]; then
    fix="A test that failed under load only waits on elapsed time instead of the event it checks: make it wait on that event (CONTRIBUTING.md, \"Waiting in tests\"), never widen a timeout, add a sleep or rerun past it. One that also failed in the unloaded Tests step is an ordinary failure."
else
    fix="A spec that fails only under load waits on elapsed time instead of the event it checks: make it wait on that event (web/CONTRIBUTING.md, \"Writing a spec\"), never widen a timeout, add a sleep or rerun past it."
fi

repro_for() {
    local title="$1" spec
    if [ "$suite" = Rust ] && [[ "$title" == *" test binary" ]]; then
        printf "\`just stress\`, which runs every test binary"
    elif [ "$suite" = Rust ]; then
        printf "\`just stress %s\`" "$title"
    else
        spec="${title%% › *}"
        printf "\`WEB_STRESS_ALL=1 just web-stress %s\`, adding \`--grep\` with the test title to narrow it" "$spec"
    fi
}

# The fenced block of a failure's messages, or nothing when there are none.
message_block() {
    [ -n "$1" ] || return 0
    printf "\n\`\`\`text\n%s\`\`\`\n" "$1"
}

open_issues="$work/open-issues.tsv"
: >"$open_issues"
status=0
if [ "$dry_run" != 1 ]; then
    if ! gh issue list --repo "$repo" --state open --label investigate --limit 1000 \
        --json number,title --jq '.[] | "\(.number)\t\(.title)"' >"$open_issues"; then
        echo "error: could not list the open investigate issues of $repo, so nothing was filed; the failures are in $run_url." >&2
        exit 1
    fi
fi

file_failure() {
    local title="$1" body="$2" comment="$3" number
    number="$(awk -F '\t' -v t="$title" '$2 == t { print $1; exit }' "$open_issues")"
    if [ "$dry_run" = 1 ]; then
        if [ -n "$number" ]; then
            printf '=== would comment on #%s (%s)\n%s\n' "$number" "$title" "$comment"
        else
            printf '=== would open: %s\n%s\n' "$title" "$body"
        fi
        return 0
    fi
    if [ -n "$number" ]; then
        if gh issue comment "$number" --repo "$repo" --body "$comment" >/dev/null; then
            echo "Commented on #$number: $title"
        else
            echo "error: could not comment on #$number ($title)" >&2
            status=1
        fi
    elif url="$(gh issue create --repo "$repo" --title "$title" --label investigate --body "$body")"; then
        echo "Opened $url: $title"
    else
        echo "error: could not open an issue titled \"$title\"" >&2
        status=1
    fi
}

if [ "${#titles[@]}" -eq 0 ]; then
    title="Nightly stress: the $suite run failed without naming a failing test"
    if [ "$logs_read" -eq 0 ]; then
        detail="No step left a log of its failure: the job failed before its tests ran, or the runner went away. The job's own log in the run says which step failed."
    else
        detail="The logs of the failed steps name no failing test. Their last lines:"
    fi
    body="The nightly stress run on \`main\`, which runs $what, failed without naming a failing test.

- Run: $run_url
- Commit: \`$commit\`

$detail
$(message_block "$unparsed")"
    comment="Failed again without naming a failing test on $today: $run_url (commit \`$commit\`).
$(message_block "$unparsed")"
    file_failure "$title" "$body" "$comment"
    exit "$status"
fi

for name in "${titles[@]}"; do
    # GitHub refuses a title over 256 characters.
    title="Nightly stress: $name"
    title="${title:0:256}"
    details="- Run: $run_url
- Commit: \`$commit\`"
    [ -z "${files[$name]}" ] || details+="
- Files: \`${files[$name]}\`"
    [ -z "${projects[$name]}" ] || details+="
- Projects: ${projects[$name]}"
    if [ "$suite" = Web ] && [ "${counts[$name]}" -gt 1 ]; then
        details+="
- Failed runs of the test: ${counts[$name]}"
    fi
    body="\`$name\` failed in the nightly stress run on \`main\`, which runs $what.

$details
$(message_block "${messages[$name]}")

$fix

Reproduce it with $(repro_for "$name")."
    comment="Failed again on $today: $run_url (commit \`$commit\`).
$(message_block "${messages[$name]}")"
    file_failure "$title" "$body" "$comment"
done

exit "$status"
