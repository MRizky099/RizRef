//! Reversible scene mutations and the bounded undo/redo history.
//!
//! Commands are the only way the UI is allowed to change the model. Routing
//! every edit — a drag, a paste, an arrange, a group — through one interface is
//! what makes undo, autosave and the dirty flag fall out of a single code path
//! instead of being maintained per feature.
//!
//! Each command keeps its own inverse rather than a snapshot of the board. A
//! transform command stores the before and after placement of exactly the items
//! it touched, so undoing a 200-command session costs the items involved rather
//! than 200 copies of the scene. Repeated gestures of the same kind coalesce
//! inside [`COALESCE_WINDOW`]: holding an arrow key for a second is one undo
//! step, not forty.

use std::any::Any;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::time::{Duration, Instant};

use crate::group::{Group, GroupChild, GroupId};
use crate::item::{Item, ItemState};
use crate::scene::{ItemId, Scene};

/// What a command changed, for the renderer and the dirty flag.
///
/// The point of the split is that a drag only touches [`Effects::geometry`], so
/// the renderer can re-upload instance data without re-rendering note textures,
/// and autosave (phase 06) can mark the scene dirty from one place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    /// Items whose transform, flip or opacity changed.
    pub geometry: BTreeSet<ItemId>,
    /// Items whose generated pixels changed (note text).
    pub content: BTreeSet<ItemId>,
    /// Items added to the board.
    pub added: BTreeSet<ItemId>,
    /// Items removed from the board.
    pub removed: BTreeSet<ItemId>,
    /// Z-order changed.
    pub order: bool,
    /// Group membership changed.
    pub groups: bool,
}

impl Effects {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.geometry.is_empty()
            && self.content.is_empty()
            && self.added.is_empty()
            && self.removed.is_empty()
            && !self.order
            && !self.groups
    }

    /// Whether the board's item set or order changed.
    pub fn structural(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty() || self.order
    }

    /// Folds `other` into `self`.
    pub fn merge(&mut self, other: &Self) {
        self.geometry.extend(other.geometry.iter().copied());
        self.content.extend(other.content.iter().copied());
        self.added.extend(other.added.iter().copied());
        self.removed.extend(other.removed.iter().copied());
        self.order |= other.order;
        self.groups |= other.groups;
    }
}

/// A command that can be merged with a newer one of the same kind.
///
/// A gesture that repeats within the coalescing window — arrow-key nudges are
/// the everyday case — should read as one undo step. The newer command's state
/// is already applied when [`Command::try_merge`] is called; a successful merge
/// only has to adopt its "after" payload and drop it from the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoalesceKey {
    /// Placement changes of the same item set.
    Transform,
    /// Typing into one note.
    NoteText(ItemId),
}

/// A reversible mutation of a [`Scene`].
pub trait Command: Debug + Any {
    /// Short human-readable label, shown in the menu and the undo tooltip.
    fn label(&self) -> &str;

    /// Applies the mutation and reports what changed.
    fn apply(&mut self, scene: &mut Scene) -> Effects;

    /// Restores the state as it was before [`Command::apply`] ran.
    fn revert(&mut self, scene: &mut Scene) -> Effects;

    /// The merge group this command belongs to, if it may coalesce.
    fn coalesce_key(&self) -> Option<CoalesceKey> {
        None
    }

    /// Adopts `newer`'s after-state when it extends this command.
    ///
    /// Called only when both commands share a [`CoalesceKey`]; implementations
    /// downcast and must refuse a payload that touches a different item set.
    fn try_merge(&mut self, _newer: &dyn Any) -> bool {
        false
    }

    /// Upcasts for [`Command::try_merge`]'s downcast.
    fn as_any(&self) -> &dyn Any;
}

/// Implements [`Command::as_any`] for a concrete command type.
macro_rules! impl_as_any {
    ($type:ty) => {
        fn as_any(&self) -> &dyn Any {
            self
        }
    };
}

