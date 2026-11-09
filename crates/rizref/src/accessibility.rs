//! UI Automation through AccessKit.
//!
//! RizRef paints its own menus, so there is no native menu tree for a screen
//! reader to discover — the exact failure noted in the PureRef study, where
//! Inspect.exe sees an empty window. This module publishes a virtual tree
//! instead: a window root, a command menu bar, every menu item with its label,
//! accelerator and disabled/checked state, plus the live HUD readout as a
//! status label.
//!
//! A menu item's accelerator is published twice, from the live (rebindable)
//! shortcut table: as AccessKit's `keyboard_shortcut`, the property meant for
//! it, and as the description. The description is not redundant on Windows:
//! `accesskit_windows` (0.32 here, and still 0.35.2, the newest release) maps
//! no node property to UIA `AcceleratorKey`, so the description — exposed as
//! UIA `FullDescription`/`HelpText` — is what Narrator reads today. When the
//! adapter gains the mapping, `keyboard_shortcut` fills `AcceleratorKey` with
//! no change here.
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
use crate::ui::image_info::{ImageInfoPanel, InfoButton};
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
/// The ☰ main-menu button.
const MAIN_MENU_ID: u64 = 14;
/// The note chrome group (hover toolbar or edit pill).
const NOTE_GROUP_ID: u64 = 6;
/// The edited note's source, as a multi-line text value.
const NOTE_SOURCE_ID: u64 = 7;
/// First note-chrome button id.
const FIRST_NOTE_BUTTON_ID: u64 = 30;
/// The modal image information dialog.
const IMAGE_INFO_ID: u64 = 8;
/// First image-info value row.
const FIRST_INFO_ROW_ID: u64 = 40;
/// First image-info button.
const FIRST_INFO_BUTTON_ID: u64 = 70;
/// First dynamically allocated menu node id.
const FIRST_MENU_ID: u64 = 100;

