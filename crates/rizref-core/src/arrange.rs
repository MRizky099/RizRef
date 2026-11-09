//! Arrange, align and distribute: pure layout functions on item rectangles.
//!
//! Each function takes the selected rectangles and returns new centres, leaving
//! sizes untouched. The caller turns that into one [`SetTransforms`] command, so
//! an arrange is a single undo step no matter how many items moved.
//!
//! The functions are pure and deterministic: the same rectangles in the same
//! order produce the same layout, which is what makes the golden tests possible.

use glam::Vec2;

use crate::geometry::Rect;
use crate::scene::ItemId;

/// How to lay a selection out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeMode {
    /// Rows packed left to right in selection order, each as tall as its
    /// tallest item, with even gaps; `columns` of `None` wraps rows at a
    /// width that makes the block near-square.
    Grid {
        /// Fixed items per row, or `None` for the near-square wrap.
        columns: Option<usize>,
    },
    /// The largest item centred, the others packed around it.
    Hierarchy,
    /// One evenly spaced row along the longer axis of the selection.
    Spread,
    /// Every item on the selection centre, in z-order.
    Stack,
}

/// Which edge or centre to align to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Flush left.
    Left,
    /// Flush right.
    Right,
    /// Flush top.
    Top,
    /// Flush bottom.
    Bottom,
    /// Centred horizontally.
    CenterHorizontal,
    /// Centred vertically.
    CenterVertical,
}

/// Axis for distribute and spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Left to right.
    Horizontal,
    /// Top to bottom.
    Vertical,
}

/// Which extent to copy when making sizes equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeAxis {
    /// Width.
    Width,
    /// Height.
    Height,
}

/// The union of every rectangle, `None` for an empty slice.
fn bounds(rects: &[(ItemId, Rect)]) -> Option<Rect> {
    rects
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|a, b| a.union(&b))
}

/// Moves each item to a new centre, in the input order.
pub fn arrange(rects: &[(ItemId, Rect)], mode: ArrangeMode, gap: f32) -> Vec<(ItemId, Vec2)> {
    if rects.is_empty() {
        return Vec::new();
    }
    let gap = gap.max(0.0);
    match mode {
        ArrangeMode::Grid { columns } => grid(rects, columns, gap),
        ArrangeMode::Hierarchy => hierarchy(rects, gap),
        ArrangeMode::Spread => spread(rects, gap),
        ArrangeMode::Stack => stack(rects),
    }
}

/// Row-packed grid: items keep their order and flow left to right, a row
/// is as tall as its tallest item (the others centred on it), and every gap —
/// between items and between rows — is `gap`.
///
/// Fixed-size cells (phase 04 to 07) sized every cell to the largest item, so
/// one 8000 px photo among 300 px sketches spread the sketches thousands of
/// pixels apart. Packing keeps the block tight whatever the mix of sizes.
fn grid(rects: &[(ItemId, Rect)], columns: Option<usize>, gap: f32) -> Vec<(ItemId, Vec2)> {
    let sizes: Vec<Vec2> = rects
        .iter()
        .map(|(_, rect)| Vec2::new(rect.width(), rect.height()))
        .collect();
    let rows = grid_rows(&sizes, columns, gap);

    // Lay the rows out from the origin, then centre the block on the
    // selection.
    let mut centers = vec![Vec2::ZERO; sizes.len()];
    let mut block = Vec2::ZERO;
    let mut y = 0.0;
    for row in &rows {
        let height = row.iter().map(|&i| sizes[i].y).fold(0.0, f32::max);
        let mut x = 0.0;
        for &index in row {
            centers[index] = Vec2::new(x + sizes[index].x * 0.5, y + height * 0.5);
            x += sizes[index].x + gap;
        }
        block.x = block.x.max(x - gap);
        y += height + gap;
    }
    block.y = (y - gap).max(0.0);
    let origin = bounds(rects)
        .map(|rect| rect.center() - block * 0.5)
        .unwrap_or(-block * 0.5);

    rects
        .iter()
        .zip(centers)
        .map(|((id, _), center)| (*id, origin + center))
        .collect()
}

