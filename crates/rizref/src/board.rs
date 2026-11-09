//! The editing controller: selection, gestures and item commands.
//!
//! `Board` owns the scene, the selection and the undo history, and turns
//! pointer and keyboard input into commands. It is deliberately free of winit,
//! wgpu and wall-clock state: the app translates window events into calls with a
//! [`Camera`] and viewport, which is what lets the whole interaction — a drag, a
//! scale, a rotate, undo and redo — run as an ordinary unit test.
//!
//! A gesture edits the scene live so the user sees the drag, then commits one
//! [`SetTransforms`] command on release. That is why a drag is one undo step
//! even though it produced hundreds of intermediate placements.

use std::collections::BTreeSet;

use glam::{DVec2, Vec2};
use rizref_core::{
    AddGroup, AddItems, Align, ArrangeMode, Camera, DEFAULT_NOTE_SIZE, DissolveGroup, Effects,
    GroupChild, Handle, History, Item, ItemId, ItemState, NOTE_PADDING, NoteContent, NoteFormat,
    Orientation, RemoveItems, ReorderItems, Selection, SelectionFrame, SetNoteContent,
    SetTransforms, SizeAxis, align, arrange, distribute, scale_item, scale_item_in_frame,
    scale_rules, screen_tolerance, snap_rect,
};

/// Screen-pixel tolerance for edge/centre snapping.
pub const SNAP_PIXELS: f32 = 3.0;

/// Half-size in screen pixels of a scale handle's hit area.
pub const HANDLE_PIXELS: f32 = 7.0;

/// Gap in screen pixels between the frame's north edge and the rotate handle.
pub const ROTATE_HANDLE_PIXELS: f32 = 24.0;

/// Screen distance, in pixels, a press must travel before it becomes a drag.
pub const CLICK_SLOP_PIXELS: f64 = 3.0;

/// Screen distance a nudge moves the selection, in pixels.
pub const NUDGE_PIXELS: f32 = 1.0;

/// Screen distance a duplicate is offset by, in pixels.
pub const DUPLICATE_PIXELS: f32 = 16.0;

/// Gap used by arrange commands, in world units.
pub const ARRANGE_GAP: f32 = 24.0;

/// Shift-rotation step.
pub const ROTATE_STEP: f32 = std::f32::consts::PI / 12.0;

/// Keyboard modifiers as of one event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt.
    pub alt: bool,
}

/// The view a pointer event is interpreted through.
#[derive(Debug, Clone, Copy)]
pub struct ViewInfo<'a> {
    /// The camera.
    pub camera: &'a Camera,
    /// Viewport size in logical pixels.
    pub viewport: DVec2,
}

impl ViewInfo<'_> {
    /// Maps a viewport pixel to the world.
    pub fn world(&self, screen: DVec2) -> Vec2 {
        self.camera.screen_to_world(screen, self.viewport).as_vec2()
    }

    /// Maps a world point to a viewport pixel.
    pub fn screen(&self, world: Vec2) -> DVec2 {
        self.camera.world_to_screen(world.as_dvec2(), self.viewport)
    }

    /// Converts a screen distance in logical pixels to world units.
    pub fn pixels_to_world(&self, pixels: f32) -> f32 {
        pixels / self.camera.zoom as f32
    }

    /// Converts a screen-space delta to a world-space delta.
    pub fn screen_delta_to_world(&self, delta: DVec2) -> Vec2 {
        let (sin, cos) = self.camera.rotation.sin_cos();
        let zoom = self.camera.zoom;
        Vec2::new(
            ((cos * delta.x + sin * delta.y) / zoom) as f32,
            ((-sin * delta.x + cos * delta.y) / zoom) as f32,
        )
    }
}

/// What the pointer should look like over the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorHint {
    /// The default arrow.
    Default,
    /// Over a movable item.
    Move,
    /// A move drag is in flight.
    Grabbing,
    /// Over a selected item that can be dragged.
    Grab,
    /// Horizontal resize.
    ResizeEw,
    /// Vertical resize.
    ResizeNs,
    /// North-west / south-east resize.
    ResizeNwSe,
    /// North-east / south-west resize.
    ResizeNeSw,
    /// The rotate handle.
    Rotate,
    /// Over a note.
    Text,
    /// A marquee drag is in flight.
    Crosshair,
}

/// The in-flight pointer gesture.
#[derive(Debug)]
enum Gesture {
    Move {
        start_world: Vec2,
        start_screen: DVec2,
        bounds: rizref_core::Rect,
        targets: Vec<rizref_core::Rect>,
        before: Vec<(ItemId, ItemState)>,
        moved: bool,
    },
    Scale {
        handle: Handle,
        frame: SelectionFrame,
        before: Vec<(ItemId, ItemState)>,
        single: bool,
        /// Whether a note is being scaled: notes resize freely by default.
        has_note: bool,
        start_screen: DVec2,
        moved: bool,
    },
    Rotate {
        pivot: Vec2,
        last_angle: f32,
        accumulated: f32,
        before: Vec<(ItemId, ItemState)>,
        moved: bool,
    },
    Marquee {
        start_world: Vec2,
        base: BTreeSet<ItemId>,
    },
}

/// The editing controller for one board.
pub struct Board {
    scene: rizref_core::Scene,
    selection: Selection,
    history: History,
    rects: rizref_core::SceneRectTracker,
    gesture: Option<Gesture>,
    hover: Option<ItemId>,
    marquee: Option<rizref_core::Rect>,
    guides: Vec<rizref_core::Guide>,
    snap_enabled: bool,
    pending: Effects,
}

impl Board {
    /// A board editing `scene` with an empty selection and history.
    pub fn new(scene: rizref_core::Scene) -> Self {
        let mut rects = rizref_core::SceneRectTracker::new();
        for item in scene.items() {
            rects.merge(item.id(), item.world_rect());
        }
        Self {
            scene,
            selection: Selection::new(),
            history: History::default(),
            rects,
            gesture: None,
            hover: None,
            marquee: None,
            guides: Vec::new(),
            snap_enabled: true,
            pending: Effects::default(),
        }
    }

    /// The board.
    pub const fn scene(&self) -> &rizref_core::Scene {
        &self.scene
    }

    /// The board, for import paths that add assets and items outside the undo
    /// history (imports are not undo steps in this phase).
    pub const fn scene_mut(&mut self) -> &mut rizref_core::Scene {
        &mut self.scene
    }

    /// The current selection.
    pub const fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The undo history.
    #[cfg(test)]
    pub const fn history(&self) -> &History {
        &self.history
    }

    /// The item under the pointer, when the pointer is over one.
    pub const fn hover(&self) -> Option<ItemId> {
        self.hover
    }

    /// The smart guides of the current drag.
    pub fn guides(&self) -> &[rizref_core::Guide] {
        &self.guides
    }

    /// The marquee rectangle of the current drag.
    pub const fn marquee(&self) -> Option<rizref_core::Rect> {
        self.marquee
    }

    /// The fitted box around the selection.
    pub fn frame(&self) -> Option<SelectionFrame> {
        SelectionFrame::for_selection(&self.scene, &self.selection)
    }

    /// Whether a gesture is in flight.
    pub const fn is_gesture_active(&self) -> bool {
        self.gesture.is_some()
    }

    /// Enables or disables snapping (the settings toggle).
    pub fn set_snap_enabled(&mut self, enabled: bool) {
        self.snap_enabled = enabled;
    }

    /// The tracked bounding box of the board.
    pub fn scene_rect(&self) -> Option<rizref_core::Rect> {
        self.rects.rect()
    }

    /// The bounding box of the selection.
    pub fn selection_bounds(&self) -> Option<rizref_core::Rect> {
        self.frame().map(|frame| frame.world_rect())
    }

    /// Drains the effects accumulated since the last call.
    pub fn take_effects(&mut self) -> Effects {
        std::mem::take(&mut self.pending)
    }

    /// Rebuilds the scene-rect tracker, for imports made through [`Board::scene_mut`].
    pub fn refresh_rects(&mut self) {
        self.rebuild_rects();
    }

    fn rebuild_rects(&mut self) {
        self.rects.clear();
        for item in self.scene.items() {
            self.rects.merge(item.id(), item.world_rect());
        }
    }

