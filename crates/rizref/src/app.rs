//! The application: command line, logging, the single-instance handshake and
//! the event loop that owns the window.
//!
//! The shape of `run` is the shape of every launch:
//!
//! 1. answer the informational flags without starting anything;
//! 2. parse the arguments;
//! 3. bring up logging and the panic hook;
//! 4. settle the single-instance question — forward and exit, or continue;
//! 5. run the event loop until the window closes.
//!
//! Frame pacing lives here as much as in the renderer: the loop redraws only
//! when something changed (resize, an explicit request), never on a timer, so
//! an idle board costs no CPU — a reference viewer sits in the background for
//! hours while its user paints elsewhere.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use glam::{DVec2, Vec2};
use rizref_core::{Asset, AssetId, Rect, Scene, SceneId, SceneRectTracker, Selection};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::canvas::{HudUpload, ItemQuad, QuadInstance};
use crate::clipboard::{self, ClipboardContent};
use crate::dev_state;
use crate::hud::HudSurface;
use crate::import::{ImportOutcome, ImportRequest, Importer};
use crate::input::{self, Button as InputButton, Command, Key as InputKey, Trigger};
use crate::memory::DecodeCache;
use crate::placement;
use crate::platform::{
    self, DropEvent, DropRegistration, SingleInstance, forward_args_to_primary,
    serve_forwarded_args,
};
use crate::renderer::{FrameError, FrameParams, GpuSurface};
use crate::text::TextEngine;
use crate::view::ViewState;

/// The window RizRef opens.
///
/// Held as a value rather than as constants scattered around so the plan and
/// the window can never drift apart. The 600 × 400 default and the title match
/// the reference application, which is what a user moving between the two
/// expects.
#[derive(Debug, Clone, Copy)]
pub struct WindowPlan {
    /// Title in the title bar and the taskbar.
    pub title: &'static str,
    /// Initial size in logical pixels; winit applies the monitor's scale factor.
    pub size: LogicalSize<f64>,
}

impl Default for WindowPlan {
    fn default() -> Self {
        Self {
            title: "RizRef",
            size: LogicalSize::new(600.0, 400.0),
        }
    }
}

/// Background the canvas is cleared to, as RGBA.
///
/// A dark grey rather than black: reference images sit on the board for hours,
/// and a pure-black surround makes mid-tone artwork look washed out.
pub const CANVAS_BACKGROUND: [u8; 4] = [24, 24, 28, 255];

/// Readout surface width in logical pixels; the upload scales with the DPI.
const READOUT_WIDTH: f64 = 420.0;

/// Readout surface height in logical pixels.
const READOUT_HEIGHT: f64 = 40.0;

/// How many daily log files to keep when pruning at startup.
const LOG_FILES_KEPT: usize = 14;

/// Version string used by `--version` and the summary line.
pub fn version_line() -> String {
    format!("rizref {}", env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

/// The parsed command line.
#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedArgs {
    /// Raise the log filter to `debug`.
    verbose: bool,
    /// Open the native file dialog once and print what came back (a smoke
    /// flag from phase 00 that needs a real desktop).
    check_dialogs: bool,
    /// Print the build summary instead of opening the window.
    build_info: bool,
    /// Positional paths — boards to open (consumed by later phases).
    paths: Vec<String>,
}

/// Parses the argument list; `Err` carries the unrecognised flag verbatim.
fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    for arg in args {
        match arg.as_str() {
            "--verbose" | "-v" => parsed.verbose = true,
            "--check-dialogs" => parsed.check_dialogs = true,
            "--build-info" => parsed.build_info = true,
            flag if flag.starts_with('-') && flag != "-" => return Err(flag.to_owned()),
            path => parsed.paths.push(path.to_owned()),
        }
    }
    Ok(parsed)
}

/// Runs the command line interface and returns the process exit code.
///
/// Exit codes: `0` after a normal run, `2` for an argument this build does not
/// understand, `1` when the application could not start at all.
pub fn run(args: Vec<String>) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("{}", version_line());
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            print_help();
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let parsed = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(flag) => {
            eprintln!("rizref: unrecognised argument {flag:?}");
            eprintln!("rizref: try --help");
            return ExitCode::from(2);
        }
    };

    let log_dir = init_logging(parsed.verbose);
    install_panic_hook(log_dir);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), ?parsed, "starting");

    if parsed.check_dialogs {
        check_dialogs();
        return ExitCode::SUCCESS;
    }
    if parsed.build_info {
        print_startup_summary();
        return ExitCode::SUCCESS;
    }

    match SingleInstance::acquire() {
        Ok(guard) => run_window(Some(guard), parsed),
        Err(()) => {
            // Either another instance owns the mutex, or the mutex could not
            // be created. Handing the arguments over is correct in both
            // cases; when nobody answers, the guard was stale and this
            // process becomes the instance.
            if forward_args_to_primary(&parsed.paths) {
                ExitCode::SUCCESS
            } else {
                tracing::warn!("no running instance answered; continuing as the instance");
                run_window(None, parsed)
            }
        }
    }
}

/// Prints the flags this build accepts.
fn print_help() {
    println!("{}", version_line());
    println!("A reference-image canvas: collect, arrange and study images on an infinite board.");
    println!();
    println!("USAGE:");
    println!("    rizref [OPTION]... [FILE]...");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help            print this help and exit");
    println!("    -V, --version         print the version and exit");
    println!("    -v, --verbose         log at debug level");
    println!("    --build-info          print what this build contains and exit");
    println!("    --check-dialogs       open the native file dialog and print the chosen paths");
    println!();
    println!("A second launch hands its FILE arguments to the running instance and exits.");
}

// ---------------------------------------------------------------------------
// Logging and panic reporting
// ---------------------------------------------------------------------------

