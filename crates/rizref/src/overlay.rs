//! Screen-space overlay geometry: outlines, handles, guides and the marquee.
//!
//! Everything here is built in logical viewport pixels on the CPU: item
//! outlines are the item corners projected through the camera, handles are
//! placed at screen-constant sizes, and guide lines are one pixel wide at any
//! zoom. That is what makes handles usable at 5% and 3200% without a second
//! world-space pipeline.
//!
//! The result is a flat triangle list uploaded once per frame. A selection of a
//! hundred items is a few thousand vertices — nothing next to one present.

use glam::{DVec2, Vec2};

use crate::note_chrome::{NoteChrome, PillButton, ScreenRect, TextBarButton, ToolbarButton};
use rizref_core::{
    Camera, Guide, Handle as SelectionHandle, Item, Rect, Scene, Selection, SelectionFrame,
    SnapAxis,
};

/// One overlay vertex: a logical-pixel position plus a straight-alpha colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayVertex {
    /// Position in logical viewport pixels.
    pub pos: [f32; 2],
    /// Straight-alpha RGBA.
    pub color: [f32; 4],
}

impl OverlayVertex {
    /// The vertex as its 24 GPU bytes.
    pub fn bytes(&self) -> [u8; 24] {
        let mut bytes = [0u8; 24];
        for (slot, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip([
            self.pos[0],
            self.pos[1],
            self.color[0],
            self.color[1],
            self.color[2],
            self.color[3],
        ]) {
            slot.copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

/// Selection accent.
pub const ACCENT: [f32; 4] = [0.30, 0.62, 1.0, 1.0];
/// Hover outline.
pub const HOVER: [f32; 4] = [0.72, 0.85, 1.0, 0.95];
/// Smart-guide line.
pub const GUIDE: [f32; 4] = [1.0, 0.78, 0.25, 0.92];
/// Marquee border.
pub const MARQUEE_BORDER: [f32; 4] = [0.45, 0.72, 1.0, 0.9];
/// Marquee fill.
pub const MARQUEE_FILL: [f32; 4] = [0.30, 0.62, 1.0, 0.10];
/// Handle body behind the accent square.
pub const HANDLE_BODY: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
/// Soft shadow drawn behind a selected item's outline.
pub const SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.30];

/// Edge width of a selection outline, in logical pixels.
pub const OUTLINE_WIDTH: f32 = 1.5;
/// Edge width of the hover outline, in logical pixels.
pub const HOVER_WIDTH: f32 = 2.0;
/// Half-size of a scale handle's dark body, in logical pixels.
pub const HANDLE_HALF: f32 = 4.5;
/// Half-size of a scale handle's accent square, in logical pixels.
pub const HANDLE_ACCENT_HALF: f32 = 3.5;
/// Radius of the rotate handle's diamond, in logical pixels.
pub const ROTATE_RADIUS: f32 = 5.0;
/// Gap between the north edge and the rotate handle, in logical pixels.
pub const ROTATE_GAP: f32 = 24.0;
/// How far a guide is extended past the rectangles it connects.
pub const GUIDE_OVERHANG: f32 = 8.0;

/// Everything the overlay needs for one frame.
pub struct OverlayState<'a> {
    /// The view transform.
    pub camera: &'a Camera,
    /// Viewport size in logical pixels.
    pub viewport: DVec2,
    /// The board, for item outlines.
    pub scene: &'a Scene,
    /// The current selection.
    pub selection: &'a Selection,
    /// The item under the pointer, when not selected.
    pub hover: Option<rizref_core::ItemId>,
    /// The marquee rectangle, in world units.
    pub marquee: Option<Rect>,
    /// The smart guides of the current drag.
    pub guides: &'a [Guide],
    /// Whether the selection may be rotated (false for note-only
    /// selections: notes are never rotated, so they get no rotate handle).
    pub rotatable: bool,
    /// Whether only the four corner handles are drawn (a selection with a
    /// text item, which is only ever scaled, never stretched).
    pub corners_only: bool,
    /// The note toolbar or edit pill, drawn on top of everything.
    pub chrome: Option<NoteChrome>,
    /// The text caret of the note or text item being edited, while it is
    /// in the visible half of its blink.
    pub caret: Option<OverlayCaret>,
}

/// A text caret as a screen-space segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayCaret {
    /// The caret's top, in logical pixels.
    pub top: Vec2,
    /// The caret's bottom, in logical pixels.
    pub bottom: Vec2,
    /// Straight-alpha RGBA.
    pub color: [f32; 4],
}

/// Width of the caret, in logical pixels: wide enough to find at a glance at
/// any zoom, where the old in-texture caret was one texture pixel.
pub const CARET_WIDTH: f32 = 2.0;
/// The dark halo behind the caret, so it reads on light and dark text alike.
const CARET_HALO: [f32; 4] = [0.0, 0.0, 0.0, 0.45];

impl OverlayState<'_> {
    fn screen(&self, world: Vec2) -> Vec2 {
        self.camera
            .world_to_screen(world.as_dvec2(), self.viewport)
            .as_vec2()
    }
}

