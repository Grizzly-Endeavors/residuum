# Turns one failed CI job's log into failure records for scripts/ci-failures.sh.
#
# Input is a job log as `gh api repos/<repo>/actions/jobs/<id>/logs` prints it: one
# line per log line with a leading timestamp. ANSI colour codes are removed first,
# so the rules see the plain text the tools print.
#
# Output has one line per record, with fields separated by US (\037): kind, key,
# title, files, count, projects, messages. Messages are separated by RS (\036).
# The kinds are playwright, vitest, rust, lint, clippy, fmt, and unparsed.
#
# A step runs from one "##[group]Run" line, or one "Post job cleanup." line, to the
# next. Only a step that failed, meaning it has an "##[error]" line, contributes
# records, so warnings from passing steps are not reported. A failed step that
# yields no record prints its last lines as one unparsed record, titled with the
# step's name.

BEGIN {
    FS = "\n"
    US = "\037"
    RS2 = "\036"
    MAXMSG = 3
    nl = 0
    n = 0
    nsteps = 0
    cur = 0
    step_title[0] = ""
    step_start[0] = 1
    step_err[0] = 0
    cur_test = ""
    collect = 0
    in_stress = 0
    stress_kind = ""
    pwleft = 0
    vtleft = 0
    esl_file = ""
    cl_pending = 0
    HANG = "libtest: still running after 60 seconds"
}

function trim(s) {
    sub(/^[ \t\r]+/, "", s)
    sub(/[ \t\r]+$/, "", s)
    return s
}

# Index of the record for key in the current step, or 0.
function findrec(key) {
    return ((cur SUBSEP key) in kidx) ? kidx[cur SUBSEP key] : 0
}

# The record for key in the current step, created on first sight. isnew says
# whether this call created it. A key is per step, so the same test failing in
# two failed steps is listed under each.
function getrec(kind, key, title, files,    i) {
    i = findrec(key)
    if (i > 0) {
        rcount[i]++
        isnew = 0
        return i
    }
    n++
    i = n
    kidx[cur SUBSEP key] = i
    rstep[i] = cur
    rk[i] = kind
    rkey[i] = key
    rtitle[i] = title
    rfiles[i] = files
    rcount[i] = 1
    rproj[i] = ""
    rmsg[i] = ""
    rnmsg[i] = 0
    isnew = 1
    return i
}

# A Rust test is named once per block that mentions it (the progress line, the
# stdout header, the panic, the summary), so its count stays at one.
function rustrec(name,    i) {
    i = getrec("rust", "rust|" name, name, "")
    rcount[i] = 1
    if (isnew && ((cur SUBSEP name) in hung)) addmsg(i, HANG)
    return i
}

function addmsg(i, t) {
    if (i == 0 || t == "" || rnmsg[i] >= MAXMSG) return
    rmsg[i] = (rnmsg[i] > 0 ? rmsg[i] RS2 : "") t
    rnmsg[i]++
}

function addfile(i, f) {
    if (f == "") return
    if (index(" " rfiles[i] " ", " " f " ") > 0) return
    rfiles[i] = (rfiles[i] == "" ? f : rfiles[i] " " f)
}

function addproj(i, p) {
    if (index(", " rproj[i] ", ", ", " p ", ") > 0) return
    rproj[i] = (rproj[i] == "" ? p : rproj[i] ", " p)
}

# Prints the last 15 relevant lines of log lines from..to-1 as one unparsed record.
function unparsed(title, from, to,    k, cnt, picked, msgs, t) {
    cnt = 0
    for (k = to - 1; k >= from && cnt < 15; k--) {
        t = L[k]
        if (t ~ /[^ \t]/ && t !~ /^##\[/ && t !~ /^shell: /) {
            cnt++
            picked[cnt] = trim(t)
        }
    }
    msgs = ""
    for (k = cnt; k >= 1; k--) {
        msgs = msgs (k < cnt ? RS2 : "") picked[k]
    }
    print "unparsed" US "" US title US "" US "0" US "" US msgs
}