    /// Folds a command's effects into the tracker and the pending set.
    fn absorb(&mut self, effects: &Effects) {
        if effects.structural() {
            self.rebuild_rects();
            self.selection.retain(|id| self.scene.contains(id));
        } else if !effects.geometry.is_empty() {
            for id in &effects.geometry {
                if let Some(item) = self.scene.item(*id) {
                    self.rects.merge(*id, item.world_rect());
                }
            }
        }
        self.pending.merge(effects);
    }

    /// Applies live states during a gesture, without touching the history.
    fn apply_states(&mut self, states: &[(ItemId, ItemState)]) {
        for (id, state) in states {
            if let Some(item) = self.scene.item_mut(*id) {
                item.set_state(*state);
            }
            if let Some(item) = self.scene.item(*id) {
                self.rects.merge(*id, item.world_rect());
            }
            self.pending.geometry.insert(*id);
        }
    }

    /// The current states of `ids`.
    fn current_states(&self, ids: &BTreeSet<ItemId>) -> Vec<(ItemId, ItemState)> {
        ids.iter()
            .filter_map(|id| self.scene.item(*id).map(|item| (*id, item.state())))
            .collect()
    }

    /// The states of the selection.
    fn selected_states(&self) -> Vec<(ItemId, ItemState)> {
        self.selection
            .iter()
            .filter_map(|id| self.scene.item(id).map(|item| (id, item.state())))
            .collect()
    }

    /// The topmost item under `world`.
    pub fn item_at(&self, world: Vec2) -> Option<ItemId> {
        self.scene
            .items()
            .iter()
            .rev()
            .find(|item| item.hit_test(world))
            .map(Item::id)
    }

    /// Pushes a command through the history and folds its effects in.
    fn run(&mut self, command: Box<dyn rizref_core::Command>) -> Effects {
        let effects = self.history.apply(&mut self.scene, command);
        self.absorb(&effects);
        effects
    }

    // -----------------------------------------------------------------------
    // Selection
    // -----------------------------------------------------------------------

    /// Selects every item.
    pub fn select_all(&mut self) {
        self.selection
            .set_all(self.scene.items().iter().map(Item::id));
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Selects exactly `id`, if it is on the board.
    pub fn select_only(&mut self, id: ItemId) {
        if self.scene.contains(id) {
            self.selection.set_single(id);
        }
    }

    // -----------------------------------------------------------------------
    // History
    // -----------------------------------------------------------------------

    /// Undoes the last command, returning its label.
    pub fn undo(&mut self) -> Option<String> {
        self.cancel_gesture();
        let (label, effects) = self.history.undo(&mut self.scene)?;
        self.absorb(&effects);
        Some(label)
    }

    /// Redoes the last undone command, returning its label.
    pub fn redo(&mut self) -> Option<String> {
        self.cancel_gesture();
        let (label, effects) = self.history.redo(&mut self.scene)?;
        self.absorb(&effects);
        Some(label)
    }

    fn cancel_gesture(&mut self) {
        self.gesture = None;
        self.marquee = None;
        self.guides.clear();
    }

    // -----------------------------------------------------------------------
    // Item commands
    // -----------------------------------------------------------------------

    /// Deletes the selected items.
    pub fn delete_selection(&mut self) -> bool {
        let ids: BTreeSet<ItemId> = self
            .selection
            .iter()
            .filter(|id| self.scene.contains(*id))
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.run(Box::new(RemoveItems::new("Delete", ids)));
        self.selection.clear();
        true
    }

    /// Duplicates the selection with a small screen-constant offset and selects
    /// the copies.
    pub fn duplicate_selection(&mut self, view: ViewInfo) -> BTreeSet<ItemId> {
        let offset = Vec2::splat(view.pixels_to_world(DUPLICATE_PIXELS));
        let sources: Vec<Item> = self
            .scene
            .items()
            .iter()
            .filter(|item| self.selection.contains(item.id()))
            .cloned()
            .collect();
        let mut copies = Vec::new();
        let mut ids = BTreeSet::new();
        for item in &sources {
            let id = self.scene.allocate_item_id();
            let mut copy = item.with_id(id);
            let state = ItemState {
                transform: item.transform().translated(offset),
                ..item.state()
            };
            copy.set_state(state);
            ids.insert(id);
            copies.push(copy);
        }
        if copies.is_empty() {
            return ids;
        }
        self.run(Box::new(AddItems::on_top("Duplicate", copies)));
        self.selection.set_all(ids.iter().copied());
        ids
    }

    /// Moves the selection by a screen-space nudge.
    ///
    /// Repeats coalesce in the history, so holding an arrow key is one undo
    /// step rather than one per repeat.
    pub fn nudge(&mut self, dx: i32, dy: i32, distance_pixels: f32, view: ViewInfo) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        let screen = DVec2::new(
            f64::from(dx) * f64::from(distance_pixels),
            f64::from(dy) * f64::from(distance_pixels),
        );
        let delta = view.screen_delta_to_world(screen);
        let after: Vec<(ItemId, ItemState)> = self
            .selected_states()
            .into_iter()
            .map(|(id, state)| {
                (
                    id,
                    ItemState {
                        transform: state.transform.translated(delta),
                        ..state
                    },
                )
            })
            .collect();
        self.run(Box::new(SetTransforms::continuous("Nudge", after)));
        true
    }

    /// Mirrors the selection horizontally or vertically.
    pub fn flip(&mut self, horizontal: bool) -> bool {
        let before = self.selected_states();
        if before.is_empty() {
            return false;
        }
        let after: Vec<(ItemId, ItemState)> = before
            .iter()
            .map(|(id, state)| {
                let mut state = *state;
                if horizontal {
                    state.flip_x = !state.flip_x;
                } else {
                    state.flip_y = !state.flip_y;
                }
                (*id, state)
            })
            .collect();
        self.run(Box::new(SetTransforms::new(
            if horizontal {
                "Flip horizontally"
            } else {
                "Flip vertically"
            },
            before,
            after,
        )));
        true
    }

    /// Restores rotation, mirroring and natural size while keeping the centre.
    pub fn reset_transform(&mut self) -> bool {
        let mut after = Vec::new();
        for (id, state) in self.selected_states() {
            let natural = self.natural_size(id);
            let mut transform = state.transform;
            transform.set_rotation(0.0);
            if let Some(natural) = natural {
                transform.set_size(natural);
            }
            after.push((
                id,
                ItemState {
                    transform,
                    flip_x: false,
                    flip_y: false,
                },
            ));
        }
        if after.is_empty() {
            return false;
        }
        self.run(Box::new(SetTransforms::from_after(
            "Reset transform",
            after,
        )));
        true
    }

    /// The on-board size an item returns to on a transform reset.
    fn natural_size(&self, id: ItemId) -> Option<Vec2> {
        let item = self.scene.item(id)?;
        match item.body() {
            rizref_core::ItemBody::Image { asset } => {
                let asset = self.scene.asset(*asset)?;
                Some(Vec2::new(
                    asset.pixel_width() as f32,
                    asset.pixel_height() as f32,
                ))
            }
            rizref_core::ItemBody::Note { .. } => Some(DEFAULT_NOTE_SIZE),
        }
    }

    /// Gives every selected item the selection's largest width or height.
    pub fn match_size(&mut self, axis: SizeAxis) -> bool {
        let before = self.selected_states();
        if before.len() < 2 {
            return false;
        }
        let target = before
            .iter()
            .map(|(_, state)| match axis {
                SizeAxis::Width => state.transform.size().x,
                SizeAxis::Height => state.transform.size().y,
            })
            .fold(0.0_f32, f32::max);
        let after: Vec<(ItemId, ItemState)> = before
            .iter()
            .map(|(id, state)| {
                let mut transform = state.transform;
                let mut size = transform.size();
                match axis {
                    SizeAxis::Width => size.x = target,
                    SizeAxis::Height => size.y = target,
                }
                transform.set_size(size);
                (
                    *id,
                    ItemState {
                        transform,
                        ..*state
                    },
                )
            })
            .collect();
        self.run(Box::new(SetTransforms::from_after(
            match axis {
                SizeAxis::Width => "Match width",
                SizeAxis::Height => "Match height",
            },
            after,
        )));
        true
    }

    /// Groups the selected items into a new group.
    pub fn group_selection(&mut self) -> bool {
        let members: BTreeSet<ItemId> = self
            .selection
            .iter()
            .filter(|id| self.scene.contains(*id))
            .collect();
        if members.len() < 2 {
            return false;
        }
        // Already exactly one group: grouping again would be a no-op.
        let common = self
            .scene
            .groups()
            .top_group_of_item(*members.iter().next().expect("non-empty"));
        if let Some(common) = common
            && self
                .scene
                .group_leaves(common)
                .into_iter()
                .collect::<BTreeSet<_>>()
                == members
        {
            return false;
        }
        let group = self.scene.allocate_group_id();
        let children = members.into_iter().map(GroupChild::Item).collect();
        self.run(Box::new(AddGroup::new(group, children)));
        true
    }

