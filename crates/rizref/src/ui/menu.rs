//! The command menu: data model, layout, hit-testing, keyboard navigation and
//! painting.
//!
//! The menu is a *specification* first and a widget second. [`build`] turns the
//! application state into a tree of [`MenuEntry`] values; [`MenuState`] lays that
//! tree out at the cursor and turns pointer/keyboard input into an [`Action`].
//! The same tree is what the accessibility adapter publishes, so the menu, the
//! shortcut table and UI Automation cannot drift apart.
//!
//! Nothing here knows about the window or the event loop: every transition is a
//! pure function of layout and input, which is what makes keyboard navigation
//! testable without a desktop.

use std::path::PathBuf;

use glam::DVec2;

use crate::input::Command;
use crate::recents::Recents;
use crate::settings::{CanvasTheme, Settings};
use crate::shortcuts::{ShortcutId, Shortcuts};
use crate::ui::paint::{
    ACCENT, ACCENT_SOFT, DIM_TEXT, DISABLED_TEXT, HOVER_BG, PANEL_BORDER, Painter, TEXT, UiRect,
};

/// Width of a top-level menu column in logical pixels.
pub const ROOT_WIDTH: f32 = 252.0;

/// Width of a submenu column.
pub const SUBMENU_WIDTH: f32 = 236.0;

/// Height of one menu row.
pub const ROW_HEIGHT: f32 = 25.0;

/// Vertical padding above the first and below the last row.
pub const MENU_PADDING: f32 = 5.0;

/// Height of a separator row.
pub const SEPARATOR_HEIGHT: f32 = 9.0;

/// Horizontal padding inside a column.
const SIDE_PADDING: f32 = 10.0;

/// Room reserved for the checkmark column.
const CHECK_WIDTH: f32 = 20.0;

/// A menu command, after the static command table has been resolved against
/// the current application state.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A canvas/board command.
    Command(Command),
    /// Create a new empty board and switch to the canvas.
    NewBoard,
    /// Pick image files and import them.
    OpenImages,
    /// Paste files or a bitmap from the clipboard.
    Paste,
    /// Pick a `.rizref` board (loading lands in phase 06).
    OpenBoard,
    /// Open one recent board.
    OpenRecent(PathBuf),
    /// Remove a board from the recent list.
    RemoveRecent(PathBuf),
    /// Show a recent board in Explorer.
    OpenContainingFolder(PathBuf),
    /// Save (phase 06).
    Save,
    /// Save as (phase 06).
    SaveAs,
    /// Export (phase 06).
    Export,
    /// Return to the start view.
    ShowStart,
    /// Toggle the F1 shortcut overlay.
    ShowShortcuts,
    /// Toggle the settings dialog.
    ShowSettings,
    /// Show the version toast.
    About,
    /// Quit the application.
    Quit,
    /// Cancel the unsaved-changes close dialog.
    ConfirmCloseCancel,
    /// Discard unsaved changes and close the application.
    ConfirmCloseDiscard,
    /// Toggle always-on-top.
    ToggleAlwaysOnTop,
    /// Minimize the window (title bar).
    WindowMinimize,
    /// Maximize or restore the window (title bar).
    WindowToggleMaximize,
    /// Set ordinary opacity.
    SetOpacity(u8),
    /// Cycle ordinary opacity.
    CycleOpacity,
    /// Toggle ordinary click-through.
    ToggleClickThrough,
    /// Toggle ghost mode.
    ToggleGhostMode,
    /// Toggle borderless fullscreen.
    ToggleBorderless,
    /// Lock item editing (navigation still works).
    ToggleCanvasLock,
    /// Toggle the grid.
    ToggleGrid,
    /// Toggle snapping.
    ToggleSnap,
    /// Toggle the HUD.
    ToggleHud,
    /// Toggle the scene border.
    ToggleBorder,
    /// Toggle the checkerboard backdrop.
    ToggleCheckerboard,
    /// Toggle grayscale display.
    ToggleGrayscale,
    /// Choose a canvas background theme.
    SetTheme(CanvasTheme),
    /// Arm the colour picker.
    ColorPicker,
}