function printrec(i) {
    print rk[i] US rkey[i] US rtitle[i] US rfiles[i] US rcount[i] US rproj[i] US rmsg[i]
}

{
    line = $0
    sub(/\r$/, "", line)
    gsub(/\033\[[0-9;]*[A-Za-z]/, "", line)
    sub(/^(\357\273\277)?[0-9][0-9T:.-]*Z ?/, "", line)
    nl++
    L[nl] = line
}

line ~ /^##\[group\]Run / || line ~ /^Post job cleanup\.$/ {
    cur = ++nsteps
    step_title[cur] = line
    sub(/^##\[group\]/, "", step_title[cur])
    step_start[cur] = nl
    step_err[cur] = 0
    cur_test = ""
    collect = 0
    in_stress = 0
    stress_kind = ""
    pwleft = 0
    vtleft = 0
    esl_file = ""
    cl_pending = 0
    next
}

line ~ /^##\[error\]/ {
    step_err[cur] = nl
}

# Rust: quiet-mode progress names, the per-test stdout blocks, and the panic.
line ~ /^---- .* stdout ----$/ {
    cur_test = line
    sub(/^---- /, "", cur_test)
    sub(/ stdout ----$/, "", cur_test)
    rustrec(cur_test)
    next
}
line ~ /^(failures:|test result:)/ {
    cur_test = ""
}
line ~ / --- FAILED$/ {
    t = line
    sub(/ --- FAILED$/, "", t)
    sub(/^.* /, "", t)
    if (t != "") rustrec(t)
    next
}
line ~ /^test .* \.\.\. FAILED$/ {
    t = line
    sub(/^test /, "", t)
    sub(/ \.\.\. FAILED$/, "", t)
    rustrec(t)
    next
}
# The note can follow quiet-mode progress dots on the same line, and it comes before
# the test's own FAILED line, so a test that is noted here is remembered until its
# record is made.
line ~ /has been running for over/ {
    t = line
    sub(/^.*test /, "", t)
    sub(/ has been running.*$/, "", t)
    hung[cur SUBSEP t] = 1
    i = findrec("rust|" t)
    if (i > 0) addmsg(i, HANG)
    next
}
line ~ /panicked at / {
    # A panic inside a test's stdout block belongs to that test, including one on
    # a thread the test spawned. Outside a block, the thread name is the only hint.
    # The file a panic was raised in counts toward the test's file whichever thread
    # raised it, because a panic in code the change edits can be what broke the
    # test. Windows prints backslashes in the location, so they become slashes.
    name = cur_test
    if (line ~ /thread '/) {
        thread = line
        sub(/^.*thread '/, "", thread)
        sub(/'.*$/, "", thread)
        if (name == "") name = thread
    }
    f = line
    sub(/^.*panicked at /, "", f)
    sub(/:[0-9]+:[0-9]+:?$/, "", f)
    gsub(/\\/, "/", f)
    if (name != "") {
        i = rustrec(name)
        if (f ~ /\.rs$/) addfile(i, f)
        collect = i
        ncollect = 0
    }
    next
}
collect > 0 {
    if (line ~ /^(failures:|----|test result:|note:|error:)/) {
        collect = 0
    } else if (line ~ /[^ \t]/) {
        addmsg(collect, trim(line))
        ncollect++
        if (ncollect >= MAXMSG) collect = 0
    }
}

# Stress: the names that failed in one of the concurrent copies. With the Tests
# step's log to compare against, the summary splits them under headings: a test
# that failed only under load waits on time, one that also failed in the Tests step
# is an ordinary failure, and a binary that exited without naming a test crashed.
# Without it the summary is one unheaded list.
line ~ /^Failures \(count out of / {
    in_stress = 1
    stress_kind = ""
    copies = line
    sub(/^Failures \(count out of /, "", copies)
    copies = copies + 0
    next
}
# The script's closing "error:" line can be printed between the heading and the
# counts, because its stderr and the summary on stdout are interleaved in the log.
# The summary therefore runs to the end of the step, and this line is not part of it.
in_stress && line ~ /^error:/ {
    next
}
in_stress && line ~ /^Failed under load only/ {
    stress_kind = "load"
    next
}
in_stress && line ~ /^Also failed in the Tests step/ {
    stress_kind = "both"
    next
}
in_stress && line ~ /^Exited without naming a failed test/ {
    stress_kind = "crashed"
    next
}
in_stress && line ~ /^ *[0-9]+ [^ ]+ \(exited [0-9]+ without naming a failed test\)$/ {
    split(trim(line), parts, " ")
    i = rustrec(parts[2])
    addmsg(i, "exited " parts[4] " without naming a failed test, in " parts[1] " of " copies " copies under load")
    next
}
in_stress && line ~ /^ *[0-9]+ [^ ]+$/ {
    split(trim(line), parts, " ")
    i = rustrec(parts[2])
    msg = "failed " parts[1] " of " copies " copies under load"
    if (stress_kind == "load") {
        msg = msg " only: the test waits on time, so fix the wait"
    } else if (stress_kind == "both") {
        msg = msg " and in the Tests step: an ordinary failure"
    }
    addmsg(i, msg)
    next
}

# Playwright: "  N) [project] › spec:line:col › title @tag", then Error/Expected/Received.
line ~ /^ *[0-9]+\) \[[^]]+\] › / {
    s = line
    proj = s
    sub(/^ *[0-9]+\) \[/, "", proj)
    sub(/\].*$/, "", proj)
    sub(/^ *[0-9]+\) \[[^]]+\] › /, "", s)
    file = ""
    title = s
    if (match(s, /:[0-9]+:[0-9]+ › /)) {
        file = substr(s, 1, RSTART - 1)
        title = substr(s, RSTART + RLENGTH)
    }
    sub(/( @[^ ]+)+ *$/, "", title)
    title = trim(title)
    i = getrec("playwright", "pw|" file "|" title, file " › " title, (file == "" ? "" : "web/" file))
    addproj(i, proj)
    pwrec = i
    pwleft = (isnew ? 12 : 0)
    next
}
pwleft > 0 {
    pwleft--
    if (line ~ /^ +Error: /) {
        addmsg(pwrec, trim(line))
    } else if (line ~ /^ +(Expected|Received): / && line !~ /__screenshots__|test-results\//) {
        addmsg(pwrec, trim(line))
    } else if (line ~ / are different/) {
        addmsg(pwrec, trim(line))
    }
}