/// Builds the frame's overlay triangle list.
pub fn build(state: &OverlayState) -> Vec<OverlayVertex> {
    let mut vertices = Vec::new();
    let frame = SelectionFrame::for_selection(state.scene, state.selection);

    // Selected items: a soft shadow behind each oriented outline.
    for id in state.selection.iter() {
        let Some(item) = state.scene.item(id) else {
            continue;
        };
        let corners = item
            .transform()
            .corners()
            .map(|corner| state.screen(corner));
        rect_outline(
            &mut vertices,
            corners.map(|corner| corner + Vec2::splat(1.5)),
            3.0,
            SHADOW,
        );
        rect_outline(&mut vertices, corners, OUTLINE_WIDTH, ACCENT);
    }

    // Hover outline, unless the hovered item is already selected.
    if let Some(id) = state.hover
        && !state.selection.contains(id)
        && let Some(item) = state.scene.item(id)
    {
        outline_item(&mut vertices, state, item, HOVER_WIDTH, HOVER);
    }

    // The selection frame and handles.
    if let Some(frame) = frame {
        let multi = state.selection.len() > 1;
        if multi {
            rect_outline(
                &mut vertices,
                frame.corners().map(|c| state.screen(c)),
                OUTLINE_WIDTH,
                ACCENT,
            );
        }
        let rotate_offset = ROTATE_GAP / state.camera.zoom as f32;
        let north = state.screen(frame.handle_position(SelectionHandle::N, 0.0));
        let rotate = state.screen(frame.handle_position(SelectionHandle::Rotate, rotate_offset));
        if state.rotatable {
            line(&mut vertices, north, rotate, 1.0, ACCENT);
        }
        for handle in SelectionHandle::SCALE {
            if state.corners_only && !handle.is_corner() {
                continue;
            }
            let center = state.screen(frame.handle_position(handle, 0.0));
            square(&mut vertices, center, HANDLE_HALF, HANDLE_BODY);
            square(&mut vertices, center, HANDLE_ACCENT_HALF, ACCENT);
        }
        if state.rotatable {
            diamond(&mut vertices, rotate, ROTATE_RADIUS, HANDLE_BODY);
            diamond(&mut vertices, rotate, ROTATE_RADIUS - 1.0, ACCENT);
        }
    }

    // The marquee: a translucent fill and a border.
    if let Some(marquee) = state.marquee {
        let min = state.screen(marquee.min);
        let max = state.screen(marquee.max());
        fill_rect(&mut vertices, min, max, MARQUEE_FILL);
        rect_outline(
            &mut vertices,
            [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)],
            1.0,
            MARQUEE_BORDER,
        );
    }

    // Smart guides.
    for guide in state.guides {
        let (a, b) = match guide.axis {
            SnapAxis::X => (
                state.screen(Vec2::new(guide.position, guide.start)),
                state.screen(Vec2::new(guide.position, guide.end)),
            ),
            SnapAxis::Y => (
                state.screen(Vec2::new(guide.start, guide.position)),
                state.screen(Vec2::new(guide.end, guide.position)),
            ),
        };
        let direction = b - a;
        let extend = if direction.length_squared() > 1.0e-6 {
            direction.normalize() * GUIDE_OVERHANG
        } else {
            Vec2::ZERO
        };
        line(&mut vertices, a - extend, b + extend, 1.0, GUIDE);
    }

    if let Some(caret) = &state.caret {
        line(
            &mut vertices,
            caret.top,
            caret.bottom,
            CARET_WIDTH + 2.0,
            CARET_HALO,
        );
        line(
            &mut vertices,
            caret.top,
            caret.bottom,
            CARET_WIDTH,
            caret.color,
        );
    }

    if let Some(chrome) = &state.chrome {
        note_chrome(&mut vertices, chrome);
    }

    vertices
}