/// Where RizRef writes its logs and panic reports.
///
/// The user's local-appdata directory, taken from the environment at runtime;
/// when the environment does not provide it (unusual, stripped service
/// contexts) logs land in the temp directory rather than being lost.
fn logs_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rizref"))
        .join("RizRef")
        .join("logs")
}

/// Picks which rotated log files to delete so `keep` newest remain.
///
/// Pure and unit-tested: names sort lexicographically, which for the appender's
/// `rizref.log.YYYY-MM-DD` scheme is chronological. Unknown names are ignored
/// (kept), so panic reports and future formats are never pruned by mistake.
fn stale_logs(names: &[String], keep: usize) -> Vec<String> {
    let mut dated: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("rizref.log."))
        .collect();
    dated.sort_unstable();
    dated.reverse(); // newest first
    dated.into_iter().skip(keep).map(String::clone).collect()
}

/// Installs the log sinks: a rolling daily file, plus `warn`+ on stderr when
/// there is a terminal to read it. Returns the log directory for the panic
/// hook.
fn init_logging(verbose: bool) -> PathBuf {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer as _, filter::LevelFilter};

    let dir = logs_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        // Logging is best-effort at startup: say so on the way past, then let
        // the subscriber fall back to whatever the appender can do.
        eprintln!("rizref: cannot create the log directory: {error}");
    } else {
        let names: Vec<String> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        for name in stale_logs(&names, LOG_FILES_KEPT) {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(if verbose { "debug" } else { "info" }));

    let file_appender = tracing_appender::rolling::daily(&dir, "rizref.log");
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_target(false)
        .with_filter(filter);

    if std::io::stderr().is_terminal() {
        tracing_subscriber::registry()
            .with(file_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_filter(LevelFilter::WARN),
            )
            .init();
    } else {
        tracing_subscriber::registry().with(file_layer).init();
    }
    dir
}

/// Replaces the panic handler: write the report, tell the user, exit.
///
/// A windowed application has no console to die in front of, so the hook
/// writes `panic-<unix-seconds>.log` beside the rotating logs, shows the
/// native message box when there is no terminal to read a backtrace from,
/// and exits non-zero (`101`, the Rust convention for a panic).
///
/// Installed only on the application path — unit tests keep the default
/// hook, which is why no test in this crate can pop a message box.
fn install_panic_hook(log_dir: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let body = format!(
            "rizref {}\npanicked:\n{info}\n\n{backtrace}\n",
            env!("CARGO_PKG_VERSION")
        );
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let report = log_dir.join(format!("panic-{stamp}.log"));
        match std::fs::write(&report, &body) {
            Ok(()) => tracing::error!(report = %report.display(), "panic"),
            Err(error) => {
                eprintln!("rizref panicked; the report could not be written ({error}):\n{body}")
            }
        }
        if !std::io::stderr().is_terminal() {
            platform::alert(
                "RizRef",
                "RizRef hit an unexpected error and must close.\nA report was written to the logs folder.",
            );
        }
        std::process::exit(101);
    }));
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Events that arrive in the loop from outside winit's own flow.
#[derive(Debug)]
enum AppEvent {
    /// A second launch handed its arguments to this instance.
    ForwardedArgs(Vec<String>),
    /// An import worker finished one request.
    Import(Box<ImportOutcome>),
    /// The OLE drop target received files, in client-area physical pixels.
    Dropped(Box<DropEvent>),
}

/// Everything the event loop owns: the window, its surface, the canvas state
/// and the view the user looks through.
struct App {
    /// The OLE drop registration. Declared first so it is dropped — and the
    /// target unregistered — while the window behind it is still alive.
    drop_target: Option<DropRegistration>,
    /// Event-loop handle the import workers and the drop target report through.
    proxy: EventLoopProxy<AppEvent>,
    plan: WindowPlan,
    window: Option<Arc<Window>>,
    renderer: Option<GpuSurface>,
    /// The camera, the active gesture and the zoom animation.
    view: ViewState,
    /// The board: items, their z-order and the shared assets they show.
    scene: Scene,
    scene_rects: SceneRectTracker,
    /// Full-resolution decoded bitmaps under the memory cap.
    decode_cache: DecodeCache,
    /// The import worker pool, `None` only if no thread could start.
    importer: Option<Importer>,
    /// Paths from the command line or a second launch, imported once the
    /// window and its camera exist.
    pending_paths: Vec<String>,
    /// Font system and glyph caches for the HUD readout.
    text: TextEngine,
    /// The CPU-raster readout surface, sized once the scale factor is known.
    hud: Option<HudSurface>,
    /// Keyboard modifiers as of the last `ModifiersChanged`.
    modifiers: ModifiersState,
    /// Items the user has picked; an animated item that is selected freezes.
    selection: Selection,
    /// Playback clocks for the animated assets on the board.
    playbacks: HashMap<AssetId, Playback>,
    /// When the next animation frame is due, for the event loop's timer.
    next_frame_at: Option<Instant>,
}

/// One asset's place in its animation.
#[derive(Debug, Clone, Copy)]
struct Playback {
    /// Frame currently uploaded to the GPU.
    frame: usize,
    /// When the next frame becomes due.
    next_at: Instant,
}

impl App {
    fn new(plan: WindowPlan, proxy: EventLoopProxy<AppEvent>) -> Self {
        let scene = Scene::new(SceneId::from_bytes(*b"rizref-dev-scene"));
        let worker_proxy = proxy.clone();
        let importer = Importer::new(move |outcome| {
            if worker_proxy
                .send_event(AppEvent::Import(Box::new(outcome)))
                .is_err()
            {
                tracing::debug!("the event loop is gone; dropping an import result");
            }
        });
        tracing::info!(workers = importer.worker_count(), "import pool started");

        // The camera survives restarts through the dev dump (phase 06 moves it
        // into the .rizref container); without one, the default view.
        let camera = dev_state::load().unwrap_or_default();
        Self {
            drop_target: None,
            proxy,
            view: ViewState::new(camera, DVec2::new(plan.size.width, plan.size.height)),
            plan,
            window: None,
            renderer: None,
            scene,
            scene_rects: SceneRectTracker::new(),
            decode_cache: DecodeCache::with_default_cap(),
            importer: Some(importer),
            pending_paths: Vec::new(),
            text: TextEngine::new(),
            hud: None,
            modifiers: ModifiersState::default(),
            selection: Selection::new(),
            playbacks: HashMap::new(),
            next_frame_at: None,
        }
    }

