# Task 7.0 — The Procedural Graph: design

**Status: implemented, verified.** This document records the *design decisions* — the shape of the
procedural layer, why each alternative was rejected, and where the code that carries the decision
lives. The evidence (test counts, gate output, what was actually run) is in `TASK-7.0-REPORT.md`;
the pre-implementation inventory is in `TASK-7.0-RECON.md`, and the five ironclad rules the user
authorized supersede the RECON's open scope wherever the two differ.

---

## §1 The shape: a registry that publishes a table, and nothing else

The procedural layer is not an evaluator of slots. It is a **second document registry** whose
evaluation produces **values**, and every other layer reads those values through the ordinary
`Parameter`/`EvaluationContext` machinery:

```text
Document
 ├── nodes            ◻ authored geometry        (Task 1.x)
 ├── operations       ◉ virtual geometry         (Task 4.0, non-destructive booleans)
 └── procedural       ⬡ nodes + wires            (Task 7.0)  ← evaluated LAST, publishes a table

Engine::settle
  1. Primitives   authored nodes resolve every slot (a `Procedural` slot reads the LAST table)
  2. Operations   booleans recompose over fresh primitives
  3. Procedural   dirty ⬡ nodes evaluate in topological order → geometry + published value ports
  4. Readers      slots that read a published port are re-read (one fixpoint round)
```

Two decisions fall out of that picture and explain the whole implementation:

1. **The pass publishes; resolution only reads (RULE 2).** `ProceduralEvaluator` is a *table
   lookup* — `ctx.procedural.value_of(port)` — never a re-evaluation. If resolution could run the
   graph, the graph would be run twice per settle (once for the table, once per slot), and a
   document-wide `with_procedural(&mut ...)` would be needed inside every evaluator. The table is
   built once, in topological order, by the one component that owns the graph.
2. **Geometry results become *live ids* (RULE 4).** A `Region` output is a scene node with its own
   `NodeId`, exactly like an operation's result. That is what makes it hit-testable, nameable in
   the inspector, and — the failure mode the rule exists for — survivable across an unrelated
   operation pass, which prunes cached geometry by `Document::is_geometry_id`.

---

## §2 Why the pass runs last, and why readers get a second round

RULE 2 fixes the order: primitives → operations → procedural. The consequence is that a slot read
is always one table-generation stale, so the readers of a published port must be re-read. That
re-read is not just a slot fix: *a slot can be a whole shape*.

```text
rect.width  ←  ⬡ grid • span          a slot reads a port          (the value path)
⬡ source    →  rect                   a node reads a shape          (a geometry read)
```

A `Source` node reads an authored node's **geometry** — there is no slot to draw a graph edge
from, exactly like an operation. So when the table moves, the readers move, and the *pass* has to
run again for the chains that read those readers. `Engine::settle` therefore runs the pass to a
**fixpoint**:

```text
round 1  ⬡ grid • span republished ─▶ rect.width re-read ─▶ rect moved
round 2  ⬡ source(rect) re-runs    ─▶ nothing new
```

A round only happens when a port's value actually changed *and* something reads it, so an
untouched document costs one table-diff. Two rounds is the last a legal document can need, because
a value reference back into the graph is refused at the command boundary (§4); the bound
(`MAX_PASS_ROUNDS = 4`) exists so a registry that arrived by some other road is *reported* by a
`debug_assert` instead of silently truncating a legal chain.

This is the bug the web smoke caught: with one round, `patch ≡ rebuild` failed — the rebuilt scene
drew the source at 50 wide and the incremental scene at 120, still drawn, just one edit behind.
`crates/vectra-procedural/tests/procedural_laws.rs::a_source_reading_a_reader_follows_in_the_same_settle`
is the regression test.

---

## §3 `GeometryData` is the currency (RULE 1)

`GeometryData` lives in **core**, is `Serialize`/`Deserialize`, and is dependency-free — no `lyon`,
no `geo` — because it is the *document-level* language the UI and the wire speak:

```rust
pub enum GeometryData {
    Scalar(f64),
    Point(Point2),
    Points(Vec<Point2>),
    Path { points: Vec<Point2>, closed: bool },
    Region { rings: Vec<Vec<Point2>> },
    Color(Color),
}
```

Port typing is strict and **pre-application**: a `Points` port into a `Region` input is refused by
`ProceduralNode::validate` with `ResolveError::ProceduralPortType`, naming both tags, before
anything is stored. There is no coercion anywhere — not in the command gate, not in the pass, not
in the wasm boundary. The reason is not pedantry: a coercion would make the *graph* dependent on
which side the implicit conversion ran from, so the same document would draw differently depending
on evaluation order.

---

## §4 RULE 3 is two gates, because there are two roads back into the graph

The rule's sentence is "a value reference back into the graph is a cycle wearing a disguise", and
it names the operand form: a procedural node's `Parameter<f64>` may read a variable, an expression
or motion, but never another node's output. That gate lives inside `ProceduralNode::validate`,
where the registry is in hand.