impl Action {
    /// The action a rebindable shortcut runs.
    pub fn from_shortcut(id: ShortcutId) -> Self {
        if let Some(command) = id.command() {
            return Self::Command(command);
        }
        match id {
            ShortcutId::Paste => Self::Paste,
            ShortcutId::NewBoard => Self::NewBoard,
            ShortcutId::OpenImages => Self::OpenImages,
            ShortcutId::Save => Self::Save,
            ShortcutId::SaveAs => Self::SaveAs,
            ShortcutId::Export => Self::Export,
            ShortcutId::ShowStart => Self::ShowStart,
            ShortcutId::ShowShortcuts => Self::ShowShortcuts,
            ShortcutId::ShowSettings => Self::ShowSettings,
            ShortcutId::ToggleAlwaysOnTop => Self::ToggleAlwaysOnTop,
            ShortcutId::CycleOpacity => Self::CycleOpacity,
            ShortcutId::ToggleClickThrough => Self::ToggleClickThrough,
            ShortcutId::ToggleGhostMode => Self::ToggleGhostMode,
            ShortcutId::Borderless => Self::ToggleBorderless,
            ShortcutId::ToggleGrid => Self::ToggleGrid,
            ShortcutId::ToggleSnap => Self::ToggleSnap,
            ShortcutId::ToggleHud => Self::ToggleHud,
            ShortcutId::ToggleBorder => Self::ToggleBorder,
            ShortcutId::ToggleCheckerboard => Self::ToggleCheckerboard,
            ShortcutId::ToggleGrayscale => Self::ToggleGrayscale,
            ShortcutId::LockCanvas => Self::ToggleCanvasLock,
            ShortcutId::ColorPicker => Self::ColorPicker,
            ShortcutId::Quit => Self::Quit,
            // Every remaining identifier is a board command handled above; a
            // new identifier without a mapping must fail a test, not silently
            // deselect the user's items.
            _ => unreachable!("every shortcut maps to an action"),
        }
    }
}

/// One entry in a menu tree.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    /// Visible label.
    pub label: String,
    /// Accelerator text, already formatted for the current bindings.
    pub accel: Option<String>,
    /// The rebindable shortcut this entry mirrors, if any.
    pub shortcut: Option<ShortcutId>,
    /// The action a leaf runs.
    pub action: Option<Action>,
    /// Submenu entries.
    pub children: Vec<MenuEntry>,
    /// Whether a checkmark is drawn.
    pub checked: bool,
    /// Whether this entry is a boolean toggle (always published as a
    /// checkbox, checked or not).
    pub toggles: bool,
    /// Whether the entry is selectable.
    pub enabled: bool,
    /// Whether this entry is a separator line.
    pub separator: bool,
}

impl MenuEntry {
    /// A plain leaf.
    pub fn item(label: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            accel: None,
            shortcut: None,
            action: Some(action),
            children: Vec::new(),
            checked: false,
            toggles: false,
            enabled: true,
            separator: false,
        }
    }

    /// A leaf whose accelerator is read from the live shortcut table.
    pub fn shortcut(label: impl Into<String>, id: ShortcutId, action: Action) -> Self {
        Self {
            label: label.into(),
            accel: None,
            shortcut: Some(id),
            action: Some(action),
            children: Vec::new(),
            checked: false,
            toggles: false,
            enabled: true,
            separator: false,
        }
    }

    /// A checked leaf.
    pub fn check(mut self, checked: bool) -> Self {
        self.checked = checked;
        self.toggles = true;
        self
    }

    /// Marks the entry disabled (shown dim, skipped by navigation).
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// A submenu.
    pub fn submenu(label: impl Into<String>, children: Vec<MenuEntry>) -> Self {
        Self {
            label: label.into(),
            accel: None,
            shortcut: None,
            action: None,
            children,
            checked: false,
            toggles: false,
            enabled: true,
            separator: false,
        }
    }

    /// A separator line.
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            accel: None,
            shortcut: None,
            action: None,
            children: Vec::new(),
            checked: false,
            toggles: false,
            enabled: false,
            separator: true,
        }
    }

    /// Whether the entry opens a submenu.
    pub fn opens_submenu(&self) -> bool {
        !self.children.is_empty()
    }

    /// Whether navigation may land on the entry.
    pub fn selectable(&self) -> bool {
        !self.separator && self.enabled
    }
}

/// Fills each entry's accelerator text from the live shortcut table.
fn accelerate(mut entries: Vec<MenuEntry>, shortcuts: &Shortcuts) -> Vec<MenuEntry> {
    for entry in &mut entries {
        if let Some(id) = entry.shortcut {
            entry.accel = shortcuts.chord_for(id).map(|chord| chord.to_string());
        }
        let children = std::mem::take(&mut entry.children);
        entry.children = accelerate(children, shortcuts);
    }
    entries
}

/// The current values the menu shows.
pub struct MenuContext<'a> {
    /// Persisted settings, for checkmarks.
    pub settings: &'a Settings,
    /// Live shortcut bindings, for accelerator labels.
    pub shortcuts: &'a Shortcuts,
    /// Recent boards, expanded in File → Open recent.
    pub recents: &'a Recents,
    /// Whether item editing is locked.
    pub locked: bool,
    /// Whether ghost mode is active.
    pub ghost: bool,
    /// Whether ordinary click-through is active.
    pub click_through: bool,
    /// Whether the window is always-on-top.
    pub always_on_top: bool,
    /// The ordinary opacity percent.
    pub opacity_percent: u8,
}

