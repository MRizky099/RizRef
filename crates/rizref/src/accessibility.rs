//! UI Automation through AccessKit.
//!
//! RizRef paints its own menus, so there is no native menu tree for a screen
//! reader to discover — the exact failure noted in the PureRef study, where
//! Inspect.exe sees an empty window. This module publishes a virtual tree
//! instead: a window root, a command menu bar, every menu item with its label,
//! accelerator (as the description) and disabled/checked state, plus the live
//! HUD readout as a status label.
//!
//! The AccessKit winit adapter owns the platform bridge. Activating a menu item
//! arrives as an [`accesskit_winit::Event::ActionRequested`]; the application
//! maps the node id back to the same [`Action`] the mouse would run, so
//! accessibility cannot drift from the visible command set.

use std::collections::HashMap;

use accesskit::{
    Action as A11yAction, ActionRequest, Node, NodeId, Role, Toggled, Tree, TreeUpdate,
};
use accesskit_winit::Adapter;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use crate::app::AppEvent;
use crate::ui::menu::{Action, MenuEntry};
use crate::ui::panels::ConfirmClose;
use crate::ui::titlebar::TitleBar;

/// The window root node.
const ROOT_ID: u64 = 1;
/// The menu bar node.
const MENUBAR_ID: u64 = 2;
/// The status/HUD label node.
const STATUS_ID: u64 = 3;
/// The custom title bar's control group.
const TITLEBAR_ID: u64 = 4;
/// The modal unsaved-changes dialog.
const CONFIRM_ID: u64 = 5;
/// The dialog's Cancel button.
const CONFIRM_CANCEL_ID: u64 = 20;
/// The dialog's destructive button.
const CONFIRM_DISCARD_ID: u64 = 21;
/// The always-on-top pin.
const PIN_ID: u64 = 10;
/// Minimize.
const MINIMIZE_ID: u64 = 11;
/// Maximize/restore.
const MAXIMIZE_ID: u64 = 12;
/// Close.
const CLOSE_ID: u64 = 13;
/// First dynamically allocated menu node id.
const FIRST_MENU_ID: u64 = 100;

/// The accessibility adapter and the node→action map for the published tree.
pub struct Accessibility {
    adapter: Adapter,
    actions: HashMap<u64, Action>,
}