But an operand is only one road. The other runs through **geometry**:

```text
rect.width  ←  ⬡ grid • span      ← legal on its own: the grid is upstream of nothing
⬡ source    →  rect               ← and a source node reads rect's shape
```

Flip one slot — let `rect.width` read a port of a node *downstream* of `rect` — and the loop
closes with no port-shaped edge in it. The dependency graph cannot see this: a source's geometry
read is deliberately not an edge (`vectra-dependency::graph::procedural_edges` documents why), and
the settle fixpoint would oscillate once per round rather than converge.

So the **boundary** asks the question the graph cannot, in `command.rs::validate_procedural_cycle`:

```rust
doc.procedural_geometry_closure(reference.node).contains(&subject)  ⇒  CyclicDependency
```

The closure walks a procedural node's wires, its kind's `source_node()`, and — through an
operation — its inputs, then recurses through any node's own port reads. Three call sites: a
geometry `SetParameter`, a `BindMotion` whose binding's inner parameters read a port, and an
`ApplyOperation` whose parameters do.

**Style is exempt, and that is a decision, not an oversight.** `Document::property_feeds_geometry`
returns false for `style.*`: a `⬡ source` reads a node's *primitive*, never its paint, so
`rect.style.fill ← ⬡ noise • tint` with `noise ← source(rect)` is a legal document (the wasm
colour-door law reads exactly that shape). Rejecting both would have made a legal document
unwritable; rejecting neither would have let a real loop through. The first implementation of this
gate *did* reject the style case, and the wasm suite caught it — see `TASK-7.0-REPORT.md` §5.

---

## §5 The five kinds, and why `Noise` is integer hashing (RULE 5)

| kind | operands | outputs | why it exists |
| --- | --- | --- | --- |
| `Source` | *subject node* (`needs_subject`) | `region` | lifts authored geometry into the graph |
| `Grid` | columns, rows, spacing, origin | `region`, `points`, `center`, `span` | a generator with four port types — the palette's proof that typing is real |
| `Repeat` | count, dx, dy | `region` | the modifier used by every law as the downstream half |
| `Noise` | amplitude, frequency, seed | `region`, `scalar`, `tint` | the colour door, and the determinism proof |
| `Smooth` | iterations, strength | `region` | a second modifier, so a chain has length 3 |

`Noise` uses **integer hashing** (splitmix-style on the `(x, y, seed)` lattice, mapped to
`[0, 1)`), never `rand`, never a wall clock, never a float-order-dependent reduction. That is RULE
5, and it is what makes the Determinism Law byte-for-byte: two engines, the same script, the same
bytes — natively and in wasm, because the arithmetic is integer until the final multiply.

---

## §6 The wire and the panel (step 5)

The React panel is a dumb remote control (Task 1.4's standing boundary): it sends commands and
renders engine strings. Three engine-side additions make that possible without the UI inventing
anything:

* **`procedural_kinds()`** — the palette, generated from the registry itself: `{tag, label,
  needs_subject, operands, inputs, outputs}`. `operands` are **kind-level** payloads
  (`Parameter<T>`, no `ParamValue` wrapper), so the panel spreads them straight into the kind it is
  building. A `{"Float": …}` wrapper at that level is exactly the bug the smoke's step 32 exists to
  catch; it was caught, and fixed here rather than in the UI.
* **`procedural_json()`** — the report: per node `{id, kind, name, description, enabled,
  operands: {port: {ty, text}}, wires, upstream, inputs, outputs: {port, ty, required, wired,
  value}}`. Operand *text* and output *values* are formatted by the engine
  (`render_param_value`, `geometry_summary`), so the panel never formats a number.
* **`AddProceduralNode` fills a missing `name`** from `kind.describe()` and a missing `enabled`
  from `true` (serde defaults), so the panel sends `{id, kind}` and the *engine* names the node.

---

## §7 What was rejected

* **An operand reading a port** (RULE 3's explicit ban), even when the dependency is provably
  acyclic at that moment. A wire expresses dependency inside the graph; permitting the value path
  too would give two encodings of one relation and a cycle gate that has to reason about both.
* **Coercing a port type** (RULE 1). See §3.
* **Re-evaluating the graph inside resolution** (RULE 2's "does NOT evaluate"). It would hide the
  evaluation order inside a lookup and make the table unobservable — and the table *is* the
  contract the UI renders.
* **An edge for a source's geometry read.** It would have to point at *every property* of the
  subject node, and it would still miss the operation case. The registry scan
  (`sources_referencing`) plus the settle fixpoint is the honest encoding.
* **Retiring a broken node's geometry silently.** A node that cannot compute contributes nothing
  *and* reports one diagnostic naming it (`procedural-missing-input`), because a stale shape that
  keeps drawing its last good result is the failure mode that is hardest to notice.