/// Builds the full command menu for the current state.
pub fn build(context: &MenuContext) -> Vec<MenuEntry> {
    use ShortcutId as S;
    let settings = context.settings;

    let mut recent_items = Vec::new();
    if context.recents.is_empty() {
        recent_items.push(MenuEntry::item("No recent boards", Action::ShowStart).disabled());
    } else {
        for entry in &context.recents.entries {
            recent_items.push(MenuEntry::item(
                format!("{}  ({} items)", entry.display_name(), entry.item_count),
                Action::OpenRecent(entry.path_buf()),
            ));
        }
    }

    let file = vec![
        MenuEntry::shortcut("New board", S::NewBoard, Action::NewBoard),
        MenuEntry::shortcut("Open images…", S::OpenImages, Action::OpenImages),
        MenuEntry::item("Open board…", Action::OpenBoard),
        MenuEntry::submenu("Open recent", recent_items),
        MenuEntry::separator(),
        MenuEntry::shortcut("Save", S::Save, Action::Save),
        MenuEntry::shortcut("Save as…", S::SaveAs, Action::SaveAs),
        MenuEntry::shortcut("Export…", S::Export, Action::Export),
        MenuEntry::separator(),
        MenuEntry::shortcut("Start view", S::ShowStart, Action::ShowStart),
        MenuEntry::shortcut("Quit", S::Quit, Action::Quit),
    ];

    let edit = vec![
        MenuEntry::shortcut("Undo", S::Undo, Action::from_shortcut(S::Undo)),
        MenuEntry::shortcut("Redo", S::Redo, Action::from_shortcut(S::Redo)),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Duplicate",
            S::DuplicateSelection,
            Action::from_shortcut(S::DuplicateSelection),
        ),
        MenuEntry::shortcut(
            "Delete",
            S::DeleteSelection,
            Action::from_shortcut(S::DeleteSelection),
        ),
        MenuEntry::separator(),
        MenuEntry::shortcut("Paste", S::Paste, Action::from_shortcut(S::Paste)),
        MenuEntry::shortcut(
            "Select all",
            S::SelectAll,
            Action::from_shortcut(S::SelectAll),
        ),
        MenuEntry::shortcut("Deselect", S::Deselect, Action::from_shortcut(S::Deselect)),
    ];

    let arrange = vec![
        MenuEntry::shortcut(
            "Grid",
            S::ArrangeGrid,
            Action::from_shortcut(S::ArrangeGrid),
        ),
        MenuEntry::shortcut(
            "Hierarchy",
            S::ArrangeHierarchy,
            Action::from_shortcut(S::ArrangeHierarchy),
        ),
        MenuEntry::shortcut(
            "Spread",
            S::ArrangeSpread,
            Action::from_shortcut(S::ArrangeSpread),
        ),
        MenuEntry::shortcut(
            "Stack",
            S::ArrangeStack,
            Action::from_shortcut(S::ArrangeStack),
        ),
    ];
    let transform = vec![
        MenuEntry::shortcut(
            "Flip horizontal",
            S::FlipHorizontal,
            Action::from_shortcut(S::FlipHorizontal),
        ),
        MenuEntry::shortcut(
            "Flip vertical",
            S::FlipVertical,
            Action::from_shortcut(S::FlipVertical),
        ),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Reset transform",
            S::ResetTransform,
            Action::from_shortcut(S::ResetTransform),
        ),
        MenuEntry::shortcut(
            "Match width",
            S::MatchWidth,
            Action::from_shortcut(S::MatchWidth),
        ),
        MenuEntry::shortcut(
            "Match height",
            S::MatchHeight,
            Action::from_shortcut(S::MatchHeight),
        ),
    ];
    let order = vec![
        MenuEntry::shortcut(
            "Raise",
            S::RaiseSelection,
            Action::from_shortcut(S::RaiseSelection),
        ),
        MenuEntry::shortcut(
            "Lower",
            S::LowerSelection,
            Action::from_shortcut(S::LowerSelection),
        ),
        MenuEntry::shortcut(
            "Bring to front",
            S::RaiseToTop,
            Action::from_shortcut(S::RaiseToTop),
        ),
        MenuEntry::shortcut(
            "Send to back",
            S::LowerToBottom,
            Action::from_shortcut(S::LowerToBottom),
        ),
    ];
    let align = vec![
        MenuEntry::shortcut("Left", S::AlignLeft, Action::from_shortcut(S::AlignLeft)),
        MenuEntry::shortcut("Right", S::AlignRight, Action::from_shortcut(S::AlignRight)),
        MenuEntry::shortcut("Top", S::AlignTop, Action::from_shortcut(S::AlignTop)),
        MenuEntry::shortcut(
            "Bottom",
            S::AlignBottom,
            Action::from_shortcut(S::AlignBottom),
        ),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Distribute horizontally",
            S::DistributeHorizontal,
            Action::from_shortcut(S::DistributeHorizontal),
        ),
        MenuEntry::shortcut(
            "Distribute vertically",
            S::DistributeVertical,
            Action::from_shortcut(S::DistributeVertical),
        ),
    ];
    let image = vec![
        MenuEntry::shortcut("Load images…", S::OpenImages, Action::OpenImages),
        MenuEntry::separator(),
        MenuEntry::submenu("Arrange", arrange),
        MenuEntry::submenu("Transform", transform),
        MenuEntry::submenu("Order", order),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Group",
            S::GroupSelection,
            Action::from_shortcut(S::GroupSelection),
        ),
        MenuEntry::shortcut(
            "Ungroup",
            S::UngroupSelection,
            Action::from_shortcut(S::UngroupSelection),
        ),
        MenuEntry::shortcut(
            "New note",
            S::CreateNote,
            Action::from_shortcut(S::CreateNote),
        ),
        MenuEntry::separator(),
        MenuEntry::submenu("Align", align),
    ];

    let zoom = vec![
        MenuEntry::shortcut(
            "Frame everything",
            S::FitContent,
            Action::from_shortcut(S::FitContent),
        ),
        MenuEntry::shortcut(
            "100%",
            S::ZoomHundred,
            Action::from_shortcut(S::ZoomHundred),
        ),
        MenuEntry::shortcut("Zoom in", S::ZoomIn, Action::from_shortcut(S::ZoomIn)),
        MenuEntry::shortcut("Zoom out", S::ZoomOut, Action::from_shortcut(S::ZoomOut)),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Reset view",
            S::ResetView,
            Action::from_shortcut(S::ResetView),
        ),
        MenuEntry::shortcut(
            "Reset rotation",
            S::ResetRotation,
            Action::from_shortcut(S::ResetRotation),
        ),
    ];
    let grid = vec![
        MenuEntry::shortcut("Show grid", S::ToggleGrid, Action::ToggleGrid)
            .check(settings.grid_visible),
        MenuEntry::shortcut("Snap to items", S::ToggleSnap, Action::ToggleSnap)
            .check(settings.snap_enabled),
        MenuEntry::shortcut("Scene border", S::ToggleBorder, Action::ToggleBorder)
            .check(settings.scene_border),
    ];
    let view = vec![
        MenuEntry::submenu("Zoom", zoom),
        MenuEntry::submenu("Grid", grid),
        MenuEntry::separator(),
        MenuEntry::shortcut("Grayscale", S::ToggleGrayscale, Action::ToggleGrayscale)
            .check(settings.grayscale),
        MenuEntry::shortcut(
            "Checkerboard",
            S::ToggleCheckerboard,
            Action::ToggleCheckerboard,
        )
        .check(settings.checkerboard),
        MenuEntry::shortcut("HUD readout", S::ToggleHud, Action::ToggleHud)
            .check(settings.hud_visible),
    ];

    let mut themes = Vec::new();
    for theme in CanvasTheme::ALL {
        themes.push(
            MenuEntry::item(theme.label(), Action::SetTheme(theme))
                .check(settings.canvas_theme == theme),
        );
    }
    let mut transparency = Vec::new();
    for percent in crate::platform::window_modes::OPACITY_STEPS {
        transparency.push(
            MenuEntry::item(format!("{percent}%"), Action::SetOpacity(percent))
                .check(context.opacity_percent == percent),
        );
    }
    transparency.push(MenuEntry::separator());
    transparency.push(
        MenuEntry::shortcut("Ghost mode", S::ToggleGhostMode, Action::ToggleGhostMode)
            .check(context.ghost),
    );

    let canvas = vec![
        MenuEntry::submenu("Background", themes),
        MenuEntry::shortcut("Lock canvas", S::LockCanvas, Action::ToggleCanvasLock)
            .check(context.locked),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Always on top",
            S::ToggleAlwaysOnTop,
            Action::ToggleAlwaysOnTop,
        )
        .check(context.always_on_top),
        MenuEntry::submenu("Transparency", transparency),
        MenuEntry::shortcut(
            "Click-through",
            S::ToggleClickThrough,
            Action::ToggleClickThrough,
        )
        .check(context.click_through),
        MenuEntry::shortcut("Ghost mode", S::ToggleGhostMode, Action::ToggleGhostMode)
            .check(context.ghost),
        MenuEntry::separator(),
        MenuEntry::shortcut(
            "Borderless fullscreen",
            S::Borderless,
            Action::ToggleBorderless,
        ),
    ];

    let tools = vec![MenuEntry::shortcut(
        "Colour picker",
        S::ColorPicker,
        Action::ColorPicker,
    )];
    let help = vec![
        MenuEntry::shortcut(
            "Keyboard shortcuts…",
            S::ShowShortcuts,
            Action::ShowShortcuts,
        ),
        MenuEntry::shortcut("Settings…", S::ShowSettings, Action::ShowSettings),
        MenuEntry::separator(),
        MenuEntry::item("About RizRef", Action::About),
    ];

    let root = vec![
        MenuEntry::submenu("File", file),
        MenuEntry::submenu("Edit", edit),
        MenuEntry::submenu("Image", image),
        MenuEntry::submenu("View", view),
        MenuEntry::submenu("Canvas", canvas),
        MenuEntry::submenu("Tools", tools),
        MenuEntry::submenu("Help", help),
    ];
    accelerate(root, context.shortcuts)
}