/// Splits item indices into rows: `columns` items each, or — without a
/// count — greedily wrapped at a target width chosen so the packed block is
/// roughly square (never narrower than the widest item).
fn grid_rows(sizes: &[Vec2], columns: Option<usize>, gap: f32) -> Vec<Vec<usize>> {
    if let Some(columns) = columns {
        let columns = columns.max(1);
        return (0..sizes.len())
            .collect::<Vec<_>>()
            .chunks(columns)
            .map(<[usize]>::to_vec)
            .collect();
    }
    let area: f32 = sizes
        .iter()
        .map(|size| (size.x + gap) * (size.y + gap))
        .sum();
    let widest = sizes.iter().map(|size| size.x).fold(0.0, f32::max);
    let target = area.sqrt().max(widest);
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row: Vec<usize> = Vec::new();
    let mut width = 0.0;
    for (index, size) in sizes.iter().enumerate() {
        let added = if row.is_empty() { size.x } else { gap + size.x };
        if !row.is_empty() && width + added > target {
            rows.push(std::mem::take(&mut row));
            width = size.x;
        } else {
            width += added;
        }
        row.push(index);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// The largest item in the middle, the rest on growing rings around it.
fn hierarchy(rects: &[(ItemId, Rect)], gap: f32) -> Vec<(ItemId, Vec2)> {
    let center = bounds(rects)
        .map(|rect| rect.center())
        .unwrap_or(Vec2::ZERO);
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by(|&a, &b| {
        rects[b]
            .1
            .area()
            .total_cmp(&rects[a].1.area())
            .then(a.cmp(&b))
    });

    let largest = rects[order[0]].1;
    let step = (largest.width().max(largest.height()) * 0.5 + gap).max(16.0);
    let mut placed: Vec<Rect> = Vec::with_capacity(rects.len());
    let mut centers = vec![Vec2::ZERO; rects.len()];

    let first = order[0];
    centers[first] = Vec2::ZERO;
    placed.push(Rect::from_center_size(Vec2::ZERO, rects[first].1.size));

    for &index in &order[1..] {
        let rect = rects[index].1;
        let mut spot = None;
        'rings: for ring in 1..=64 {
            let samples = 12 * ring;
            for sample in 0..samples {
                let angle = std::f32::consts::TAU * sample as f32 / samples as f32;
                let candidate = Vec2::new(angle.cos(), angle.sin()) * (ring as f32 * step);
                let box_ = Rect::from_center_size(candidate, rect.size);
                if placed.iter().all(|other| !touches(&box_, other, gap)) {
                    spot = Some(candidate);
                    break 'rings;
                }
            }
        }
        // A selection dense enough to exhaust the rings falls back to a row
        // beyond the current extent rather than stacking items on top of one
        // another.
        let spot = spot.unwrap_or_else(|| {
            Vec2::new(
                placed.iter().map(|rect| rect.max().x).fold(0.0, f32::max) + gap + rect.width(),
                0.0,
            )
        });
        centers[index] = spot;
        placed.push(Rect::from_center_size(spot, rect.size));
    }

    // Centre the arrangement on the selection's original centre, with the
    // largest item on that centre.
    let shift = center - centers[order[0]];
    rects
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, centers[index] + shift))
        .collect()
}

/// One evenly spaced row along the selection's longer axis.
fn spread(rects: &[(ItemId, Rect)], gap: f32) -> Vec<(ItemId, Vec2)> {
    let union = bounds(rects).expect("non-empty");
    let horizontal = union.width() >= union.height();
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by(|&a, &b| {
        let (ca, cb) = if horizontal {
            (rects[a].1.center().x, rects[b].1.center().x)
        } else {
            (rects[a].1.center().y, rects[b].1.center().y)
        };
        ca.total_cmp(&cb).then(a.cmp(&b))
    });

    let extent: f32 = order
        .iter()
        .map(|&index| {
            if horizontal {
                rects[index].1.width()
            } else {
                rects[index].1.height()
            }
        })
        .sum();
    let total = extent + gap * (order.len().saturating_sub(1)) as f32;
    let mut cursor = if horizontal {
        union.center().x - total * 0.5
    } else {
        union.center().y - total * 0.5
    };

    let mut centers = vec![Vec2::ZERO; rects.len()];
    for &index in &order {
        let rect = rects[index].1;
        centers[index] = if horizontal {
            Vec2::new(cursor + rect.width() * 0.5, union.center().y)
        } else {
            Vec2::new(union.center().x, cursor + rect.height() * 0.5)
        };
        cursor += if horizontal {
            rect.width() + gap
        } else {
            rect.height() + gap
        };
    }
    rects
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, centers[index]))
        .collect()
}