    /// Dissolves every group represented in the selection.
    pub fn ungroup_selection(&mut self) -> bool {
        let groups: BTreeSet<_> = self
            .selection
            .iter()
            .filter_map(|id| self.scene.groups().top_group_of_item(id))
            .collect();
        if groups.is_empty() {
            return false;
        }
        for group in groups {
            self.run(Box::new(DissolveGroup::new(group)));
        }
        true
    }

    /// Moves the selection one step towards the front.
    pub fn raise_selection(&mut self) -> bool {
        self.reorder_selection(Order::Raise)
    }

    /// Moves the selection one step towards the back.
    pub fn lower_selection(&mut self) -> bool {
        self.reorder_selection(Order::Lower)
    }

    /// Moves the selection to the front.
    pub fn front_selection(&mut self) -> bool {
        self.reorder_selection(Order::Front)
    }

    /// Moves the selection to the back.
    pub fn back_selection(&mut self) -> bool {
        self.reorder_selection(Order::Back)
    }

    fn reorder_selection(&mut self, order: Order) -> bool {
        let before: Vec<ItemId> = self.scene.items().iter().map(Item::id).collect();
        let selected: BTreeSet<ItemId> = self
            .selection
            .iter()
            .filter(|id| self.scene.contains(*id))
            .collect();
        if selected.is_empty() {
            return false;
        }
        let mut after = before.clone();
        match order {
            Order::Raise => {
                for index in (0..after.len().saturating_sub(1)).rev() {
                    if selected.contains(&after[index]) && !selected.contains(&after[index + 1]) {
                        after.swap(index, index + 1);
                    }
                }
            }
            Order::Lower => {
                for index in 1..after.len() {
                    if selected.contains(&after[index]) && !selected.contains(&after[index - 1]) {
                        after.swap(index, index - 1);
                    }
                }
            }
            Order::Front => {
                let mut rest: Vec<ItemId> = before
                    .iter()
                    .copied()
                    .filter(|id| !selected.contains(id))
                    .collect();
                rest.extend(before.iter().copied().filter(|id| selected.contains(id)));
                after = rest;
            }
            Order::Back => {
                let mut ordered: Vec<ItemId> = before
                    .iter()
                    .copied()
                    .filter(|id| selected.contains(id))
                    .collect();
                ordered.extend(before.iter().copied().filter(|id| !selected.contains(id)));
                after = ordered;
            }
        }
        if after == before {
            return false;
        }
        self.run(Box::new(ReorderItems::new(order.label(), before, after)));
        true
    }

    /// Arranges the selection.
    pub fn arrange_selection(&mut self, mode: ArrangeMode) -> bool {
        let rects = self.selected_rects();
        if rects.len() < 2 {
            return false;
        }
        let centers = arrange(&rects, mode, ARRANGE_GAP);
        let after: Vec<(ItemId, ItemState)> = centers
            .into_iter()
            .filter_map(|(id, center)| {
                self.scene.item(id).map(|item| {
                    let mut transform = item.transform();
                    transform.set_center(center);
                    (
                        id,
                        ItemState {
                            transform,
                            ..item.state()
                        },
                    )
                })
            })
            .collect();
        self.run(Box::new(SetTransforms::from_after("Arrange", after)));
        true
    }

    /// Aligns the selection to one edge or centre.
    pub fn align_selection(&mut self, mode: Align) -> bool {
        let rects = self.selected_rects();
        if rects.len() < 2 {
            return false;
        }
        let centers = align(&rects, mode);
        self.apply_centers("Align", centers)
    }

    /// Distributes the selection evenly.
    pub fn distribute_selection(&mut self, orientation: Orientation) -> bool {
        let rects = self.selected_rects();
        if rects.len() < 3 {
            return false;
        }
        let centers = distribute(&rects, orientation);
        self.apply_centers("Distribute", centers)
    }

    fn apply_centers(&mut self, label: &str, centers: Vec<(ItemId, Vec2)>) -> bool {
        let after: Vec<(ItemId, ItemState)> = centers
            .into_iter()
            .filter_map(|(id, center)| {
                self.scene.item(id).map(|item| {
                    let mut transform = item.transform();
                    transform.set_center(center);
                    (
                        id,
                        ItemState {
                            transform,
                            ..item.state()
                        },
                    )
                })
            })
            .collect();
        if after.is_empty() {
            return false;
        }
        self.run(Box::new(SetTransforms::from_after(label, after)));
        true
    }

    /// The selected items' world rectangles, in selection order.
    fn selected_rects(&self) -> Vec<(ItemId, rizref_core::Rect)> {
        self.selection
            .iter()
            .filter_map(|id| self.scene.item(id).map(|item| (id, item.world_rect())))
            .collect()
    }

    /// Creates an empty markdown note, selects it and returns its id — as a
    /// *provisional* item: it is on the board but not in the history yet.
    /// [`Self::commit_new_note`] turns it into one undo step; cancelling
    /// removes it with [`Self::discard_provisional_note`].
    pub fn create_provisional_note(&mut self, center: Vec2) -> ItemId {
        let id = self.scene.allocate_item_id();
        let item = Item::note(id, "", center, DEFAULT_NOTE_SIZE);
        let top = self.scene.items().len();
        self.scene.insert_item_at(top, item);
        self.pending.added.insert(id);
        self.pending.content.insert(id);
        self.selection.set_single(id);
        id
    }

    /// Removes a provisional note without touching the history.
    pub fn discard_provisional_note(&mut self, id: ItemId) -> bool {
        if self.scene.remove(id).is_none() {
            return false;
        }
        self.selection.remove(id);
        if self.hover == Some(id) {
            self.hover = None;
        }
        self.pending.removed.insert(id);
        true
    }

    /// Commits a provisional note: the live item is replaced by the same
    /// item inserted through `AddItems`, so create + first text are one undo
    /// step. An empty note is discarded instead and adds nothing.
    pub fn commit_new_note(&mut self, id: ItemId, content: &NoteContent) -> bool {
        let Some(mut item) = self.scene.item(id).cloned() else {
            return false;
        };
        self.discard_provisional_note(id);
        if content.text.trim().is_empty() {
            return false;
        }
        item.set_note(content);
        self.run(Box::new(AddItems::on_top("Add note", vec![item])));
        self.selection.set_single(id);
        true
    }

    /// Writes note content during an edit session, outside the history.
    pub fn set_note_live(&mut self, id: ItemId, content: &NoteContent) -> bool {
        let changed = self
            .scene
            .item_mut(id)
            .is_some_and(|item| item.set_note(content));
        if changed {
            self.pending.content.insert(id);
        }
        changed
    }

    /// Restores a note's pre-edit content outside the history (cancel).
    pub fn restore_note(&mut self, id: ItemId, content: &NoteContent) -> bool {
        self.set_note_live(id, content)
    }

    /// Commits an edit session as one undo step.
    pub fn commit_note(&mut self, id: ItemId, before: &NoteContent, after: &NoteContent) -> bool {
        if before == after {
            return false;
        }
        self.run(Box::new(SetNoteContent::new(
            id,
            before.clone(),
            after.clone(),
        )));
        true
    }

    /// Replaces a note's content as one undo step (background, reflow).
    pub fn set_note_content(&mut self, id: ItemId, after: &NoteContent) -> bool {
        let Some(before) = self.scene.item(id).and_then(Item::note_content) else {
            return false;
        };
        self.commit_note(id, &before, after)
    }

    /// The single selected note, when the selection is exactly one note.
    pub fn selected_note(&self) -> Option<ItemId> {
        let mut ids = self.selection.iter();
        let id = ids.next()?;
        (ids.next().is_none() && self.scene.item(id).is_some_and(Item::is_note)).then_some(id)
    }

    /// The single selected image, when the selection is exactly one image.
    ///
    /// Notes, empty selections and multi-selections (including a group of
    /// images) yield `None`: the provenance commands act on one image only.
    pub fn selected_image(&self) -> Option<ItemId> {
        let mut ids = self.selection.iter();
        let id = ids.next()?;
        (ids.next().is_none()
            && self
                .scene
                .item(id)
                .is_some_and(|item| item.asset().is_some()))
        .then_some(id)
    }