# Vitest: "FAIL  [project] file > suite > test", or "FAIL  [project] file [ file ]"
# for a file that did not load, then the assertion message. Vitest puts a project
# label before the file when the config has several projects: "|unit|" without
# colour, and a bare word, which after the colour codes are stripped looks like
# "unit  ", with colour.
line ~ /^ *FAIL +[^ ]/ {
    s = line
    sub(/^ *FAIL +/, "", s)
    if (s ~ /^\|[^|]*\| +/) {
        sub(/^\|[^|]*\| +/, "", s)
    } else if (s ~ /^[A-Za-z0-9_-]+ +[^ ]*\//) {
        sub(/^[A-Za-z0-9_-]+ +/, "", s)
    } else if (s ~ /^[A-Za-z0-9_-]+ +[^ ]+\.(ts|tsx|js|jsx|mjs|svelte)( |$)/) {
        # A root-level file has no slash, so only its test file extension tells it
        # from the label.
        sub(/^[A-Za-z0-9_-]+ +/, "", s)
    }
    file = s
    sub(/ (>|\[) .*$/, "", file)
    vtleft = 0
    if (file ~ /\.(ts|tsx|js|jsx|mjs|svelte)$/) {
        title = (index(s, " > ") > 0 ? s : file)
        i = getrec("vitest", "vt|" s, title, "web/" file)
        if (isnew) {
            vtrec = i
            vtleft = 10
        }
    }
    next
}
vtleft > 0 {
    vtleft--
    if (rnmsg[vtrec] == 0 && line ~ /(Error|expected)/) addmsg(vtrec, trim(line))
}

# ESLint: an absolute file path. Stylelint: a path relative to web/, as its string
# formatter prints it. Each file's problems end at a blank line. Only errors are
# listed: a warning does not fail a step, so a warning in a failed step is not the
# failure.
line ~ /^\/[^ ]+\.(svelte|ts|tsx|js|mjs|cjs|css)$/ {
    esl_file = line
    sub(/^\/home\/runner\/_work\/[^\/]+\/[^\/]+\//, "", esl_file)
    next
}
line ~ /^(src|e2e)\/[^ ]+\.(css|svelte)$/ {
    esl_file = "web/" line
    next
}
line ~ /^[ \t]*$/ {
    esl_file = ""
    next
}
esl_file != "" && line ~ /^ +[0-9]+:[0-9]+ +(error|✖) / {
    loc = trim(line)
    sub(/ .*$/, "", loc)
    rest = trim(line)
    sub(/^[0-9]+:[0-9]+ +/, "", rest)
    sub(/^[^ ]+ +/, "", rest)
    rule = rest
    sub(/^.* /, "", rule)
    msg = rest
    sub(/ +[^ ]+$/, "", msg)
    i = getrec("lint", "lint|" esl_file "|" rule, esl_file " " rule, esl_file)
    if (isnew) addmsg(i, loc "  " msg)
    next
}

# Cargo and clippy: "error[code]: message", then "  --> file:line:col". Only errors:
# CI runs clippy with -D warnings, so every lint prints as an error, and a rustc
# warning in a failed step must not stand in for that step's real failure.
line ~ /^error(\[[A-Za-z0-9_]+\])?: / && line !~ /could not compile|test failed|tests failed|aborting due to|didn.t exit/ {
    cmsg = line
    sub(/^error(\[[A-Za-z0-9_]+\])?: /, "", cmsg)
    cl_pending = 1
    next
}
cl_pending == 1 {
    cl_pending = 0
    if (line ~ /^ +--> /) {
        loc = line
        sub(/^ +--> /, "", loc)
        f = loc
        sub(/:[0-9]+:[0-9]+$/, "", f)
        i = getrec("clippy", "clippy|" f "|" cmsg, f ": " cmsg, f)
        if (isnew) addmsg(i, loc)
        next
    }
}

# cargo fmt --check: "Diff in <file>:<line>:", then the diff.
line ~ /^Diff in / {
    f = line
    sub(/^Diff in /, "", f)
    sub(/:[0-9]+:$/, "", f)
    sub(/^\/home\/runner\/_work\/[^\/]+\/[^\/]+\//, "", f)
    i = getrec("fmt", "fmt|" f, "rustfmt: " f, f)
    if (isnew) addmsg(i, "not formatted; cargo fmt --all rewrites it")
    next
}

END {
    anyfail = 0
    for (s = 0; s <= nsteps; s++) {
        if (step_err[s] > 0) anyfail = 1
    }
    if (!anyfail) {
        # Nothing in the log says which step failed, so report what was found, or
        # the log's last lines when nothing was found.
        for (i = 1; i <= n; i++) printrec(i)
        if (n == 0) unparsed("", 1, nl + 1)
        exit
    }
    for (s = 0; s <= nsteps; s++) {
        if (step_err[s] == 0) continue
        shown = 0
        for (i = 1; i <= n; i++) {
            if (rstep[i] == s) {
                printrec(i)
                shown++
            }
        }
        if (shown == 0) unparsed(step_title[s], step_start[s], step_err[s])
    }
}