/// The chrome pill colour.
const CHROME_PILL: [f32; 4] = [0.08, 0.08, 0.10, 0.82];
/// A hovered chrome button.
const CHROME_HOVER: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
/// Chrome icon strokes.
const CHROME_ICON: [f32; 4] = [0.95, 0.93, 0.90, 1.0];
/// The confirm tick.
const CHROME_CONFIRM: [f32; 4] = [0.45, 0.85, 0.50, 1.0];
/// A format toggle that is on.
const CHROME_ACTIVE: [f32; 4] = [0.30, 0.62, 1.0, 0.45];

/// A polyline of `width` logical pixels through `points` around `c`.
fn polyline(vertices: &mut Vec<OverlayVertex>, c: Vec2, points: &[(f32, f32)], width: f32) {
    for pair in points.windows(2) {
        let a = c + Vec2::new(pair[0].0, pair[0].1);
        let b = c + Vec2::new(pair[1].0, pair[1].1);
        line(vertices, a, b, width, CHROME_ICON);
        // A square joint so corners do not notch.
        square(vertices, b, width * 0.5, CHROME_ICON);
    }
}

/// The ✗ icon.
fn cancel_icon(vertices: &mut Vec<OverlayVertex>, c: Vec2) {
    line(
        vertices,
        c - Vec2::splat(6.0),
        c + Vec2::splat(6.0),
        2.0,
        CHROME_ICON,
    );
    line(
        vertices,
        c + Vec2::new(-6.0, 6.0),
        c + Vec2::new(6.0, -6.0),
        2.0,
        CHROME_ICON,
    );
}

/// The ✓ icon.
fn confirm_icon(vertices: &mut Vec<OverlayVertex>, c: Vec2) {
    line(
        vertices,
        c + Vec2::new(-7.0, 0.0),
        c + Vec2::new(-2.0, 5.0),
        2.5,
        CHROME_CONFIRM,
    );
    line(
        vertices,
        c + Vec2::new(-2.0, 5.0),
        c + Vec2::new(7.0, -6.0),
        2.5,
        CHROME_CONFIRM,
    );
}

