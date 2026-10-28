//! Groups: item lists that move together.
//!
//! A group is a node with children that are either items or other groups, so
//! the model is recursive from the start even though the v1 interface only
//! creates one level. Membership is exclusive — an item lives in at most one
//! group, a group in at most one parent — which is what lets a click on any
//! member resolve to the same group deterministically.
//!
//! Groups do not own geometry and they do not reorder drawing: z-order stays
//! the flat item list's order. A group is a selection and transform unit, not
//! a render node.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::scene::ItemId;

/// Identifier of a group inside one scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupId(u64);

impl GroupId {
    /// Wraps a raw identifier, as read from a file.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw identifier, as written to a file.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A member of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupChild {
    /// A leaf item.
    Item(ItemId),
    /// A nested group.
    Group(GroupId),
}

/// One group node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    id: GroupId,
    children: Vec<GroupChild>,
}

impl Group {
    /// A group with the given members.
    pub fn new(id: GroupId, children: Vec<GroupChild>) -> Self {
        Self { id, children }
    }

    /// The group's identity.
    pub const fn id(&self) -> GroupId {
        self.id
    }

    /// The group's members, in insertion order.
    pub fn children(&self) -> &[GroupChild] {
        &self.children
    }
}

/// Deepest nesting accepted while walking the tree.
///
/// Files can carry arbitrary structures; a cycle written by a broken writer
/// must not hang the loader, so every walk stops at this depth. The interface
/// creates one level, and a sane board never approaches the cap.
pub const MAX_GROUP_DEPTH: usize = 64;

/// Every group in the scene, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Groups {
    map: BTreeMap<GroupId, Group>,
}

impl Groups {
    /// No groups.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of groups.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether no groups exist.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The group with `id`.
    pub fn get(&self, id: GroupId) -> Option<&Group> {
        self.map.get(&id)
    }

