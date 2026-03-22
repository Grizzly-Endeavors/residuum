---
name: wonk-check
description: >
  A code review lens that finds technically correct but weirdly implemented,
  needlessly complicated, or pointlessly existing code in a single file or
  module. This is NOT a linter or bug-finder — it catches the stuff that
  passes CI but makes a human reviewer go "wait... what? why?" Trigger when
  the user says "wonk-check", "wonk check", "check this for wonk", or
  similar. Also trigger when the user asks for a code review focused on
  weirdness, unnecessary complexity, or "does this look weird / off / funny",
  or when they ask "why does this exist" about a piece of code. Do NOT
  trigger for general code review, bug-finding, performance optimization,
  or security audits — this skill is specifically for the structurally
  suspicious, the ceremonially pointless, and the architecturally baffling.
---

# Wonk Check

## What This Skill Does

Wonk-check reads a single file or module and identifies code that is
technically correct but shouldn't exist in its current form. The things
it catches share a common trait: someone who understands the code would
find it confusing not because it's broken, but because there's no good
reason for it to be the way it is.

This is complementary to linters, type checkers, and test suites. Those
tools verify correctness. Wonk-check questions intent.

## Scope

**One file or module at a time.** The user will point you at a specific
file. Read the whole thing, then report findings. If the user pastes code
inline, treat that as the file.

Do not audit an entire codebase, project structure, or dependency tree.
If the user wants broader review, suggest running wonk-check file by file
on the areas that concern them.

## The Wonk Taxonomy

When reviewing, scan for these categories. Not every file will have hits
in every category — most won't. Only report what you actually find.

### Rube Goldberg Logic
Code that takes a complex path to a simple outcome. Five steps where one
would do. A chain of transformations that round-trips back to where it
started. Conditionals that collapse to a single branch.

*The tell:* You can describe what it does in one sentence, but it takes
a paragraph to describe how.

### Cargo Cult Patterns
Design patterns applied ritualistically without the problem they solve
being present. A factory that constructs exactly one type. A strategy
pattern with one strategy. An event bus with one publisher and one
subscriber. The pattern isn't wrong — it's just solving a problem that
doesn't exist here.

*The tell:* Removing the pattern and inlining the logic changes nothing
about the code's behavior or extensibility story.

### Ghost Code
Code that exists but functionally does nothing. A variable assigned and
never read. A condition that's always true. An exception handler that
re-raises unchanged. A function that wraps another function with no
added behavior. Different from dead code (which is unreachable) — ghost
code runs, it just doesn't matter.

*The tell:* Deleting it and running the tests changes nothing.

### Reinvented Wheels
Custom implementations of functionality the language or its standard
library already provides. A hand-rolled `leftPad`, a custom deep-clone,
a bespoke argument parser in a language with `argparse`. Sometimes the
custom version is subtly worse (missing edge cases the stdlib handles).

*The tell:* There's a well-known function or library that does this,
and the custom version doesn't handle anything the standard one doesn't.

### Premature Abstraction
Interfaces, generics, base classes, or extension points with exactly one
concrete implementation and no realistic prospect of a second. An
`IUserRepository` backed by a single `PostgresUserRepository`. A generic
`DataProcessor<T>` instantiated only as `DataProcessor<String>`. The
abstraction adds indirection without adding flexibility.

*The tell:* There's a 1:1 mapping between abstraction and implementation,
and no evidence in the codebase that a second implementation is planned
or plausible.

### Ceremony Code
Boilerplate that serves no purpose in context. Verbose null checks in a
language with null safety. Explicit type annotations where inference is
obvious and conventional. Configuration objects with every field set to
its default. Code that exists because a template or generator put it
there, not because someone needed it.

*The tell:* It's the kind of thing you'd write on autopilot or paste
from a tutorial without checking if it applies.

### Time Capsule Workarounds
Code written to work around a limitation, bug, or version constraint
that no longer applies. Polyfills for features the minimum supported
version now includes. Workarounds for library bugs fixed three major
versions ago. Compatibility shims for a migration that already completed.

*The tell:* There's a comment like `// TODO: remove when we upgrade`
from two years ago, or the workaround references a version/issue that
has since been resolved.

### Suspicious Reshuffling
Code that transforms data into a structurally equivalent shape for no
apparent reason. Mapping a list of objects into a different list of
objects with the same data in the same fields. Destructuring and
reassembling the same struct. Converting to JSON and back for no
serialization purpose.