/// Draws one format-bar button's icon centred on `c`.
fn text_bar_icon(
    vertices: &mut Vec<OverlayVertex>,
    button: TextBarButton,
    c: Vec2,
    colour: [u8; 3],
) {
    match button {
        TextBarButton::Bold => {
            // A heavy "B": a stem and two bowls.
            line(
                vertices,
                c + Vec2::new(-4.0, -7.0),
                c + Vec2::new(-4.0, 7.0),
                3.0,
                CHROME_ICON,
            );
            polyline(
                vertices,
                c,
                &[
                    (-4.0, -7.0),
                    (1.5, -7.0),
                    (4.0, -5.0),
                    (4.0, -2.0),
                    (1.5, 0.0),
                    (-4.0, 0.0),
                ],
                2.5,
            );
            polyline(
                vertices,
                c,
                &[
                    (-4.0, 0.0),
                    (2.5, 0.0),
                    (5.0, 2.5),
                    (5.0, 4.5),
                    (2.5, 7.0),
                    (-4.0, 7.0),
                ],
                2.5,
            );
        }
        TextBarButton::Italic => {
            // A slanted "I" with serifs.
            line(
                vertices,
                c + Vec2::new(3.0, -7.0),
                c + Vec2::new(-3.0, 7.0),
                2.0,
                CHROME_ICON,
            );
            line(
                vertices,
                c + Vec2::new(0.0, -7.0),
                c + Vec2::new(6.0, -7.0),
                1.5,
                CHROME_ICON,
            );
            line(
                vertices,
                c + Vec2::new(-6.0, 7.0),
                c + Vec2::new(0.0, 7.0),
                1.5,
                CHROME_ICON,
            );
        }
        TextBarButton::Underline => {
            // A "U" over a bar.
            polyline(
                vertices,
                c,
                &[
                    (-5.0, -8.0),
                    (-5.0, 0.5),
                    (-3.0, 3.5),
                    (3.0, 3.5),
                    (5.0, 0.5),
                    (5.0, -8.0),
                ],
                2.0,
            );
            line(
                vertices,
                c + Vec2::new(-6.5, 7.5),
                c + Vec2::new(6.5, 7.5),
                1.5,
                CHROME_ICON,
            );
        }
        TextBarButton::Smaller | TextBarButton::Larger => {
            // An "A" with a minus or a plus beside it; the smaller button's
            // letter is smaller.
            let (scale, offset) = if button == TextBarButton::Smaller {
                (0.7, Vec2::new(-3.0, 1.5))
            } else {
                (1.0, Vec2::new(-3.0, 0.0))
            };
            let a = c + offset;
            let points =
                [(-5.0, 7.0), (0.0, -7.0), (5.0, 7.0)].map(|(x, y)| (x * scale, y * scale));
            polyline(vertices, a, &points, 2.0);
            line(
                vertices,
                a + Vec2::new(-2.6, 2.0) * scale,
                a + Vec2::new(2.6, 2.0) * scale,
                1.5,
                CHROME_ICON,
            );
            let sign = c + Vec2::new(6.5, -4.0);
            line(
                vertices,
                sign - Vec2::new(3.0, 0.0),
                sign + Vec2::new(3.0, 0.0),
                1.5,
                CHROME_ICON,
            );
            if button == TextBarButton::Larger {
                line(
                    vertices,
                    sign - Vec2::new(0.0, 3.0),
                    sign + Vec2::new(0.0, 3.0),
                    1.5,
                    CHROME_ICON,
                );
            }
        }
        TextBarButton::Colour => {
            // An "A" over a bar in the current colour.
            polyline(vertices, c, &[(-5.0, 3.0), (0.0, -8.0), (5.0, 3.0)], 2.0);
            line(
                vertices,
                c + Vec2::new(-2.6, -1.5),
                c + Vec2::new(2.6, -1.5),
                1.5,
                CHROME_ICON,
            );
            let rgb = colour.map(|v| f32::from(v) / 255.0);
            let min = c + Vec2::new(-7.0, 5.5);
            let max = c + Vec2::new(7.0, 9.5);
            fill_rect(vertices, min, max, [rgb[0], rgb[1], rgb[2], 1.0]);
            rect_outline(
                vertices,
                [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)],
                1.0,
                CHROME_ICON,
            );
        }
        TextBarButton::Cancel => cancel_icon(vertices, c),
        TextBarButton::Confirm => confirm_icon(vertices, c),
    }
}

