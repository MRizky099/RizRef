//! Reversible scene mutations and the bounded undo/redo history.

use std::fmt::Debug;

use crate::scene::Scene;

/// A reversible mutation of a [`Scene`].
///
/// Commands are the only way the UI is allowed to change the model. Routing
/// every edit — a drag, a paste, a menu item, a batch arrange — through one
/// interface is what makes undo, autosave and the dirty flag fall out of a
/// single code path instead of being maintained per feature.
pub trait Command: Debug {
    /// Short human-readable label, shown in the menu and the undo tooltip.
    fn label(&self) -> &str;

    /// Applies the mutation.
    fn apply(&mut self, scene: &mut Scene);

    /// Restores the state as it was before [`Command::apply`] ran.
    fn revert(&mut self, scene: &mut Scene);
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
}

impl History {
    /// Commands kept for undo by default.
    ///
    /// Sized against the phase-07 memory budget: a clear/arrange of a large
    /// board is a handful of commands, so this is generous without being able to
    /// hold megabytes of transient data.
    pub const DEFAULT_LIMIT: usize = 256;

    /// A history holding at most `limit` commands; `0` is raised to `1`, because
    /// a history that cannot undo the command it just applied is a bug factory.
    pub fn new(limit: usize) -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Applies `command`, records it for undo and discards the redo stack.
    pub fn apply(&mut self, scene: &mut Scene, mut command: Box<dyn Command>) {
        command.apply(scene);
        self.undone.clear();
        self.done.push(command);
        if self.done.len() > self.limit {
            self.done.remove(0);
        }
    }

    /// Reverts the most recent command, returning its label.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<String> {
        let mut command = self.done.pop()?;
        command.revert(scene);
        let label = command.label().to_owned();
        self.undone.push(command);
        Some(label)
    }

    /// Re-applies the most recently undone command, returning its label.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<String> {
        let mut command = self.undone.pop()?;
        command.apply(scene);
        let label = command.label().to_owned();
        self.done.push(command);
        Some(label)
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

#[cfg(test)]
mod tests {
    use super::{Command, History};
    use crate::asset::AssetId;
    use crate::item::ImageItem;
    use crate::scene::{ItemId, Scene, SceneId};
    use glam::Vec2;

    /// Test-only command: appends one item, remembers the z-position it occupied.
    #[derive(Debug)]
    struct AddItem {
        item: ImageItem,
        position: Option<usize>,
    }

    impl AddItem {
        fn new(raw: u64) -> Self {
            let id = ItemId::from_raw(raw);
            Self {
                item: ImageItem::new(
                    id,
                    AssetId::from_checksum([raw as u8; 32]),
                    Vec2::splat(raw as f32),
                    Vec2::splat(4.0),
                ),
                position: None,
            }
        }
    }

    impl Command for AddItem {
        fn label(&self) -> &'static str {
            "Add item"
        }

        fn apply(&mut self, scene: &mut Scene) {
            scene.push(self.item.clone());
            self.position = Some(scene.len() - 1);
        }

        fn revert(&mut self, scene: &mut Scene) {
            let removed = scene.remove(self.item.id());
            assert_eq!(removed, self.position, "undo must restore the z-order");
        }
    }

    fn scene() -> Scene {
        Scene::new(SceneId::from_bytes([7; SceneId::BYTE_LEN]))
    }

    #[test]
    fn undo_and_redo_move_the_board_back_and_forth() {
        let mut scene = scene();
        let mut history = History::default();

        history.apply(&mut scene, Box::new(AddItem::new(1)));
        history.apply(&mut scene, Box::new(AddItem::new(2)));
        assert_eq!(scene.len(), 2);
        assert!(history.can_undo() && !history.can_redo());

        assert_eq!(history.undo(&mut scene).as_deref(), Some("Add item"));
        assert_eq!(scene.len(), 1);
        assert!(history.can_redo());

        assert_eq!(history.redo(&mut scene).as_deref(), Some("Add item"));
        assert_eq!(scene.len(), 2);
    }

    #[test]
    fn undo_on_an_empty_history_is_a_no_op() {
        let mut scene = scene();
        let mut history = History::default();

        assert_eq!(history.undo(&mut scene), None);
        assert_eq!(history.redo(&mut scene), None);
        assert!(scene.is_empty());
    }

    #[test]
    fn a_new_command_discards_the_redo_stack() {
        let mut scene = scene();
        let mut history = History::default();
        history.apply(&mut scene, Box::new(AddItem::new(1)));
        history.undo(&mut scene);
        assert_eq!(history.redo_len(), 1);

        history.apply(&mut scene, Box::new(AddItem::new(2)));

        assert_eq!(history.redo_len(), 0);
        assert!(!history.can_redo());
        assert_eq!(
            scene.items.iter().map(|item| item.id()).collect::<Vec<_>>(),
            vec![ItemId::from_raw(2)]
        );
    }

    #[test]
    fn the_oldest_command_is_dropped_at_the_limit() {
        let mut scene = scene();
        let mut history = History::new(2);

        for raw in 1..=3 {
            history.apply(&mut scene, Box::new(AddItem::new(raw)));
        }
        assert_eq!(history.limit(), 2);
        assert_eq!(history.undo_len(), 2);

        history.undo(&mut scene);
        history.undo(&mut scene);

        // The first command fell out of the history, so its item stays on the board.
        assert!(!history.can_undo());
        assert_eq!(
            scene.items.iter().map(|item| item.id()).collect::<Vec<_>>(),
            vec![ItemId::from_raw(1)]
        );
    }

    #[test]
    fn a_zero_limit_still_allows_one_undo() {
        let history = History::new(0);

        assert_eq!(history.limit(), 1);
    }
}