/// One laid-out row inside an open column.
#[derive(Debug, Clone)]
struct RowLayout {
    /// Index into the column's entry list.
    index: usize,
    /// The row rectangle.
    rect: UiRect,
}

/// One open menu column.
#[derive(Debug, Clone)]
struct MenuColumn {
    /// Entries shown in this column.
    entries: Vec<MenuEntry>,
    /// The rows, excluding separators.
    rows: Vec<RowLayout>,
    /// The panel rectangle.
    rect: UiRect,
}

/// What a key or click wants the application to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuResult {
    /// Nothing happened.
    None,
    /// The menu state changed and needs repainting.
    Changed,
    /// The menu closed (escape, click outside, a leaf was chosen).
    Closed,
    /// Run this action and close the menu.
    Action(Action),
}

/// An open menu at the cursor.
#[derive(Debug, Clone)]
pub struct MenuState {
    root: Vec<MenuEntry>,
    columns: Vec<MenuColumn>,
    /// Selected row index per open column level.
    selection: Vec<usize>,
    /// The point the menu opened at, logical pixels.
    origin: DVec2,
}

impl MenuState {
    /// Opens `root` at `origin`, clamped into `viewport` (logical pixels).
    pub fn open(root: Vec<MenuEntry>, origin: DVec2, viewport: DVec2) -> Self {
        let mut state = Self {
            root,
            columns: Vec::new(),
            selection: vec![0],
            origin,
        };
        state.relayout(viewport);
        state
    }