    /// Sets a note's wrap width to its current inner box width: one undo
    /// step, and the only way line breaks follow a resize.
    pub fn reflow_note(&mut self, id: ItemId) -> bool {
        let Some(item) = self.scene.item(id) else {
            return false;
        };
        let Some(mut content) = item.note_content() else {
            return false;
        };
        content.wrap_width = Some(rizref_core::inner_note_width(item.size().x));
        if content.format == NoteFormat::Plain {
            content.format = NoteFormat::Markdown;
        }
        self.set_note_content(id, &content)
    }

    /// Sets a note's box height to fit `content_height` world units of laid
    /// out content (plus padding), keeping its top edge and width: one
    /// undoable transform.
    pub fn fit_note_height(&mut self, id: ItemId, content_height: f32) -> bool {
        let Some(item) = self.scene.item(id) else {
            return false;
        };
        let before = item.state();
        let height = (content_height + 2.0 * NOTE_PADDING).max(rizref_core::MIN_ITEM_SIZE);
        let old = before.transform.size();
        if (old.y - height).abs() < 0.01 {
            return false;
        }
        let mut transform = before.transform;
        // Grow or shrink downwards along the note's own vertical axis.
        let (sin, cos) = transform.rotation().sin_cos();
        let shift = (height - old.y) * 0.5;
        let down = Vec2::new(-sin, cos) * if before.flip_y { -shift } else { shift };
        transform.set_center(transform.center() + down);
        transform.set_size(Vec2::new(old.x, height));
        let after = ItemState {
            transform,
            ..before
        };
        self.run(Box::new(SetTransforms::new(
            "Fit note to content",
            vec![(id, before)],
            vec![(id, after)],
        )));
        true
    }

    /// Whether a selection-frame handle (scale, or rotate when allowed) is
    /// under `screen`.
    pub fn handle_at(&self, screen: DVec2, view: ViewInfo) -> bool {
        let world = view.world(screen);
        let rotate_offset = view.pixels_to_world(ROTATE_HANDLE_PIXELS);
        let radius = view.pixels_to_world(HANDLE_PIXELS);
        self.frame().is_some_and(|frame| {
            frame
                .hit(world, radius, rotate_offset)
                .is_some_and(|handle| handle != Handle::Rotate || self.selection_rotatable())
        })
    }

    /// Whether the selection has anything the rotate gesture may turn.
    /// Notes are never rotated, so a note-only selection has no rotate
    /// handle.
    pub fn selection_rotatable(&self) -> bool {
        self.selection
            .iter()
            .any(|id| self.scene.item(id).is_some_and(|item| !item.is_note()))
    }

    // -----------------------------------------------------------------------
    // Pointer gestures
    // -----------------------------------------------------------------------

    /// The snap targets for a move: every unselected item, plus the board's
    /// bounding box when there is something else on the board.
    fn snap_targets(&self, moving: &[(ItemId, ItemState)]) -> Vec<rizref_core::Rect> {
        let moving_ids: BTreeSet<ItemId> = moving.iter().map(|(id, _)| *id).collect();
        let mut targets: Vec<rizref_core::Rect> = self
            .scene
            .items()
            .iter()
            .filter(|item| !moving_ids.contains(&item.id()))
            .map(Item::world_rect)
            .collect();
        if !targets.is_empty()
            && let Some(rect) = self.rects.rect()
        {
            targets.push(rect);
        }
        targets
    }

    /// Presses the pointer.
    pub fn pointer_down(&mut self, screen: DVec2, view: ViewInfo, mods: Modifiers) -> bool {
        let world = view.world(screen);
        let rotate_offset = view.pixels_to_world(ROTATE_HANDLE_PIXELS);
        let radius = view.pixels_to_world(HANDLE_PIXELS);

        if let Some(frame) = self.frame()
            && let Some(handle) = frame
                .hit(world, radius, rotate_offset)
                .filter(|handle| *handle != Handle::Rotate || self.selection_rotatable())
        {
            let before = self.selected_states();
            self.hover = None;
            self.gesture = Some(if handle == Handle::Rotate {
                let offset = world - frame.center;
                Gesture::Rotate {
                    pivot: frame.center,
                    last_angle: offset.y.atan2(offset.x),
                    accumulated: 0.0,
                    before,
                    moved: false,
                }
            } else {
                let has_note = before
                    .iter()
                    .any(|(id, _)| self.scene.item(*id).is_some_and(Item::is_note));
                Gesture::Scale {
                    handle,
                    frame,
                    before,
                    single: self.selection.len() == 1,
                    has_note,
                    start_screen: screen,
                    moved: false,
                }
            });
            return true;
        }

        if let Some(id) = self.item_at(world) {
            if mods.shift {
                let members: Vec<ItemId> = self.scene.expand_item(id).into_iter().collect();
                self.selection.toggle_all(members);
                self.hover = Some(id);
                return true;
            }
            if !self.selection.contains(id) {
                let members: Vec<ItemId> = self.scene.expand_item(id).into_iter().collect();
                self.selection.set_all(members);
            }
            let before = self.selected_states();
            let bounds = before
                .iter()
                .map(|(_, state)| state.transform.world_rect())
                .reduce(|a, b| a.union(&b))
                .unwrap_or(rizref_core::Rect::from_center_size(world, Vec2::ZERO));
            let targets = self.snap_targets(&before);
            self.hover = Some(id);
            self.gesture = Some(Gesture::Move {
                start_world: world,
                start_screen: screen,
                bounds,
                targets,
                before,
                moved: false,
            });
            return true;
        }

        if !mods.shift {
            self.selection.clear();
        }
        self.hover = None;
        self.gesture = Some(Gesture::Marquee {
            start_world: world,
            base: self.selection.iter().collect(),
        });
        true
    }