/// Bounded undo/redo history.
///
/// A board is edited for hours at a time, so the history is capped: once the
/// limit is reached the oldest command is dropped and its effect becomes
/// permanent. The redo stack is cleared whenever a new command is applied, which
/// is what users expect from every editor.
#[derive(Debug)]
pub struct History {
    done: Vec<Box<dyn Command>>,
    undone: Vec<Box<dyn Command>>,
    limit: usize,
    last_apply: Option<Instant>,
}

impl History {
    /// Commands kept for undo by default.
    pub const DEFAULT_LIMIT: usize = 200;

    /// Repeated commands of one kind inside this window become one undo step.
    pub const COALESCE_WINDOW: Duration = Duration::from_millis(100);

    /// A history holding at most `limit` commands; `0` is raised to `1`, because
    /// a history that cannot undo the command it just applied is a bug factory.
    pub fn new(limit: usize) -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            limit: limit.max(1),
            last_apply: None,
        }
    }

    /// Applies `command`, records it for undo and discards the redo stack.
    pub fn apply(&mut self, scene: &mut Scene, command: Box<dyn Command>) -> Effects {
        self.apply_at(scene, command, Instant::now())
    }

    /// Applies `command` at `now`, coalescing with the previous one when both
    /// allow it and the gap is inside [`History::COALESCE_WINDOW`].
    pub fn apply_at(
        &mut self,
        scene: &mut Scene,
        mut command: Box<dyn Command>,
        now: Instant,
    ) -> Effects {
        let effects = command.apply(scene);
        self.undone.clear();

        let key = command.coalesce_key();
        if let (Some(key), Some(previous), Some(last)) =
            (key, self.done.last_mut(), self.last_apply)
        {
            let recent = now.saturating_duration_since(last) <= Self::COALESCE_WINDOW;
            if recent
                && previous.coalesce_key() == Some(key)
                && previous.try_merge(command.as_any())
            {
                self.last_apply = Some(now);
                return effects;
            }
        }

        self.done.push(command);
        if self.done.len() > self.limit {
            self.done.remove(0);
        }
        self.last_apply = Some(now);
        effects
    }

    /// Reverts the most recent command, returning its label and effects.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<(String, Effects)> {
        let mut command = self.done.pop()?;
        let effects = command.revert(scene);
        let label = command.label().to_owned();
        self.undone.push(command);
        self.last_apply = None;
        Some((label, effects))
    }

    /// Re-applies the most recently undone command, returning its label and
    /// effects.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<(String, Effects)> {
        let mut command = self.undone.pop()?;
        let effects = command.apply(scene);
        let label = command.label().to_owned();
        self.done.push(command);
        self.last_apply = None;
        Some((label, effects))
    }

    /// Whether [`History::undo`] would do anything.
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    /// Whether [`History::redo`] would do anything.
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Number of commands that can be undone.
    pub fn undo_len(&self) -> usize {
        self.done.len()
    }

    /// Number of commands that can be redone.
    pub fn redo_len(&self) -> usize {
        self.undone.len()
    }

    /// Maximum number of commands retained.
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(Self::DEFAULT_LIMIT)
    }
}

// ---------------------------------------------------------------------------
// Item commands
// ---------------------------------------------------------------------------

/// Adds items at recorded z-positions.
///
/// The positions are captured on the first apply: a duplicate or a paste lands
/// on top of the board, and redo — which runs after the inverse removed exactly
/// those items — finds the same stack height again.
#[derive(Debug)]
pub struct AddItems {
    label: String,
    items: Vec<Item>,
    positions: Vec<usize>,
}

impl AddItems {
    /// Adds `items` on top of the board in the order given.
    pub fn on_top(label: impl Into<String>, items: Vec<Item>) -> Self {
        Self {
            label: label.into(),
            items,
            positions: Vec::new(),
        }
    }

