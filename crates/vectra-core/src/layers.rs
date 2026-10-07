//! **Layers and artboards** (Task 10.2 RULEs 1 and 2): the document's
//! organizational spine.
//!
//! # Layers own membership; `Document::order` owns the drawing
//!
//! The flat `Document::order` vector is what the evaluator, the scene cache, the
//! renderer, the hit index and every exporter read for z-order, and it has been
//! since Task 1. This module adds a *second* structure — which layer a node
//! belongs to, and what that layer is called — and keeps the two in lockstep by
//! construction rather than by convention:
//!
//! ```text
//!   order == concat(layers in back→front order, each layer's children in order)
//!            ++ every id no layer lists            (unassigned: legacy documents)
//! ```
//!
//! That invariant is `Document::order_matches_layers`, and it is a test law. It
//! is the reason a layer reorder can be a plain reorder of `order`: there is one
//! source of z-truth, and the layer tree is a *view* of it with names and flags
//! attached. A second z-truth is exactly how a "move to front" ends up drawing
//! behind something.
//!
//! # Groups nest; layers are the top level
//!
//! A `Group` is already a [`crate::document::NodeKind`] — it nests arbitrarily
//! and its children are real nodes. A `Layer` is *not* a node: it is a record in
//! this registry. The split is deliberate. A layer is presentation state a
//! designer renames and eyeballs hundreds of times a session; a group is
//! geometry that can be selected, moved and exported. Making layers nodes would
//! have put them in the dependency graph, in `Dirty`, in hit-testing and in the
//! AI's node summary, and every one of those places would need a special case
//! saying "this node draws nothing". The registry needs none.
//!
//! # Visibility and locking are flags, and that is a performance decision
//!
//! RULE 4 says toggling the eye or the padlock must not re-evaluate the
//! document. It does not, and the shape of these records is why: a `bool` on a
//! layer or a node never reaches the evaluator. The boundary derives a
//! **visibility mask** into the scene cache (see `vectra-wasm::settle`), and the
//! renderer skips a masked node in its draw list. Nothing is re-resolved, nothing
//! is re-tessellated; a hidden layer costs one frame of drawing fewer triangles.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::geom::Color;
use crate::ids::{ArtboardId, LayerId, NodeId};

/// One layer: identity, presentation, and the nodes it holds.
///
/// `children` is an ordered list because a layer's contents are drawn in the
/// order the designer arranged them; the layer's own position in
/// [`LayerRegistry::layers`] is its z-position relative to other layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerRecord {
    pub id: LayerId,
    pub name: String,
    #[serde(default = "default_visible")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    /// **Alpha Lock** (Task 10.7 RULE 3a): new artwork on this layer is
    /// constrained to the layer's *existing* content.
    ///
    /// Like the eye and the padlock this is a flag, not a document rewrite: it
    /// never reaches the evaluator, and toggling it changes no geometry. What it
    /// changes is the **drawing boundary** — a committed stroke on an
    /// alpha-locked layer is intersected with the layer's content before it is
    /// stored (see `vectra_operations::clip` and the engine's `commit_draft`),
    /// which is exactly what Procreate's alpha lock does to a stroke: it clips
    /// it to the alpha the layer already has.
    ///
    /// On an *empty* locked layer there is nothing to clip against, so nothing
    /// is added — a refusal a designer can read, not a silently empty stroke.
    #[serde(default)]
    pub alpha_locked: bool,
    /// **Clipping Mask** (Task 10.7 RULE 3b): this layer shows only where it
    /// overlaps the layer **below** it.
    ///
    /// Unlike alpha lock this *is* geometry: the scene pass reshapes every node
    /// on a clipping layer to `region(node) ∩ region(layer below)`, live (see
    /// `vectra_operations::clip::ClipState::apply`, run by the engine's
    /// `settle`). The layer below keeps drawing in full — it is the mask, not a
    /// victim of it, which is Procreate's rule.
    #[serde(default)]
    pub clipping_mask: bool,
    #[serde(default)]
    pub children: Vec<NodeId>,
}

fn default_visible() -> bool {
    true
}

impl LayerRecord {
    pub fn new(id: LayerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            alpha_locked: false,
            clipping_mask: false,
            children: Vec::new(),
        }
    }

    pub fn contains(&self, node: NodeId) -> bool {
        self.children.contains(&node)
    }
}

/// The document's layers, **back → front**.
///
/// A plain ordered `Vec` rather than a map: layer count is small (tens), every
/// operation on it is a reorder or a walk, and `Vec` makes the z-order the
/// literal memory order instead of a sort key that can drift.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerRegistry {
    pub layers: Vec<LayerRecord>,
}

