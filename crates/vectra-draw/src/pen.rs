//! The pen's handle algebra (Task 10.1 RULE 2).
//!
//! RULE 2 defines four gestures, and all four are statements about *handles*:
//!
//! | gesture | result |
//! | --- | --- |
//! | click | a corner — no handles |
//! | click-drag | a **smooth** anchor — two mirrored handles |
//! | Alt/Option-drag | a **broken** anchor — the dragged handle moves, the other does not |
//! | click on the first point | close the path |
//!
//! Two pure pieces carry that: [`PenSession`] for the placing gesture, and
//! [`drag_handle`] for the direct-selection gesture that moves a handle on an
//! anchor that already exists. Both live here — not in React — for the reason the
//! crate docs give: the laws that prove them (Handle Symmetry, Handle
//! Independence) must run against the *same* code the pointer drives.
//!
//! ## What "smooth" and "broken" mean, exactly
//!
//! An anchor is a point plus up to two handles. *Symmetry* is the constraint that
//! the handles reflect each other through the point:
//!
//! ```text
//!    handle_in ●──────● anchor ●──────▶ handle_out
//!                     ▲           ▲
//!                     │  d        │  d          d = the drag vector
//! ```
//!
//! *Broken* is the absence of that constraint: the handles are independent, and
//! (as in Illustrator) the way to break them is to hold Alt while dragging one.
//! Nothing else in the tool creates a broken anchor, which is what makes the
//! independence law a one-line statement — *the other handle is where it was*.

use vectra_core::{PathSegment, Point2};

use crate::bezier::{self, closing_segment, is_drag, segment_between, Anchor};

/// Which handle a gesture is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleSide {
    /// The handle on the incoming side (controls the segment that arrives).
    In,
    /// The handle on the outgoing side (controls the segment that leaves).
    Out,
}

impl HandleSide {
    pub fn opposite(self) -> Self {
        match self {
            Self::In => Self::Out,
            Self::Out => Self::In,
        }
    }
}

/// Which tool-level gesture the pen is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenGesture {
    /// No button down.
    Idle,
    /// Placing an anchor at the last position (its handles follow the pointer).
    Placing,
}

/// The path a pen session currently describes.
///
/// `segments` never includes a `Close`: closing is a property of the draft, and
/// the caller asks for it explicitly — because "is this path closed?" is a state
/// the user can still change (click the first point) right up to the moment they
/// commit.
#[derive(Debug, Clone, PartialEq)]
pub struct PathDraft {
    pub start: Point2,
    pub segments: Vec<PathSegment>,
    pub closed: bool,
}

