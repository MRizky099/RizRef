//! The current selection.

use std::collections::BTreeSet;

use crate::scene::ItemId;

/// The items the user has selected.
///
/// A `BTreeSet` rather than a `Vec` or a `HashSet`: iteration order is stable
/// and independent of the order the user clicked in, which keeps hit-testing,
/// the marquee and batch operations deterministic — and therefore reproducible
/// in tests and in a bug report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    items: BTreeSet<ItemId>,
}

impl Selection {
    /// An empty selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of selected items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether `item` is selected.
    pub fn contains(&self, item: ItemId) -> bool {
        self.items.contains(&item)
    }

    /// Adds `item`; returns whether it was newly added.
    pub fn insert(&mut self, item: ItemId) -> bool {
        self.items.insert(item)
    }

    /// Removes `item`; returns whether it had been selected.
    pub fn remove(&mut self, item: ItemId) -> bool {
        self.items.remove(&item)
    }

    /// Ctrl-click semantics: selects `item`, or deselects it if already selected.
    ///
    /// Returns whether the item is selected afterwards.
    pub fn toggle(&mut self, item: ItemId) -> bool {
        if self.items.remove(&item) {
            false
        } else {
            self.items.insert(item);
            true
        }
    }

    /// Replaces the selection with a single item, as a plain click does.
    pub fn set_single(&mut self, item: ItemId) {
        self.items.clear();
        self.items.insert(item);
    }

    /// Empties the selection.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// The selected items in ascending id order.
    pub fn iter(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.items.iter().copied()
    }
}

impl FromIterator<ItemId> for Selection {
    fn from_iter<T: IntoIterator<Item = ItemId>>(iter: T) -> Self {
        Self {
            items: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;
    use crate::scene::ItemId;

    fn id(raw: u64) -> ItemId {
        ItemId::from_raw(raw)
    }

    #[test]
    fn a_new_selection_is_empty() {
        let selection = Selection::new();

        assert!(selection.is_empty());
        assert_eq!(selection.len(), 0);
        assert_eq!(selection.iter().count(), 0);
    }

    #[test]
    fn toggle_selects_then_deselects() {
        let mut selection = Selection::new();

        assert!(selection.toggle(id(4)), "first toggle selects");
        assert!(selection.contains(id(4)));

        assert!(!selection.toggle(id(4)), "second toggle deselects");
        assert!(selection.is_empty());
    }

    #[test]
    fn insert_reports_whether_the_selection_changed() {
        let mut selection = Selection::new();

        assert!(selection.insert(id(1)));
        assert!(!selection.insert(id(1)), "inserting twice is not a change");
        assert_eq!(selection.len(), 1);
    }

    #[test]
    fn set_single_replaces_the_selection() {
        let mut selection: Selection = [id(3), id(1), id(2)].into_iter().collect();
        assert_eq!(selection.len(), 3);

        selection.set_single(id(9));

        assert_eq!(selection.len(), 1);
        assert!(selection.contains(id(9)));
        assert_eq!(selection.iter().collect::<Vec<_>>(), vec![id(9)]);
    }

    #[test]
    fn iteration_is_sorted_and_stable() {
        let mut selection = Selection::new();
        for raw in [5, 2, 9, 1] {
            selection.insert(id(raw));
        }

        let order: Vec<u64> = selection.iter().map(ItemId::raw).collect();

        assert_eq!(order, vec![1, 2, 5, 9]);
    }

    #[test]
    fn remove_and_clear() {
        let mut selection: Selection = [id(1), id(2)].into_iter().collect();

        assert!(selection.remove(id(1)));
        assert!(!selection.remove(id(1)));
        assert_eq!(selection.len(), 1);

        selection.clear();
        assert!(selection.is_empty());
    }
}
