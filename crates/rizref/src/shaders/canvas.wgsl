// The canvas shaders: background (grid, origin cross, scene-rect border),
// items (instanced world quads with texture array), and the HUD layer
// (screen-space textured quad).
//
// One module, three pipeline pairs, sharing the Globals uniform at group 0.
// All coordinates start in logical viewport pixels — device pixels only
// appear where the HUD sizes itself and where the grid width is expressed.

struct Globals {
    eye: vec2f,
    viewport: vec2f,      // logical px
    rot_cs: vec2f,        // (cos, sin) of the view rotation
    zoom: f32,            // logical px per world unit
    physical_scale: f32,  // device px per logical px
    scene_min: vec2f,
    scene_max: vec2f,
    flags: vec2f,         // x: grid visible, y: scene border visible
    hud_size: vec2f,      // HUD texture size in device px
}

@group(0) @binding(0) var<uniform> g: Globals;

// Rotation-scale block of the view transform, matching glam's DMat2::from_angle
// convention (column 0 = (c, s), column 1 = (-s, c)).
fn view_linear() -> mat2x2f {
    return mat2x2f(g.rot_cs.x, g.rot_cs.y, -g.rot_cs.y, g.rot_cs.x);
}

fn world_to_screen(world: vec2f) -> vec2f {
    return view_linear() * ((world - g.eye) * g.zoom) + g.viewport * 0.5;
}

fn screen_to_world(screen: vec2f) -> vec2f {
    let off = screen - g.viewport * 0.5;
    return g.eye + vec2f(
        g.rot_cs.x * off.x + g.rot_cs.y * off.y,
        -g.rot_cs.y * off.x + g.rot_cs.x * off.y,
    ) / g.zoom;
}

fn screen_to_clip(screen: vec2f) -> vec4f {
    return vec4f(
        screen.x / g.viewport.x * 2.0 - 1.0,
        1.0 - screen.y / g.viewport.y * 2.0,
        0.0,
        1.0,
    );
}

// Straight-alpha source-over, accumulated while drawing the overlays.
fn blend_over(src: vec4f, dst: vec4f) -> vec4f {
    let a = src.a + dst.a * (1.0 - src.a);
    let rgb = (src.rgb * src.a + dst.rgb * dst.a * (1.0 - src.a)) / max(a, 1.0e-5);
    return vec4f(rgb, a);
}

// ---------------------------------------------------------------------------
// Background: one full-screen triangle, the world position derived per pixel.
// ---------------------------------------------------------------------------

struct BgOut {
    @builtin(position) clip: vec4f,
    @location(0) screen: vec2f,
}

@vertex
fn vs_background(@builtin(vertex_index) vi: u32) -> BgOut {
    var p = vec2f(0.0, 0.0);
    if vi == 1u {
        p = vec2f(g.viewport.x * 2.0, 0.0);
    } else if vi == 2u {
        p = vec2f(0.0, g.viewport.y * 2.0);
    }
    var out: BgOut;
    out.screen = p;
    out.clip = screen_to_clip(p);
    return out;
}

@fragment
fn fs_background(in: BgOut) -> @location(0) vec4f {
    let world = screen_to_world(in.screen);
    var color = vec4f(0.0, 0.0, 0.0, 0.0);

    // Grid: world-space lines at integer coordinates with a screen-space width
    // of about one logical pixel, so the grid never thins out with zoom. Major
    // lines every 8 units. Hidden above 400% zoom by the CPU-side flag.
    if g.flags.x > 0.5 {
        let near = fract(world + vec2f(0.5)) - vec2f(0.5);
        let dpx = abs(near) * g.zoom;
        let minor = 1.0 - smoothstep(0.4, 0.9, min(dpx.x, dpx.y));

        let near8 = fract(world / vec2f(8.0) + vec2f(0.5)) - vec2f(0.5);
        let dpx8 = abs(near8) * g.zoom;
        let major = 1.0 - smoothstep(0.4, 0.9, min(dpx8.x, dpx8.y));

        let grid_rgb = vec3f(1.0, 1.0, 1.0);
        color = blend_over(vec4f(grid_rgb, 0.05 * minor * (1.0 - major)), color);
        color = blend_over(vec4f(grid_rgb, 0.10 * major), color);

        // Origin cross, slightly brighter than the major lines.
        let to_axis = min(abs(world.x), abs(world.y)) * g.zoom;
        let cross = 1.0 - smoothstep(0.6, 1.4, to_axis);
        color = blend_over(vec4f(vec3f(1.0, 0.9, 0.6), 0.35 * cross), color);
    }

    // Scene-rect border: a dashed outline of the board's bounding box, so the
    // edge of the content is visible against the void.
    if g.flags.y > 0.5 {
        let center = (g.scene_min + g.scene_max) * vec2f(0.5);
        let half = (g.scene_max - g.scene_min) * vec2f(0.5);
        let q = abs(world - center) - half;
        let dist_world = length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0);
        let dist_px = abs(dist_world) * g.zoom;
        let band = 1.0 - smoothstep(0.8, 1.5, dist_px);
        // Dash phase along the screen diagonal: cheap, uniform, and stable
        // under pan; it need not follow the border's own direction.
        let phase = fract((in.screen.x + in.screen.y) / 14.0);
        let dash = step(0.5, phase);
        color = blend_over(vec4f(vec3f(1.0, 1.0, 1.0), band * dash * 0.45), color);
    }

    // Premultiplied out; the pipeline blends with One / OneMinusSrcAlpha.
    return vec4f(color.rgb * color.a, color.a);
}