/// Draws the note toolbar or the edit pill as screen-space geometry.
fn note_chrome(vertices: &mut Vec<OverlayVertex>, chrome: &NoteChrome) {
    let fill = |vertices: &mut Vec<OverlayVertex>, rect: ScreenRect, color| {
        fill_rect(vertices, rect.min, rect.max, color);
    };
    match chrome {
        NoteChrome::Toolbar {
            layout,
            background,
            hovered,
        } => {
            fill(vertices, layout.pill, CHROME_PILL);
            for &(button, rect) in layout.buttons() {
                if *hovered == Some(button) {
                    fill(vertices, rect, CHROME_HOVER);
                }
                let c = (rect.min + rect.max) * 0.5;
                match button {
                    ToolbarButton::Background => {
                        let swatch = ScreenRect {
                            min: c - Vec2::splat(8.0),
                            max: c + Vec2::splat(8.0),
                        };
                        let rgb = background.map(|v| f32::from(v) / 255.0);
                        fill(vertices, swatch, [rgb[0], rgb[1], rgb[2], 1.0]);
                        let corners = [
                            swatch.min,
                            Vec2::new(swatch.max.x, swatch.min.y),
                            swatch.max,
                            Vec2::new(swatch.min.x, swatch.max.y),
                        ];
                        rect_outline(vertices, corners, 1.0, CHROME_ICON);
                    }
                    ToolbarButton::Edit => {
                        // A pencil: a slanted body and its tip.
                        let a = c + Vec2::new(5.0, -6.0);
                        let b = c + Vec2::new(-4.0, 3.0);
                        line(vertices, a, b, 3.0, CHROME_ICON);
                        line(vertices, b, c + Vec2::new(-6.0, 6.0), 1.5, CHROME_ICON);
                    }
                    ToolbarButton::Delete => {
                        // A trash can: lid, handle, body and ribs.
                        line(
                            vertices,
                            c + Vec2::new(-6.0, -5.0),
                            c + Vec2::new(6.0, -5.0),
                            1.5,
                            CHROME_ICON,
                        );
                        line(
                            vertices,
                            c + Vec2::new(-2.0, -7.0),
                            c + Vec2::new(2.0, -7.0),
                            1.5,
                            CHROME_ICON,
                        );
                        rect_outline(
                            vertices,
                            [
                                c + Vec2::new(-4.5, -4.0),
                                c + Vec2::new(4.5, -4.0),
                                c + Vec2::new(4.0, 7.0),
                                c + Vec2::new(-4.0, 7.0),
                            ],
                            1.5,
                            CHROME_ICON,
                        );
                        line(
                            vertices,
                            c + Vec2::new(0.0, -2.0),
                            c + Vec2::new(0.0, 5.0),
                            1.0,
                            CHROME_ICON,
                        );
                    }
                }
            }
        }
        NoteChrome::Pill { layout, hovered } => {
            fill(vertices, layout.pill, CHROME_PILL);
            for (button, rect) in [
                (PillButton::Cancel, layout.cancel),
                (PillButton::Confirm, layout.confirm),
            ] {
                if *hovered == Some(button) {
                    fill(vertices, rect, CHROME_HOVER);
                }
                let c = (rect.min + rect.max) * 0.5;
                match button {
                    PillButton::Cancel => cancel_icon(vertices, c),
                    PillButton::Confirm => confirm_icon(vertices, c),
                }
            }
        }
        NoteChrome::TextBar {
            layout,
            bold,
            italic,
            underline,
            colour,
            hovered,
        } => {
            fill(vertices, layout.pill, CHROME_PILL);
            line(
                vertices,
                Vec2::new(layout.divider_x, layout.pill.min.y + 7.0),
                Vec2::new(layout.divider_x, layout.pill.max.y - 7.0),
                1.0,
                CHROME_HOVER,
            );
            for (button, rect) in layout.buttons {
                let active = match button {
                    TextBarButton::Bold => *bold,
                    TextBarButton::Italic => *italic,
                    TextBarButton::Underline => *underline,
                    _ => false,
                };
                if active {
                    fill(vertices, rect, CHROME_ACTIVE);
                }
                if *hovered == Some(button) {
                    fill(vertices, rect, CHROME_HOVER);
                }
                text_bar_icon(vertices, button, (rect.min + rect.max) * 0.5, *colour);
            }
        }
    }
}

/// Appends one item's outline.
fn outline_item(
    vertices: &mut Vec<OverlayVertex>,
    state: &OverlayState,
    item: &Item,
    width: f32,
    color: [f32; 4],
) {
    let corners = item.transform().corners().map(|c| state.screen(c));
    rect_outline(vertices, corners, width, color);
}

/// Appends a quad as two triangles.
fn quad(vertices: &mut Vec<OverlayVertex>, corners: [Vec2; 4], color: [f32; 4]) {
    for index in [0, 1, 2, 2, 1, 3] {
        vertices.push(OverlayVertex {
            pos: [corners[index].x, corners[index].y],
            color,
        });
    }
}

/// Appends a filled screen-space rectangle.
fn fill_rect(vertices: &mut Vec<OverlayVertex>, min: Vec2, max: Vec2, color: [f32; 4]) {
    quad(
        vertices,
        [min, Vec2::new(max.x, min.y), Vec2::new(min.x, max.y), max],
        color,
    );
}

/// Appends a line segment of `width` logical pixels.
fn line(vertices: &mut Vec<OverlayVertex>, a: Vec2, b: Vec2, width: f32, color: [f32; 4]) {
    let direction = b - a;
    let length = direction.length();
    if length < 1.0e-4 {
        return;
    }
    let normal = Vec2::new(-direction.y, direction.x) / length * (width * 0.5);
    quad(
        vertices,
        [a - normal, b - normal, a + normal, b + normal],
        color,
    );
}