    /// The group with `id` for mutation.
    pub fn get_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.map.get_mut(&id)
    }

    /// Whether `id` exists.
    pub fn contains(&self, id: GroupId) -> bool {
        self.map.contains_key(&id)
    }

    /// Every group, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Group> {
        self.map.values()
    }

    /// Inserts `group`, returning the previous group with the same id.
    pub fn insert(&mut self, group: Group) -> Option<Group> {
        self.map.insert(group.id(), group)
    }

    /// Removes the group node, leaving its children parentless.
    pub fn remove(&mut self, id: GroupId) -> Option<Group> {
        self.map.remove(&id)
    }

    /// The direct parent of `child`, if any.
    pub fn parent_of(&self, child: GroupChild) -> Option<GroupId> {
        self.map
            .values()
            .find(|group| group.children.contains(&child))
            .map(Group::id)
    }

    /// The position of `child` inside `parent`.
    pub fn position_of(&self, parent: GroupId, child: GroupChild) -> Option<usize> {
        self.map
            .get(&parent)?
            .children
            .iter()
            .position(|entry| *entry == child)
    }

    /// Removes `child` from its parent, returning that parent and the slot it
    /// occupied so undo can put it back exactly.
    pub fn detach(&mut self, child: GroupChild) -> Option<(GroupId, usize)> {
        for group in self.map.values_mut() {
            if let Some(index) = group.children.iter().position(|entry| *entry == child) {
                group.children.remove(index);
                return Some((group.id, index));
            }
        }
        None
    }

    /// Adds `child` to `parent` at `index` (clamped), or does nothing when
    /// `parent` is `None` (a root member needs no record).
    ///
    /// A child that is already in another group is detached first, so the
    /// exclusivity invariant survives a sloppy caller.
    pub fn attach(&mut self, parent: Option<GroupId>, child: GroupChild, index: Option<usize>) {
        let Some(parent) = parent else {
            return;
        };
        if !self.map.contains_key(&parent) {
            return;
        }
        self.detach(child);
        let Some(group) = self.map.get_mut(&parent) else {
            return;
        };
        let index = index
            .unwrap_or(group.children.len())
            .min(group.children.len());
        group.children.insert(index, child);
    }

    /// The leaf items under `group`, depth first, stopping at [`MAX_GROUP_DEPTH`].
    ///
    /// Groups already walked are skipped, so a cycle written by a broken writer
    /// yields each reachable item once instead of recursing forever.
    pub fn leaves(&self, group: GroupId) -> Vec<ItemId> {
        let mut out = Vec::new();
        let mut visited = std::collections::BTreeSet::new();
        self.collect_leaves(group, 0, &mut visited, &mut out);
        out
    }

    fn collect_leaves(
        &self,
        group: GroupId,
        depth: usize,
        visited: &mut std::collections::BTreeSet<GroupId>,
        out: &mut Vec<ItemId>,
    ) {
        if depth > MAX_GROUP_DEPTH || !visited.insert(group) {
            return;
        }
        let Some(node) = self.map.get(&group) else {
            return;
        };
        for child in &node.children {
            match child {
                GroupChild::Item(item) => out.push(*item),
                GroupChild::Group(nested) => {
                    self.collect_leaves(*nested, depth + 1, visited, out);
                }
            }
        }
    }

    /// The outermost group containing `item`, if any.
    pub fn top_group_of_item(&self, item: ItemId) -> Option<GroupId> {
        let mut current = self.parent_of(GroupChild::Item(item))?;
        for _ in 0..MAX_GROUP_DEPTH {
            match self.parent_of(GroupChild::Group(current)) {
                Some(parent) => current = parent,
                None => return Some(current),
            }
        }
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::{Group, GroupChild, GroupId, Groups};
    use crate::scene::ItemId;

    fn item(raw: u64) -> GroupChild {
        GroupChild::Item(ItemId::from_raw(raw))
    }

    fn group(raw: u64) -> GroupChild {
        GroupChild::Group(GroupId::from_raw(raw))
    }

    #[test]
    fn leaves_walk_nested_groups() {
        let mut groups = Groups::new();
        groups.insert(Group::new(GroupId::from_raw(2), vec![item(5), item(6)]));
        groups.insert(Group::new(GroupId::from_raw(1), vec![item(1), group(2)]));

        assert_eq!(
            groups.leaves(GroupId::from_raw(1)),
            vec![
                ItemId::from_raw(1),
                ItemId::from_raw(5),
                ItemId::from_raw(6)
            ]
        );
    }

    #[test]
    fn the_outermost_group_is_what_a_click_resolves_to() {
        let mut groups = Groups::new();
        groups.insert(Group::new(GroupId::from_raw(2), vec![item(5)]));
        groups.insert(Group::new(GroupId::from_raw(1), vec![group(2)]));

        assert_eq!(
            groups.top_group_of_item(ItemId::from_raw(5)),
            Some(GroupId::from_raw(1))
        );
        assert_eq!(groups.top_group_of_item(ItemId::from_raw(9)), None);
    }

    #[test]
    fn detaching_reports_the_slot_and_attach_restores_it() {
        let mut groups = Groups::new();
        groups.insert(Group::new(
            GroupId::from_raw(1),
            vec![item(1), item(2), item(3)],
        ));

        let (parent, index) = groups.detach(item(2)).expect("member exists");
        assert_eq!((parent, index), (GroupId::from_raw(1), 1));
        assert_eq!(
            groups.leaves(parent),
            vec![ItemId::from_raw(1), ItemId::from_raw(3)]
        );

        groups.attach(Some(parent), item(2), Some(index));
        assert_eq!(
            groups.leaves(parent),
            vec![
                ItemId::from_raw(1),
                ItemId::from_raw(2),
                ItemId::from_raw(3)
            ]
        );
    }

    #[test]
    fn attaching_moves_a_child_between_groups() {
        let mut groups = Groups::new();
        groups.insert(Group::new(GroupId::from_raw(1), vec![item(1)]));
        groups.insert(Group::new(GroupId::from_raw(2), vec![item(2)]));

        groups.attach(Some(GroupId::from_raw(2)), item(1), None);

        assert_eq!(groups.leaves(GroupId::from_raw(1)), Vec::<ItemId>::new());
        assert_eq!(
            groups.leaves(GroupId::from_raw(2)),
            vec![ItemId::from_raw(2), ItemId::from_raw(1)]
        );
    }

    #[test]
    fn a_cycle_cannot_hang_a_walk() {
        let mut groups = Groups::new();
        groups.insert(Group::new(GroupId::from_raw(1), vec![group(2)]));
        groups.insert(Group::new(GroupId::from_raw(2), vec![group(1), item(7)]));

        let leaves = groups.leaves(GroupId::from_raw(1));
        assert_eq!(
            leaves,
            vec![ItemId::from_raw(7)],
            "the walk survives the loop"
        );
    }
}
