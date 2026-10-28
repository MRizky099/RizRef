//! Where a newly imported image lands on the board.
//!
//! Dropping twenty files must not stack twenty items on one point. Each result
//! lands around the drop point on a small ring search: the first free slot wins,
//! with a fixed gap to every item already placed. The function is pure — given
//! the drop point, the new item's size and the occupied rectangles, it returns
//! one centre — so the packing is unit-tested without a window.

use glam::Vec2;
use rizref_core::Rect;

/// Gap left between neighbouring imported items, in world units.
pub const ITEM_GAP: f32 = 24.0;

/// Biggest ring the search walks before falling back to a far-away slot.
const MAX_RING: i32 = 64;

/// Finds a free centre near `drop` for an item of `size`.
///
/// Candidates form square rings around the drop point, stepping by the item's
/// own extent plus [`ITEM_GAP`]. `occupied` is tested with the gap included, so
/// items never touch. The search is deterministic: the same board and the same
/// drop point always place the item in the same spot.
pub fn find_spot(drop: Vec2, size: Vec2, occupied: &[Rect], gap: f32) -> Vec2 {
    let size = size.abs().max(Vec2::splat(1.0));
    let step = size + Vec2::splat(gap.max(0.0));

    for ring in 0..=MAX_RING {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if dx.abs().max(dy.abs()) != ring {
                    continue;
                }
                let candidate = drop + Vec2::new(dx as f32 * step.x, dy as f32 * step.y);
                let rect = Rect::from_center_size(candidate, size);
                if occupied.iter().all(|other| !touches(&rect, other, gap)) {
                    return candidate;
                }
            }
        }
    }

    // A board dense enough to fill 64 rings is already pathological. Returning
    // a deterministic point beyond the searched area is still better than
    // stacking on top of an existing item.
    drop + Vec2::new(step.x * (MAX_RING as f32) * 2.0, 0.0)
}

/// Places a batch of sizes around one drop point, returning their centres.
///
/// Used by tests and by future batch imports that know their sizes up front;
/// the live import path places each result as it arrives with [`find_spot`].
#[cfg(test)]
pub fn place_batch(drop: Vec2, sizes: &[Vec2], gap: f32) -> Vec<Vec2> {
    let mut occupied = Vec::with_capacity(sizes.len());
    let mut centers = Vec::with_capacity(sizes.len());
    for &size in sizes {
        let center = find_spot(drop, size, &occupied, gap);
        occupied.push(Rect::from_center_size(center, size.abs()));
        centers.push(center);
    }
    centers
}

/// Whether `rect` is within `gap` of `other`, counting touching as a conflict.
fn touches(rect: &Rect, other: &Rect, gap: f32) -> bool {
    let padded = Rect::from_center_size(other.center(), other.size + Vec2::splat(gap.max(0.0)));
    rect.intersects(&padded)
}

#[cfg(test)]
mod tests {
    use super::{ITEM_GAP, find_spot, place_batch};
    use glam::Vec2;
    use rizref_core::Rect;

    #[test]
    fn an_empty_board_lands_exactly_on_the_drop_point() {
        let spot = find_spot(Vec2::new(10.0, 20.0), Vec2::new(100.0, 50.0), &[], ITEM_GAP);

        assert_eq!(spot, Vec2::new(10.0, 20.0));
    }

    #[test]
    fn a_second_item_gets_its_own_slot() {
        let size = Vec2::new(100.0, 50.0);
        let first = Vec2::new(10.0, 20.0);
        let occupied = [Rect::from_center_size(first, size)];

        let second = find_spot(first, size, &occupied, ITEM_GAP);

        assert_ne!(second, first);
        let gap = (second - first).abs();
        assert!(
            gap.x >= size.x + ITEM_GAP - 0.001 || gap.y >= size.y + ITEM_GAP - 0.001,
            "the second slot must clear the first, got {second:?}"
        );
    }

    #[test]
    fn thirty_items_never_overlap_and_stay_near_the_drop() {
        let drop = Vec2::new(-100.0, 40.0);
        let sizes: Vec<Vec2> = (0..30)
            .map(|index| Vec2::new(80.0 + index as f32 * 3.0, 60.0))
            .collect();

        let centers = place_batch(drop, &sizes, ITEM_GAP);
        let rects: Vec<Rect> = centers
            .iter()
            .zip(&sizes)
            .map(|(&center, &size)| Rect::from_center_size(center, size))
            .collect();

        for (index, rect) in rects.iter().enumerate() {
            for other in &rects[index + 1..] {
                assert!(
                    !rect.intersects(other),
                    "items {index} and a later item overlap: {rect:?} vs {other:?}"
                );
            }
            assert!(
                (rect.center() - drop).length() < 1_500.0,
                "item {index} strayed too far: {rect:?}"
            );
        }
    }

    #[test]
    fn mixed_sizes_are_accommodated() {
        let sizes = [
            Vec2::new(400.0, 300.0),
            Vec2::new(40.0, 30.0),
            Vec2::new(200.0, 500.0),
            Vec2::new(80.0, 80.0),
        ];

        let centers = place_batch(Vec2::ZERO, &sizes, ITEM_GAP);
        let rects: Vec<Rect> = centers
            .iter()
            .zip(sizes)
            .map(|(&center, size)| Rect::from_center_size(center, size))
            .collect();

        for (index, rect) in rects.iter().enumerate() {
            for other in &rects[index + 1..] {
                assert!(!rect.intersects(other), "{rect:?} vs {other:?}");
            }
        }
    }
}