    /// Adds items at explicit z-positions, in ascending order.
    pub fn at_positions(label: impl Into<String>, entries: Vec<(usize, Item)>) -> Self {
        let (positions, items) = entries.into_iter().unzip();
        Self {
            label: label.into(),
            items,
            positions,
        }
    }
}

impl Command for AddItems {
    fn label(&self) -> &str {
        &self.label
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        if self.positions.len() != self.items.len() {
            let start = scene.len();
            self.positions = (0..self.items.len()).map(|offset| start + offset).collect();
        }
        let mut effects = Effects::default();
        for (index, item) in self.positions.iter().zip(&self.items) {
            scene.insert_item_at(*index, item.clone());
            effects.added.insert(item.id());
        }
        effects.order = !self.items.is_empty();
        effects
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        let mut effects = Effects::default();
        for item in &self.items {
            scene.remove(item.id());
            effects.removed.insert(item.id());
        }
        effects.order = !self.items.is_empty();
        effects
    }

    impl_as_any!(AddItems);
}

/// Removes items, remembering where each one sat so undo restores z-order.
#[derive(Debug)]
pub struct RemoveItems {
    label: String,
    ids: BTreeSet<ItemId>,
    captured: Vec<(usize, Item)>,
}

impl RemoveItems {
    /// Removes `ids` on apply.
    pub fn new(label: impl Into<String>, ids: impl IntoIterator<Item = ItemId>) -> Self {
        Self {
            label: label.into(),
            ids: ids.into_iter().collect(),
            captured: Vec::new(),
        }
    }

    /// The command's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The items the command removes.
    pub fn ids(&self) -> &BTreeSet<ItemId> {
        &self.ids
    }
}

impl Command for RemoveItems {
    fn label(&self) -> &str {
        &self.label
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        self.captured.clear();
        for id in &self.ids {
            if let (Some(position), Some(item)) = (scene.position(*id), scene.item(*id)) {
                self.captured.push((position, item.clone()));
            }
        }
        self.captured.sort_by_key(|(position, _)| *position);
        for (position, _) in self.captured.iter().rev() {
            scene.remove_at(*position);
        }
        Effects {
            removed: self.ids.clone(),
            order: !self.ids.is_empty(),
            ..Effects::default()
        }
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        for (position, item) in &self.captured {
            scene.insert_item_at(*position, item.clone());
        }
        Effects {
            added: self.ids.clone(),
            order: !self.ids.is_empty(),
            ..Effects::default()
        }
    }

    impl_as_any!(RemoveItems);
}

/// Sets item transforms and mirror flags, storing both sides.
///
/// One command covers move, scale, rotate, flip, align and arrange: they are
/// all "these items now have these placements", and keeping one implementation
/// is what makes all of them undo correctly with the same tested code.
#[derive(Debug)]
pub struct SetTransforms {
    label: String,
    before: Vec<(ItemId, ItemState)>,
    after: Vec<(ItemId, ItemState)>,
    coalescing: bool,
}

impl SetTransforms {
    /// Builds a discrete command from explicit before and after states.
    pub fn new(
        label: impl Into<String>,
        before: Vec<(ItemId, ItemState)>,
        after: Vec<(ItemId, ItemState)>,
    ) -> Self {
        Self {
            label: label.into(),
            before,
            after,
            coalescing: false,
        }
    }

    /// Builds a discrete command from an after-state map; the before-states are
    /// captured on the first apply.
    ///
    /// Capturing at apply time keeps the caller's scene borrow free — the app
    /// builds the command and hands it straight to the history — and makes redo
    /// restore the placement from before the gesture, not from the previous
    /// gesture step. Discrete commands never merge: a scale followed by a
    /// rotate is two undo steps even when the mouse moves fast.
    pub fn from_after(label: impl Into<String>, after: Vec<(ItemId, ItemState)>) -> Self {
        Self {
            label: label.into(),
            before: Vec::new(),
            after,
            coalescing: false,
        }
    }