    /// Whether closing needs a confirmation dialog.
    ///
    /// Always `false` today: nothing in this phase can be lost. Phase 05
    /// replaces the body with the dirty-scene check — the call site in
    /// `CloseRequested` is the one and only place that will ask.
    fn should_confirm_close(&self) -> bool {
        false
    }

    /// Asks for one more frame; the loop draws only when something changed.
    fn redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Applies a resolved command with the current scene rect and redraws.
    fn apply(&mut self, command: Command) {
        self.view.apply_command(command, self.scene_rects.rect());
        self.redraw();
    }

    /// Scales a wheel command's notch count by the event delta and applies it.
    fn apply_wheel(&mut self, command: Option<Command>, notches: i32) {
        let Some(mut command) = command else {
            return;
        };
        if notches == 0 {
            return;
        }
        match &mut command {
            Command::ZoomNotch(count) | Command::RotateNotch(count) => *count *= notches,
            _ => {}
        }
        self.apply(command);
    }

    /// The HUD readout line: zoom percentage, rotation, cursor world position.
    fn readout_text(&self) -> String {
        let zoom = self.view.camera.zoom * 100.0;
        let zoom = if (zoom - zoom.round()).abs() < 0.05 {
            format!("{}", zoom.round() as i64)
        } else {
            format!("{zoom:.1}")
        };
        let rotation = self.view.camera.rotation.to_degrees();
        let animation = self
            .selected_animation_note()
            .map(|note| format!(" · {note}"))
            .unwrap_or_default();
        if self.view.has_cursor() {
            let world = self.view.cursor_world();
            format!(
                "{zoom}% · {rotation:.0}° · ({:.1}, {:.1}){animation}",
                world.x, world.y
            )
        } else {
            format!("{zoom}% · {rotation:.0}°{animation}")
        }
    }

    /// Rasterises the readout into the HUD surface and returns the upload.
    fn render_hud(&mut self, text: &str) -> Option<(Vec<u8>, u32, u32)> {
        let hud = self.hud.as_mut()?;
        hud.draw_readout(text, &mut self.text);
        let bytes = hud.straight_rgba();
        Some((bytes, hud.width(), hud.height()))
    }

    /// (Re)creates the HUD surface for the current scale factor.
    fn rebuild_hud(&mut self, scale: f64) {
        let width = ((READOUT_WIDTH * scale).round() as u32).max(1);
        let height = ((READOUT_HEIGHT * scale).round() as u32).max(1);
        self.hud = HudSurface::new(width, height);
    }

    /// The world point new items should gather around.
    ///
    /// The cursor when it is over the canvas, the view centre otherwise. A
    /// dropped file carries no coordinates in winit's event, so the last known
    /// cursor position is the drop point.
    fn drop_world(&self) -> Vec2 {
        if self.view.has_cursor() {
            self.view.cursor_world().as_vec2()
        } else {
            self.view.camera.eye.as_vec2()
        }
    }

    /// Submits one import request to the worker pool.
    fn submit_import(&mut self, request: ImportRequest) {
        let label = request.source.label.clone();
        let drop_world = request.drop_world;
        match self.importer.as_ref() {
            Some(importer) if importer.submit(request) => {
                tracing::info!(%label, ?drop_world, "import queued");
            }
            _ => tracing::error!(%label, "the import pool is not running"),
        }
    }

    /// Queues every path in `paths` as an image import around `drop_world`.
    fn import_paths(&mut self, paths: Vec<PathBuf>, drop_world: Vec2) {
        for path in paths {
            if looks_like_image(&path) {
                self.submit_import(ImportRequest::path(path, drop_world));
            } else {
                tracing::info!(
                    path = %path.display(),
                    "not an image; board files are phase 05+"
                );
            }
        }
    }

    /// Ctrl+V: files first, then a raw bitmap from the clipboard.
    fn paste(&mut self) {
        let drop_world = self.drop_world();
        match clipboard::read() {
            Ok(ClipboardContent::Files(paths)) => self.import_paths(paths, drop_world),
            Ok(ClipboardContent::Image { name, png }) => {
                self.submit_import(ImportRequest::bytes(name, png, drop_world));
            }
            Err(reason) => tracing::warn!(%reason, "nothing pasteable on the clipboard"),
        }
    }

    /// Ctrl+O: the native multi-select image dialog.
    fn open_images(&mut self) {
        if let Some(paths) = crate::dialogs::pick_images() {
            let drop_world = self.drop_world();
            self.import_paths(paths, drop_world);
        }
    }

    /// Registers the OLE drop target for the window.
    ///
    /// winit's own target understands only `CF_HDROP`; this one also reads the
    /// virtual-file formats, so an attachment dragged out of Outlook becomes an
    /// item. Registration replaces winit's target, which is why a failure leaves
    /// the application without OS drag & drop rather than with a half-working
    /// one; Ctrl+O and paste still work.
    fn install_drop_target(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let proxy = self.proxy.clone();
        self.drop_target = platform::drop_target::register(window, move |event| {
            if proxy
                .send_event(AppEvent::Dropped(Box::new(event)))
                .is_err()
            {
                tracing::debug!("the event loop is gone; dropping a drop event");
            }
        });
    }

