Analyze this module for code organization and cleanup opportunities.

For each finding, be specific — reference file paths and line numbers. Focus on actionable findings, not praise.

## What to look for

### File decomposition
- Monolithic files (300+ lines of logic) that mix unrelated concerns.
- How to split them: by domain concept, by layer (types/logic/handlers), or by responsibility.
- Sub-module opportunities for large sections of code with shared responsibilities.
- Small imporvements are worth noting.

### Internal duplication
- Repeated patterns, copy-pasted blocks, or near-duplicate functions within the module.
- Opportunities for shared internal helpers or consolidation.

### Cross-module shared utilities
- Generic utilities in this module that could be extracted to a shared/common location (e.g., `src/util/`).
- Code that duplicates functionality already available elsewhere in the project.

### Structural clarity
- Separation of concerns: types, constants, business logic, I/O, and handlers should not be mixed in one file.
- Whether `mod.rs` has a clear structure (declarations, re-exports, and any module-level coordination logic).
- Dead code, unused imports, and orphaned files.

### Naming and conventions
- Files, functions, or types with unclear or inconsistent names.
- Deviations from Rust conventions or the project's style (see the project CLAUDE.md).

## Output format

If the module's error handling is clean, say so. "No findings" is a valid and good outcome. Don't manufacture findings.

If there are findings, organize by category (use the headings above). For each finding:
- State what the problem is
- Reference the specific file and line(s)
- Propose a concrete fix

Skip any category that has no findings.