impl LayerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &LayerRecord> {
        self.layers.iter()
    }

    pub fn get(&self, id: &LayerId) -> Option<&LayerRecord> {
        self.layers.iter().find(|layer| &layer.id == id)
    }

    pub fn get_mut(&mut self, id: &LayerId) -> Option<&mut LayerRecord> {
        self.layers.iter_mut().find(|layer| &layer.id == id)
    }

    pub fn position(&self, id: &LayerId) -> Option<usize> {
        self.layers.iter().position(|layer| &layer.id == id)
    }

    /// Insert a record. A duplicate id is refused by returning `false` — the
    /// caller (a command) reports it rather than creating two layers with one
    /// identity, which would make every later lookup ambiguous.
    pub fn insert(&mut self, record: LayerRecord, index: Option<usize>) -> bool {
        if self.get(&record.id).is_some() {
            return false;
        }
        let at = index.unwrap_or(self.layers.len()).min(self.layers.len());
        self.layers.insert(at, record);
        true
    }

    /// Remove a layer **without touching its children** (they become
    /// unassigned). Callers that must not orphan nodes use
    /// `Document::remove_layer_and_children`.
    pub fn remove(&mut self, id: &LayerId) -> Option<LayerRecord> {
        let index = self.position(id)?;
        Some(self.layers.remove(index))
    }

    /// Move a layer to `index`, shifting its neighbours. The layer's children
    /// move with it — that is what "moving a group moves all children" means one
    /// level up, and the caller rewrites `Document::order` to match.
    pub fn reorder(&mut self, id: &LayerId, index: usize) -> bool {
        let Some(from) = self.position(id) else {
            return false;
        };
        let record = self.layers.remove(from);
        let at = index.min(self.layers.len());
        self.layers.insert(at, record);
        true
    }

    /// The artboard that lists this layer, if any.
    pub fn board_of_layer<'a>(
        &'a self,
        boards: &'a ArtboardRegistry,
        layer: &LayerId,
    ) -> Option<&'a ArtboardRecord> {
        boards.iter().find(|board| board.has_layer(layer))
    }

    /// The layer directly **below** `id` — the mask a clipping layer clips to.
    ///
    /// `None` for the bottom layer, and for an id no layer carries. Both mean
    /// the same thing to a clipping layer: there is nothing to clip to, so
    /// nothing shows (the scene pass reads it that way, and the panel disables
    /// the toggle for the bottom row).
    pub fn below(&self, id: &LayerId) -> Option<&LayerRecord> {
        let index = self.layers.iter().position(|layer| &layer.id == id)?;
        index
            .checked_sub(1)
            .and_then(|below| self.layers.get(below))
    }

    /// Where the layer sits in the back → front order.
    pub fn index_of(&self, id: &LayerId) -> Option<usize> {
        self.layers.iter().position(|layer| &layer.id == id)
    }

    /// Which layer lists this node, and where in that layer's order.
    pub fn layer_of(&self, node: NodeId) -> Option<(&LayerRecord, usize)> {
        self.layers.iter().find_map(|layer| {
            layer
                .children
                .iter()
                .position(|child| *child == node)
                .map(|index| (layer, index))
        })
    }

    /// The ids every layer lists, as a set (for the unassigned tail of `order`).
    pub fn assigned_ids(&self) -> HashSet<NodeId> {
        self.layers
            .iter()
            .flat_map(|layer| layer.children.iter().copied())
            .collect()
    }

    /// Drop a node id from whichever layer lists it. Returns the layer it left.
    pub fn detach(&mut self, node: NodeId) -> Option<LayerId> {
        for layer in &mut self.layers {
            if let Some(index) = layer.children.iter().position(|child| *child == node) {
                layer.children.remove(index);
                return Some(layer.id);
            }
        }
        None
    }

    /// Effective visibility of a node: `false` if any layer hides it, or if any
    /// layer **above** it contains it (a node can only be listed once, so this is
    /// simply "the layer that lists it is visible").
    pub fn visible_for(&self, node: NodeId) -> bool {
        match self.layer_of(node) {
            Some((layer, _)) => layer.visible,
            // Unassigned nodes are always drawn: a legacy document has no layers
            // and must keep rendering exactly as it always did.
            None => true,
        }
    }

    pub fn locked_for(&self, node: NodeId) -> bool {
        match self.layer_of(node) {
            Some((layer, _)) => layer.locked,
            None => false,
        }
    }
}