// ---------------------------------------------------------------------------
// Items: one instanced quad pipeline. Draw order is the instance order, which
// is the scene z-order.
// ---------------------------------------------------------------------------

struct ItemIn {
    @location(0) pos_size: vec4f, // center.xy, size.xy (world units)
    @location(1) misc: vec4f,     // rotation (rad), -, -, -
    @location(2) uv: vec4f,       // uv rect (min.xy, max.xy)
    @location(3) tint: vec4f,
}

struct ItemOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) world: vec2f,
    @location(2) tint: vec4f,
}

@group(1) @binding(0) var item_tex: texture_2d<f32>;
@group(1) @binding(1) var item_samp: sampler;

@vertex
fn vs_items(item: ItemIn, @builtin(vertex_index) vi: u32) -> ItemOut {
    var corners = array(
        vec2f(-0.5, -0.5),
        vec2f(0.5, -0.5),
        vec2f(-0.5, 0.5),
        vec2f(0.5, 0.5),
    );
    let corner = corners[vi];
    let c = cos(item.misc.x);
    let s = sin(item.misc.x);
    let local = mat2x2f(c, s, -s, c) * (corner * item.pos_size.zw);
    let world = item.pos_size.xy + local;

    var out: ItemOut;
    out.clip = screen_to_clip(world_to_screen(world));
    let t = corner + vec2f(0.5);
    out.uv = vec2f(
        mix(item.uv.x, item.uv.z, t.x),
        mix(item.uv.y, item.uv.w, t.y),
    );
    out.world = world;
    out.tint = item.tint;
    return out;
}

@fragment
fn fs_items(in: ItemOut) -> @location(0) vec4f {
    var color = textureSample(item_tex, item_samp, in.uv) * in.tint;
    // Checkerboard behind translucent pixels: world-space 16-unit cells, so
    // the checker sticks to the board rather than to the screen.
    if color.a < 0.999 {
        let cell = floor(in.world / vec2f(16.0));
        let parity = cell.x + cell.y;
        let even = parity - 2.0 * floor(parity / 2.0);
        let shade = select(0.05, 0.10, even > 0.5);
        color = vec4f(mix(vec3f(shade), color.rgb, color.a), 1.0);
    }
    return color;
}

// ---------------------------------------------------------------------------
// Overlay: screen-space coloured triangles for outlines, handles, guides and
// the marquee. Positions are already in logical viewport pixels; the CPU builds
// them so handle sizes and line widths stay constant at every zoom.
// ---------------------------------------------------------------------------

struct OverlayIn {
    @location(0) pos: vec2f,
    @location(1) color: vec4f,
}

struct OverlayOut {
    @builtin(position) clip: vec4f,
    @location(0) color: vec4f,
}

@vertex
fn vs_overlay(in: OverlayIn) -> OverlayOut {
    var out: OverlayOut;
    out.clip = screen_to_clip(in.pos);
    out.color = in.color;
    return out;
}

@fragment
fn fs_overlay(in: OverlayOut) -> @location(0) vec4f {
    return in.color;
}

// ---------------------------------------------------------------------------
// HUD: a small screen-space quad at the bottom-left, sampling the CPU-raster
// readout uploaded each frame. Everything here is in device pixels.
// ---------------------------------------------------------------------------

struct HudOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
}

@group(1) @binding(0) var hud_tex: texture_2d<f32>;
@group(1) @binding(1) var hud_samp: sampler;

@vertex
fn vs_hud(@builtin(vertex_index) vi: u32) -> HudOut {
    var corners = array(
        vec2f(0.0, 0.0),
        vec2f(1.0, 0.0),
        vec2f(0.0, 1.0),
        vec2f(1.0, 1.0),
    );
    let corner = corners[vi];
    let size = g.hud_size / vec2f(g.physical_scale);
    let margin = 14.0;
    let origin = vec2f(margin, g.viewport.y - margin - size.y);
    let position = origin + corner * size;

    var out: HudOut;
    out.clip = screen_to_clip(position);
    out.uv = corner;
    return out;
}

@fragment
fn fs_hud(in: HudOut) -> @location(0) vec4f {
    return textureSample(hud_tex, hud_samp, in.uv);
}