/// Every item on the selection centre.
fn stack(rects: &[(ItemId, Rect)]) -> Vec<(ItemId, Vec2)> {
    let center = bounds(rects)
        .map(|rect| rect.center())
        .unwrap_or(Vec2::ZERO);
    rects.iter().map(|(id, _)| (*id, center)).collect()
}

/// Aligns items to one edge or centre of their union.
pub fn align(rects: &[(ItemId, Rect)], align: Align) -> Vec<(ItemId, Vec2)> {
    let Some(union) = bounds(rects) else {
        return Vec::new();
    };
    rects
        .iter()
        .map(|(id, rect)| {
            let center = match align {
                Align::Left => Vec2::new(union.min.x + rect.width() * 0.5, rect.center().y),
                Align::Right => Vec2::new(union.max().x - rect.width() * 0.5, rect.center().y),
                Align::Top => Vec2::new(rect.center().x, union.min.y + rect.height() * 0.5),
                Align::Bottom => Vec2::new(rect.center().x, union.max().y - rect.height() * 0.5),
                Align::CenterHorizontal => Vec2::new(union.center().x, rect.center().y),
                Align::CenterVertical => Vec2::new(rect.center().x, union.center().y),
            };
            (*id, center)
        })
        .collect()
}

/// Spaces the items' centres evenly between the first and the last.
pub fn distribute(rects: &[(ItemId, Rect)], orientation: Orientation) -> Vec<(ItemId, Vec2)> {
    if rects.len() < 3 {
        return rects
            .iter()
            .map(|(id, rect)| (*id, rect.center()))
            .collect();
    }
    let horizontal = orientation == Orientation::Horizontal;
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = if horizontal {
            (rects[a].1.center().x, rects[b].1.center().x)
        } else {
            (rects[a].1.center().y, rects[b].1.center().y)
        };
        a.total_cmp(&b)
    });

    let first = rects[order[0]].1.center();
    let last = rects[order[order.len() - 1]].1.center();
    let mut centers: Vec<Vec2> = rects.iter().map(|(_, rect)| rect.center()).collect();
    let steps = (order.len() - 1) as f32;
    for (position, &index) in order.iter().enumerate() {
        let t = position as f32 / steps;
        centers[index] = first + (last - first) * t;
    }
    rects
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, centers[index]))
        .collect()
}

/// Sets every item's width or height to the selection's largest on that axis.
pub fn match_size(rects: &[(ItemId, Rect)], axis: SizeAxis) -> Vec<(ItemId, Vec2)> {
    let target = rects.iter().fold(0.0_f32, |acc, (_, rect)| match axis {
        SizeAxis::Width => acc.max(rect.width()),
        SizeAxis::Height => acc.max(rect.height()),
    });
    rects
        .iter()
        .map(|(id, rect)| {
            let size = match axis {
                SizeAxis::Width => Vec2::new(target, rect.height()),
                SizeAxis::Height => Vec2::new(rect.width(), target),
            };
            (*id, size)
        })
        .collect()
}

/// Whether `rect` is within `gap` of `other`, counting touching as a conflict.
fn touches(rect: &Rect, other: &Rect, gap: f32) -> bool {
    let padded = Rect::from_center_size(other.center(), other.size + Vec2::splat(gap));
    rect.intersects(&padded)
}

#[cfg(test)]
mod tests {
    use super::{
        Align, ArrangeMode, Orientation, SizeAxis, align, arrange, distribute, match_size,
    };
    use crate::geometry::Rect;
    use crate::scene::ItemId;
    use glam::Vec2;

    fn mixed() -> Vec<(ItemId, Rect)> {
        (0..12)
            .map(|index| {
                let width = 40.0 + index as f32 * 13.0;
                let height = 30.0 + (index % 4) as f32 * 20.0;
                let x = (index % 3) as f32 * 300.0;
                let y = (index / 3) as f32 * 200.0;
                (
                    ItemId::from_raw(index as u64 + 1),
                    Rect::from_min_size(Vec2::new(x, y), Vec2::new(width, height)),
                )
            })
            .collect()
    }

