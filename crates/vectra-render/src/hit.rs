//! Hit testing: the bridge between a pointer and a `NodeId` (Task 5.0 RULE 3).
//!
//! A **uniform grid** of bounding boxes is the spatial index. It is the right
//! structure for this workload: shapes are spread over the document, cell size
//! is derived from the scene's own extent, and a pointer query is an O(1) cell
//! lookup rather than an O(n) scan. (An R-tree would win for very uneven
//! distributions; the grid is simpler, has no balancing, and — crucially —
//! rebuilds in one linear pass, which is all a click ever needs.)
//!
//! The grid is *coarse*: it decides **which nodes to test**, never what the
//! answer is. The exact answer comes from an even-odd containment test on the
//! node's own flattened rings (`RenderScene::hit_test`), so clicking the
//! bounding-box corner of a circle correctly returns `None`.
//!
//! Candidates are returned **front to back**, because the topmost shape under
//! the pointer is the one the user means — a click on a stack of layers picks
//! the layer the canvas paints last, which is exactly what `z_order` encodes.

use std::collections::BTreeSet;

use vectra_core::NodeId;

use crate::geometry::Bounds;

/// Cells per side of the grid box: 32×32 is 1024 buckets — small enough to
/// build in microseconds, dense enough that a candidate list is short.
const GRID_DIVISIONS: usize = 32;

/// A uniform grid over the scene's bounding box.
#[derive(Debug, Clone, Default)]
pub struct HitIndex {
    /// Top-left of the grid in document space.
    origin: (f32, f32),
    /// Cell size in document units (square cells).
    cell: f32,
    /// `cols * rows` buckets, row-major.
    cols: usize,
    rows: usize,
    cells: Vec<Vec<NodeId>>,
    /// Cells that were populated at all.
    occupied: usize,
}

impl HitIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.cells.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    /// Rebuild from every node's world bounds, **back to front** — the reverse
    /// of the order [`HitIndex::candidates`] hands back, so the shape the canvas
    /// paints last is the first candidate. (Insertion order *is* the answer for
    /// overlapping shapes; it is not a hint.)
    pub fn rebuild<I>(&mut self, nodes: I)
    where
        I: IntoIterator<Item = (NodeId, Bounds)>,
    {
        let mut all = Bounds::EMPTY;
        let entries: Vec<(NodeId, Bounds)> = nodes.into_iter().collect();
        for (_, bounds) in &entries {
            if !bounds.is_empty() {
                all = all.union(bounds);
            }
        }
        self.cells.clear();
        self.occupied = 0;
        if all.is_empty() || entries.is_empty() {
            self.origin = (0.0, 0.0);
            self.cell = 1.0;
            self.cols = 0;
            self.rows = 0;
            return;
        }
        // A degenerate scene (a single point, a thin line) still needs a usable
        // grid: pad the extent so `cell` can never be zero.
        let width = all.width().max(1.0);
        let height = all.height().max(1.0);
        let cell = (width.max(height) / GRID_DIVISIONS as f32).max(1.0);
        self.origin = (all.min_x, all.min_y);
        self.cell = cell;
        self.cols = ((width / cell).ceil() as usize + 1).max(1);
        self.rows = ((height / cell).ceil() as usize + 1).max(1);
        self.cells = vec![Vec::new(); self.cols * self.rows];

        for (id, bounds) in entries {
            if bounds.is_empty() {
                continue;
            }
            let (min_col, min_row) = self.cell_of(bounds.min_x, bounds.min_y);
            let (max_col, max_row) = self.cell_of(bounds.max_x, bounds.max_y);
            for row in min_row..=max_row {
                for col in min_col..=max_col {
                    let index = row * self.cols + col;
                    if let Some(bucket) = self.cells.get_mut(index) {
                        if bucket.is_empty() {
                            self.occupied += 1;
                        }
                        bucket.push(id);
                    }
                }
            }
        }
    }

    fn cell_of(&self, x: f32, y: f32) -> (usize, usize) {
        let col = ((x - self.origin.0) / self.cell).floor();
        let row = ((y - self.origin.1) / self.cell).floor();
        let col = col.max(0.0).min((self.cols.saturating_sub(1)) as f32) as usize;
        let row = row.max(0.0).min((self.rows.saturating_sub(1)) as f32) as usize;
        (col, row)
    }

    /// Node ids whose bounds cover the cell containing `(x, y)`, front to back.
    ///
    /// A point outside the grid returns nothing — a click far from the art is a
    /// hit miss, not a scan of the whole scene. The grid covers the scene's own
    /// extent plus one cell of slack, so "outside" means genuinely outside.
    pub fn candidates(&self, x: f32, y: f32) -> Vec<NodeId> {
        if self.cells.is_empty() {
            return Vec::new();
        }
        let col = ((x - self.origin.0) / self.cell).floor();
        let row = ((y - self.origin.1) / self.cell).floor();
        if !(0.0..self.cols as f32).contains(&col) || !(0.0..self.rows as f32).contains(&row) {
            return Vec::new();
        }
        let (col, row) = (col as usize, row as usize);
        let bucket = &self.cells[row * self.cols + col];
        if bucket.is_empty() {
            return Vec::new();
        }
        // The bucket holds back-to-front insertion order; the caller wants the
        // front-most first, so walk it in reverse and deduplicate (a large
        // node may appear in several cells, but not twice in one).
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut out = Vec::with_capacity(bucket.len());
        for id in bucket.iter().rev() {
            if seen.insert(*id) {
                out.push(*id);
            }
        }
        out
    }

    /// Grid geometry, for the renderer's status line and the tests.
    pub fn shape(&self) -> (usize, usize, f32) {
        (self.cols, self.rows, self.cell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
        Bounds::new(x, y, x + w, y + h)
    }

    #[test]
    fn a_point_inside_one_shape_yields_it() {
        let mut index = HitIndex::new();
        index.rebuild([(NodeId::nil(), bounds(0.0, 0.0, 10.0, 10.0))]);
        let candidates = index.candidates(5.0, 5.0);
        assert_eq!(candidates, vec![NodeId::nil()]);
    }

    #[test]
    fn a_point_far_outside_yields_nothing() {
        let mut index = HitIndex::new();
        index.rebuild([(NodeId::nil(), bounds(0.0, 0.0, 10.0, 10.0))]);
        assert!(index.candidates(500.0, 500.0).is_empty());
        assert!(index.candidates(-5.0, 5.0).is_empty());
    }

    #[test]
    fn overlapping_shapes_come_back_front_to_back() {
        let back = vectra_core::new_node_id();
        let front = vectra_core::new_node_id();
        let mut index = HitIndex::new();
        // Insertion order is draw order (back → front).
        index.rebuild([
            (back, bounds(0.0, 0.0, 100.0, 100.0)),
            (front, bounds(10.0, 10.0, 10.0, 10.0)),
        ]);
        let candidates = index.candidates(15.0, 15.0);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], front, "the front-most node is offered first");
    }

    #[test]
    fn an_empty_scene_is_a_valid_index() {
        let mut index = HitIndex::new();
        index.rebuild([]);
        assert!(index.is_empty());
        assert!(index.candidates(0.0, 0.0).is_empty());
    }

    #[test]
    fn a_degenerate_single_point_scene_does_not_divide_by_zero() {
        let mut index = HitIndex::new();
        index.rebuild([(NodeId::nil(), bounds(7.0, 7.0, 0.0, 0.0))]);
        let (cols, rows, cell) = index.shape();
        assert!(cols >= 1 && rows >= 1 && cell > 0.0);
        assert_eq!(index.candidates(7.0, 7.0), vec![NodeId::nil()]);
    }
}
