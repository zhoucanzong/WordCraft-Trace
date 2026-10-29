# Trace 0.2 validation

Validated on macOS arm64, Rust 1.96, Python 3.9.6 and 3.14, OfficeCLI 1.0.159.

- Engine library: 1,085 passed, including the ignored real OfficeCLI test.
- Python operation runtime/compiler: 8 passed (Unicode, formatting, target stability, input/output guards, unsupported changes, literal escaping, metadata rejection, compaction).
- MCP library: 19 passed; 5 existing loopback networking tests could not bind sockets in the restricted test environment (`Operation not permitted`). They are not reported as passing.
- Actual MCP stdio session: all six added tools exercised; stale version rejected; 2 OP / 18-line independent Python export verified; OfficeCLI validation passed.
- Actual offscreen GUI: opened DOCX, replaced title, formatted, undo/redo, appended text, inserted an image and table; 8 committed edits compiled to 7 OP / 23 lines of Python. Independent execution passed document-model equality. OfficeCLI validation of the Python result reported no errors. Selected-step native replay passed. GUI screenshots inspected.
- Clippy for engine, UI, CLI and MCP with `--no-deps -- -D warnings`: passed.
- Dependency layering: 20 crates, no violations. Asset attribution: all 57 asset files attributed.
- Native application, CLI and screenshot harness built successfully. Packaged macOS application and executables passed ad-hoc signature verification.

`cargo xtask ci` could not complete locally: its initial workspace-wide formatting check hits an upstream craft-libs manifest inheritance error in the local offline path dependency layout. Modified Rust files were formatted separately. The portable repository retains the original pinned Git dependency URLs instead of these local offline paths. WASM was not built because its Rust target is not installed; Web semantic operations return an explicit desktop-required error. Windows and Linux application builds are not validated by this report.

The verifier compares the re-imported WordCraft model, ignoring only save timestamps/revision/last saver and normalizing image resource names by content. This is not byte-identical ZIP output or a claim of Microsoft Word rendering equivalence. Unsupported compact edits fail explicitly or fail the model comparison; full native archives remain available.

## CI follow-up

The first full GitHub gate exposed a missing `trace-replay` entry in the CLI command list. All 43 CLI tests passed locally after restoring the entry, including subcommand help.

The native UI suite also exposed hard-coded Chinese Trace labels in English, unnamed Trace text inputs, untranslated history counters, and old geometry fixtures assuming there was no sidebar. The fixes use locale-aware labels and named fields, keep ruler assertions scoped to the ruler, and explicitly configure full-width test views or reserve space for the Trace pane. No workflow gate or existing assertion is disabled. The added Trace UI regression exercises the expanded sidebar and its archive/OfficeCLI controls.

The final local test run was interrupted by loss of the local execution environment; the GitHub Actions runs on this follow-up commit are the authoritative full-suite result.