    /// Builds a command that may merge with the previous one inside the
    /// coalescing window, for key-repeat gestures such as arrow-key nudges.
    pub fn continuous(label: impl Into<String>, after: Vec<(ItemId, ItemState)>) -> Self {
        Self {
            label: label.into(),
            before: Vec::new(),
            after,
            coalescing: true,
        }
    }

    /// The command's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The items the command touches.
    pub fn ids(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.after.iter().map(|(id, _)| *id)
    }
}

impl Command for SetTransforms {
    fn label(&self) -> &str {
        &self.label
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        if self.before.len() != self.after.len() {
            self.before = self
                .after
                .iter()
                .filter_map(|(id, _)| scene.item(*id).map(|item| (*id, item.state())))
                .collect();
        }
        let mut effects = Effects::default();
        for (id, state) in &self.after {
            if let Some(item) = scene.item_mut(*id) {
                item.set_state(*state);
                effects.geometry.insert(*id);
            }
        }
        effects
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        let mut effects = Effects::default();
        for (id, state) in &self.before {
            if let Some(item) = scene.item_mut(*id) {
                item.set_state(*state);
                effects.geometry.insert(*id);
            }
        }
        effects
    }

    fn coalesce_key(&self) -> Option<CoalesceKey> {
        self.coalescing.then_some(CoalesceKey::Transform)
    }

    fn try_merge(&mut self, newer: &dyn Any) -> bool {
        let Some(newer) = newer.downcast_ref::<Self>() else {
            return false;
        };
        let same_items = self.after.len() == newer.after.len()
            && self
                .after
                .iter()
                .zip(&newer.after)
                .all(|((a, _), (b, _))| a == b);
        if same_items {
            self.after.clone_from(&newer.after);
            self.label.clone_from(&newer.label);
        }
        same_items
    }

    impl_as_any!(SetTransforms);
}

/// Reorders the whole stack, storing both orders.
#[derive(Debug)]
pub struct ReorderItems {
    label: String,
    before: Vec<ItemId>,
    after: Vec<ItemId>,
}

impl ReorderItems {
    /// Builds a reorder command from two full orders.
    pub fn new(label: impl Into<String>, before: Vec<ItemId>, after: Vec<ItemId>) -> Self {
        debug_assert_eq!(
            before.len(),
            after.len(),
            "a reorder must list every item exactly once"
        );
        Self {
            label: label.into(),
            before,
            after,
        }
    }

    /// The command's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl Command for ReorderItems {
    fn label(&self) -> &str {
        &self.label
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        scene.reorder(&self.after);
        Effects {
            order: true,
            ..Effects::default()
        }
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        scene.reorder(&self.before);
        Effects {
            order: true,
            ..Effects::default()
        }
    }

    impl_as_any!(ReorderItems);
}

/// Replaces one note's text.
#[derive(Debug)]
pub struct SetNoteText {
    item: ItemId,
    before: String,
    after: String,
}

impl SetNoteText {
    /// Builds a text command from both sides.
    pub fn new(item: ItemId, before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            item,
            before: before.into(),
            after: after.into(),
        }
    }

    /// The note the command edits.
    pub const fn item(&self) -> ItemId {
        self.item
    }
}

impl Command for SetNoteText {
    fn label(&self) -> &str {
        "Edit note"
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        if let Some(item) = scene.item_mut(self.item) {
            item.set_text(&self.after);
        }
        Effects {
            content: std::iter::once(self.item).collect(),
            ..Effects::default()
        }
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        if let Some(item) = scene.item_mut(self.item) {
            item.set_text(&self.before);
        }
        Effects {
            content: std::iter::once(self.item).collect(),
            ..Effects::default()
        }
    }

    fn coalesce_key(&self) -> Option<CoalesceKey> {
        Some(CoalesceKey::NoteText(self.item))
    }