    /// The first column's rectangle (test helper).
    #[cfg(test)]
    pub fn bounds(&self) -> Option<UiRect> {
        self.columns.first().map(|column| column.rect)
    }

    /// Recomputes columns from `root`, selection path and viewport.
    fn relayout(&mut self, viewport: DVec2) {
        self.columns.clear();
        let mut entries = self.root.clone();
        let mut origin = self.origin;
        let mut level = 0;

        loop {
            let width = if level == 0 {
                ROOT_WIDTH
            } else {
                SUBMENU_WIDTH
            };
            let height = menu_height(&entries);
            let x = origin.x.min(viewport.x - f64::from(width) - 6.0).max(6.0);
            let y = origin.y.min(viewport.y - f64::from(height) - 6.0).max(6.0);
            let rect = UiRect::new(x as f32, y as f32, width, height);

            let mut rows = Vec::new();
            let mut row_y = y as f32 + MENU_PADDING;
            for (index, entry) in entries.iter().enumerate() {
                if entry.separator {
                    row_y += SEPARATOR_HEIGHT;
                    continue;
                }
                rows.push(RowLayout {
                    index,
                    rect: UiRect::new(rect.x, row_y, width, ROW_HEIGHT),
                });
                row_y += ROW_HEIGHT;
            }

            // Keep the selection on a selectable row.
            if self.selection.len() <= level {
                self.selection.push(0);
            }
            if !entries
                .get(self.selection[level])
                .is_some_and(MenuEntry::selectable)
            {
                self.selection[level] = first_selectable(&entries).unwrap_or(0);
            }

            let selected_entry = entries.get(self.selection[level]).cloned();
            self.columns.push(MenuColumn {
                entries,
                rows,
                rect,
            });

            let Some(selected) = selected_entry else {
                break;
            };
            if !selected.opens_submenu() || self.selection.len() <= level + 1 {
                break;
            }
            // Open the next column beside this one.
            let parent = &self.columns[level];
            let row_y = parent
                .rows
                .iter()
                .find(|row| row.index == self.selection[level])
                .map_or(parent.rect.y + MENU_PADDING, |row| row.rect.y);
            origin = DVec2::new(
                f64::from(parent.rect.right() - 4.0),
                f64::from(row_y - MENU_PADDING),
            );
            // Flip left when the submenu would leave the viewport.
            if origin.x + f64::from(SUBMENU_WIDTH) > viewport.x - 6.0 {
                origin.x = f64::from(parent.rect.x - SUBMENU_WIDTH + 4.0);
            }
            entries = selected.children;
            level += 1;
        }
    }

    /// Updates the hovered row and opens/closes submenus as needed.
    pub fn pointer_move(&mut self, position: DVec2, viewport: DVec2) -> bool {
        let Some(level) = self.column_at(position) else {
            return false;
        };
        let Some(index) = self.row_at(level, position) else {
            return false;
        };
        let entry = self.columns[level].entries[index].clone();
        let mut changed = false;
        if self.selection.get(level) != Some(&index) {
            self.selection.truncate(level + 1);
            if self.selection.len() <= level {
                self.selection.push(index);
            } else {
                self.selection[level] = index;
            }
            changed = true;
        }
        if entry.opens_submenu() {
            if self.selection.len() == level + 1 {
                self.selection
                    .push(first_selectable(&entry.children).unwrap_or(0));
                changed = true;
            }
        } else if self.selection.len() > level + 1 {
            self.selection.truncate(level + 1);
            changed = true;
        }
        if changed {
            self.relayout(viewport);
        }
        changed
    }