/// Appends the four edges of a screen-space quad.
fn rect_outline(
    vertices: &mut Vec<OverlayVertex>,
    corners: [Vec2; 4],
    width: f32,
    color: [f32; 4],
) {
    for index in 0..4 {
        line(
            vertices,
            corners[index],
            corners[(index + 1) % 4],
            width,
            color,
        );
    }
}

/// Appends an axis-aligned square.
fn square(vertices: &mut Vec<OverlayVertex>, center: Vec2, half: f32, color: [f32; 4]) {
    let extent = Vec2::splat(half);
    quad(
        vertices,
        [
            center - extent,
            Vec2::new(center.x + half, center.y - half),
            Vec2::new(center.x - half, center.y + half),
            center + extent,
        ],
        color,
    );
}

/// Appends a diamond (a square turned 45°), used for the rotate handle.
fn diamond(vertices: &mut Vec<OverlayVertex>, center: Vec2, radius: f32, color: [f32; 4]) {
    quad(
        vertices,
        [
            Vec2::new(center.x, center.y - radius),
            Vec2::new(center.x + radius, center.y),
            Vec2::new(center.x, center.y + radius),
            Vec2::new(center.x - radius, center.y),
        ],
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::{OverlayState, build};
    use glam::{DVec2, Vec2};
    use rizref_core::{Camera, Guide, ItemId, Scene, SceneId, Selection, SnapAxis};

    fn scene() -> (Scene, ItemId) {
        let mut scene = Scene::new(SceneId::from_bytes([1; 16]));
        let id = scene.insert_note("note", Vec2::new(100.0, 50.0), Vec2::new(80.0, 40.0));
        (scene, id)
    }

    fn state<'a>(
        scene: &'a Scene,
        selection: &'a Selection,
        camera: &'a Camera,
    ) -> OverlayState<'a> {
        OverlayState {
            camera,
            viewport: DVec2::new(800.0, 600.0),
            scene,
            selection,
            hover: None,
            marquee: None,
            guides: &[],
            rotatable: true,
            corners_only: false,
            chrome: None,
            caret: None,
        }
    }

    /// The bounding box of vertices near `center`.
    fn nearby(
        vertices: &[super::OverlayVertex],
        center: Vec2,
        radius: f32,
    ) -> Option<(Vec2, Vec2)> {
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        let mut found = false;
        for vertex in vertices {
            let position = Vec2::new(vertex.pos[0], vertex.pos[1]);
            if (position - center).length() <= radius {
                min = min.min(position);
                max = max.max(position);
                found = true;
            }
        }
        found.then_some((min, max))
    }

    #[test]
    fn a_selection_produces_outlines_and_handles() {
        let (scene, id) = scene();
        let selection: Selection = [id].into_iter().collect();
        let camera = Camera::default();

        let vertices = build(&state(&scene, &selection, &camera));

        assert!(!vertices.is_empty());
        assert!(
            vertices.iter().any(|vertex| vertex.color == super::SHADOW),
            "a selected item casts its outline shadow"
        );
        // A 10 x 10 logical pixel handle body near each corner.
        let frame = rizref_core::SelectionFrame::for_selection(&scene, &selection).expect("frame");
        for handle in rizref_core::Handle::SCALE {
            let center = camera
                .world_to_screen(
                    frame.handle_position(handle, 0.0).as_dvec2(),
                    DVec2::new(800.0, 600.0),
                )
                .as_vec2();
            let (min, max) = nearby(&vertices, center, 7.0)
                .unwrap_or_else(|| panic!("no handle near {handle:?}"));
            assert!((max - min).x >= 8.0 && (max - min).x <= 10.0);
            assert!((max - min).y >= 8.0 && (max - min).y <= 10.0);
        }
    }

    #[test]
    fn handles_stay_screen_constant_at_every_zoom() {
        // A large item keeps the eight handles apart even at 5%.
        let mut scene = Scene::new(SceneId::from_bytes([2; 16]));
        let id = scene.insert_note("big", Vec2::ZERO, Vec2::new(4000.0, 3000.0));
        let selection: Selection = [id].into_iter().collect();
        for zoom in [0.05, 0.25, 1.0, 8.0, 32.0] {
            let camera = Camera::new(DVec2::ZERO, zoom, 0.0);
            let vertices = build(&state(&scene, &selection, &camera));
            let frame =
                rizref_core::SelectionFrame::for_selection(&scene, &selection).expect("frame");
            let center = camera
                .world_to_screen(
                    frame
                        .handle_position(rizref_core::Handle::Nw, 0.0)
                        .as_dvec2(),
                    DVec2::new(800.0, 600.0),
                )
                .as_vec2();
            let (min, max) = nearby(&vertices, center, 7.0)
                .unwrap_or_else(|| panic!("no handle at zoom {zoom}"));
            assert!(
                ((max - min).x - 9.0).abs() <= 0.5 && ((max - min).y - 9.0).abs() <= 0.5,
                "handle changed size at zoom {zoom}: {:?}",
                max - min
            );
        }
    }

    #[test]
    fn the_marquee_is_filled_and_outlined() {
        let (scene, _) = scene();
        let selection = Selection::new();
        let camera = Camera::default();
        let mut overlay = state(&scene, &selection, &camera);
        overlay.marquee = Some(rizref_core::Rect::from_min_max(
            Vec2::new(10.0, 20.0),
            Vec2::new(110.0, 70.0),
        ));

        let vertices = build(&overlay);

        assert!(vertices.len() >= 12);
        assert!(vertices.iter().any(|v| v.color == super::MARQUEE_FILL));
    }

    #[test]
    fn guides_are_one_pixel_lines_with_the_right_orientation() {
        let (scene, _) = scene();
        let selection = Selection::new();
        let camera = Camera::default();
        let guides = [
            Guide {
                axis: SnapAxis::X,
                position: 100.0,
                start: 0.0,
                end: 50.0,
            },
            Guide {
                axis: SnapAxis::Y,
                position: 50.0,
                start: 0.0,
                end: 100.0,
            },
        ];
        let mut overlay = state(&scene, &selection, &camera);
        overlay.guides = &guides;

        let vertices = build(&overlay);

        // The x guide is vertical at screen x = 500 (100 world + viewport/2).
        let vertical: Vec<&super::OverlayVertex> = vertices
            .iter()
            .filter(|v| v.color == super::GUIDE && (v.pos[0] - 500.0).abs() <= 0.6)
            .collect();
        assert!(!vertical.is_empty(), "the vertical guide is drawn");
        assert!(vertical.iter().all(|v| (v.pos[0] - 500.0).abs() <= 0.6));
    }

    #[test]
    fn a_text_selection_draws_only_corner_handles() {
        let (scene, id) = scene();
        let selection: Selection = [id].into_iter().collect();
        let camera = Camera::default();
        let frame = rizref_core::SelectionFrame::for_selection(&scene, &selection).expect("frame");
        let mut overlay = state(&scene, &selection, &camera);
        overlay.corners_only = true;
        let vertices = build(&overlay);
        for handle in rizref_core::Handle::SCALE {
            let center = camera
                .world_to_screen(
                    frame.handle_position(handle, 0.0).as_dvec2(),
                    DVec2::new(800.0, 600.0),
                )
                .as_vec2();
            let body = vertices.iter().any(|v| {
                v.color == super::HANDLE_BODY
                    && (Vec2::new(v.pos[0], v.pos[1]) - center).length() < 7.0
            });
            assert_eq!(body, handle.is_corner(), "{handle:?}");
        }
    }

    #[test]
    fn the_caret_is_a_two_pixel_bar_over_a_halo() {
        let (scene, _) = scene();
        let selection = Selection::new();
        let camera = Camera::default();
        let mut overlay = state(&scene, &selection, &camera);
        let color = [1.0, 0.8, 0.4, 1.0];
        overlay.caret = Some(super::OverlayCaret {
            top: Vec2::new(100.0, 50.0),
            bottom: Vec2::new(100.0, 80.0),
            color,
        });

        let vertices = build(&overlay);

        let bar: Vec<f32> = vertices
            .iter()
            .filter(|v| v.color == color)
            .map(|v| v.pos[0])
            .collect();
        assert!(!bar.is_empty());
        let width = bar.iter().copied().fold(f32::MIN, f32::max)
            - bar.iter().copied().fold(f32::MAX, f32::min);
        assert!((width - super::CARET_WIDTH).abs() < 0.01, "{width}");
        assert!(vertices.iter().any(|v| v.color == super::CARET_HALO));
    }
}