    fn try_merge(&mut self, newer: &dyn Any) -> bool {
        let Some(newer) = newer.downcast_ref::<Self>() else {
            return false;
        };
        if newer.item != self.item {
            return false;
        }
        self.after.clone_from(&newer.after);
        true
    }

    impl_as_any!(SetNoteText);
}

// ---------------------------------------------------------------------------
// Group commands
// ---------------------------------------------------------------------------

/// Creates a group from existing members.
#[derive(Debug)]
pub struct AddGroup {
    group_id: GroupId,
    children: Vec<GroupChild>,
    detached: Vec<(GroupId, usize, GroupChild)>,
}

impl AddGroup {
    /// Creates group `group_id` containing `children`.
    pub fn new(group_id: GroupId, children: Vec<GroupChild>) -> Self {
        Self {
            group_id,
            children,
            detached: Vec::new(),
        }
    }

    /// The new group's id.
    pub const fn group_id(&self) -> GroupId {
        self.group_id
    }
}

impl Command for AddGroup {
    fn label(&self) -> &str {
        "Group"
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        self.detached = scene.attach_group(self.group_id, self.children.clone());
        Effects {
            groups: true,
            ..Effects::default()
        }
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        scene.remove_group(self.group_id);
        for (parent, index, child) in self.detached.drain(..).rev() {
            scene.attach_child(Some(parent), child, Some(index));
        }
        Effects {
            groups: true,
            ..Effects::default()
        }
    }

    impl_as_any!(AddGroup);
}

/// Dissolves a group one level, re-parenting its members to the group's parent.
#[derive(Debug)]
pub struct DissolveGroup {
    group_id: GroupId,
    group: Option<Group>,
    parent: Option<(GroupId, usize)>,
}

impl DissolveGroup {
    /// Dissolves `group_id`.
    pub const fn new(group_id: GroupId) -> Self {
        Self {
            group_id,
            group: None,
            parent: None,
        }
    }
}

impl Command for DissolveGroup {
    fn label(&self) -> &str {
        "Ungroup"
    }

    fn apply(&mut self, scene: &mut Scene) -> Effects {
        let parent = scene.groups().parent_of(GroupChild::Group(self.group_id));
        let slot = parent
            .and_then(|parent| {
                scene
                    .groups()
                    .position_of(parent, GroupChild::Group(self.group_id))
            })
            .unwrap_or(0);
        let Some(group) = scene.remove_group(self.group_id) else {
            return Effects::default();
        };
        for child in group.children() {
            scene.attach_child(parent, *child, None);
        }
        self.parent = parent.map(|parent| (parent, slot));
        self.group = Some(group);
        Effects {
            groups: true,
            ..Effects::default()
        }
    }

    fn revert(&mut self, scene: &mut Scene) -> Effects {
        let Some(group) = self.group.clone() else {
            return Effects::default();
        };
        for child in group.children() {
            scene.detach_child(*child);
        }
        scene.insert_group(group.clone());
        if let Some((parent, slot)) = self.parent {
            scene.attach_child(Some(parent), GroupChild::Group(group.id()), Some(slot));
        }
        Effects {
            groups: true,
            ..Effects::default()
        }
    }

    impl_as_any!(DissolveGroup);
}

#[cfg(test)]
mod tests {
    use super::{
        AddGroup, AddItems, CoalesceKey, Command, DissolveGroup, History, RemoveItems,
        ReorderItems, SetNoteText, SetTransforms,
    };
    use crate::group::{GroupChild, GroupId};
    use crate::item::{Item, ItemState};
    use crate::scene::{ItemId, Scene, SceneId};
    use crate::transform::Transform;
    use glam::Vec2;
    use std::time::{Duration, Instant};

    fn scene() -> Scene {
        Scene::new(SceneId::from_bytes([7; SceneId::BYTE_LEN]))
    }

    fn note(scene: &mut Scene, text: &str, x: f32) -> ItemId {
        scene.insert_note(text, Vec2::new(x, 0.0), Vec2::splat(10.0))
    }

