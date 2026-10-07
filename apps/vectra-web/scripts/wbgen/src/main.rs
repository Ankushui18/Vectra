//! `wasm-bindgen --target web --out-dir <dir> --out-name <name> <input>`.
//!
//! ## Why this exists
//!
//! `apps/vectra-web/scripts/build-wasm.sh` is the normal path: it runs the
//! `wasm-bindgen` CLI, pinned to the version in `Cargo.lock`. Some environments
//! cannot run it — the CLI's release assets are served from
//! `release-assets.githubusercontent.com`, which is firewalled in the sandbox
//! this was written in, and crates.io is unreachable too — but *can* build the
//! crate the CLI calls alongside its argument parsing:
//! `wasm-bindgen-cli-support`. That crate's dependency closure is small
//! (`walrus` + `wasmparser` + `serde` + a little more), so it can be vendored
//! from the same reachable sources the workspace's dependencies come from.
//!
//! ## Fidelity
//!
//! The flags below are exactly what the CLI's `rmain` sets for
//! `--target web`, including the two that are easy to get wrong because the
//! library's *defaults* disagree with the CLI's:
//!
//! * `typescript(true)`: `Bindgen::new()` defaults typescript to `false`, while
//!   the CLI's `--no-typescript` defaults to false — i.e. the CLI *emits*
//!   `*.d.ts`, and the browser build's `vectra_wasm.d.ts` is relied on.
//! * `omit_default_module_path(false)`: `Bindgen::new()` defaults this to
//!   **true**, the CLI's `--omit-default-module-path` to false. With it true the
//!   glue loses the `new URL('…_bg.wasm', import.meta.url)` fallback that both
//!   `client.ts` (`?url` import) and `scripts/smoke.mjs` (explicit path) reload
//!   the binary through.
//!
//! `reference_types`/`multi_value` are deliberately **not** set: the library
//! re-reads the module's own `target_features` and enables each transform only
//! if the module already uses it (`cli-support/src/lib.rs`), which is what the
//! CLI's defaults do and what makes the generated ABI match the module.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release --manifest-path apps/vectra-web/scripts/wbgen/Cargo.toml \
//!     -- <input.wasm> <out-dir> [out-name]
//! ```

use wasm_bindgen_cli_support::Bindgen;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: wbgen <input.wasm> <out-dir> [out-name]";
    let input = args.next().ok_or(usage)?;
    let out_dir = args.next().ok_or(usage)?;
    let out_name = args.next().map(|name| name.to_string_lossy().into_owned());

    let mut bindgen = Bindgen::new();
    // `--target web` (the CLI's `Target::Web` arm).
    bindgen.web(true)?;
    bindgen.input_path(&input);
    bindgen
        .debug(false)
        .demangle(true)
        .keep_lld_exports(false)
        .keep_debug(false)
        .split_debug_info(false)
        .remove_name_section(false)
        .remove_producers_section(false)
        .typescript(true)
        .omit_imports(false)
        .omit_default_module_path(false)
        .split_linked_modules(false)
        .ts_typed_array_buffers(false)
        .reset_state_function(false)
        .force_enable_abort_handler(false);
    if let Some(name) = out_name {
        bindgen.out_name(&name);
    }
    bindgen.generate(&out_dir)?;
    Ok(())
}