    /// Moves the pointer.
    pub fn pointer_move(&mut self, screen: DVec2, view: ViewInfo, mods: Modifiers) -> bool {
        let world = view.world(screen);
        let Some(mut gesture) = self.gesture.take() else {
            let hover = self.item_at(world);
            let changed = hover != self.hover;
            self.hover = hover;
            return changed;
        };

        let changed = match &mut gesture {
            Gesture::Move {
                start_world,
                start_screen,
                bounds,
                targets,
                before,
                moved,
            } => {
                if (screen - *start_screen).length() > CLICK_SLOP_PIXELS {
                    *moved = true;
                }
                if *moved {
                    let mut delta = world - *start_world;
                    self.guides.clear();
                    if self.snap_enabled && !mods.ctrl {
                        let snap = snap_rect(
                            bounds.translated(delta),
                            targets,
                            screen_tolerance(SNAP_PIXELS, view.camera.zoom),
                        );
                        delta += snap.offset;
                        self.guides = snap.guides;
                    }
                    let states: Vec<(ItemId, ItemState)> = before
                        .iter()
                        .map(|(id, state)| {
                            (
                                *id,
                                ItemState {
                                    transform: state.transform.translated(delta),
                                    ..*state
                                },
                            )
                        })
                        .collect();
                    self.apply_states(&states);
                }
                *moved
            }
            Gesture::Scale {
                handle,
                frame,
                before,
                single,
                has_note,
                start_screen,
                moved,
            } => {
                if (screen - *start_screen).length() > CLICK_SLOP_PIXELS {
                    *moved = true;
                }
                if *moved {
                    let (keep_aspect, pivot) =
                        scale_rules(*handle, mods.alt, mods.shift, *has_note);
                    let states: Vec<(ItemId, ItemState)> = if *single {
                        before
                            .iter()
                            .map(|(id, state)| {
                                (
                                    *id,
                                    ItemState {
                                        transform: scale_item(
                                            state.transform,
                                            *handle,
                                            world,
                                            keep_aspect,
                                            pivot,
                                        ),
                                        ..*state
                                    },
                                )
                            })
                            .collect()
                    } else {
                        let new_frame = frame.scaled(*handle, world, keep_aspect, pivot);
                        before
                            .iter()
                            .map(|(id, state)| {
                                (
                                    *id,
                                    ItemState {
                                        transform: scale_item_in_frame(
                                            state.transform,
                                            *frame,
                                            new_frame,
                                        ),
                                        ..*state
                                    },
                                )
                            })
                            .collect()
                    };
                    self.guides.clear();
                    self.apply_states(&states);
                }
                *moved
            }
            Gesture::Rotate {
                pivot,
                last_angle,
                accumulated,
                before,
                moved,
            } => {
                let offset = world - *pivot;
                let angle = offset.y.atan2(offset.x);
                let mut delta = angle - *last_angle;
                // Wrap to (-pi, pi] so crossing the atan2 seam does not jump.
                delta = (delta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                *last_angle = angle;
                *accumulated += delta;
                if accumulated.abs() > 0.003 {
                    *moved = true;
                }
                if *moved {
                    let applied = if mods.shift {
                        (*accumulated / ROTATE_STEP).round() * ROTATE_STEP
                    } else {
                        *accumulated
                    };
                    // Notes are never rotated: in a mixed selection they
                    // keep their exact state while the images turn.
                    let states: Vec<(ItemId, ItemState)> = before
                        .iter()
                        .map(|(id, state)| {
                            let note = self.scene.item(*id).is_some_and(Item::is_note);
                            let transform = if note {
                                state.transform
                            } else {
                                state.transform.rotated_about(*pivot, applied)
                            };
                            (
                                *id,
                                ItemState {
                                    transform,
                                    ..*state
                                },
                            )
                        })
                        .collect();
                    self.guides.clear();
                    self.apply_states(&states);
                }
                *moved
            }
            Gesture::Marquee { start_world, base } => {
                let rect = rizref_core::Rect::from_min_max(*start_world, world);
                self.marquee = Some(rect);
                let mut selected = base.clone();
                for item in self.scene.items() {
                    if item.transform().intersects_rect(rect) {
                        selected.extend(self.scene.expand_item(item.id()));
                    }
                }
                self.selection.set_all(selected);
                true
            }
        };
        self.gesture = Some(gesture);
        changed
    }

    /// Releases the pointer, committing a gesture as one command.
    pub fn pointer_up(&mut self, _screen: DVec2, _view: ViewInfo, _mods: Modifiers) -> bool {
        let Some(gesture) = self.gesture.take() else {
            return false;
        };
        self.guides.clear();
        self.marquee = None;
        let committed = match gesture {
            Gesture::Move { moved, before, .. } if moved => {
                let ids: BTreeSet<ItemId> = before.iter().map(|(id, _)| *id).collect();
                Some(("Move", before, self.current_states(&ids)))
            }
            Gesture::Scale { moved, before, .. } if moved => {
                let ids: BTreeSet<ItemId> = before.iter().map(|(id, _)| *id).collect();
                Some(("Scale", before, self.current_states(&ids)))
            }
            Gesture::Rotate { moved, before, .. } if moved => {
                let ids: BTreeSet<ItemId> = before.iter().map(|(id, _)| *id).collect();
                Some(("Rotate", before, self.current_states(&ids)))
            }
            _ => None,
        };
        if let Some((label, before, after)) = committed {
            self.run(Box::new(SetTransforms::new(label, before, after)));
            true
        } else {
            false
        }
    }

    /// Clears the hover highlight when the pointer leaves the window.
    pub fn pointer_leave(&mut self) -> bool {
        let changed = self.hover.is_some();
        self.hover = None;
        changed
    }

    /// What the cursor should look like at `screen`.
    pub fn cursor(&self, screen: DVec2, view: ViewInfo, _mods: Modifiers) -> CursorHint {
        let rotate_offset = view.pixels_to_world(ROTATE_HANDLE_PIXELS);
        let radius = view.pixels_to_world(HANDLE_PIXELS);
        let world = view.world(screen);

        if let Some(gesture) = &self.gesture {
            return match gesture {
                Gesture::Move { .. } => CursorHint::Grabbing,
                Gesture::Scale { handle, frame, .. } => resize_cursor(*handle, frame, view),
                Gesture::Rotate { .. } => CursorHint::Rotate,
                Gesture::Marquee { .. } => CursorHint::Crosshair,
            };
        }

        if let Some(frame) = self.frame()
            && let Some(handle) = frame
                .hit(world, radius, rotate_offset)
                .filter(|handle| *handle != Handle::Rotate || self.selection_rotatable())
        {
            return resize_cursor(handle, &frame, view);
        }

        match self.item_at(world) {
            Some(id) => {
                if self.scene.item(id).is_some_and(Item::is_note) {
                    CursorHint::Text
                } else if self.selection.contains(id) {
                    CursorHint::Grab
                } else {
                    CursorHint::Move
                }
            }
            None => CursorHint::Default,
        }
    }
}

/// Which way a reorder moves the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Raise,
    Lower,
    Front,
    Back,
}

impl Order {
    const fn label(self) -> &'static str {
        match self {
            Self::Raise => "Bring forward",
            Self::Lower => "Send backward",
            Self::Front => "Bring to front",
            Self::Back => "Send to back",
        }
    }
}

/// Maps a handle and its on-screen direction to the matching resize cursor.
fn resize_cursor(handle: Handle, frame: &SelectionFrame, view: ViewInfo) -> CursorHint {
    if handle == Handle::Rotate {
        return CursorHint::Rotate;
    }
    let center = view.screen(frame.center);
    let point = view.screen(frame.handle_position(handle, 0.0));
    let angle = (point - center).y.atan2((point - center).x).to_degrees();
    let angle = angle.rem_euclid(180.0);
    if angle < 22.5 || angle >= 157.5 {
        CursorHint::ResizeEw
    } else if angle < 67.5 {
        CursorHint::ResizeNwSe
    } else if angle < 112.5 {
        CursorHint::ResizeNs
    } else {
        CursorHint::ResizeNeSw
    }
}

#[cfg(test)]
mod tests {
    use super::{Board, CLICK_SLOP_PIXELS, Modifiers, SNAP_PIXELS, ViewInfo};
    use glam::{DVec2, Vec2};
    use rizref_core::{
        ArrangeMode, Camera, Item, ItemId, NOTE_PADDING, NoteContent, Scene, SceneId,
        SelectionFrame, Transform,
    };
    use std::f32::consts::FRAC_PI_2;

    fn view() -> Camera {
        Camera::default()
    }

    fn viewport() -> DVec2 {
        DVec2::new(1000.0, 800.0)
    }