/// The note chrome as accessibility sees it: the visible buttons, by name
/// and action, and the source text of a note being edited.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NoteControls {
    /// Toolbar or edit-pill buttons, in visual order.
    pub buttons: Vec<(&'static str, Action)>,
    /// The markdown source while a note is being edited.
    ///
    /// Published read-only: a set-value request from an assistive tool is
    /// not applied yet (documented gap; typing goes through the editor).
    pub source: Option<String>,
}

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
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        menus: &[MenuEntry],
        status: &str,
        window: &Window,
        title_bar: &TitleBar,
        confirm_close: Option<&ConfirmClose>,
        image_info: Option<&ImageInfoPanel>,
        notes: &NoteControls,
    ) {
        let (update, actions) = build_update(
            menus,
            status,
            window,
            title_bar,
            confirm_close,
            image_info,
            notes,
        );
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
    image_info: Option<&ImageInfoPanel>,
    notes: &NoteControls,
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
    if !notes.buttons.is_empty() || notes.source.is_some() {
        root.push_child(NodeId(NOTE_GROUP_ID));
    }
    if confirm_close.is_some() {
        root.push_child(NodeId(CONFIRM_ID));
    }
    if image_info.is_some() {
        root.push_child(NodeId(IMAGE_INFO_ID));
    }

    let mut controls = Node::new(Role::Group);
    controls.set_label("Window controls");
    for id in [MAIN_MENU_ID, PIN_ID, MINIMIZE_ID, MAXIMIZE_ID, CLOSE_ID] {
        controls.push_child(NodeId(id));
    }
    nodes.push((NodeId(TITLEBAR_ID), controls));

    let mut main_menu = Node::new(Role::Button);
    main_menu.set_label("Main menu");
    main_menu.add_action(A11yAction::Click);
    actions.insert(MAIN_MENU_ID, Action::OpenMainMenu);
    nodes.push((NodeId(MAIN_MENU_ID), main_menu));

    if !notes.buttons.is_empty() || notes.source.is_some() {
        let mut group = Node::new(Role::Toolbar);
        group.set_label("Note controls");
        if let Some(source) = &notes.source {
            let mut editor = Node::new(Role::MultilineTextInput);
            editor.set_label("Note source");
            editor.set_value(source.as_str());
            editor.set_read_only();
            group.push_child(NodeId(NOTE_SOURCE_ID));
            nodes.push((NodeId(NOTE_SOURCE_ID), editor));
        }
        for (index, (label, action)) in notes.buttons.iter().enumerate() {
            let id = FIRST_NOTE_BUTTON_ID + index as u64;
            let mut button = Node::new(Role::Button);
            button.set_label(*label);
            button.add_action(A11yAction::Click);
            actions.insert(id, action.clone());
            group.push_child(NodeId(id));
            nodes.push((NodeId(id), button));
        }
        nodes.push((NodeId(NOTE_GROUP_ID), group));
    }

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
        node.set_label(dialog.heading.as_str());
        node.set_description(dialog.message.as_str());
        node.push_child(NodeId(CONFIRM_CANCEL_ID));
        node.push_child(NodeId(CONFIRM_DISCARD_ID));
        nodes.push((NodeId(CONFIRM_ID), node));

        let mut cancel = Node::new(Role::Button);
        cancel.set_label(dialog.cancel_label.as_str());
        cancel.add_action(A11yAction::Click);
        actions.insert(CONFIRM_CANCEL_ID, Action::ConfirmCloseCancel);
        nodes.push((NodeId(CONFIRM_CANCEL_ID), cancel));

        let mut discard = Node::new(Role::Button);
        discard.set_label(dialog.discard_label.as_str());
        discard.add_action(A11yAction::Click);
        actions.insert(CONFIRM_DISCARD_ID, Action::ConfirmCloseDiscard);
        nodes.push((NodeId(CONFIRM_DISCARD_ID), discard));
    }

    if let Some(dialog) = image_info {
        push_image_info(&mut nodes, &mut actions, dialog);
    }

    // A dialog owns focus while it is open (the close guard above the info
    // dialog); otherwise the window root does.
    let focus = if confirm_close.is_some() {
        NodeId(CONFIRM_ID)
    } else if let Some(dialog) = image_info {
        let index = dialog
            .info
            .buttons()
            .iter()
            .position(|button| *button == dialog.focused())
            .unwrap_or(0);
        NodeId(FIRST_INFO_BUTTON_ID + index as u64)
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

/// Publishes the image information dialog: a `Dialog` (UIA
/// `ControlType.Window`) with one label per value row, carrying the full,
/// un-elided value, and one named button per footer action. Nothing traps
/// focus: every button is a click target and Close is always present.
fn push_image_info(
    nodes: &mut Vec<(NodeId, Node)>,
    actions: &mut HashMap<u64, Action>,
    dialog: &ImageInfoPanel,
) {
    let info = &dialog.info;
    let mut node = Node::new(Role::Dialog);
    node.set_label("Image info");
    node.set_description(format!("{}. {}", info.title, info.location_note()));
    node.set_modal();

    for (index, (label, value)) in info.rows().into_iter().enumerate() {
        let id = FIRST_INFO_ROW_ID + index as u64;
        let mut row = Node::new(Role::Label);
        row.set_label(format!("{label}: {value}"));
        row.set_value(value);
        node.push_child(NodeId(id));
        nodes.push((NodeId(id), row));
    }

    for (index, button) in info.buttons().into_iter().enumerate() {
        let id = FIRST_INFO_BUTTON_ID + index as u64;
        let mut control = Node::new(Role::Button);
        control.set_label(info.button_label(button));
        control.add_action(A11yAction::Click);
        actions.insert(
            id,
            match button {
                InfoButton::Open => Action::OpenImageLocation,
                InfoButton::CopyLocation => Action::CopyImageLocation,
                InfoButton::CopyId => Action::CopyImageAssetId,
                InfoButton::Close => Action::CloseImageInfo,
            },
        );
        node.push_child(NodeId(id));
        nodes.push((NodeId(id), control));
    }
    nodes.push((NodeId(IMAGE_INFO_ID), node));
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
            node.set_keyboard_shortcut(accel.as_str());
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
    use super::{
        FIRST_INFO_BUTTON_ID, FIRST_INFO_ROW_ID, FIRST_MENU_ID, IMAGE_INFO_ID, build_children,
        push_image_info,
    };

    #[test]
    fn the_image_info_dialog_publishes_rows_and_buttons() {
        use crate::ui::image_info::{ImageInfo, ImageInfoPanel};
        use glam::Vec2;
        use rizref_codecs::{ImageBuffer, ImageFormat};
        use rizref_core::{Asset, Scene, SceneId};
        use rizref_format::{AssetOrigin, AssetProvenance};

        let mut scene = Scene::new(SceneId::random());
        let image = ImageBuffer::filled(2, 2, [0, 0, 0, 255]);
        let asset = Asset::from_decoded(
            std::sync::Arc::from(vec![1u8; 4]),
            ImageFormat::Png,
            &image,
            image.clone(),
        );
        let (asset, _) = scene.insert_asset(asset);
        let item = scene
            .insert_image(asset, Vec2::ZERO, Vec2::ONE)
            .expect("asset");
        let url = format!("https://e.com/{}.png", "long".repeat(80));
        let provenance = AssetProvenance {
            origin: AssetOrigin::Clipboard,
            source_path: None,
            source_url: Some(url.clone()),
        };
        let info = ImageInfo::from_scene(&scene, item, Some(&provenance)).expect("image");
        let dialog = ImageInfoPanel::new(info, None);

        let mut nodes = Vec::new();
        let mut actions = HashMap::new();
        push_image_info(&mut nodes, &mut actions, &dialog);

        let node = |id: u64| {
            nodes
                .iter()
                .find(|(node_id, _)| node_id.0 == id)
                .map(|(_, node)| node)
                .unwrap_or_else(|| panic!("node {id}"))
        };
        assert_eq!(node(IMAGE_INFO_ID).role(), accesskit::Role::Dialog);
        assert_eq!(
            node(FIRST_INFO_ROW_ID).value(),
            Some(url.as_str()),
            "the full URL, not the elided one"
        );
        assert_eq!(actions.len(), 4, "open, copy URL, copy id, close");
        assert_eq!(
            actions.get(&FIRST_INFO_BUTTON_ID),
            Some(&Action::OpenImageLocation)
        );
        assert_eq!(
            actions.get(&(FIRST_INFO_BUTTON_ID + 3)),
            Some(&Action::CloseImageInfo)
        );
        assert_eq!(node(FIRST_INFO_BUTTON_ID + 1).label(), Some("Copy URL"));
    }

    use crate::ui::menu::Action;
    use crate::ui::menu::MenuEntry;
    use std::collections::HashMap;

    /// The published node for the menu entry labelled `label`.
    fn menu_node(shortcuts: &crate::shortcuts::Shortcuts, label: &str) -> accesskit::Node {
        use crate::recents::Recents;
        use crate::settings::Settings;
        use crate::ui::menu::{MenuContext, build};

        let settings = Settings::default();
        let recents = Recents::default();
        let menu = build(&MenuContext {
            settings: &settings,
            shortcuts,
            recents: &recents,
            locked: false,
            ghost: false,
            click_through: false,
            always_on_top: false,
            opacity_percent: 100,
            note_selected: false,
            selected_image: None,
        });
        let mut nodes = Vec::new();
        let mut actions = HashMap::new();
        let mut next_id = FIRST_MENU_ID;
        build_children(&mut nodes, &mut actions, &mut next_id, &menu);
        nodes
            .into_iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some(label))
            .unwrap_or_else(|| panic!("a {label} menu node"))
    }

    #[test]
    fn menu_items_publish_their_keyboard_shortcut() {
        let shortcuts = crate::shortcuts::Shortcuts::new();

        let save = menu_node(&shortcuts, "Save");

        assert_eq!(save.keyboard_shortcut(), Some("Ctrl+S"));
        assert_eq!(
            save.description(),
            Some("Ctrl+S"),
            "what Narrator reads today"
        );
    }

    #[test]
    fn a_rebound_shortcut_is_what_the_menu_node_publishes() {
        use crate::shortcuts::{Key, KeyChord, ShortcutId};
        let mut shortcuts = crate::shortcuts::Shortcuts::new();
        shortcuts
            .bind(ShortcutId::Undo, KeyChord::ctrl(Key::U))
            .expect("free chord");

        let undo = menu_node(&shortcuts, "Undo");

        assert_eq!(undo.keyboard_shortcut(), Some("Ctrl+U"));
    }

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