    /// Places each rect at its arranged centre.
    fn placed(rects: &[(ItemId, Rect)], centers: &[(ItemId, Vec2)]) -> Vec<Rect> {
        centers
            .iter()
            .zip(rects)
            .map(|((_, center), (_, rect))| Rect::from_center_size(*center, rect.size))
            .collect()
    }

    fn assert_no_overlap(placed: &[Rect]) {
        for (index, rect) in placed.iter().enumerate() {
            for other in &placed[index + 1..] {
                assert!(!rect.intersects(other), "{rect:?} overlaps {other:?}");
            }
        }
    }

    #[test]
    fn a_grid_of_twelve_mixed_items_is_tight_and_non_overlapping() {
        let rects = mixed();
        let gap = 8.0;
        let centers = arrange(&rects, ArrangeMode::Grid { columns: None }, gap);

        assert_eq!(centers.len(), 12);
        let placed = placed(&rects, &centers);
        assert_no_overlap(&placed);

        // Neighbours in a row sit exactly one gap apart, in selection order.
        for pair in placed.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            let same_row = (left.center().y - right.center().y).abs() < 1.0e-3;
            if same_row {
                assert!((right.min.x - left.max().x - gap).abs() < 1.0e-3);
            }
        }
        // Tight: the block wastes at most the row-height slack, so its area
        // stays within a small multiple of the items' own.
        let block = placed
            .iter()
            .copied()
            .reduce(|a, b| a.union(&b))
            .expect("non-empty");
        let items: f32 = placed.iter().map(Rect::area).sum();
        assert!(block.area() < items * 2.0, "{} vs {items}", block.area());
    }

    #[test]
    fn one_huge_item_does_not_spread_the_small_ones() {
        // The manual-test case: an 8000 px photo among 300-2400 px images.
        let mut rects = vec![(
            ItemId::from_raw(1),
            Rect::from_min_size(Vec2::ZERO, Vec2::new(8000.0, 6000.0)),
        )];
        for index in 0..9 {
            let side = 300.0 + index as f32 * 260.0;
            rects.push((
                ItemId::from_raw(index + 2),
                Rect::from_min_size(Vec2::new(index as f32 * 9000.0, 0.0), Vec2::splat(side)),
            ));
        }
        let gap = 16.0;
        let centers = arrange(&rects, ArrangeMode::Grid { columns: None }, gap);
        let placed = placed(&rects, &centers);
        assert_no_overlap(&placed);

        // Every small item has a neighbour within one gap (plus the row-height
        // slack of centring in its row): nothing is stranded in a huge cell.
        for (index, rect) in placed.iter().enumerate().skip(1) {
            let nearest = placed
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, other)| {
                    let dx = (other.min.x - rect.max().x).max(rect.min.x - other.max().x);
                    let dy = (other.min.y - rect.max().y).max(rect.min.y - other.max().y);
                    dx.max(dy).max(0.0)
                })
                .fold(f32::INFINITY, f32::min);
            assert!(nearest <= gap + 1.0e-3, "item {index} is {nearest} away");
        }
    }

    #[test]
    fn a_grid_stays_centred_on_the_selection() {
        let rects = mixed();
        let before = rects
            .iter()
            .map(|(_, rect)| *rect)
            .reduce(|a, b| a.union(&b))
            .expect("non-empty")
            .center();
        let centers = arrange(&rects, ArrangeMode::Grid { columns: Some(3) }, 8.0);
        let after = placed(&rects, &centers)
            .into_iter()
            .reduce(|a, b| a.union(&b))
            .expect("non-empty")
            .center();

        assert!((after - before).length() < 1.0e-3);
    }

    #[test]
    fn a_fixed_column_count_is_honoured() {
        let rects = mixed();
        let centers = arrange(&rects, ArrangeMode::Grid { columns: Some(5) }, 0.0);
        let ys: std::collections::BTreeSet<u32> = centers
            .iter()
            .map(|(_, center)| center.y.to_bits())
            .collect();

        assert_eq!(ys.len(), 3, "12 items in five columns need three rows");
    }

    #[test]
    fn hierarchy_centres_the_largest_and_never_overlaps() {
        let rects = mixed();
        let centers = arrange(&rects, ArrangeMode::Hierarchy, 4.0);
        let largest = rects
            .iter()
            .enumerate()
            .max_by(|(_, (_, a)), (_, (_, b))| a.area().total_cmp(&b.area()))
            .map(|(index, _)| index)
            .expect("non-empty");

        let placed: Vec<Rect> = centers
            .iter()
            .zip(&rects)
            .map(|((_, center), (_, rect))| Rect::from_center_size(*center, rect.size))
            .collect();
        for (index, rect) in placed.iter().enumerate() {
            for other in &placed[index + 1..] {
                assert!(!rect.intersects(other), "{rect:?} overlaps {other:?}");
            }
        }
        let original_center = rects
            .iter()
            .map(|(_, rect)| *rect)
            .reduce(|a, b| a.union(&b))
            .expect("non-empty")
            .center();
        assert!(
            (centers[largest].1 - original_center).length() < 1.0e-3,
            "the largest item sits on the selection's centre"
        );
    }

    #[test]
    fn spread_uses_equal_gaps_and_stays_centred() {
        let rects = mixed();
        let centers = arrange(&rects, ArrangeMode::Spread, 10.0);
        let placed: Vec<Rect> = centers
            .iter()
            .zip(&rects)
            .map(|((_, center), (_, rect))| Rect::from_center_size(*center, rect.size))
            .collect();
        let union = placed
            .iter()
            .copied()
            .reduce(|a, b| a.union(&b))
            .expect("non-empty");

        // All on one row, sorted, with the configured gap between neighbours.
        let mut sorted = placed.clone();
        sorted.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        for pair in sorted.windows(2) {
            assert!((pair[1].min.x - pair[0].max().x - 10.0).abs() < 1.0e-3);
        }
        assert!((union.center().y - placed[0].center().y).abs() < 1.0e-3);
    }

    #[test]
    fn stack_piles_everything_on_one_centre() {
        let rects = mixed();
        let centers = arrange(&rects, ArrangeMode::Stack, 0.0);
        let first = centers[0].1;

        assert!(centers.iter().all(|(_, center)| *center == first));
    }

    #[test]
    fn align_puts_every_item_on_the_requested_edge() {
        let rects = mixed();
        let union = rects
            .iter()
            .map(|(_, rect)| *rect)
            .reduce(|a, b| a.union(&b))
            .expect("non-empty");

        let left = align(&rects, Align::Left);
        for ((_, center), (_, rect)) in left.iter().zip(&rects) {
            assert!((center.x - rect.width() * 0.5 - union.min.x).abs() < 1.0e-3);
        }

        let top = align(&rects, Align::Top);
        for ((_, center), (_, rect)) in top.iter().zip(&rects) {
            assert!((center.y - rect.height() * 0.5 - union.min.y).abs() < 1.0e-3);
        }
    }

    #[test]
    fn distribute_keeps_the_ends_and_evens_the_centres() {
        let rects = mixed();
        let distributed = distribute(&rects, Orientation::Horizontal);
        let mut xs: Vec<f32> = distributed.iter().map(|(_, center)| center.x).collect();
        xs.sort_by(f32::total_cmp);

        let step = xs[1] - xs[0];
        for pair in xs.windows(2) {
            assert!((pair[1] - pair[0] - step).abs() < 1.0e-3);
        }
    }

    #[test]
    fn match_size_copies_the_largest_extent() {
        let rects = mixed();
        let widths = match_size(&rects, SizeAxis::Width);
        let target = rects
            .iter()
            .map(|(_, rect)| rect.width())
            .fold(0.0, f32::max);

        for ((_, size), _) in widths.iter().zip(&rects) {
            assert!((size.x - target).abs() < 1.0e-3);
        }
    }

    #[test]
    fn an_empty_selection_arranges_to_nothing() {
        assert!(arrange(&[], ArrangeMode::Grid { columns: None }, 0.0).is_empty());
        assert!(align(&[], Align::Left).is_empty());
        assert!(distribute(&[], Orientation::Horizontal).is_empty());
        assert!(match_size(&[], SizeAxis::Width).is_empty());
    }
}