    fn info(camera: &Camera) -> ViewInfo<'_> {
        ViewInfo {
            camera,
            viewport: viewport(),
        }
    }

    fn screen(camera: &Camera, world: Vec2) -> DVec2 {
        camera.world_to_screen(world.as_dvec2(), viewport())
    }

    /// A shared 20 x 20 image asset.
    fn image_asset(scene: &mut Scene) -> rizref_core::AssetId {
        let pixels = rizref_codecs::ImageBuffer::filled(20, 20, [90, 90, 90, 255]);
        let asset = rizref_core::Asset::from_decoded(
            std::sync::Arc::from(&b"test image"[..]),
            rizref_codecs::ImageFormat::Png,
            &pixels,
            rizref_codecs::ImageBuffer::filled(2, 2, [90, 90, 90, 255]),
        );
        scene.insert_asset(asset).0
    }

    /// A board of `count` images in a grid, ids 1..=count, 20 x 20 each.
    /// Images rather than notes: notes are never rotated, and the scripted
    /// session rotates.
    fn board(count: usize) -> Board {
        let mut scene = Scene::new(SceneId::from_bytes([5; 16]));
        let asset = image_asset(&mut scene);
        let columns = 5;
        for index in 0..count {
            let column = (index % columns) as f32;
            let row = (index / columns) as f32;
            scene
                .insert_image(
                    asset,
                    Vec2::new(column * 40.0, row * 40.0),
                    Vec2::splat(20.0),
                )
                .expect("asset stored");
        }
        Board::new(scene)
    }

    fn item(board: &Board, raw: u64) -> &Item {
        board
            .scene()
            .item(ItemId::from_raw(raw))
            .expect("item exists")
    }

    fn state_signature(board: &Board) -> Vec<(u64, [f32; 6])> {
        board
            .scene()
            .items()
            .iter()
            .map(|item| {
                (
                    item.id().raw(),
                    [
                        item.center().x,
                        item.center().y,
                        item.size().x,
                        item.size().y,
                        item.rotation(),
                        item.opacity(),
                    ],
                )
            })
            .collect()
    }

    #[test]
    fn a_scripted_twenty_item_session_drags_scales_rotates_undoes_and_redoes() {
        let camera = view();
        let view = info(&camera);
        let mut board = board(20);
        board.set_snap_enabled(false);
        board.select_all();
        assert_eq!(board.selection().len(), 20);
        let initial = state_signature(&board);

        // --- Drag the whole board by (30, 40) --------------------------------
        let start = Vec2::new(0.0, 0.0);
        board.pointer_down(screen(&camera, start), view, Modifiers::default());
        board.pointer_move(
            screen(&camera, Vec2::new(30.0, 40.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(30.0, 40.0)),
            view,
            Modifiers::default(),
        );
        assert_eq!(board.history().undo_len(), 1, "the drag is one command");
        for item in board.scene().items() {
            let expected = Vec2::new(
                initial[item.id().raw() as usize - 1].1[0],
                initial[item.id().raw() as usize - 1].1[1],
            ) + Vec2::new(30.0, 40.0);
            assert_eq!(item.center(), expected);
        }

        // --- Scale the first item to twice its size ---------------------------
        // Click empty space to clear the selection, then the item.
        board.pointer_down(
            screen(&camera, Vec2::new(2_000.0, 2_000.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(2_000.0, 2_000.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_down(
            screen(&camera, Vec2::new(30.0, 40.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(30.0, 40.0)),
            view,
            Modifiers::default(),
        );
        assert_eq!(board.selection().len(), 1);

        let frame = board.frame().expect("selection frame");
        let se = frame.handle_position(rizref_core::Handle::Se, 0.0);
        board.pointer_down(screen(&camera, se), view, Modifiers::default());
        let target = frame.center + (se - frame.center) * 2.0;
        // Alt scales about the centre (the default anchors the opposite corner).
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        board.pointer_move(screen(&camera, target), view, alt);
        board.pointer_up(screen(&camera, target), view, alt);
        let scaled = item(&board, 1);
        assert_eq!(scaled.size(), Vec2::splat(40.0));
        assert_eq!(scaled.center(), Vec2::new(30.0, 40.0));
        assert_eq!(board.history().undo_len(), 2, "the scale is one command");

        // --- Rotate it a quarter turn ----------------------------------------
        let frame = board.frame().expect("selection frame");
        let rotate_offset = super::ROTATE_HANDLE_PIXELS;
        let rotate_handle = frame.handle_position(rizref_core::Handle::Rotate, rotate_offset);
        board.pointer_down(screen(&camera, rotate_handle), view, Modifiers::default());
        let target = frame.center + Vec2::new(frame.size.x, 0.0);
        board.pointer_move(screen(&camera, target), view, Modifiers::default());
        board.pointer_up(screen(&camera, target), view, Modifiers::default());
        assert!((item(&board, 1).rotation() - FRAC_PI_2).abs() < 1.0e-3);
        assert_eq!(item(&board, 1).center(), Vec2::new(30.0, 40.0));
        assert_eq!(board.history().undo_len(), 3, "the rotate is one command");

        let final_state = state_signature(&board);
        assert_ne!(final_state, initial);

        // --- Undo everything --------------------------------------------------
        for _ in 0..3 {
            board.undo().expect("a command to undo");
        }
        assert_eq!(state_signature(&board), initial);

        // --- Redo everything --------------------------------------------------
        for _ in 0..3 {
            board.redo().expect("a command to redo");
        }
        assert_eq!(state_signature(&board), final_state);
    }

    #[test]
    fn clicking_selects_the_topmost_item_and_shift_toggles() {
        let mut scene = Scene::new(SceneId::from_bytes([9; 16]));
        let bottom = scene.insert_note("bottom", Vec2::ZERO, Vec2::splat(40.0));
        let top = scene.insert_note("top", Vec2::ZERO, Vec2::splat(40.0));
        let mut board = Board::new(scene);
        let camera = view();
        let view = info(&camera);

        board.pointer_down(screen(&camera, Vec2::ZERO), view, Modifiers::default());
        board.pointer_up(screen(&camera, Vec2::ZERO), view, Modifiers::default());
        assert_eq!(
            board.selection().iter().collect::<Vec<_>>(),
            vec![top],
            "the last item in z-order wins"
        );
        assert!(!board.selection().contains(bottom));

        board.pointer_down(
            screen(&camera, Vec2::ZERO),
            view,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert!(
            board.selection().is_empty(),
            "shift-click removed the top item"
        );
    }

    #[test]
    fn a_marquee_selects_every_item_it_touches_and_respects_rotation() {
        let mut scene = Scene::new(SceneId::from_bytes([10; 16]));
        let straight = scene.insert_note("a", Vec2::new(100.0, 0.0), Vec2::splat(20.0));
        let rotated = scene.insert_note("b", Vec2::new(200.0, 0.0), Vec2::new(20.0, 100.0));
        if let Some(item) = scene.item_mut(rotated) {
            let mut transform = item.transform();
            transform.set_rotation(std::f32::consts::FRAC_PI_4);
            item.set_transform(transform);
        }
        let far = scene.insert_note("c", Vec2::new(1000.0, 0.0), Vec2::splat(20.0));
        let mut board = Board::new(scene);
        let camera = view();
        let view = info(&camera);

        // A box that only reaches the rotated item along its diagonal.
        board.pointer_down(
            screen(&camera, Vec2::new(150.0, 40.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_move(
            screen(&camera, Vec2::new(220.0, 60.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(220.0, 60.0)),
            view,
            Modifiers::default(),
        );

        assert!(board.selection().contains(rotated));
        assert!(!board.selection().contains(straight));
        assert!(!board.selection().contains(far));
    }

    #[test]
    fn snapping_aligns_an_edge_without_overshooting() {
        let mut scene = Scene::new(SceneId::from_bytes([11; 16]));
        scene.insert_note("target", Vec2::new(100.0, 0.0), Vec2::splat(20.0));
        let moving = scene.insert_note("moving", Vec2::new(-100.0, 0.0), Vec2::splat(20.0));
        let mut board = Board::new(scene);
        let camera = view();
        let view = info(&camera);

        board.pointer_down(
            screen(&camera, Vec2::new(-100.0, 0.0)),
            view,
            Modifiers::default(),
        );
        // Drop the moving item two units short of the target, so its right
        // edge is two units from the target's centre line.
        let desired = Vec2::new(98.0, 0.0);
        board.pointer_move(screen(&camera, desired), view, Modifiers::default());
        assert!(!board.guides().is_empty(), "the drag shows its guide");
        board.pointer_up(screen(&camera, desired), view, Modifiers::default());

        let moved = item(&board, moving.raw());
        assert_eq!(
            moved.center().x,
            100.0,
            "the edge lands exactly on the target centre line"
        );
    }

    #[test]
    fn ctrl_disables_snapping() {
        let mut scene = Scene::new(SceneId::from_bytes([12; 16]));
        scene.insert_note("target", Vec2::new(100.0, 0.0), Vec2::splat(20.0));
        let moving = scene.insert_note("moving", Vec2::new(-100.0, 0.0), Vec2::splat(20.0));
        let mut board = Board::new(scene);
        let camera = view();
        let view = info(&camera);

        board.pointer_down(
            screen(&camera, Vec2::new(-100.0, 0.0)),
            view,
            Modifiers::default(),
        );
        let desired = Vec2::new(12.0, 0.0);
        board.pointer_move(
            screen(&camera, desired),
            view,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        board.pointer_up(screen(&camera, desired), view, Modifiers::default());

        assert_eq!(item(&board, moving.raw()).center(), desired);
        assert!(board.guides().is_empty());
    }

    #[test]
    fn resize_cursors_follow_the_handle_direction() {
        let camera = view();
        let view = info(&camera);
        let frame = SelectionFrame::new(Vec2::ZERO, Vec2::splat(100.0), 0.0);

        assert_eq!(
            super::resize_cursor(rizref_core::Handle::E, &frame, view),
            super::CursorHint::ResizeEw
        );
        assert_eq!(
            super::resize_cursor(rizref_core::Handle::N, &frame, view),
            super::CursorHint::ResizeNs
        );
        assert_eq!(
            super::resize_cursor(rizref_core::Handle::Nw, &frame, view),
            super::CursorHint::ResizeNwSe
        );
        assert_eq!(
            super::resize_cursor(rizref_core::Handle::Ne, &frame, view),
            super::CursorHint::ResizeNeSw
        );
    }

    #[test]
    fn duplicate_offsets_the_copies_and_selects_them() {
        let mut board = board(2);
        let camera = view();
        let view = info(&camera);
        board.select_all();
        let originals: Vec<Vec2> = board.scene().items().iter().map(Item::center).collect();

        let copies = board.duplicate_selection(view);

        assert_eq!(copies.len(), 2);
        assert_eq!(
            board.selection().iter().collect::<Vec<_>>(),
            copies.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(board.scene().len(), 4);
        let offset = Vec2::splat(view.pixels_to_world(super::DUPLICATE_PIXELS));
        for (id, original) in copies.iter().zip(&originals) {
            assert_eq!(item(&board, id.raw()).center(), *original + offset);
        }
        board.undo();
        assert_eq!(board.scene().len(), 2);
    }

    #[test]
    fn delete_and_undo_restore_the_board() {
        let mut board = board(3);
        let initial = state_signature(&board);
        board.select_all();

        assert!(board.delete_selection());
        assert!(board.scene().is_empty());
        assert!(board.selection().is_empty());

        board.undo();
        assert_eq!(state_signature(&board), initial);
    }

    #[test]
    fn grouping_makes_one_click_select_the_whole_group() {
        let mut board = board(3);
        let camera = view();
        let view = info(&camera);
        board.pointer_down(screen(&camera, Vec2::ZERO), view, Modifiers::default());
        board.pointer_up(screen(&camera, Vec2::ZERO), view, Modifiers::default());
        board.pointer_down(
            screen(&camera, Vec2::new(40.0, 0.0)),
            view,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(board.selection().len(), 2);
        assert!(board.group_selection());

        board.clear_selection();
        board.pointer_down(
            screen(&camera, Vec2::new(40.0, 0.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(40.0, 0.0)),
            view,
            Modifiers::default(),
        );
        assert_eq!(
            board.selection().len(),
            2,
            "clicking one member selects both"
        );

        board.pointer_down(
            screen(&camera, Vec2::new(40.0, 0.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_move(
            screen(&camera, Vec2::new(60.0, 20.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(60.0, 20.0)),
            view,
            Modifiers::default(),
        );
        assert_eq!(item(&board, 1).center(), Vec2::new(20.0, 20.0));
        assert_eq!(item(&board, 2).center(), Vec2::new(60.0, 20.0));

        board.ungroup_selection();
        board.clear_selection();
        board.pointer_down(
            screen(&camera, Vec2::new(60.0, 20.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(60.0, 20.0)),
            view,
            Modifiers::default(),
        );
        assert_eq!(board.selection().len(), 1, "ungrouped again");
    }

    #[test]
    fn z_order_commands_move_the_selection_as_a_block() {
        let mut board = board(4);
        board.pointer_down(
            screen(&view(), Vec2::new(40.0, 0.0)),
            info(&view()),
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&view(), Vec2::new(40.0, 0.0)),
            info(&view()),
            Modifiers::default(),
        );
        board.pointer_down(
            screen(&view(), Vec2::new(80.0, 0.0)),
            info(&view()),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );

        assert!(board.front_selection());
        let order: Vec<u64> = board
            .scene()
            .items()
            .iter()
            .map(|item| item.id().raw())
            .collect();
        assert_eq!(order, vec![1, 4, 2, 3]);

        board.undo();
        assert_eq!(
            board
                .scene()
                .items()
                .iter()
                .map(|item| item.id().raw())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn arrange_is_one_undo_step() {
        let mut board = board(12);
        board.select_all();
        let initial = state_signature(&board);

        assert!(board.arrange_selection(ArrangeMode::Grid { columns: Some(4) }));

        let centers: Vec<Vec2> = board.scene().items().iter().map(Item::center).collect();
        for (index, center) in centers.iter().enumerate() {
            for other in &centers[index + 1..] {
                assert_ne!(center, other);
            }
        }
        board.undo();
        assert_eq!(state_signature(&board), initial);
    }

    #[test]
    fn nudges_use_screen_distance_and_coalesce() {
        let mut board = board(1);
        let camera = Camera::new(DVec2::ZERO, 4.0, 0.0);
        let view = info(&camera);
        board.select_all();

        board.nudge(1, 0, super::NUDGE_PIXELS, view);
        board.nudge(1, 0, super::NUDGE_PIXELS, view);
        board.nudge(1, 0, super::NUDGE_PIXELS, view);

        assert_eq!(item(&board, 1).center().x, 0.75, "three pixels at 400%");
        assert_eq!(board.history().undo_len(), 1, "one nudge gesture");
        board.undo();
        assert_eq!(item(&board, 1).center(), Vec2::ZERO);
    }

    #[test]
    fn a_tiny_click_does_not_create_a_command() {
        let mut board = board(1);
        let camera = view();
        let view = info(&camera);
        board.select_all();
        let undo_before = board.history().undo_len();

        board.pointer_down(screen(&camera, Vec2::ZERO), view, Modifiers::default());
        board.pointer_move(
            screen(&camera, Vec2::new(CLICK_SLOP_PIXELS as f32 * 0.5, 0.0)),
            view,
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&camera, Vec2::new(CLICK_SLOP_PIXELS as f32 * 0.5, 0.0)),
            view,
            Modifiers::default(),
        );

        assert_eq!(board.history().undo_len(), undo_before);
        assert_eq!(item(&board, 1).center(), Vec2::ZERO);
    }

    #[test]
    fn a_default_corner_drag_keeps_the_opposite_corner_fixed() {
        let mut board = board(2);
        board.set_snap_enabled(false);
        board.select_all();
        let camera = view();
        let view = info(&camera);
        let frame = board.frame().expect("two items");
        let nw = frame.handle_position(rizref_core::Handle::Nw, 0.0);
        let se = frame.handle_position(rizref_core::Handle::Se, 0.0);

        // Drag the south-east corner out by half the frame, askew.
        let target = se + Vec2::new(30.0, 25.0);
        board.pointer_down(screen(&camera, se), view, Modifiers::default());
        board.pointer_move(screen(&camera, target), view, Modifiers::default());
        board.pointer_up(screen(&camera, target), view, Modifiers::default());

        let after = board.frame().expect("still selected");
        let new_nw = after.handle_position(rizref_core::Handle::Nw, 0.0);
        assert!((new_nw - nw).length() < 1e-3, "{new_nw} vs {nw}");
        assert!(
            (after.size.x / after.size.y - frame.size.x / frame.size.y).abs() < 1e-4,
            "images keep their aspect on a corner drag"
        );
    }

    #[test]
    fn a_multi_selection_scales_around_its_frame_centre_with_alt() {
        let mut board = board(2);
        board.set_snap_enabled(false);
        board.select_all();
        let camera = view();
        let view = info(&camera);
        let frame = board.frame().expect("two items");
        assert_eq!(frame.center, Vec2::new(20.0, 0.0));
        assert_eq!(frame.size, Vec2::new(60.0, 20.0));

        let se = frame.handle_position(rizref_core::Handle::Se, 0.0);
        let target = frame.center + (se - frame.center) * 2.0;
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        board.pointer_down(screen(&camera, se), view, Modifiers::default());
        board.pointer_move(screen(&camera, target), view, alt);
        board.pointer_up(screen(&camera, target), view, alt);

        assert_eq!(item(&board, 1).center(), Vec2::new(-20.0, 0.0));
        assert_eq!(item(&board, 2).center(), Vec2::new(60.0, 0.0));
        assert_eq!(item(&board, 1).size(), Vec2::splat(40.0));
        assert_eq!(item(&board, 2).size(), Vec2::splat(40.0));
    }

    #[test]
    fn create_and_first_commit_are_one_undo_step() {
        let mut board = Board::new(Scene::new(SceneId::from_bytes([13; 16])));
        let note = board.create_provisional_note(Vec2::new(10.0, 20.0));
        assert_eq!(
            board.history().undo_len(),
            0,
            "a provisional note is not history"
        );
        assert!(board.selection().contains(note));

        let mut content = NoteContent::markdown("first draft");
        assert!(board.set_note_live(note, &content));
        content.wrap_width = Some(300.0);
        assert!(board.commit_new_note(note, &content));
        assert_eq!(board.history().undo_len(), 1);
        assert_eq!(
            board.scene().item(note).and_then(Item::note_content),
            Some(content)
        );
        board.undo();
        assert!(
            board.scene().item(note).is_none(),
            "one undo removes the note"
        );
        board.redo();
        assert_eq!(
            board.scene().item(note).and_then(Item::text),
            Some("first draft")
        );
    }

    #[test]
    fn an_empty_or_cancelled_new_note_leaves_no_trace() {
        let mut board = Board::new(Scene::new(SceneId::from_bytes([13; 16])));
        let empty = board.create_provisional_note(Vec2::ZERO);
        assert!(!board.commit_new_note(empty, &NoteContent::markdown("  \n")));
        assert!(board.scene().is_empty());
        let cancelled = board.create_provisional_note(Vec2::ZERO);
        board.set_note_live(cancelled, &NoteContent::markdown("typed"));
        assert!(board.discard_provisional_note(cancelled));
        assert!(board.scene().is_empty());
        assert_eq!(board.history().undo_len(), 0);
        let effects = board.take_effects();
        assert!(effects.removed.contains(&cancelled));
    }

    #[test]
    fn editing_an_existing_note_is_one_step_and_restore_is_exact() {
        let mut scene = Scene::new(SceneId::from_bytes([13; 16]));
        let id = scene.insert_note("before", Vec2::ZERO, Vec2::splat(100.0));
        let mut board = Board::new(scene);
        let before = board
            .scene()
            .item(id)
            .and_then(Item::note_content)
            .expect("note");
        let mut after = before.clone();
        after.text = "after".into();
        assert!(board.set_note_live(id, &after));
        assert!(board.restore_note(id, &before));
        assert_eq!(
            board.scene().item(id).and_then(Item::note_content),
            Some(before.clone())
        );
        assert_eq!(board.history().undo_len(), 0, "cancel adds no history");

        board.set_note_live(id, &after);
        assert!(board.commit_note(id, &before, &after));
        assert!(!board.commit_note(id, &after, &after), "no change, no step");
        assert_eq!(board.history().undo_len(), 1);
        board.undo();
        assert_eq!(
            board.scene().item(id).and_then(Item::note_content),
            Some(before)
        );
    }

    #[test]
    fn the_selected_image_needs_exactly_one_image() {
        let mut board = board(2);
        let note = board
            .scene_mut()
            .insert_note("n", Vec2::new(500.0, 0.0), Vec2::splat(50.0));
        assert_eq!(board.selected_image(), None, "nothing selected");
        board.select_only(ItemId::from_raw(1));
        assert_eq!(board.selected_image(), Some(ItemId::from_raw(1)));
        assert_eq!(board.selected_note(), None);
        board.select_all();
        assert_eq!(board.selected_image(), None, "two images and a note");
        board.select_only(note);
        assert_eq!(board.selected_image(), None, "a note is not an image");
        board.clear_selection();
        assert_eq!(board.selected_image(), None);
    }

    #[test]
    fn background_reflow_and_fit_are_undoable_steps() {
        let mut scene = Scene::new(SceneId::from_bytes([13; 16]));
        let id = scene.insert_note("text", Vec2::ZERO, Vec2::new(300.0, 100.0));
        let mut board = Board::new(scene);
        board.select_only(id);
        assert_eq!(board.selected_note(), Some(id));

        let mut content = board
            .scene()
            .item(id)
            .and_then(Item::note_content)
            .expect("note");
        content.background = Some([250, 250, 250]);
        assert!(board.set_note_content(id, &content));
        assert_eq!(board.history().undo_len(), 1);

        // Resize the box, then reflow to the new inner width.
        if let Some(item) = board.scene_mut().item_mut(id) {
            let mut transform = item.transform();
            transform.set_size(Vec2::new(500.0, 100.0));
            item.set_transform(transform);
        }
        assert!(board.reflow_note(id));
        assert_eq!(
            board
                .scene()
                .item(id)
                .and_then(Item::note_content)
                .and_then(|c| c.wrap_width),
            Some(500.0 - 2.0 * NOTE_PADDING)
        );
        assert!(!board.reflow_note(id), "already at the box width");

        let top = board
            .scene()
            .item(id)
            .map(|item| item.world_rect().min.y)
            .expect("note");
        assert!(board.fit_note_height(id, 40.0));
        let item = board.scene().item(id).expect("note");
        assert_eq!(item.size(), Vec2::new(500.0, 40.0 + 2.0 * NOTE_PADDING));
        assert!(
            (item.world_rect().min.y - top).abs() < 1.0e-3,
            "the top edge stays"
        );
        assert_eq!(board.history().undo_len(), 3);
        board.undo();
        assert_eq!(
            board.scene().item(id).map(Item::size),
            Some(Vec2::new(500.0, 100.0))
        );
    }

    #[test]
    fn notes_are_never_rotated_and_have_no_rotate_handle() {
        let camera = view();
        let info = info(&camera);
        let mut scene = Scene::new(SceneId::from_bytes([14; 16]));
        let note = scene.insert_note("n", Vec2::ZERO, Vec2::splat(100.0));
        let mut board = Board::new(scene);
        board.select_only(note);
        assert!(!board.selection_rotatable());

        // Where the rotate handle would be: pressing there starts no rotation.
        let frame = board.frame().expect("frame");
        let handle =
            frame.handle_position(rizref_core::Handle::Rotate, super::ROTATE_HANDLE_PIXELS);
        assert!(!board.handle_at(screen(&camera, handle), info));
        board.pointer_down(screen(&camera, handle), info, Modifiers::default());
        board.pointer_move(
            screen(&camera, handle + Vec2::new(200.0, 200.0)),
            info,
            Modifiers::default(),
        );
        board.pointer_up(DVec2::ZERO, info, Modifiers::default());
        assert_eq!(board.scene().item(note).map(Item::rotation), Some(0.0));
    }

    #[test]
    fn a_mixed_selection_rotates_only_its_images() {
        let camera = view();
        let info = info(&camera);
        let mut scene = Scene::new(SceneId::from_bytes([15; 16]));
        let asset = image_asset(&mut scene);
        let image = scene
            .insert_image(asset, Vec2::new(-100.0, 0.0), Vec2::splat(40.0))
            .expect("image");
        let note = scene.insert_note("n", Vec2::new(100.0, 0.0), Vec2::splat(40.0));
        let note_before = scene.item(note).map(Item::state).expect("note");
        let mut board = Board::new(scene);
        board.select_all();
        assert!(board.selection_rotatable());

        let frame = board.frame().expect("frame");
        let handle =
            frame.handle_position(rizref_core::Handle::Rotate, super::ROTATE_HANDLE_PIXELS);
        board.pointer_down(screen(&camera, handle), info, Modifiers::default());
        board.pointer_move(
            screen(&camera, frame.center + Vec2::new(300.0, 0.0)),
            info,
            Modifiers::default(),
        );
        board.pointer_up(DVec2::ZERO, info, Modifiers::default());

        assert!(
            board
                .scene()
                .item(image)
                .map(Item::rotation)
                .expect("image")
                .abs()
                > 0.5
        );
        assert_eq!(board.scene().item(note).map(Item::state), Some(note_before));
    }

    #[test]
    fn the_selection_frame_is_oriented_for_one_item_and_axis_aligned_for_many() {
        let mut scene = Scene::new(SceneId::from_bytes([14; 16]));
        let a = scene.insert_note("a", Vec2::ZERO, Vec2::splat(20.0));
        let b = scene.insert_note("b", Vec2::new(100.0, 0.0), Vec2::splat(20.0));
        if let Some(item) = scene.item_mut(a) {
            item.set_transform(Transform::new(Vec2::ZERO, Vec2::splat(20.0)).with_rotation(0.5));
        }
        let expected = scene
            .item(a)
            .expect("a")
            .world_rect()
            .union(&scene.item(b).expect("b").world_rect());
        let mut board = Board::new(scene);
        board.select_all();

        let frame = board.frame().expect("selection frame");
        assert!(
            (frame.center - expected.center()).length() < 1.0e-4,
            "frame center {:?} vs expected {:?}",
            frame.center,
            expected.center()
        );
        assert!(
            (frame.size - expected.size).length() < 1.0e-4,
            "frame size {:?} vs expected {:?}",
            frame.size,
            expected.size
        );
        assert_eq!(frame.rotation, 0.0);

        board.clear_selection();
        board.pointer_down(
            screen(&view(), Vec2::ZERO),
            info(&view()),
            Modifiers::default(),
        );
        board.pointer_up(
            screen(&view(), Vec2::ZERO),
            info(&view()),
            Modifiers::default(),
        );
        let frame = board.frame().expect("one item");
        assert!((frame.rotation - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn snap_tolerance_is_three_screen_pixels() {
        assert_eq!(SNAP_PIXELS, 3.0);
        assert_eq!(rizref_core::screen_tolerance(SNAP_PIXELS, 0.05), 60.0);
        assert!((rizref_core::screen_tolerance(SNAP_PIXELS, 32.0) - 0.09375).abs() < 1.0e-6);
    }
}