/// One artboard: a named rectangle with a background, and the layer stack that
/// belongs to it.
///
/// **Artboards own layers**, which is the Procreate reading of the idea (each
/// canvas has its own stack) and also the Illustrator one for everything a
/// designer notices day to day: "Export current artboard" exports the artwork
/// assigned to it, and switching artboards switches the panel.
///
/// A layer belongs to exactly one artboard, so the two registries stay in
/// agreement by construction: `LayerRegistry` is the flat list the evaluator
/// walks, and each record's `artboard` says which board it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtboardRecord {
    pub id: ArtboardId,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default = "default_background")]
    pub background: Color,
    /// This artboard's layer stack, **back → front** (Task 10.2 RULE 2).
    ///
    /// Ownership lives here rather than on the layer record because the question
    /// the software asks constantly is "what does this artboard contain", and the
    /// reverse — "which board is this layer on" — is a lookup nobody performs
    /// more than once a click. A layer no artboard lists still draws (it is
    /// simply unlisted), which is what keeps a document made before artboards
    /// existed rendering exactly as before.
    #[serde(default)]
    pub layers: Vec<LayerId>,
}

fn default_background() -> Color {
    Color::WHITE
}

impl ArtboardRecord {
    pub fn new(id: ArtboardId, name: impl Into<String>, x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            id,
            name: name.into(),
            x,
            y,
            width: w,
            height: h,
            background: Color::WHITE,
            layers: Vec::new(),
        }
    }

    /// Does this artboard list the layer?
    pub fn has_layer(&self, id: &LayerId) -> bool {
        self.layers.contains(id)
    }

    /// The artboard's layer stack, in draw order, as records.
    pub fn layer_records<'a>(&'a self, registry: &'a LayerRegistry) -> Vec<&'a LayerRecord> {
        self.layers
            .iter()
            .filter_map(|id| registry.get(id))
            .collect()
    }

    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        (self.x, self.y, self.width, self.height)
    }

    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// The document's artboards, in the order the UI lists them, plus which one is
/// active (the one new artwork lands in, and the one "Export current" exports).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtboardRegistry {
    pub boards: Vec<ArtboardRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ArtboardId>,
}

impl ArtboardRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.boards.is_empty()
    }

    pub fn len(&self) -> usize {
        self.boards.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ArtboardRecord> {
        self.boards.iter()
    }

    pub fn get(&self, id: &ArtboardId) -> Option<&ArtboardRecord> {
        self.boards.iter().find(|board| &board.id == id)
    }

    pub fn get_mut(&mut self, id: &ArtboardId) -> Option<&mut ArtboardRecord> {
        self.boards.iter_mut().find(|board| &board.id == id)
    }

    pub fn insert(&mut self, record: ArtboardRecord, index: Option<usize>) -> bool {
        if self.get(&record.id).is_some() {
            return false;
        }
        let at = index.unwrap_or(self.boards.len()).min(self.boards.len());
        self.boards.insert(at, record);
        true
    }

    pub fn remove(&mut self, id: &ArtboardId) -> Option<ArtboardRecord> {
        let index = self.boards.iter().position(|board| &board.id == id)?;
        let removed = self.boards.remove(index);
        if self.active.as_ref() == Some(id) {
            self.active = self.boards.first().map(|board| board.id);
        }
        Some(removed)
    }

    /// Which artboard is active, defaulting to the first one when the flag is
    /// unset — so a document that never chose has a well-defined answer instead
    /// of `None`, and the UI never has to invent one.
    pub fn active_id(&self) -> Option<ArtboardId> {
        self.active
            .filter(|id| self.get(id).is_some())
            .or_else(|| self.boards.first().map(|board| board.id))
    }

    pub fn active_board(&self) -> Option<&ArtboardRecord> {
        self.active_id().and_then(|id| self.get(&id))
    }

    /// The artboard a point falls in, front-to-back (later boards win an
    /// overlap, matching the UI's mental model of "the one on top").
    pub fn board_at(&self, x: f64, y: f64) -> Option<&ArtboardRecord> {
        self.boards
            .iter()
            .rev()
            .find(|board| board.contains_point(x, y))
    }

    /// Union of every artboard's bounds: what "Export all artboards" frames.
    pub fn union_bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let first = self.boards.first()?;
        let (mut x0, mut y0) = (first.x, first.y);
        let (mut x1, mut y1) = (first.x + first.width, first.y + first.height);
        for board in &self.boards[1..] {
            x0 = x0.min(board.x);
            y0 = y0.min(board.y);
            x1 = x1.max(board.x + board.width);
            y1 = y1.max(board.y + board.height);
        }
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}