    /// Turns one OLE drop into import requests around the drop point.
    fn apply_drop(&mut self, event: DropEvent) {
        let drop_world = self.world_from_physical(event.client_physical);
        if !event.paths.is_empty() {
            self.import_paths(event.paths, drop_world);
        }
        for file in event.virtual_files {
            if !looks_like_image(std::path::Path::new(&file.name)) {
                tracing::info!(name = %file.name, "virtual file is not an image; skipped");
                continue;
            }
            tracing::info!(
                name = %file.name,
                bytes = file.bytes.len(),
                ?drop_world,
                "virtual file dropped"
            );
            self.submit_import(ImportRequest::bytes(file.name, file.bytes, drop_world));
        }
        self.redraw();
    }

    /// Maps a client-area position in physical pixels onto the board.
    fn world_from_physical(&self, (x, y): (f32, f32)) -> Vec2 {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
        let logical = DVec2::new(f64::from(x) / scale, f64::from(y) / scale);
        self.view
            .camera
            .screen_to_world(logical, self.view.viewport())
            .as_vec2()
    }

    /// Turns one worker result into an asset, an item and a GPU texture.
    fn apply_import(&mut self, outcome: ImportOutcome) {
        let imported = match outcome.result {
            Ok(imported) => imported,
            Err(error) => {
                tracing::warn!(
                    label = %error.label,
                    reason = %error.reason,
                    latency_ms = outcome.latency.as_secs_f64() * 1000.0,
                    "import failed"
                );
                return;
            }
        };

        let width = imported.image.width() as f32;
        let height = imported.image.height() as f32;
        let display = imported.display;
        let mut asset = Asset::from_parts(
            imported.encoded,
            imported.format,
            imported.checksum,
            &imported.image,
            imported.preview,
        );
        if let Some(animation) = imported.animation {
            tracing::info!(
                label = %outcome.label,
                frames = animation.len(),
                loop_ms = animation.total_duration().as_millis(),
                "animated image"
            );
            asset = asset.with_animation(animation);
        }
        let encoded_len = asset.encoded().len();
        let (asset_id, was_new) = self.scene.insert_asset(asset);
        if was_new {
            tracing::info!(
                asset = %asset_id,
                bytes = encoded_len,
                "asset interned"
            );
        } else {
            tracing::info!(
                asset = %asset_id,
                bytes = encoded_len,
                "asset deduplicated; adding another item"
            );
        }

        let occupied: Vec<Rect> = self
            .scene
            .items()
            .iter()
            .map(rizref_core::ImageItem::world_rect)
            .collect();
        let center = placement::find_spot(
            outcome.drop_world,
            Vec2::new(width, height),
            &occupied,
            placement::ITEM_GAP,
        );
        let Some(item_id) = self
            .scene
            .insert_image(asset_id, center, Vec2::new(width, height))
        else {
            tracing::error!(asset = %asset_id, "asset vanished between intern and insert");
            return;
        };
        if let Some(item) = self.scene.item(item_id) {
            self.scene_rects.merge(item_id, item.world_rect());
        }

        let pixels = Arc::new(imported.image);
        let full_bytes = (pixels.width() as u64) * (pixels.height() as u64) * 4;
        let evicted = self.decode_cache.insert(asset_id, Arc::clone(&pixels));
        for id in evicted {
            tracing::debug!(asset = %id, "decoded bitmap evicted from the memory cache");
        }

        if let Some(renderer) = self.renderer.as_mut() {
            let upload = display.as_ref().unwrap_or(&pixels);
            if !renderer.has_texture(asset_id) || renderer.texture_is_preview(asset_id) {
                renderer.upload_texture(asset_id, upload, false);
            }
            tracing::debug!(
                textures = renderer.texture_count(),
                vram_bytes = renderer.texture_bytes(),
                "texture store after import"
            );
        }

        tracing::info!(
            label = %outcome.label,
            item = %item_id,
            asset = %asset_id,
            width = pixels.width(),
            height = pixels.height(),
            decoded_bytes = full_bytes,
            cache_bytes = self.decode_cache.bytes(),
            cache_entries = self.decode_cache.len(),
            latency_ms = outcome.latency.as_secs_f64() * 1000.0,
            "image placed"
        );
        self.redraw();
    }

    /// Builds this frame's drawable items in scene z-order.
    fn build_items(&self) -> Vec<ItemQuad> {
        self.scene
            .items()
            .iter()
            .map(|item| {
                let center = item.center();
                let size = item.size();
                let mut instance =
                    QuadInstance::new([center.x, center.y], [size.x, size.y], item.rotation());
                instance.tint = [1.0, 1.0, 1.0, item.opacity()];
                ItemQuad {
                    instance,
                    asset: item.asset(),
                }
            })
            .collect()
    }