    /// Handles a click. A click outside every column closes the menu.
    pub fn pointer_click(&mut self, position: DVec2, viewport: DVec2) -> MenuResult {
        let Some(level) = self.column_at(position) else {
            return MenuResult::Closed;
        };
        let Some(index) = self.row_at(level, position) else {
            return MenuResult::None;
        };
        let entry = self.columns[level].entries[index].clone();
        if !entry.enabled {
            return MenuResult::None;
        }
        if entry.opens_submenu() {
            self.selection.truncate(level + 1);
            self.selection[level] = index;
            self.selection
                .push(first_selectable(&entry.children).unwrap_or(0));
            self.relayout(viewport);
            return MenuResult::Changed;
        }
        match entry.action {
            Some(action) => MenuResult::Action(action),
            None => MenuResult::None,
        }
    }

    /// Handles one keyboard event. `key` is already mapped to the shortcut
    /// vocabulary; only navigation keys are consumed here.
    pub fn key(&mut self, key: crate::shortcuts::Key, viewport: DVec2) -> MenuResult {
        use crate::shortcuts::Key;
        let level = self.selection.len().saturating_sub(1);
        match key {
            Key::Up => {
                self.step(-1);
                self.selection.truncate(level + 1);
                self.relayout(viewport);
                MenuResult::Changed
            }
            Key::Down => {
                self.step(1);
                self.selection.truncate(level + 1);
                self.relayout(viewport);
                MenuResult::Changed
            }
            Key::Right => {
                let Some(entry) = self
                    .columns
                    .get(level)
                    .and_then(|column| column.entries.get(self.selection[level]))
                else {
                    return MenuResult::None;
                };
                if entry.opens_submenu() {
                    self.selection
                        .push(first_selectable(&entry.children).unwrap_or(0));
                    self.relayout(viewport);
                    MenuResult::Changed
                } else {
                    MenuResult::None
                }
            }
            Key::Left => {
                if self.selection.len() > 1 {
                    self.selection.pop();
                    self.relayout(viewport);
                    MenuResult::Changed
                } else {
                    MenuResult::Closed
                }
            }
            Key::Escape => MenuResult::Closed,
            Key::Enter => {
                let entry = self
                    .columns
                    .get(level)
                    .and_then(|column| column.entries.get(self.selection[level]))
                    .cloned();
                let Some(entry) = entry else {
                    return MenuResult::None;
                };
                if !entry.enabled {
                    return MenuResult::None;
                }
                if entry.opens_submenu() {
                    self.selection
                        .push(first_selectable(&entry.children).unwrap_or(0));
                    self.relayout(viewport);
                    MenuResult::Changed
                } else {
                    match entry.action {
                        Some(action) => MenuResult::Action(action),
                        None => MenuResult::None,
                    }
                }
            }
            Key::Home => {
                self.selection[level] = first_selectable(&self.columns[level].entries).unwrap_or(0);
                self.selection.truncate(level + 1);
                self.relayout(viewport);
                MenuResult::Changed
            }
            Key::End => {
                self.selection[level] =
                    last_selectable(&self.columns[level].entries).unwrap_or(self.selection[level]);
                self.selection.truncate(level + 1);
                self.relayout(viewport);
                MenuResult::Changed
            }
            _ => MenuResult::None,
        }
    }

    /// Moves the current column's selection by one selectable row.
    fn step(&mut self, delta: isize) {
        let level = self.selection.len().saturating_sub(1);
        let Some(column) = self.columns.get(level) else {
            return;
        };
        let current = self.selection[level];
        let mut index = current as isize;
        for _ in 0..column.entries.len() {
            index += delta;
            if index < 0 {
                index = column.entries.len() as isize - 1;
            }
            if index >= column.entries.len() as isize {
                index = 0;
            }
            if column.entries[index as usize].selectable() {
                self.selection[level] = index as usize;
                return;
            }
        }
    }

