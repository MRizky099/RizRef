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
}

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
        line(&mut vertices, north, rotate, 1.0, ACCENT);
        for handle in SelectionHandle::SCALE {
            let center = state.screen(frame.handle_position(handle, 0.0));
            square(&mut vertices, center, HANDLE_HALF, HANDLE_BODY);
            square(&mut vertices, center, HANDLE_ACCENT_HALF, ACCENT);
        }
        diamond(&mut vertices, rotate, ROTATE_RADIUS, HANDLE_BODY);
        diamond(&mut vertices, rotate, ROTATE_RADIUS - 1.0, ACCENT);
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

    vertices
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
}
