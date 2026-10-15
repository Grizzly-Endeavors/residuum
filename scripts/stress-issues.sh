#!/usr/bin/env bash
# Files the failures of a nightly stress run (.github/workflows/nightly-stress.yml)
# as GitHub issues labelled `investigate`, one per failing test. When an open
# issue with the same title exists, the run is added to it as a comment instead,
# so a test that keeps failing collects its runs in one place. An issue's title
# is "Nightly stress: <test>", and it is matched exactly.
#
# Usage: scripts/stress-issues.sh <suite> <job-result> [log...]
#   suite       Rust or Web
#   job-result  the stress job's result: failure files the failing tests, cancelled
#               files that the run did not finish (the job's timeout), and success
#               or skipped files nothing
#   log         the output of a step that failed. The failures are read from it
#               by scripts/ci-failures-parse.awk, the parser behind `just ci-failures`.
#
# A failed job whose logs name no failing test (the build broke, a step failed
# before any test ran, the runner went away) is filed as one issue for the suite,
# with the logs' last lines when there are any.
#
# A run that would open more than MAX_ISSUES_PER_RUN new issues opens one issue for
# the suite instead, listing every failing test, so a broken main or runner cannot
# flood the tracker. The tests that already have an open issue still get their
# comments.
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

readonly MAX_ISSUES_PER_RUN=10
# GitHub caps an issue body or comment at 65,536 characters, so a flood summary
# names at most this many tests and says how many it left out.
readonly LIST_LIMIT=200

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

case "$result" in
failure | cancelled) ;;
*)
    echo "$suite stress: the job's result is $result, so there is nothing to file."
    exit 0
    ;;
esac

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

# The number of the open investigate issue with exactly this title, if there is one.
# The title goes in through the environment, because awk -v would read its backslashes.
open_number() {
    T="$1" awk -F '\t' '$2 == ENVIRON["T"] { print $1; exit }' "$open_issues"
}

# The first 256 characters of a title, the most GitHub accepts. bash counts
# characters only under a UTF-8 locale, and the test names carry multi-byte
# characters (Playwright's ›), so a byte count could cut one in half.
truncate_title() {
    local LC_ALL=C.UTF-8
    printf '%s' "${1:0:256}"
}

file_failure() {
    local title="$1" body="$2" comment="$3" number
    number="$(open_number "$title")"
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

# A cancelled job did not finish its tests, so a test may have hung under load. The
# job's timeout is the usual cause of the cancellation.
if [ "$result" = cancelled ]; then
    title="$(truncate_title "Nightly stress: the $suite run did not finish")"
    body="The nightly stress run on \`main\`, which runs $what, was cancelled before it finished. The job's timeout is the usual cause: a test that hangs under load keeps the job running until the timeout cancels it, so the tests after it never run.

- Run: $run_url
- Commit: \`$commit\`
$(message_block "$unparsed")
Find the job that was cancelled in the run and the test it was in when it stopped. A test that waits on elapsed time instead of the event it checks is the usual cause: make it wait on that event (CONTRIBUTING.md, \"Waiting in tests\"), never widen a timeout, add a sleep or rerun past it."
    comment="Did not finish on $today: $run_url (commit \`$commit\`). The job was cancelled before it finished, most likely by its timeout.
$(message_block "$unparsed")"
    file_failure "$title" "$body" "$comment"
fi

if [ "${#titles[@]}" -eq 0 ]; then
    if [ "$result" = cancelled ]; then
        exit "$status"
    fi
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

new_count=0
for name in "${titles[@]}"; do
    if [ -z "$(open_number "$(truncate_title "Nightly stress: $name")")" ]; then
        new_count=$((new_count + 1))
    fi
done

# The failing tests of the run as a Markdown list, LIST_LIMIT of them at most.
test_list() {
    local name shown=0
    for name in "${titles[@]}"; do
        if [ "$shown" -eq "$LIST_LIMIT" ]; then
            echo "- and $(( ${#titles[@]} - LIST_LIMIT )) more, which the run's logs name"
            break
        fi
        echo "- \`$name\`"
        shown=$((shown + 1))
    done
}

if [ "$new_count" -gt "$MAX_ISSUES_PER_RUN" ]; then
    title="$(truncate_title "Nightly stress: the $suite run failed more than $MAX_ISSUES_PER_RUN tests")"
    body="The nightly stress run on \`main\`, which runs $what, failed ${#titles[@]} tests, $new_count of them with no open issue yet. That is more than $MAX_ISSUES_PER_RUN new issues, so the run is filed here as one issue instead of one issue per test. A broken main or runner usually causes this: a full disk, a broken Docker daemon, or a dependency that breaks many tests at once.

- Run: $run_url
- Commit: \`$commit\`

Failing tests:

$(test_list)

A test that already has an open \`investigate\` issue got a comment on that issue. Reproduce one test with $(repro_for "${titles[0]}") and fix the cause before the others."
    comment="Failed again on $today: $run_url (commit \`$commit\`). ${#titles[@]} tests failed.

$(test_list)"
    file_failure "$title" "$body" "$comment"
fi

for name in "${titles[@]}"; do
    title="$(truncate_title "Nightly stress: $name")"
    if [ "$new_count" -gt "$MAX_ISSUES_PER_RUN" ] && [ -z "$(open_number "$title")" ]; then
        continue
    fi
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