    /// Which column contains `position`, if any.
    fn column_at(&self, position: DVec2) -> Option<usize> {
        self.columns
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, column)| {
                column
                    .rect
                    .contains(position.x as f32, position.y as f32)
                    .then_some(index)
            })
    }

    /// Which row of `level` contains `position`, if any.
    fn row_at(&self, level: usize, position: DVec2) -> Option<usize> {
        self.columns.get(level)?.rows.iter().find_map(|row| {
            row.rect
                .contains(position.x as f32, position.y as f32)
                .then_some(row.index)
        })
    }

    /// Paints every open column.
    pub fn paint(&self, painter: &mut Painter) {
        for (level, column) in self.columns.iter().enumerate() {
            painter.panel(column.rect, 7.0);
            for row in &column.rows {
                let entry = &column.entries[row.index];
                let selected = self.selection.get(level) == Some(&row.index);
                if selected && entry.enabled {
                    painter.fill_round_rect(row.rect.inset(3.0), 5.0, HOVER_BG);
                }
                let text_color = if !entry.enabled {
                    DISABLED_TEXT
                } else if selected {
                    TEXT
                } else {
                    [214, 216, 224, 255]
                };
                let text_x = row.rect.x + SIDE_PADDING + CHECK_WIDTH;
                let text_y = row.rect.y + (ROW_HEIGHT - 16.0) * 0.5;
                if entry.checked {
                    painter.text("✓", row.rect.x + SIDE_PADDING + 2.0, text_y, 13.0, ACCENT);
                }
                painter.text(&entry.label, text_x, text_y, 13.0, text_color);
                if entry.opens_submenu() {
                    painter.text(
                        "›",
                        row.rect.right() - SIDE_PADDING - 8.0,
                        text_y,
                        14.0,
                        if entry.enabled {
                            DIM_TEXT
                        } else {
                            DISABLED_TEXT
                        },
                    );
                } else if let Some(accel) = &entry.accel {
                    painter.text_right(
                        accel,
                        row.rect.right() - SIDE_PADDING,
                        text_y,
                        12.0,
                        if entry.enabled {
                            DIM_TEXT
                        } else {
                            DISABLED_TEXT
                        },
                    );
                }
                if selected && entry.enabled {
                    painter.fill_rect(
                        UiRect::new(row.rect.x + 2.0, row.rect.y + 4.0, 2.0, ROW_HEIGHT - 8.0),
                        ACCENT,
                    );
                }
            }
            // Separators: draw a thin line through each separator row.
            let mut y = column.rect.y + MENU_PADDING;
            for entry in &column.entries {
                if entry.separator {
                    painter.fill_rect(
                        UiRect::new(column.rect.x + 10.0, y + 4.0, column.rect.width - 20.0, 1.0),
                        [255, 255, 255, 26],
                    );
                    y += SEPARATOR_HEIGHT;
                } else {
                    y += ROW_HEIGHT;
                }
            }
            if level == 0 {
                painter.stroke_round_rect(column.rect, 7.0, 1.0, PANEL_BORDER);
            }
        }
        // A soft accent under the selected submenu parent ties the columns
        // together visually.
        if self.columns.len() > 1 {
            let parent = &self.columns[0];
            if let Some(row) = parent
                .rows
                .iter()
                .find(|row| row.index == self.selection.first().copied().unwrap_or(0))
            {
                painter.fill_rect(
                    UiRect::new(
                        row.rect.right() - 4.0,
                        row.rect.y + 5.0,
                        2.0,
                        ROW_HEIGHT - 10.0,
                    ),
                    ACCENT_SOFT,
                );
            }
        }
    }
}

/// The panel height of a column.
fn menu_height(entries: &[MenuEntry]) -> f32 {
    let mut height = MENU_PADDING * 2.0;
    for entry in entries {
        height += if entry.separator {
            SEPARATOR_HEIGHT
        } else {
            ROW_HEIGHT
        };
    }
    height
}

/// The first selectable entry index.
fn first_selectable(entries: &[MenuEntry]) -> Option<usize> {
    entries.iter().position(MenuEntry::selectable)
}

/// The last selectable entry index.
fn last_selectable(entries: &[MenuEntry]) -> Option<usize> {
    entries.iter().rposition(MenuEntry::selectable)
}

#[cfg(test)]
mod tests {
    use super::{Action, MenuContext, MenuEntry, MenuResult, MenuState, build};
    use crate::input::Command;
    use crate::recents::Recents;
    use crate::settings::Settings;
    use crate::shortcuts::Shortcuts;
    use glam::DVec2;