    fn item_of(scene: &Scene, id: ItemId) -> Item {
        scene.item(id).expect("item exists").clone()
    }

    fn state_at(x: f32) -> ItemState {
        ItemState {
            transform: Transform::new(Vec2::new(x, 0.0), Vec2::splat(10.0)),
            flip_x: false,
            flip_y: false,
        }
    }

    #[test]
    fn add_and_remove_round_trip_through_undo() {
        let mut scene = scene();
        let mut history = History::default();
        let id = scene.allocate_item_id();
        let item = Item::note(id, "note", Vec2::ZERO, Vec2::splat(10.0));

        history.apply(&mut scene, Box::new(AddItems::on_top("Add", vec![item])));
        assert_eq!(scene.len(), 1);

        history.apply(&mut scene, Box::new(RemoveItems::new("Delete", [id])));
        assert!(scene.is_empty());

        history.undo(&mut scene).expect("undo the delete");
        assert_eq!(scene.len(), 1);
        history.undo(&mut scene).expect("undo the add");
        assert!(scene.is_empty());
    }

    #[test]
    fn undo_and_redo_move_the_board_back_and_forth() {
        let mut scene = scene();
        let mut history = History::default();
        let a = scene.allocate_item_id();
        let b = scene.allocate_item_id();

        history.apply(
            &mut scene,
            Box::new(AddItems::on_top(
                "Add",
                vec![
                    Item::note(a, "a", Vec2::ZERO, Vec2::splat(10.0)),
                    Item::note(b, "b", Vec2::splat(20.0), Vec2::splat(10.0)),
                ],
            )),
        );
        assert_eq!(scene.len(), 2);
        assert!(history.can_undo() && !history.can_redo());

        let (label, _) = history.undo(&mut scene).expect("undo");
        assert_eq!(label, "Add");
        assert!(scene.is_empty());
        assert!(history.can_redo());

        history.redo(&mut scene).expect("redo");
        assert_eq!(scene.len(), 2);
    }

    #[test]
    fn undo_on_an_empty_history_is_a_no_op() {
        let mut scene = scene();
        let mut history = History::default();

        assert!(history.undo(&mut scene).is_none());
        assert!(history.redo(&mut scene).is_none());
        assert!(scene.is_empty());
    }

    #[test]
    fn a_new_command_discards_the_redo_stack() {
        let mut scene = scene();
        let mut history = History::default();
        let id = scene.allocate_item_id();
        history.apply(
            &mut scene,
            Box::new(AddItems::on_top(
                "Add",
                vec![Item::note(id, "a", Vec2::ZERO, Vec2::splat(10.0))],
            )),
        );
        history.undo(&mut scene);
        assert_eq!(history.redo_len(), 1);

        let other = scene.allocate_item_id();
        history.apply(
            &mut scene,
            Box::new(AddItems::on_top(
                "Add",
                vec![Item::note(other, "b", Vec2::ZERO, Vec2::splat(10.0))],
            )),
        );

        assert_eq!(history.redo_len(), 0);
        assert!(!history.can_redo());
        assert_eq!(
            scene.items.iter().map(Item::id).collect::<Vec<_>>(),
            vec![other]
        );
    }