impl PathDraft {
    /// Does this draft describe anything the engine should be given?
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// The pen's state: the anchors placed so far, and the gesture in progress.
///
/// Deliberately a *pure* machine: every method takes the pointer position and the
/// modifier state and mutates only this struct. Nothing here reads a document or
/// writes a command, which is what lets the UI run the identical logic natively
/// in a `proptest` (see the laws) and in the browser through the wasm boundary.
#[derive(Debug, Clone, Default)]
pub struct PenSession {
    anchors: Vec<Anchor>,
    gesture: Option<PenGestureState>,
}

#[derive(Debug, Clone, Copy)]
struct PenGestureState {
    /// Index of the anchor the button is down on.
    index: usize,
    /// Whether the button has moved far enough to be a handle pull.
    dragged: bool,
}

impl PenSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// The anchors placed so far (read-only; the UI draws these as nodes).
    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }

    pub fn gesture(&self) -> PenGesture {
        match self.gesture {
            Some(_) => PenGesture::Placing,
            None => PenGesture::Idle,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// True when the pointer is close enough to the first anchor to mean
    /// **close the path** rather than place another point.
    pub fn closes_at(&self, point: Point2, radius: f64) -> bool {
        self.anchors
            .first()
            .map(|first| bezier::distance(first.point, point) <= radius)
            .unwrap_or(false)
    }

    /// Pointer **down**: place a corner anchor and start watching the drag.
    ///
    /// The anchor is a corner at this instant. If the pointer then moves past
    /// [`crate::bezier::DRAG_THRESHOLD`], [`PenSession::pointer_move`] turns it
    /// into a smooth or broken anchor — which is exactly the Illustrator
    /// behaviour, and the reason an accidental 1-pixel wobble does not curve a
    /// segment.
    pub fn pointer_down(&mut self, point: Point2) -> usize {
        self.anchors.push(Anchor::corner(point));
        let index = self.anchors.len() - 1;
        self.gesture = Some(PenGestureState {
            index,
            dragged: false,
        });
        index
    }

    /// Pointer **move while the button is down**: pull handles out of the anchor
    /// being placed.
    ///
    /// `alt` switches the anchor from *smooth* to *broken* mid-gesture, which is
    /// how Illustrator's Alt-drag works: hold it and the in-handle stops following.
    /// Releasing Alt puts the handles back in mirror — the user can therefore see
    /// both results before committing, which is the whole point of doing this
    /// during the drag rather than after it.
    pub fn pointer_move(&mut self, point: Point2, alt: bool) -> bool {
        let Some(state) = self.gesture else {
            return false;
        };
        let Some(anchor) = self.anchors.get(state.index).copied() else {
            return false;
        };
        let drag = Point2::new(point.x - anchor.point.x, point.y - anchor.point.y);
        if !is_drag(drag) {
            return false;
        }
        let rebuilt = if alt {
            Anchor::broken(anchor.point, drag, anchor.handle_in)
        } else {
            Anchor::smooth(anchor.point, drag)
        };
        self.anchors[state.index] = rebuilt;
        if let Some(state) = self.gesture.as_mut() {
            state.dragged = true;
        }
        true
    }

    /// Pointer **up**: the anchor is finished exactly as it looks.
    pub fn pointer_up(&mut self) {
        self.gesture = None;
    }

    /// Undo the anchor being placed (a stray click, or an Esc mid-drag).
    pub fn pop(&mut self) -> bool {
        if self.gesture.is_some() {
            self.gesture = None;
        }
        self.anchors.pop().is_some()
    }

    /// Escape / Enter: **finish without closing** (RULE 2's last bullet).
    pub fn finish(&self) -> PathDraft {
        self.draft(false)
    }

    /// Click on the first point: **close the path**.
    pub fn close(&self) -> PathDraft {
        self.draft(true)
    }

    /// The path as it stands, with an optional "rubber band" to the pointer.
    ///
    /// The rubber band is what makes the pen feel alive: the segment from the last
    /// placed anchor to the *cursor* exists only on screen, and is never part of
    /// the draft the engine is given. Passing `preview: None` is therefore the
    /// commit path, and passing a point is the drawing path — one function, so the
    /// preview and the result cannot disagree.
    pub fn draft_with_preview(&self, closed: bool, preview: Option<Point2>) -> PathDraft {
        let mut anchors = self.anchors.clone();
        if !closed {
            if let Some(point) = preview {
                if !anchors.is_empty() {
                    // The preview anchor is a corner: the cursor is not dragging,
                    // so there is no handle to show.
                    anchors.push(Anchor::corner(point));
                }
            }
        }
        draft_from_anchors(&anchors, closed)
    }

    /// The committed draft.
    pub fn draft(&self, closed: bool) -> PathDraft {
        self.draft_with_preview(closed, None)
    }
}

/// Chain anchors into segments.
///
/// Each interior anchor contributes *two* things to two different segments — its
/// `handle_in` curves the segment arriving at it, its `handle_out` the one
/// leaving — which is why an anchor can be smooth (both curves flow through it)
/// and why breaking one handle visibly changes only one side.
pub fn draft_from_anchors(anchors: &[Anchor], closed: bool) -> PathDraft {
    let start = anchors.first().map(|a| a.point).unwrap_or(Point2::ZERO);
    let mut segments = Vec::new();
    for pair in anchors.windows(2) {
        segments.push(segment_between(&pair[0], &pair[1]));
    }
    if closed && anchors.len() > 1 {
        let last = anchors[anchors.len() - 1];
        let first = anchors[0];
        match closing_segment(&last, &first) {
            // A closing segment that would arrive at the start point *and* be a
            // plain `Close` is the common case (`Close` implies "back to start");
            // a curved close carries the two handles involved.
            PathSegment::Close => segments.push(PathSegment::Close),
            cubic => segments.push(cubic),
        }
    }
    PathDraft {
        start,
        segments,
        closed,
    }
}

/// **Move one handle of an existing anchor** (RULE 2's Alt-drag, RULE 4's handle
/// drag) — the function the Handle Symmetry and Handle Independence laws target.
///
/// * `break_symmetry == false` and the anchor was smooth ⇒ the opposite handle
///   follows, mirrored. The anchor stays smooth.
/// * `break_symmetry == true` ⇒ **only the dragged handle moves**; the opposite
///   one is returned untouched, and the anchor becomes broken.
///
/// `was_smooth` is the input's own state, and it matters: dragging a handle of an
/// anchor that was *never* smooth (a corner the user gave one handle) must not
/// conjure a second handle. Illustrator behaves the same way — Alt is what lets
/// you move a handle without its partner, but no modifier invents a partner that
/// was not there.
pub fn drag_handle(
    point: Point2,
    handle_in: Option<Point2>,
    handle_out: Option<Point2>,
    side: HandleSide,
    to: Point2,
    break_symmetry: bool,
) -> (Option<Point2>, Option<Point2>) {
    let mirrored = Point2::new(2.0 * point.x - to.x, 2.0 * point.y - to.y);
    match side {
        HandleSide::Out => {
            let new_in = if break_symmetry || handle_in.is_none() {
                handle_in
            } else {
                Some(mirrored)
            };
            (new_in, Some(to))
        }
        HandleSide::In => {
            let new_out = if break_symmetry || handle_out.is_none() {
                handle_out
            } else {
                Some(mirrored)
            };
            (Some(to), new_out)
        }
    }
}

/// Rebuild an anchor after [`drag_handle`].
pub fn with_handles(point: Point2, handles: (Option<Point2>, Option<Point2>)) -> Anchor {
    Anchor {
        point,
        handle_in: handles.0,
        handle_out: handles.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn a_click_places_a_corner_and_two_clicks_make_a_line() {
        let mut pen = PenSession::new();
        pen.pointer_down(p(0.0, 0.0));
        pen.pointer_up();
        pen.pointer_down(p(10.0, 0.0));
        pen.pointer_up();
        let draft = pen.finish();
        assert_eq!(draft.segments.len(), 1);
        assert!(matches!(draft.segments[0], PathSegment::Line { .. }));
        assert!(!draft.closed);
    }

    #[test]
    fn a_click_drag_pulls_symmetric_handles() {
        let mut pen = PenSession::new();
        pen.pointer_down(p(0.0, 0.0));
        assert!(pen.pointer_move(p(10.0, 0.0), false), "the drag registers");
        pen.pointer_up();
        let anchor = pen.anchors()[0];
        assert_eq!(anchor.handle_out, Some(p(10.0, 0.0)));
        assert_eq!(anchor.handle_in, Some(p(-10.0, 0.0)));
        assert!(anchor.is_smooth(1e-12));
    }

    #[test]
    fn an_alt_drag_breaks_symmetry_during_the_gesture() {
        let mut pen = PenSession::new();
        pen.pointer_down(p(0.0, 0.0));
        pen.pointer_move(p(10.0, 0.0), true);
        pen.pointer_up();
        let anchor = pen.anchors()[0];
        assert_eq!(anchor.handle_out, Some(p(10.0, 0.0)));
        assert_eq!(
            anchor.handle_in, None,
            "a broken anchor keeps no mirrored partner"
        );
        assert!(!anchor.is_smooth(1e-12));
    }

    #[test]
    fn a_tiny_wobble_is_still_a_click() {
        let mut pen = PenSession::new();
        pen.pointer_down(p(0.0, 0.0));
        assert!(!pen.pointer_move(p(0.4, 0.3), false));
        pen.pointer_up();
        assert_eq!(pen.anchors()[0], Anchor::corner(p(0.0, 0.0)));
    }

    #[test]
    fn closing_adds_a_segment_back_to_the_start() {
        let mut pen = PenSession::new();
        for point in [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)] {
            pen.pointer_down(point);
            pen.pointer_up();
        }
        let open = pen.finish();
        assert_eq!(open.segments.len(), 2);
        let closed = pen.close();
        assert_eq!(closed.segments.len(), 3);
        assert!(matches!(closed.segments[2], PathSegment::Close));
        assert!(closed.closed);
    }

    #[test]
    fn the_rubber_band_is_a_preview_only() {
        let mut pen = PenSession::new();
        pen.pointer_down(p(0.0, 0.0));
        pen.pointer_up();
        let preview = pen.draft_with_preview(false, Some(p(50.0, 0.0)));
        assert_eq!(preview.segments.len(), 1);
        let committed = pen.finish();
        assert!(
            committed.is_empty(),
            "a single anchor is not a path, and the preview must not become one"
        );
    }

    #[test]
    fn pop_removes_the_anchor_being_placed() {
        let mut pen = PenSession::new();
        pen.pointer_down(p(0.0, 0.0));
        pen.pointer_up();
        pen.pointer_down(p(10.0, 0.0));
        assert!(pen.pop(), "the placed anchor goes");
        assert_eq!(pen.anchors().len(), 1);
        assert_eq!(pen.gesture(), PenGesture::Idle);
    }

    #[test]
    fn a_simple_triangle_closes_without_a_kink_when_no_handles_were_dragged() {
        let mut pen = PenSession::new();
        for point in [p(0.0, 0.0), p(20.0, 0.0), p(10.0, 15.0)] {
            pen.pointer_down(point);
            pen.pointer_up();
        }
        let draft = pen.close();
        assert!(draft
            .segments
            .iter()
            .all(|s| matches!(s, PathSegment::Line { .. } | PathSegment::Close)));
    }

    #[test]
    fn drag_handle_without_alt_mirrors_the_opposite_handle() {
        let point = p(0.0, 0.0);
        let (handle_in, handle_out) = drag_handle(
            point,
            Some(p(-5.0, 0.0)),
            Some(p(5.0, 0.0)),
            HandleSide::Out,
            p(0.0, 12.0),
            false,
        );
        assert_eq!(handle_out, Some(p(0.0, 12.0)));
        assert_eq!(handle_in, Some(p(0.0, -12.0)), "mirrored through the point");
    }

    #[test]
    fn drag_handle_with_alt_leaves_the_opposite_handle_alone() {
        let point = p(0.0, 0.0);
        let keep = Some(p(-5.0, 3.0));
        let (handle_in, handle_out) = drag_handle(
            point,
            keep,
            Some(p(5.0, 0.0)),
            HandleSide::Out,
            p(0.0, 12.0),
            true,
        );
        assert_eq!(handle_out, Some(p(0.0, 12.0)));
        assert_eq!(handle_in, keep, "the other handle did not move at all");
    }

    #[test]
    fn dragging_a_handle_never_invents_a_partner() {
        // A corner given a single handle stays single-sided under either modifier.
        let point = p(0.0, 0.0);
        let (handle_in, _) = drag_handle(
            point,
            None,
            Some(p(1.0, 0.0)),
            HandleSide::Out,
            p(0.0, 9.0),
            false,
        );
        assert_eq!(handle_in, None);
    }
}
