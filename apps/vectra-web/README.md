# vectra-web — Dumb React Remote over the WASM Engine (Task 1.4)

> The UI owns **zero** document truth: it sends `Command` JSON to
> `VectraEngine.dispatch_command`, logs the returned `EngineEvent`s, and
> re-renders `get_snapshot()`. All geometry, resolution, and undo live in Rust.

## Quick start

```bash
# 1. Build the engine + regenerate bindings (from repo root or here)
bash scripts/build-wasm.sh            # debug; append `release` for release

# 2. Install + run the host
npm install
npm run dev                           # → http://localhost:5173

# 3. Automated E2E gate (real wasm in Node, no browser needed)
npm run smoke

# 4. Production bundle (typecheck + Vite build incl. hashed .wasm asset)
npm run build
```

`wasm-bindgen` CLI version must match `Cargo.lock` exactly —
`build-wasm.sh` enforces this (install hint on mismatch).

## The loop (MES §16)

1. Click **+ Circle** → `commands.createCircle()` builds the full typed
   `CreateNode` JSON (client-side UUID + literal params).
2. `client.dispatch(cmd)` → `{"status":"ok","events":[…]}` → events logged.
3. `client.snapshot()` → Layers, Variables, diagnostics, and the SVG preview
   re-render from the one snapshot. **Undo**/**Redo** round-trip the same way.

## Manual verification checklist (E2E proof)

1. `npm run dev`, open the URL — status pill turns green (`Engine ready`).
2. Click **+ Circle** — log shows `CreateNode` JSON then `NodesUpdated` /
   `OrderChanged`; Layers gains `circle`; canvas draws it.
3. Set `$base = 300` — Variables lists `$base 300`; log shows
   `VariablesUpdated`. (Parametric binding is exercised by `npm run smoke`
   step 4 until the inspector lands.)
4. Click **↩ Undo** — log shows the inverse `NodesRemoved`; layer + shape
   disappear; **↪ Redo** restores them.
5. Delete a layer via its ✕ — same inverse-event flow per row.

## Wire-contract note

The schematic `{"type":"CreateNode","kind":"Circle","geometry":{…}}` from the
task brief is NOT the wire format — real commands carry the full serde schema
(client-generated `id`, externally-tagged `NodeKind`, `Parameter` wrappers).
The exact contract is mirrored in [`src/engine/wire.ts`](./src/engine/wire.ts),
and [`src/engine/commands.ts`](./src/engine/commands.ts) builders own its
construction so UI code never hand-writes JSON.

## Files

```text
apps/vectra-web/
├── scripts/
│   ├── build-wasm.sh   # cargo build (wasm32) + wasm-bindgen --target web
│   └── smoke.mjs        # Node E2E: instantiate real wasm, assert the loop
├── src/
│   ├── engine/
│   │   ├── wire.ts      # TS mirror of the Rust serde contract
│   │   ├── commands.ts  # Typed command builders (ids, literals)
│   │   └── client.ts    # WASM singleton init + JSON round-trip wrapper
│   ├── wasm/            # GENERATED bindings (committed so previews work)
│   ├── App.tsx          # Remote UI: preview, layers, variables, log
│   └── main.tsx
└── README.md
```