impl Accessibility {
    /// Creates the adapter. Must be called before the window is first shown.
    pub fn new(
        event_loop: &ActiveEventLoop,
        window: &Window,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Option<Self> {
        let adapter = Adapter::with_event_loop_proxy(event_loop, window, proxy);
        Some(Self {
            adapter,
            actions: HashMap::new(),
        })
    }

    /// Forwards a window event to the platform adapter before the app handles
    /// it (the adapter needs focus and resize notifications).
    pub fn process_event(&mut self, window: &Window, event: &WindowEvent) {
        self.adapter.process_event(window, event);
    }

    /// Publishes the current command tree, window controls and status line.
    pub fn update(
        &mut self,
        menus: &[MenuEntry],
        status: &str,
        window: &Window,
        title_bar: &TitleBar,
        confirm_close: Option<&ConfirmClose>,
    ) {
        let (update, actions) = build_update(menus, status, window, title_bar, confirm_close);
        self.actions = actions;
        self.adapter.update_if_active(|| update);
    }

    /// The action a menu node stands for, for a click request.
    pub fn action(&self, node: NodeId) -> Option<Action> {
        self.actions.get(&node.0).cloned()
    }

    /// The action request's target action, when it is a click on a leaf.
    pub fn requested_action(&self, request: &ActionRequest) -> Option<Action> {
        if request.action == A11yAction::Click {
            self.action(request.target_node)
        } else {
            None
        }
    }
}

/// Builds the tree update and its node→action map.
fn build_update(
    menus: &[MenuEntry],
    status: &str,
    window: &Window,
    title_bar: &TitleBar,
    confirm_close: Option<&ConfirmClose>,
) -> (TreeUpdate, HashMap<u64, Action>) {
    let size = window.inner_size();
    let mut nodes: Vec<(NodeId, Node)> = Vec::new();
    let mut actions = HashMap::new();

    let mut root = Node::new(Role::Window);
    root.set_label(title_bar.title().unwrap_or("RizRef"));
    root.set_bounds(accesskit::Rect {
        x0: 0.0,
        y0: 0.0,
        x1: f64::from(size.width),
        y1: f64::from(size.height),
    });
    root.push_child(NodeId(TITLEBAR_ID));
    root.push_child(NodeId(MENUBAR_ID));
    root.push_child(NodeId(STATUS_ID));
    if confirm_close.is_some() {
        root.push_child(NodeId(CONFIRM_ID));
    }

    let mut controls = Node::new(Role::Group);
    controls.set_label("Window controls");
    for id in [PIN_ID, MINIMIZE_ID, MAXIMIZE_ID, CLOSE_ID] {
        controls.push_child(NodeId(id));
    }
    nodes.push((NodeId(TITLEBAR_ID), controls));

    let mut pin = Node::new(Role::Switch);
    pin.set_label("Always on top");
    pin.set_toggled(if title_bar.pinned() {
        Toggled::True
    } else {
        Toggled::False
    });
    pin.add_action(A11yAction::Click);
    actions.insert(PIN_ID, Action::ToggleAlwaysOnTop);
    nodes.push((NodeId(PIN_ID), pin));

    let mut minimize = Node::new(Role::Button);
    minimize.set_label("Minimize");
    minimize.add_action(A11yAction::Click);
    actions.insert(MINIMIZE_ID, Action::WindowMinimize);
    nodes.push((NodeId(MINIMIZE_ID), minimize));

    let mut maximize = Node::new(Role::Button);
    maximize.set_label(if title_bar.maximized() {
        "Restore"
    } else {
        "Maximize"
    });
    maximize.add_action(A11yAction::Click);
    actions.insert(MAXIMIZE_ID, Action::WindowToggleMaximize);
    nodes.push((NodeId(MAXIMIZE_ID), maximize));

    let mut close = Node::new(Role::Button);
    close.set_label("Close");
    close.add_action(A11yAction::Click);
    actions.insert(CLOSE_ID, Action::Quit);
    nodes.push((NodeId(CLOSE_ID), close));

    let mut menubar = Node::new(Role::MenuBar);
    menubar.set_label("Command menu");

    let mut next_id = FIRST_MENU_ID;
    for entry in menus {
        let mut top = Node::new(Role::Menu);
        top.set_label(entry.label.as_str());
        let id = next_id;
        next_id += 1;
        let children = build_children(&mut nodes, &mut actions, &mut next_id, &entry.children);
        for child in children {
            top.push_child(child);
        }
        menubar.push_child(NodeId(id));
        nodes.push((NodeId(id), top));
    }
    nodes.push((NodeId(MENUBAR_ID), menubar));

    let mut status_node = Node::new(Role::Label);
    status_node.set_label(if status.is_empty() {
        "Board status unavailable"
    } else {
        status
    });
    nodes.push((NodeId(STATUS_ID), status_node));

    if let Some(dialog) = confirm_close {
        let mut node = Node::new(Role::Dialog);
        node.set_label("Unsaved changes");
        node.set_description(dialog.title.as_str());
        node.push_child(NodeId(CONFIRM_CANCEL_ID));
        node.push_child(NodeId(CONFIRM_DISCARD_ID));
        nodes.push((NodeId(CONFIRM_ID), node));

        let mut cancel = Node::new(Role::Button);
        cancel.set_label("Cancel");
        cancel.add_action(A11yAction::Click);
        actions.insert(CONFIRM_CANCEL_ID, Action::ConfirmCloseCancel);
        nodes.push((NodeId(CONFIRM_CANCEL_ID), cancel));

        let mut discard = Node::new(Role::Button);
        discard.set_label("Close without saving");
        discard.add_action(A11yAction::Click);
        actions.insert(CONFIRM_DISCARD_ID, Action::ConfirmCloseDiscard);
        nodes.push((NodeId(CONFIRM_DISCARD_ID), discard));
    }

    // The dialog owns focus while it is open; otherwise the window root does.
    let focus = if confirm_close.is_some() {
        NodeId(CONFIRM_ID)
    } else {
        NodeId(ROOT_ID)
    };
    nodes.push((NodeId(ROOT_ID), root));

    let mut tree = Tree::new(NodeId(ROOT_ID));
    tree.toolkit_name = Some("RizRef".to_owned());
    tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").to_owned());

    (
        TreeUpdate {
            nodes,
            tree: Some(tree),
            tree_id: accesskit::TreeId::ROOT,
            focus,
        },
        actions,
    )
}

/// Recursively adds `entries` and returns their node ids.
fn build_children(
    nodes: &mut Vec<(NodeId, Node)>,
    actions: &mut HashMap<u64, Action>,
    next_id: &mut u64,
    entries: &[MenuEntry],
) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for entry in entries {
        if entry.separator {
            continue;
        }
        let id = *next_id;
        *next_id += 1;

        let role = if !entry.children.is_empty() {
            Role::Menu
        } else if entry.toggles {
            Role::MenuItemCheckBox
        } else {
            Role::MenuItem
        };
        let mut node = Node::new(role);
        node.set_label(entry.label.as_str());
        if let Some(accel) = &entry.accel {
            node.set_description(accel.as_str());
        }
        if entry.toggles {
            node.set_toggled(if entry.checked {
                Toggled::True
            } else {
                Toggled::False
            });
        }
        if !entry.enabled {
            node.set_disabled();
        }

        if entry.children.is_empty() {
            if let Some(action) = &entry.action
                && entry.enabled
            {
                node.add_action(A11yAction::Click);
                actions.insert(id, action.clone());
            }
        } else {
            node.add_child_action(A11yAction::Click);
            let children = build_children(nodes, actions, next_id, &entry.children);
            for child in children {
                node.push_child(child);
            }
        }

        nodes.push((NodeId(id), node));
        ids.push(NodeId(id));
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::{FIRST_MENU_ID, build_children};
    use crate::ui::menu::Action;
    use crate::ui::menu::MenuEntry;
    use std::collections::HashMap;

    #[test]
    fn leaf_nodes_carry_their_action_and_checked_state() {
        let entries = vec![
            MenuEntry::item("Undo", Action::ShowStart),
            MenuEntry::item("Grid", Action::ToggleGrid).check(true),
            MenuEntry::item("Disabled", Action::ShowSettings).disabled(),
            MenuEntry::separator(),
        ];
        let mut nodes = Vec::new();
        let mut actions = HashMap::new();
        let mut next_id = FIRST_MENU_ID;

        let ids = build_children(&mut nodes, &mut actions, &mut next_id, &entries);

        assert_eq!(ids.len(), 3, "separators are not published");
        assert_eq!(actions.len(), 2, "disabled leaves are not actionable");
        assert_eq!(actions.get(&ids[0].0), Some(&Action::ShowStart));
        assert_eq!(actions.get(&ids[1].0), Some(&Action::ToggleGrid));
        assert!(!actions.contains_key(&ids[2].0));
        let grid = nodes
            .iter()
            .find(|(id, _)| *id == ids[1])
            .map(|(_, node)| node)
            .expect("the grid node exists");
        assert_eq!(grid.role(), accesskit::Role::MenuItemCheckBox);
        assert_eq!(grid.toggled(), Some(accesskit::Toggled::True));
    }

    #[test]
    fn submenus_recurse_and_keep_stable_ids() {
        let entries = vec![MenuEntry::submenu(
            "File",
            vec![MenuEntry::item("Quit", Action::Quit)],
        )];
        let mut nodes = Vec::new();
        let mut actions = HashMap::new();
        let mut next_id = FIRST_MENU_ID;

        let ids = build_children(&mut nodes, &mut actions, &mut next_id, &entries);

        assert_eq!(ids.len(), 1, "the submenu itself is a node");
        assert_eq!(actions.len(), 1, "its child is actionable");
        assert_eq!(next_id, FIRST_MENU_ID + 2);
    }
}
