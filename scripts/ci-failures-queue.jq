# Selects the latest merge queue run of each workflow for one pull request, from a
# repository's merge_group runs. Input: the runs of every page, as slurped JSON
# objects. Arguments: $prefix is the queue branch of the PR's batches,
# gh-readonly-queue/<base>/pr-<n>-; $since is the epoch second of the PR head's
# commit, so a batch from before a newer push is not reported. Only runs that did
# not succeed are kept, the batch in progress included.
[ .[].workflow_runs[]
    | select(.name != $excluded
        and ((.head_branch // "") | startswith($prefix))
        and ((.created_at | fromdateiso8601) >= $since)) ]
| group_by(.name)
| map(max_by(.id))
| .[]
| select(.conclusion != "success")
| {id, name, run_attempt, status, conclusion, head_branch, head_sha, html_url}