*The tell:* The input and output are semantically identical, and no
consumer downstream requires the different shape.

### Symmetry Compulsion
Implementing both sides of an operation when only one is needed.
Serialize and deserialize when the code only reads. Full CRUD when
the app only creates and reads. Getter/setter pairs for every field
on a struct that's only ever read. The "other half" exists because
it feels incomplete without it, not because anything calls it.

*The tell:* One side of the pair has zero call sites, or the only
call site is a test that exists solely to test the unused side.

### Paranoid Guarding
Defensive code against states that are structurally impossible.
Null-checking a value the type system guarantees is present.
Try/catching code that cannot throw. Re-validating inputs that
were validated at the boundary two layers up. Different from
legitimate defensive programming — paranoid guards protect against
scenarios the architecture already prevents.

*The tell:* You can prove from the call site, type signature, or
control flow that the guarded condition literally cannot occur.

### Uncategorized Wonk
The categories above aren't exhaustive. Code can be wonky in ways
that don't fit neatly into any listed pattern. If something in the
file is technically correct but makes you pause and wonder why it
exists in its current form, report it even if it doesn't match a
named category. Label it as **Uncategorized** and describe what's
odd about it. The goal is to surface anything that would make a
reviewer's eyebrow go up — not to force every finding into a box.

## Output Format

### If the file is clean

Say so. "No wonk found" is a valid and good outcome. Don't manufacture
findings. One sentence is enough.

### If there are findings

Report each finding as a block with these six fields, in this order:

1. **Severity.** One of the three levels below.
2. **Title.** A single-line summary of the finding. Should read like a
   commit subject — short enough to scan, specific enough to distinguish
   from other findings in the same file.
3. **What.** Quote or reference the specific lines. Keep it tight.
4. **Category.** Which wonk category it falls into (can be more than one).
5. **Why it's wonky.** One or two sentences explaining what's suspicious.
   Be specific — "this is complex" is not useful. "This maps a
   Vec<User> to a Vec<User> with identical fields" is.
6. **Suggested fix.** A single definitive action. Not "you could do X
   or Y" — pick the best option and state it. If that option might be
   blocked (e.g., by a downstream consumer you can't verify), use a
   conditional: "Do X. If blocked by Y, do Z instead." The reader
   should be able to act on the fix without making judgment calls
   about which suggestion to follow.

### Severity scale

- **🤨 Huh?** — Mildly suspicious. Could be intentional, could be
  accidental. Worth a second look. (e.g., ceremony code, mild premature
  abstraction)
- **🧐 Wait, what?** — Clearly unnecessary complexity or indirection.
  Someone should explain why this exists or simplify it. (e.g., Rube
  Goldberg logic, cargo cult patterns)
- **😶 ...why?** — Code that actively makes the file harder to
  understand for no benefit. Strong candidate for removal or rewrite.
  (e.g., ghost code, suspicious reshuffling, stale workarounds)

### After all findings

End with a one-line summary: total finding count and severity breakdown.
For example: *"3 findings: 1× 🤨, 1× 🧐, 1× 😶"*

### Example finding

> **🧐 Wait, what?** — `build_user_list` reconstructs identical Users
>
> Lines 42–58
>
> **Category:** Suspicious Reshuffling
>
> **Why:** `build_user_list` destructures each `User`, then rebuilds an
> identical `User` with the same fields in the same order. The input and
> output types are identical and no consumer requires a different shape.
>
> **Fix:** Delete `build_user_list` and pass the original `Vec<User>`
> directly. If a call site relies on the function for a side effect not
> visible here, inline that side effect at the call site instead.

## Tone

Collegial code review, not a lecture. Think "hey, is there a reason for
this?" not "this is wrong and bad." The whole point of wonk-check is
that the code *works* — the question is whether it needs to work
*this way*.

## What This Skill Is NOT

- **Not a linter.** It doesn't check formatting, naming, or style.
- **Not a bug finder.** If code is broken, that's a different tool.
- **Not a performance audit.** Slow-but-clear code isn't wonky.
- **Not a security review.** Insecure code is dangerous, not wonky.
- **Not a design review.** It won't judge your architecture choices at
  the project level — only the implementation choices in a single file.