    #[test]
    fn the_oldest_command_is_dropped_at_the_limit() {
        let mut scene = scene();
        let mut history = History::new(2);

        for index in 0..3 {
            let id = scene.allocate_item_id();
            history.apply(
                &mut scene,
                Box::new(AddItems::on_top(
                    "Add",
                    vec![Item::note(
                        id,
                        "n",
                        Vec2::splat(index as f32 * 10.0),
                        Vec2::splat(10.0),
                    )],
                )),
            );
        }
        assert_eq!(history.limit(), 2);
        assert_eq!(history.undo_len(), 2);

        history.undo(&mut scene);
        history.undo(&mut scene);

        // The first command fell out of the history, so its item stays on the board.
        assert!(!history.can_undo());
        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn a_zero_limit_still_allows_one_undo() {
        assert_eq!(History::new(0).limit(), 1);
    }

    #[test]
    fn a_transform_undo_restores_the_exact_placement() {
        let mut scene = scene();
        let id = note(&mut scene, "a", 0.0);
        let before = item_of(&scene, id).state();
        let mut history = History::default();

        let after = state_at(50.0);
        history.apply(
            &mut scene,
            Box::new(SetTransforms::from_after("Move", vec![(id, after)])),
        );
        assert_eq!(scene.item(id).expect("item").center(), Vec2::new(50.0, 0.0));

        history.undo(&mut scene);
        assert_eq!(item_of(&scene, id).state(), before);
        history.redo(&mut scene);
        assert_eq!(scene.item(id).expect("item").center(), Vec2::new(50.0, 0.0));
    }

    #[test]
    fn repeated_transforms_coalesce_inside_the_window() {
        let mut scene = scene();
        let id = note(&mut scene, "a", 0.0);
        let mut history = History::default();
        let start = Instant::now();

        for step in 1..=5 {
            let after = state_at(step as f32);
            history.apply_at(
                &mut scene,
                Box::new(SetTransforms::continuous("Nudge", vec![(id, after)])),
                start + Duration::from_millis(step * 10),
            );
        }

        assert_eq!(history.undo_len(), 1, "one gesture, one undo step");
        history.undo(&mut scene);
        assert_eq!(item_of(&scene, id).center(), Vec2::ZERO);
        assert!(!history.can_undo());
    }

    #[test]
    fn a_pause_splits_the_gesture() {
        let mut scene = scene();
        let id = note(&mut scene, "a", 0.0);
        let mut history = History::default();
        let start = Instant::now();

        history.apply_at(
            &mut scene,
            Box::new(SetTransforms::continuous(
                "Nudge",
                vec![(id, state_at(1.0))],
            )),
            start,
        );
        history.apply_at(
            &mut scene,
            Box::new(SetTransforms::continuous(
                "Nudge",
                vec![(id, state_at(2.0))],
            )),
            start + History::COALESCE_WINDOW + Duration::from_millis(1),
        );

        assert_eq!(history.undo_len(), 2, "a pause starts a new undo step");
    }

    #[test]
    fn transforms_of_different_items_do_not_coalesce() {
        let mut scene = scene();
        let a = note(&mut scene, "a", 0.0);
        let b = note(&mut scene, "b", 100.0);
        let mut history = History::default();
        let now = Instant::now();

        history.apply_at(
            &mut scene,
            Box::new(SetTransforms::continuous("Move", vec![(a, state_at(10.0))])),
            now,
        );
        history.apply_at(
            &mut scene,
            Box::new(SetTransforms::continuous(
                "Move",
                vec![(b, state_at(110.0))],
            )),
            now + Duration::from_millis(10),
        );

        assert_eq!(history.undo_len(), 2);
    }

    #[test]
    fn note_text_coalesces_and_undo_restores_the_original() {
        let mut scene = scene();
        let id = note(&mut scene, "a", 0.0);
        let mut history = History::default();
        let now = Instant::now();

        for (step, text) in ["ab", "abc", "abcd"].iter().enumerate() {
            history.apply_at(
                &mut scene,
                Box::new(SetNoteText::new(id, "a", *text)),
                now + Duration::from_millis(step as u64 * 10),
            );
        }

        assert_eq!(scene.item(id).and_then(Item::text), Some("abcd"));
        assert_eq!(history.undo_len(), 1);
        history.undo(&mut scene);
        assert_eq!(scene.item(id).and_then(Item::text), Some("a"));
    }

    #[test]
    fn reorder_is_undoable() {
        let mut scene = scene();
        let a = note(&mut scene, "a", 0.0);
        let b = note(&mut scene, "b", 10.0);
        let c = note(&mut scene, "c", 20.0);
        let before: Vec<ItemId> = scene.items.iter().map(Item::id).collect();
        let mut history = History::default();
        let after = vec![c, a, b];

        history.apply(
            &mut scene,
            Box::new(ReorderItems::new("Bring to front", before.clone(), after)),
        );
        assert_eq!(
            scene.items.iter().map(Item::id).collect::<Vec<_>>(),
            vec![c, a, b]
        );

        history.undo(&mut scene);
        assert_eq!(scene.items.iter().map(Item::id).collect::<Vec<_>>(), before);
    }

    #[test]
    fn group_creation_and_dissolve_are_undoable() {
        let mut scene = scene();
        let a = note(&mut scene, "a", 0.0);
        let b = note(&mut scene, "b", 10.0);
        let group = scene.allocate_group_id();
        let mut history = History::default();

        history.apply(
            &mut scene,
            Box::new(AddGroup::new(
                group,
                vec![GroupChild::Item(a), GroupChild::Item(b)],
            )),
        );
        assert_eq!(scene.group_leaves(group), vec![a, b]);
        assert_eq!(
            scene.groups().top_group_of_item(a),
            Some(GroupId::from_raw(group.raw()))
        );

        history.apply(&mut scene, Box::new(DissolveGroup::new(group)));
        assert!(scene.group(group).is_none());
        assert_eq!(
            scene.groups().top_group_of_item(a),
            None,
            "dissolving frees the members"
        );

        history.undo(&mut scene);
        assert_eq!(scene.group_leaves(group), vec![a, b]);
        history.undo(&mut scene);
        assert!(scene.group(group).is_none());
    }

    #[test]
    fn dissolving_a_nested_group_restores_its_slot() {
        let mut scene = scene();
        let a = note(&mut scene, "a", 0.0);
        let b = note(&mut scene, "b", 10.0);
        let inner = scene.allocate_group_id();
        scene.attach_group(inner, vec![GroupChild::Item(a)]);
        let outer = scene.allocate_group_id();
        scene.attach_group(outer, vec![GroupChild::Item(b), GroupChild::Group(inner)]);
        let before = serde_json::to_vec(&scene).expect("serialises");

        let mut history = History::default();
        history.apply(&mut scene, Box::new(DissolveGroup::new(outer)));
        assert!(scene.group(outer).is_none(), "the outer group is gone");
        history.undo(&mut scene);

        assert_eq!(
            serde_json::to_vec(&scene).expect("serialises"),
            before,
            "undo puts the group back in its original slot"
        );
    }

    #[test]
    fn a_hundred_commands_undo_to_the_initial_bytes() {
        let mut scene = scene();
        let mut history = History::default();
        let ids: Vec<ItemId> = (0..10)
            .map(|index| note(&mut scene, "seed", index as f32 * 20.0))
            .collect();
        let before = serde_json::to_vec(&scene).expect("serialises");
        let now = Instant::now();

        for step in 0..100u64 {
            let id = ids[step as usize % ids.len()];
            let state = state_at(step as f32);
            history.apply_at(
                &mut scene,
                Box::new(SetTransforms::from_after("Move", vec![(id, state)])),
                now + Duration::from_millis(step * 500),
            );
        }
        assert_eq!(scene.items.len(), 10);

        while history.can_undo() {
            history.undo(&mut scene);
        }

        assert_eq!(
            serde_json::to_vec(&scene).expect("serialises"),
            before,
            "undoing everything reproduces the saved bytes"
        );
    }

    #[test]
    fn a_command_reports_its_label_and_coalesce_key() {
        let command = SetNoteText::new(ItemId::from_raw(3), "a", "b");
        assert_eq!(command.label(), "Edit note");
        assert_eq!(
            command.coalesce_key(),
            Some(CoalesceKey::NoteText(ItemId::from_raw(3)))
        );
    }
}