    /// Uploads previews for items whose full texture is missing or evicted,
    /// and upgrades a preview to the full bitmap when the cache still has it.
    fn ensure_textures(&mut self, items: &[ItemQuad]) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for item in items {
            renderer.touch_texture(item.asset);
            if !renderer.has_texture(item.asset) {
                if let Some(asset) = self.scene.asset(item.asset) {
                    renderer.upload_texture(item.asset, asset.preview().image(), true);
                }
            } else if renderer.texture_is_preview(item.asset)
                && let Some(full) = self.decode_cache.get(item.asset)
            {
                renderer.upload_texture(item.asset, &full, false);
            }
        }
    }

    /// Selects the topmost item under the cursor, or clears the selection.
    ///
    /// This is the minimum selection phase 04 builds on, and it is what makes
    /// "freeze the animation" meaningful: whatever is selected stops playing so
    /// its current frame can be studied.
    fn select_at_cursor(&mut self) {
        let Some(cursor) = self.view.cursor_position() else {
            return;
        };
        let world = self
            .view
            .camera
            .screen_to_world(cursor, self.view.viewport())
            .as_vec2();
        // Topmost first: the last item in z-order is drawn on top.
        let hit = self
            .scene
            .items()
            .iter()
            .rev()
            .find(|item| item.hit_test(world))
            .map(rizref_core::ImageItem::id);

        match hit {
            Some(id) => self.selection.set_single(id),
            None => self.selection.clear(),
        }
        self.redraw();
    }

    /// Advances every playing animation to `now`, returning when the next frame
    /// is due.
    ///
    /// Only animations that are on screen and not selected play: an item
    /// scrolled out of view costs nothing, and a selected item is frozen so its
    /// current frame can be read.
    fn advance_animations(&mut self, now: Instant) -> Option<Instant> {
        let visible = self.view.camera.visible_world_rect(self.view.viewport());
        let (uploads, next) = tick_animations(
            &self.scene,
            &self.selection,
            &mut self.playbacks,
            visible,
            now,
        );

        if !uploads.is_empty()
            && let Some(renderer) = self.renderer.as_mut()
        {
            for (asset, frame) in uploads {
                if let Some(animation) = self.scene.asset(asset).and_then(Asset::animation) {
                    renderer.upload_texture(asset, animation.frame(frame).image(), false);
                }
            }
        }

        self.next_frame_at = next;
        next
    }

    /// The HUD note for a frozen animation, when one is selected.
    fn selected_animation_note(&self) -> Option<String> {
        let id = self.selection.iter().next()?;
        let item = self.scene.item(id)?;
        let asset = self.scene.asset(item.asset())?;
        let animation = asset.animation()?;
        let frame = self
            .playbacks
            .get(&item.asset())
            .map_or(0, |playback| playback.frame);
        Some(format!("GIF frozen {}/{}", frame + 1, animation.len()))
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            // Windows never suspends a desktop window; if it ever did, the
            // existing window and surface are still valid.
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(self.plan.title)
            .with_inner_size(self.plan.size);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                tracing::error!(%error, "cannot create the window");
                platform::alert("RizRef", "RizRef could not open its window and must close.");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let scale = window.scale_factor();
        self.view.set_viewport(DVec2::new(
            size.width as f64 / scale,
            size.height as f64 / scale,
        ));
        self.rebuild_hud(scale);
        match GpuSurface::new(window.clone(), (size.width, size.height)) {
            Some(renderer) => {
                tracing::info!(?size, scale, "window ready");
                self.renderer = Some(renderer);
                self.window = Some(window);
                self.install_drop_target();
                self.window.as_ref().expect("just set").request_redraw();

                // Command-line and forwarded paths were held until the window
                // (and therefore the camera and viewport) existed.
                let pending = std::mem::take(&mut self.pending_paths);
                if !pending.is_empty() {
                    let drop_world = self.drop_world();
                    let paths = pending.into_iter().map(PathBuf::from).collect();
                    self.import_paths(paths, drop_world);
                }
            }
            None => {
                // No GPU surface at all. The CPU raster fallback is a later
                // phase; for now be loud and honest rather than a grey box.
                platform::alert(
                    "RizRef",
                    "RizRef could not initialise a GPU surface (Vulkan or DirectX 12)\nand must close.",
                );
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window_id != window.id() {
            return;
        }
        match event {
            WindowEvent::Resized(size) => {
                self.renderer
                    .as_mut()
                    .expect("a resized window has a renderer")
                    .resize(size.width, size.height);
                let scale = window.scale_factor();
                self.view.set_viewport(DVec2::new(
                    size.width as f64 / scale,
                    size.height as f64 / scale,
                ));
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // winit follows this with `Resized`, which reconfigures the
                // swapchain and the view; the HUD surface rescales here.
                tracing::debug!(scale_factor, "scale factor changed");
                self.rebuild_hud(scale_factor);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = window.scale_factor();
                let logical = DVec2::new(position.x / scale, position.y / scale);
                let previous = self.view.cursor_position();
                self.view.set_cursor(logical);
                if self.view.pan_active()
                    && let Some(previous) = previous
                {
                    self.view.drag_pan(logical - previous);
                }
                self.redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                let button = match button {
                    MouseButton::Left => InputButton::Left,
                    MouseButton::Middle => InputButton::Middle,
                    _ => return,
                };
                if let Some(command) = input::resolve(&Trigger::Button {
                    button,
                    pressed,
                    space_held: self.view.space_held(),
                }) {
                    self.apply(command);
                } else if button == InputButton::Left && pressed && !self.view.space_held() {
                    // Plain left click: pick the item under the cursor. Phase 04
                    // replaces this with drag-select, but the selection it sets
                    // already means something (it freezes an animation).
                    self.select_at_cursor();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(_, y) => {
                    // Mouse wheels: one notch per detent. Alt turns the view,
                    // everything else zooms (instant with ctrl, per the view).
                    let notches = y.round() as i32;
                    let alt = self.modifiers.alt_key();
                    let command = input::resolve(&Trigger::Wheel { alt });
                    self.apply_wheel(command, notches);
                }
                MouseScrollDelta::PixelDelta(delta) => {
                    // Trackpad two-finger scroll: pan by the pixel delta.
                    let scale = window.scale_factor();
                    let logical: LogicalPosition<f64> = delta.to_logical(scale);
                    self.view.scroll_pan(DVec2::new(logical.x, logical.y));
                    self.redraw();
                }
            },
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match &event.logical_key {
                    Key::Named(NamedKey::Space) => self.view.set_space_held(pressed),
                    key => {
                        if !pressed {
                            return;
                        }
                        if self.modifiers.control_key()
                            && let Key::Character(text) = key
                        {
                            match text.as_str().to_ascii_lowercase().as_str() {
                                "v" => {
                                    self.paste();
                                    return;
                                }
                                "o" => {
                                    self.open_images();
                                    return;
                                }
                                _ => {}
                            }
                        }
                        let bound = match key {
                            Key::Character(text) => match text.as_str() {
                                "0" => Some(InputKey::Digit0),
                                "1" => Some(InputKey::Digit1),
                                "2" => Some(InputKey::Digit2),
                                "f" | "F" => Some(InputKey::F),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some(key) = bound
                            && let Some(command) = input::resolve(&Trigger::Key { key, pressed })
                        {
                            self.apply(command);
                        }
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                self.view.set_ctrl_held(modifiers.state().control_key());
            }
            WindowEvent::HoveredFile(path) => {
                tracing::debug!(path = %path.display(), "file hovering; drop to import");
            }
            WindowEvent::HoveredFileCancelled => {
                tracing::debug!("file drag left the window");
            }
            WindowEvent::DroppedFile(path) => {
                let drop_world = self.drop_world();
                tracing::info!(path = %path.display(), ?drop_world, "file dropped");
                self.import_paths(vec![path], drop_world);
            }
            WindowEvent::RedrawRequested => {
                // Advance the zoom animation first: this frame shows the new
                // state, and if it is still running the next frame is queued.
                // The HUD and the frame params are gathered before the
                // renderer is taken mutably, so the borrows never overlap.
                let animating = self.view.advance(Instant::now());
                let _next_frame = self.advance_animations(Instant::now());
                let items = self.build_items();
                self.ensure_textures(&items);
                let text = self.readout_text();
                let hud_layer = self.render_hud(&text);
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let hud = match &hud_layer {
                    Some((bytes, width, height)) => HudUpload::Layer {
                        bytes,
                        width: *width,
                        height: *height,
                    },
                    None => HudUpload::None,
                };
                let params = FrameParams {
                    camera: &self.view.camera,
                    viewport: self.view.viewport(),
                    physical_scale: window.scale_factor(),
                    scene_rect: self.scene_rects.rect(),
                    items: &items,
                    hud,
                };
                match renderer.render(&params) {
                    Ok(()) | Err(FrameError::NeedsReconfigure) => {}
                    Err(FrameError::Fatal(why)) => {
                        // Log and stop: retrying into a lost surface would
                        // spin the loop. The CPU fallback (phase 02+) will
                        // take over cases a fresh adapter could have served.
                        tracing::error!(%why, "render failed; window will stay as-is");
                    }
                }
                if animating {
                    window.request_redraw();
                }
            }
            WindowEvent::CloseRequested => {
                if self.should_confirm_close() {
                    // Phase 05 asks "save changes?" here. Nothing can be dirty
                    // yet, so the stub never routes through this branch.
                }
                // Dev-only checkpoint of the view transform; phase 06 folds
                // this into the .rizref container and deletes the module.
                dev_state::store(&self.view.camera);
                tracing::info!("close requested; exiting");
                event_loop.exit();
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::ForwardedArgs(args) => {
                tracing::info!(count = args.len(), "accepted forwarded args");
                self.pending_paths = args;
                let pending = std::mem::take(&mut self.pending_paths);
                let drop_world = self.drop_world();
                let paths = pending.into_iter().map(PathBuf::from).collect();
                self.import_paths(paths, drop_world);
            }
            AppEvent::Import(outcome) => self.apply_import(*outcome),
            AppEvent::Dropped(event) => self.apply_drop(*event),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The board idles at zero CPU unless something is animating: a playing
        // GIF arms a one-shot timer for its next frame, and everything else
        // waits for an event.
        match self.next_frame_at {
            Some(deadline) if deadline <= Instant::now() => {
                self.redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Desktop Windows does not suspend windows; nothing to release.
    }
}

/// Whether a path is worth handing to the import worker.
///
/// The decoder sniffs magic, not names, so an extensionless file is submitted;
/// a name that clearly belongs to another format (a board, a document) is not.
fn looks_like_image(path: &std::path::Path) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        None => true,
        Some(extension) => rizref_codecs::ImageFormat::from_extension(extension).is_some(),
    }
}

/// Advances the animation clocks, returning the frames to upload and the next
/// deadline.
///
/// Split out of [`App`] so the playback rules — freeze what is selected, skip
/// what is off screen, loop — are testable without a window or a GPU.
fn tick_animations(
    scene: &Scene,
    selection: &Selection,
    playbacks: &mut HashMap<AssetId, Playback>,
    visible: Rect,
    now: Instant,
) -> (Vec<(AssetId, usize)>, Option<Instant>) {
    let mut uploads = Vec::new();
    let mut next: Option<Instant> = None;

    for item in scene.items() {
        let Some(asset) = scene.asset(item.asset()) else {
            continue;
        };
        let Some(animation) = asset.animation() else {
            continue;
        };
        // A selected item is frozen: that is the whole point of selecting a GIF
        // on a reference board.
        if selection.contains(item.id()) || !item.world_rect().intersects(&visible) {
            continue;
        }

        let playback = playbacks.entry(item.asset()).or_insert(Playback {
            frame: 0,
            next_at: now,
        });
        if now >= playback.next_at {
            let frame = (playback.frame + 1) % animation.len();
            playback.frame = frame;
            playback.next_at = now + animation.delay_after(frame);
            uploads.push((item.asset(), frame));
        }
        next = Some(next.map_or(playback.next_at, |soonest| soonest.min(playback.next_at)));
    }

    (uploads, next)
}

/// Runs the event loop as the primary instance.
///
/// `guard` is `Some` when the single-instance mutex is held; it lives in the
/// frame of this function, so the mutex is released exactly when the loop
/// ends — before `run` returns and the process exits.
fn run_window(guard: Option<SingleInstance>, parsed: ParsedArgs) -> ExitCode {
    let event_loop = match EventLoop::<AppEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            tracing::error!(%error, "cannot create the event loop");
            platform::alert(
                "RizRef",
                "RizRef could not start its event loop and must close.",
            );
            return ExitCode::FAILURE;
        }
    };

    // The pipe server must exist before the window: a secondary that arrives
    // during startup finds it waiting instead of racing it.
    let proxy = event_loop.create_proxy();
    let app_proxy = proxy.clone();
    serve_forwarded_args(move |args| {
        let _ = proxy.send_event(AppEvent::ForwardedArgs(args));
    });

    let mut app = App::new(WindowPlan::default(), app_proxy);
    app.pending_paths = parsed.paths;
    match event_loop.run_app(&mut app) {
        Ok(()) => {
            tracing::info!("exited cleanly");
            drop(guard);
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "the event loop terminated on an error");
            drop(guard);
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// Build summary and dialog smoke flag (phase-00 diagnostics, still useful)
// ---------------------------------------------------------------------------

/// Prints the startup summary: what this build is made of.
fn print_startup_summary() {
    let plan = WindowPlan::default();
    let window = format!(
        "{} x {} logical px, title {:?}",
        plan.size.width, plan.size.height, plan.title
    );

    println!("{}", version_line());
    println!("  window:      {window} (opened by a normal launch)");
    println!(
        "  gpu backends: {}",
        crate::renderer::describe_backends(crate::renderer::preferred_backends())
    );

    match crate::hud::HudSurface::new(2, 2) {
        Some(mut surface) => {
            surface.clear(CANVAS_BACKGROUND);
            let pixel = surface
                .pixel(0, 0)
                .expect("a 2 x 2 surface has a first pixel");
            println!(
                "  rasterise:    tiny-skia {} x {} px, {} bytes per row, clear {pixel:?}",
                surface.width(),
                surface.height(),
                surface.stride()
            );
        }
        None => println!("  rasterise:    tiny-skia surface allocation failed"),
    }

    let fonts = crate::text::TextEngine::new();
    println!(
        "  text:         cosmic-text, {} font faces available",
        fonts.font_count()
    );

    match crate::clipboard::availability() {
        Ok(()) => println!("  clipboard:    available"),
        Err(reason) => println!("  clipboard:    unavailable ({reason})"),
    }

    if crate::platform::heif::supported() {
        println!("  heic:         decodable through the Windows HEIF codec");
    } else {
        println!("  heic:         unavailable (install the HEIF Image Extensions package)");
    }
}

/// Opens the native image dialog once and prints what came back.
///
/// The dialog cannot be covered by a unit test, and a broken file dialog is a
/// phase-05 blocker best discovered on a real desktop now, which is the whole
/// purpose of this flag.
fn check_dialogs() {
    match crate::dialogs::pick_images() {
        Some(paths) if !paths.is_empty() => {
            println!("chose {} file(s):", paths.len());
            for path in paths {
                println!("  {}", path.display());
            }
        }
        _ => println!("no files chosen"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CANVAS_BACKGROUND, LOG_FILES_KEPT, Playback, WindowPlan, looks_like_image, parse_args,
        stale_logs, tick_animations, version_line,
    };
    use glam::Vec2;
    use rizref_codecs::ImageBuffer;
    use rizref_core::{
        Animation, AnimationFrame, Asset, AssetId, ImageItem, ItemId, Rect, Scene, SceneId,
        Selection,
    };
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn the_default_window_matches_the_reference_default() {
        let plan = WindowPlan::default();

        assert_eq!(plan.title, "RizRef");
        assert_eq!(plan.size.width, 600.0);
        assert_eq!(plan.size.height, 400.0);
    }

    #[test]
    fn the_version_line_names_the_binary_and_its_version() {
        let line = version_line();

        assert!(line.starts_with("rizref "));
        assert!(line.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn the_canvas_background_is_opaque() {
        assert_eq!(CANVAS_BACKGROUND[3], 255, "an opaque board, not a ghost");
    }

    #[test]
    fn plain_paths_parse_as_paths() {
        let parsed = parse_args(&args(&["board.rizref", "other board.rizref"])).expect("valid");
        assert_eq!(parsed.paths, ["board.rizref", "other board.rizref"]);
        assert!(!parsed.verbose);
    }

    #[test]
    fn verbose_accepts_both_spellings() {
        for flag in ["--verbose", "-v"] {
            let parsed = parse_args(&args(&[flag])).expect("valid");
            assert!(parsed.verbose, "{flag} should raise the level");
        }
    }

    #[test]
    fn a_lone_dash_is_a_path_not_a_flag() {
        let parsed = parse_args(&args(&["-"])).expect("valid");
        assert_eq!(parsed.paths, ["-"]);
    }

    #[test]
    fn unknown_flags_are_rejected_verbatim() {
        assert_eq!(
            parse_args(&args(&["--nonsense"])),
            Err("--nonsense".to_owned())
        );
        assert_eq!(parse_args(&args(&["-x"])), Err("-x".to_owned()));
    }

    #[test]
    fn only_the_newest_logs_survive_pruning() {
        let names: Vec<String> = [
            "rizref.log.2026-10-24",
            "rizref.log.2026-10-23",
            "rizref.log.2026-10-22",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let remove = stale_logs(&names, 2);
        assert_eq!(remove, ["rizref.log.2026-10-22".to_owned()]);

        let keep_all = stale_logs(&names, LOG_FILES_KEPT);
        assert!(
            keep_all.is_empty(),
            "the configured retention prunes nothing at {LOG_FILES_KEPT}"
        );
    }

    #[test]
    fn only_plausible_image_paths_reach_the_importer() {
        assert!(looks_like_image(Path::new("reference.png")));
        assert!(looks_like_image(Path::new("REFERENCE.JPG")));
        assert!(
            looks_like_image(Path::new("download")),
            "extensionless files are sniffed by the worker"
        );
        assert!(!looks_like_image(Path::new("board.rizref")));
        assert!(!looks_like_image(Path::new("notes.txt")));
    }

    #[test]
    fn pruning_ignores_files_it_does_not_own() {
        let names: Vec<String> = ["panic-1760000000.log", "rizref.log.2026-10-24", "notes.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let remove = stale_logs(&names, 0);
        assert_eq!(remove, ["rizref.log.2026-10-24".to_owned()]);
    }

    /// A scene with one animated item at the origin, 10 x 10 world units.
    fn animated_scene() -> (Scene, AssetId, ItemId) {
        let frame = |value: u8| {
            AnimationFrame::new(
                ImageBuffer::filled(4, 4, [value, value, value, 255]),
                Duration::from_millis(100),
            )
        };
        let animation = Animation::new(vec![frame(1), frame(2), frame(3)]).expect("three frames");
        let still = ImageBuffer::filled(4, 4, [1, 1, 1, 255]);
        let asset = Asset::from_decoded(
            Arc::from(&b"animated gif"[..]),
            rizref_codecs::ImageFormat::Gif,
            &still,
            still.clone(),
        )
        .with_animation(animation);

        let mut scene = Scene::new(SceneId::from_bytes([3; 16]));
        let asset_id = scene.insert_asset(asset).0;
        let item = scene
            .insert_image(asset_id, Vec2::ZERO, Vec2::splat(10.0))
            .expect("the asset is stored");
        (scene, asset_id, item)
    }

    fn visible() -> Rect {
        Rect::from_min_max(Vec2::splat(-100.0), Vec2::splat(100.0))
    }

    #[test]
    fn an_unselected_animation_advances_and_reports_its_next_deadline() {
        let (scene, asset, _) = animated_scene();
        let selection = Selection::new();
        let mut playbacks = HashMap::new();
        let now = Instant::now();

        let (uploads, next) = tick_animations(&scene, &selection, &mut playbacks, visible(), now);

        assert_eq!(uploads, vec![(asset, 1)], "the first tick leaves frame 0");
        assert_eq!(next, Some(now + Duration::from_millis(100)));
        assert_eq!(playbacks[&asset].frame, 1);
    }

    #[test]
    fn a_selected_animation_is_frozen() {
        let (scene, asset, item) = animated_scene();
        let selection = Selection::from_iter([item]);
        let mut playbacks = HashMap::new();
        let now = Instant::now();
        playbacks.insert(
            asset,
            Playback {
                frame: 1,
                next_at: now,
            },
        );

        let (uploads, next) = tick_animations(&scene, &selection, &mut playbacks, visible(), now);

        assert!(uploads.is_empty(), "a frozen animation must not advance");
        assert_eq!(next, None, "and must not keep the event loop awake");
        assert_eq!(playbacks[&asset].frame, 1, "the frame stays where it was");
    }

    #[test]
    fn an_off_screen_animation_is_not_played() {
        let (scene, _, _) = animated_scene();
        let selection = Selection::new();
        let mut playbacks = HashMap::new();
        let away = Rect::from_min_max(Vec2::splat(1_000.0), Vec2::splat(2_000.0));

        let (uploads, next) =
            tick_animations(&scene, &selection, &mut playbacks, away, Instant::now());

        assert!(uploads.is_empty());
        assert_eq!(next, None);
    }

    #[test]
    fn playback_wraps_around_the_frame_list() {
        let (scene, asset, _) = animated_scene();
        let selection = Selection::new();
        let now = Instant::now();
        let mut playbacks = HashMap::new();
        playbacks.insert(
            asset,
            Playback {
                frame: 2,
                next_at: now,
            },
        );

        let (uploads, _) = tick_animations(&scene, &selection, &mut playbacks, visible(), now);

        assert_eq!(
            uploads,
            vec![(asset, 0)],
            "after the last frame comes the first"
        );
    }

    #[test]
    fn a_frame_that_is_not_due_yet_waits() {
        let (scene, asset, _) = animated_scene();
        let selection = Selection::new();
        let now = Instant::now();
        let mut playbacks = HashMap::new();
        playbacks.insert(
            asset,
            Playback {
                frame: 1,
                next_at: now + Duration::from_millis(50),
            },
        );

        let (uploads, next) = tick_animations(&scene, &selection, &mut playbacks, visible(), now);

        assert!(uploads.is_empty());
        assert_eq!(next, Some(now + Duration::from_millis(50)));
    }

    #[test]
    fn an_item_without_an_animation_never_advances() {
        let still = ImageBuffer::filled(4, 4, [9, 9, 9, 255]);
        let asset = Asset::from_decoded(
            Arc::from(&b"a still image"[..]),
            rizref_codecs::ImageFormat::Png,
            &still,
            still.clone(),
        );
        let mut scene = Scene::new(SceneId::from_bytes([4; 16]));
        let asset_id = scene.insert_asset(asset).0;
        scene
            .insert_image(asset_id, Vec2::ZERO, Vec2::splat(10.0))
            .expect("inserted");
        let mut playbacks = HashMap::new();

        let (uploads, next) = tick_animations(
            &scene,
            &Selection::new(),
            &mut playbacks,
            visible(),
            Instant::now(),
        );

        assert!(uploads.is_empty());
        assert_eq!(next, None);
        assert!(playbacks.is_empty(), "still images get no playback clock");
    }

    #[test]
    fn a_plain_click_selects_the_topmost_item_under_the_cursor() {
        let (mut scene, asset, first) = animated_scene();
        let second = scene
            .insert_image(asset, Vec2::ZERO, Vec2::splat(10.0))
            .expect("inserted");
        let mut selection = Selection::new();

        // Both items cover the origin; the later one is drawn on top.
        let hit = scene
            .items()
            .iter()
            .rev()
            .find(|item| item.hit_test(Vec2::ZERO))
            .map(ImageItem::id)
            .expect("something is under the cursor");
        selection.set_single(hit);

        assert_eq!(hit, second);
        assert!(!selection.contains(first));
    }
}