    fn context<'a>(
        settings: &'a Settings,
        shortcuts: &'a Shortcuts,
        recents: &'a Recents,
    ) -> MenuContext<'a> {
        MenuContext {
            settings,
            shortcuts,
            recents,
            locked: false,
            ghost: false,
            click_through: false,
            always_on_top: false,
            opacity_percent: 100,
        }
    }

    fn viewport() -> DVec2 {
        DVec2::new(1280.0, 800.0)
    }

    #[test]
    fn every_phase_04_editing_command_is_reachable_from_the_menu() {
        let settings = Settings::default();
        let shortcuts = Shortcuts::new();
        let recents = Recents::default();
        let menu = build(&context(&settings, &shortcuts, &recents));

        let mut actions = Vec::new();
        fn collect(entries: &[MenuEntry], actions: &mut Vec<Action>) {
            for entry in entries {
                if let Some(action) = &entry.action {
                    actions.push(action.clone());
                }
                collect(&entry.children, actions);
            }
        }
        collect(&menu, &mut actions);

        for command in [
            Command::Undo,
            Command::Redo,
            Command::SelectAll,
            Command::Deselect,
            Command::DuplicateSelection,
            Command::DeleteSelection,
            Command::FlipHorizontal,
            Command::FlipVertical,
            Command::ResetTransform,
            Command::MatchWidth,
            Command::MatchHeight,
            Command::GroupSelection,
            Command::UngroupSelection,
            Command::RaiseSelection,
            Command::LowerSelection,
            Command::RaiseToTop,
            Command::LowerToBottom,
            Command::ArrangeGrid,
            Command::ArrangeHierarchy,
            Command::ArrangeSpread,
            Command::ArrangeStack,
            Command::CreateNote,
            Command::AlignLeft,
            Command::AlignRight,
            Command::AlignTop,
            Command::AlignBottom,
            Command::DistributeHorizontal,
            Command::DistributeVertical,
            Command::FitContent,
            Command::ZoomHundred,
            Command::ResetRotation,
            Command::ResetView,
        ] {
            assert!(
                actions.contains(&Action::Command(command)),
                "{command:?} is not reachable from the menu"
            );
        }
    }

    #[test]
    fn every_shortcut_maps_to_an_action() {
        for id in crate::shortcuts::ShortcutId::ALL {
            let _ = Action::from_shortcut(id);
        }
    }

    #[test]
    fn accelerators_come_from_the_live_shortcut_table() {
        let settings = Settings::default();
        let mut shortcuts = Shortcuts::new();
        shortcuts
            .bind(
                crate::shortcuts::ShortcutId::Undo,
                crate::shortcuts::KeyChord::ctrl(crate::shortcuts::Key::U),
            )
            .expect("free chord");
        let recents = Recents::default();
        let menu = build(&context(&settings, &shortcuts, &recents));

        let undo = menu[1]
            .children
            .iter()
            .find(|entry| entry.label == "Undo")
            .expect("edit menu has undo");
        assert_eq!(undo.accel.as_deref(), Some("Ctrl+U"));
    }

    #[test]
    fn opening_clamps_the_menu_into_the_viewport() {
        let menu = MenuState::open(
            vec![MenuEntry::item("A", Action::ShowStart)],
            DVec2::new(5000.0, 5000.0),
            viewport(),
        );
        let bounds = menu.bounds().expect("a column");
        assert!(bounds.right() <= viewport().x as f32);
        assert!(bounds.bottom() <= viewport().y as f32);
    }

    #[test]
    fn keyboard_navigation_reaches_a_submenu_action() {
        let settings = Settings::default();
        let shortcuts = Shortcuts::new();
        let recents = Recents::default();
        let root = build(&context(&settings, &shortcuts, &recents));
        let mut menu = MenuState::open(root, DVec2::new(20.0, 20.0), viewport());

        // Open the File submenu, then walk from New board past Open board and
        // Open recent to Save.
        assert!(matches!(
            menu.key(crate::shortcuts::Key::Right, viewport()),
            MenuResult::Changed
        ));
        for _ in 0..4 {
            assert!(matches!(
                menu.key(crate::shortcuts::Key::Down, viewport()),
                MenuResult::Changed
            ));
        }
        match menu.key(crate::shortcuts::Key::Enter, viewport()) {
            MenuResult::Action(Action::Save) => {}
            other => panic!("expected Save, got {other:?}"),
        }
    }

    #[test]
    fn submenu_opens_to_the_right_and_closes_to_the_left() {
        let mut menu = MenuState::open(
            vec![MenuEntry::submenu(
                "Sub",
                vec![MenuEntry::item("Child", Action::ShowSettings)],
            )],
            DVec2::new(100.0, 100.0),
            viewport(),
        );
        assert!(matches!(
            menu.key(crate::shortcuts::Key::Right, viewport()),
            MenuResult::Changed
        ));
        assert!(matches!(
            menu.key(crate::shortcuts::Key::Left, viewport()),
            MenuResult::Changed
        ));
        assert!(matches!(
            menu.key(crate::shortcuts::Key::Left, viewport()),
            MenuResult::Closed
        ));
    }

    #[test]
    fn disabled_rows_are_skipped() {
        let mut menu = MenuState::open(
            vec![
                MenuEntry::item("Disabled", Action::ShowStart).disabled(),
                MenuEntry::item("Enabled", Action::ShowSettings),
            ],
            DVec2::new(10.0, 10.0),
            DVec2::new(400.0, 400.0),
        );
        // The initial selection already skips the disabled first row.
        match menu.key(crate::shortcuts::Key::Enter, DVec2::new(400.0, 400.0)) {
            MenuResult::Action(Action::ShowSettings) => {}
            other => panic!("expected the enabled row, got {other:?}"),
        }
    }
}
