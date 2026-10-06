# Cross-crate scenario tests

Per-crate property suites live next to their owner
(e.g. `crates/vectra-core/tests/`, `crates/vectra-geometry/tests/`).

This directory is reserved for **end-to-end engine scenarios** that span crates
and therefore belong to no single one:

* `variable → expression → constraint → evaluated scene` propagation
* `command → undo → redo → snapshot` round-trips through `vectra-wasm`
* motion + procedural + boolean composition (Phase 2)

Planned harness: a `vectra-scenarios` dev-crate (not shipped) so scenarios can
depend on all engine crates without polluting the workspace graph.
