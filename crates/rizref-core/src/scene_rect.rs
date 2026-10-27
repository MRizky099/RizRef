//! The scene rect: the bounding box of everything on the board.
//!
//! The scene rect is what "frame all" fits the camera to, what the canvas
//! border outlines, and later what autosave thumbnails frame. It is *tracked*
//! rather than recomputed from the scene on demand: the renderer and the view
//! ask for it every frame, and a board can hold hundreds of items, so the
//! tracker keeps one rect per item and folds them together on query. Boards
//! are small enough that the fold is a linear scan that never shows up next to
//! a frame budget — the tracking exists so item changes push once instead of
//! every consumer polling the whole scene.

use std::collections::HashMap;

use crate::geometry::Rect;
use crate::scene::ItemId;

/// Incrementally tracked bounding box of the board's items.
///
/// Callers report item geometry as it changes ([`SceneRectTracker::merge`]) and
/// items leaving the board ([`SceneRectTracker::withdraw`]); the union is
/// available at any time from [`SceneRectTracker::rect`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneRectTracker {
    rects: HashMap<ItemId, Rect>,
    /// Cached union; `None` means the board is empty.
    rect: Option<Rect>,
}

impl SceneRectTracker {
    /// An empty tracker with no items.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an item's rect, or updates it after the item moved or resized.
    ///
    /// Reporting the same item twice replaces its contribution rather than
    /// growing it, so a moved item does not leave its old footprint behind in
    /// the union. A replacement cannot be folded into the cached union
    /// incrementally, so the union is rebuilt over the tracked items.
    pub fn merge(&mut self, item: ItemId, rect: Rect) {
        if self.rects.insert(item, rect).is_some() {
            self.rect = Self::union_of(self.rects.values());
        } else {
            self.rect = Some(match self.rect {
                Some(current) => current.union(&rect),
                None => rect,
            });
        }
    }

    /// Removes an item's contribution, returning the rect it had.
    ///
    /// Removing cannot be folded into the cached union incrementally (a union
    /// has no inverse), so the union is rebuilt from the remaining items.
    pub fn withdraw(&mut self, item: ItemId) -> Option<Rect> {
        let removed = self.rects.remove(&item)?;
        self.rect = Self::union_of(self.rects.values());
        Some(removed)
    }

    /// Folds a set of rects into their union, `None` when the set is empty.
    fn union_of<'a>(rects: impl Iterator<Item = &'a Rect>) -> Option<Rect> {
        rects.fold(None, |acc, rect| match acc {
            Some(current) => Some(current.union(rect)),
            None => Some(*rect),
        })
    }

    /// The bounding box of every tracked item, or `None` when the board is
    /// empty.
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// Whether an item currently contributes to the rect.
    pub fn contains(&self, item: ItemId) -> bool {
        self.rects.contains_key(&item)
    }

    /// How many items are tracked.
    pub fn len(&self) -> usize {
        self.rects.len()
    }

    /// Whether no item is tracked.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Forgets every item.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.rect = None;
    }
}

#[cfg(test)]
mod tests {
    use super::SceneRectTracker;
    use crate::geometry::Rect;
    use crate::scene::ItemId;
    use glam::Vec2;

    fn item(raw: u64) -> ItemId {
        ItemId::from_raw(raw)
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Vec2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn an_empty_tracker_has_no_rect() {
        let tracker = SceneRectTracker::new();

        assert!(tracker.is_empty());
        assert_eq!(tracker.rect(), None);
    }

    #[test]
    fn one_item_merges_to_its_own_rect() {
        let mut tracker = SceneRectTracker::new();

        tracker.merge(item(1), rect(10.0, 20.0, 30.0, 40.0));

        assert_eq!(tracker.rect(), Some(rect(10.0, 20.0, 30.0, 40.0)));
        assert_eq!(tracker.len(), 1);
    }

    #[test]
    fn merging_grows_the_union() {
        let mut tracker = SceneRectTracker::new();
        tracker.merge(item(1), rect(0.0, 0.0, 10.0, 10.0));
        tracker.merge(item(2), rect(20.0, 5.0, 10.0, 10.0));

        let union = tracker.rect().expect("two items");

        assert_eq!(union.min, Vec2::ZERO);
        assert_eq!(union.max(), Vec2::new(30.0, 15.0));
    }

    #[test]
    fn re_merging_an_item_replaces_its_footprint() {
        let mut tracker = SceneRectTracker::new();
        tracker.merge(item(1), rect(0.0, 0.0, 10.0, 10.0));
        tracker.merge(item(2), rect(100.0, 100.0, 10.0, 10.0));

        // Item 1 moves next to item 2; its old corner must not linger.
        tracker.merge(item(1), rect(50.0, 100.0, 10.0, 10.0));

        let union = tracker.rect().expect("two items");
        assert_eq!(union.min, Vec2::new(50.0, 100.0));
        assert_eq!(union.max(), Vec2::new(110.0, 110.0));
    }

    #[test]
    fn withdrawing_an_item_shrinks_the_union_back() {
        let mut tracker = SceneRectTracker::new();
        tracker.merge(item(1), rect(0.0, 0.0, 10.0, 10.0));
        tracker.merge(item(2), rect(20.0, 0.0, 10.0, 10.0));

        let removed = tracker.withdraw(item(2));

        assert_eq!(removed, Some(rect(20.0, 0.0, 10.0, 10.0)));
        assert_eq!(tracker.rect(), Some(rect(0.0, 0.0, 10.0, 10.0)));
        assert!(!tracker.contains(item(2)));
        assert_eq!(tracker.withdraw(item(2)), None, "withdrawing twice");
    }

    #[test]
    fn withdrawing_the_last_item_empties_the_rect() {
        let mut tracker = SceneRectTracker::new();
        tracker.merge(item(1), rect(0.0, 0.0, 10.0, 10.0));

        tracker.withdraw(item(1));

        assert_eq!(tracker.rect(), None);
        assert!(tracker.is_empty());
    }

    #[test]
    fn clear_forgets_everything() {
        let mut tracker = SceneRectTracker::new();
        tracker.merge(item(1), rect(0.0, 0.0, 10.0, 10.0));

        tracker.clear();

        assert_eq!(tracker.rect(), None);
        assert_eq!(tracker.len(), 0);
    }
}
