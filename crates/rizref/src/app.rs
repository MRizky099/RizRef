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
//! when something changed (resize, a gesture, an import, an animation), never
//! on a timer, so an idle board costs no CPU — a reference viewer sits in the
//! background for hours while its user paints elsewhere.
//!
//! The editing rules themselves live in [`crate::board`]. This module translates
//! window events into board calls, owns the note editor and the note textures,
//! and draws whatever the board reports.

use std::collections::{BTreeSet, HashMap};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glam::{DVec2, Vec2};
use rizref_core::{Asset, AssetId, Item, ItemBody, ItemId, Rect, Scene, SceneId, Selection};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::board::{Board, CursorHint, Modifiers, ViewInfo};
use crate::canvas::{HudUpload, ItemQuad, QuadInstance};
use crate::clipboard::{self, ClipboardContent};
use crate::dev_state;
use crate::hud::HudSurface;
use crate::import::{ImportOutcome, ImportRequest, Importer};
use crate::input::{self, Button as InputButton, Command, Key as InputKey, Trigger};
use crate::memory::DecodeCache;
use crate::note::{self, NoteSession};
use crate::overlay::{self, OverlayState, OverlayVertex};
use crate::placement;
use crate::platform::{
    self, DropEvent, DropRegistration, SingleInstance, forward_args_to_primary,
    serve_forwarded_args,
};
use crate::renderer::{FrameError, FrameParams, GpuSurface};
use crate::text::TextEngine;
use crate::view::{FRAME_PADDING, ViewState};

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

/// Readout surface width in logical pixels.
/// Readout surface width in logical pixels.
///
/// Narrow enough for the 600 px default window with its 14 px margin, and wide
/// enough for the zoom readout plus a selection size.
const READOUT_WIDTH: f64 = 560.0;

/// Readout surface height in logical pixels.
const READOUT_HEIGHT: f64 = 40.0;

/// How many daily log files to keep when pruning at startup.
const LOG_FILES_KEPT: usize = 14;

/// Double-click window for zoom-to-item and note editing.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

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
    println!();
    println!("CANVAS: middle-drag or space+drag pans, the wheel zooms, alt+wheel turns the view.");
    println!("ITEMS:  click/shift-click selects, drag moves, handles scale and rotate.");
    println!("        ctrl+A all, ctrl+D duplicate, del delete, ctrl+Z/Y undo/redo,");
    println!("        ctrl+G / ctrl+shift+G group and ungroup, pgup/pgdn/home/end order,");
    println!("        3/4/5/6 arrange (grid/hierarchy/spread/stack), N adds a note,");
    println!("        ctrl+shift+arrows align, ctrl+shift+,/. distribute,");
    println!("        shift+H/V flip, ctrl+shift+W/H match size, ctrl+shift+R reset.");
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

/// One note's resident texture.
#[derive(Debug, Clone, Copy)]
struct NoteTexture {
    /// Content-addressed texture id.
    id: AssetId,
    /// World size the texture was rendered for.
    world_size: Vec2,
}

/// Everything the event loop owns: the window, its surface, the board state
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
    /// The board: items, selection, groups and the undo history.
    board: Board,
    /// Full-resolution decoded bitmaps under the memory cap.
    decode_cache: DecodeCache,
    /// The import worker pool, `None` only if no thread could start.
    importer: Option<Importer>,
    /// Paths from the command line or a second launch, imported once the
    /// window and its camera exist.
    pending_paths: Vec<String>,
    /// Font system and glyph caches for the HUD and notes.
    text: TextEngine,
    /// The CPU-raster readout surface, sized once the scale factor is known.
    hud: Option<HudSurface>,
    /// Keyboard modifiers as of the last `ModifiersChanged`.
    modifiers: ModifiersState,
    /// Playback clocks for the animated assets on the board.
    playbacks: HashMap<AssetId, Playback>,
    /// When the next animation frame is due, for the event loop's timer.
    next_frame_at: Option<Instant>,
    /// The active note edit session, if any.
    note_session: Option<NoteSession>,
    /// Resident note textures.
    note_textures: HashMap<ItemId, NoteTexture>,
    /// Notes whose texture must be re-rendered on the next frame.
    dirty_notes: BTreeSet<ItemId>,
    /// The last click, for double-click detection.
    last_click: Option<(Instant, Option<ItemId>)>,
    /// The cursor currently set on the window.
    cursor_icon: Option<CursorIcon>,
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
            board: Board::new(scene),
            decode_cache: DecodeCache::with_default_cap(),
            importer: Some(importer),
            pending_paths: Vec::new(),
            text: TextEngine::new(),
            hud: None,
            modifiers: ModifiersState::default(),
            playbacks: HashMap::new(),
            next_frame_at: None,
            note_session: None,
            note_textures: HashMap::new(),
            dirty_notes: BTreeSet::new(),
            last_click: None,
            cursor_icon: None,
        }
    }

    /// Whether closing needs a confirmation dialog.
    ///
    /// Always `false` today: the dev board is not saved anywhere yet. Phase 05
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

    /// The keyboard modifiers as the board sees them.
    fn current_mods(&self) -> Modifiers {
        Modifiers {
            shift: self.modifiers.shift_key(),
            ctrl: self.modifiers.control_key(),
            alt: self.modifiers.alt_key(),
        }
    }

    /// The current view, for board calls.
    fn view_info(&self) -> ViewInfo<'_> {
        ViewInfo {
            camera: &self.view.camera,
            viewport: self.view.viewport(),
        }
    }

    /// Drains the board's effects into the note-texture bookkeeping and asks
    /// for a redraw.
    fn after_board(&mut self) {
        let effects = self.board.take_effects();
        if effects.is_empty() {
            return;
        }
        for id in &effects.content {
            self.dirty_notes.insert(*id);
        }
        if effects.structural() {
            self.note_textures
                .retain(|id, _| self.board.scene().contains(*id));
        }
        self.redraw();
    }

    /// Applies a resolved command with the current scene rect and redraws.
    fn apply(&mut self, command: Command) {
        use Command::{
            AlignBottom, AlignLeft, AlignRight, AlignTop, ArrangeGrid, ArrangeHierarchy,
            ArrangeSpread, ArrangeStack, CreateNote, DeleteSelection, Deselect,
            DistributeHorizontal, DistributeVertical, DuplicateSelection, FitContent,
            FlipHorizontal, FlipVertical, GroupSelection, LowerSelection, LowerToBottom,
            MatchHeight, MatchWidth, RaiseSelection, RaiseToTop, Redo, ResetRotation,
            ResetTransform, SelectAll, Undo, UngroupSelection, ZoomHundred,
        };

        if matches!(
            command,
            Command::BeginPan(_)
                | Command::EndPan
                | Command::ZoomNotch(_)
                | Command::RotateNotch(_)
                | FitContent
                | ZoomHundred
                | ResetRotation
        ) {
            self.view.apply_command(command, self.board.scene_rect());
            self.redraw();
            return;
        }

        // Any non-view shortcut while a note is being edited ends the edit
        // first, so the text becomes an undo step before the command runs.
        self.commit_note_edit();

        let changed = match command {
            Undo => {
                if let Some(label) = self.board.undo() {
                    tracing::debug!(%label, "undo");
                }
                true
            }
            Redo => {
                if let Some(label) = self.board.redo() {
                    tracing::debug!(%label, "redo");
                }
                true
            }
            SelectAll => {
                self.board.select_all();
                true
            }
            Deselect => {
                self.board.clear_selection();
                true
            }
            DuplicateSelection => {
                let view = ViewInfo {
                    camera: &self.view.camera,
                    viewport: self.view.viewport(),
                };
                self.board.duplicate_selection(view);
                true
            }
            DeleteSelection => self.board.delete_selection(),
            Command::Nudge { dx, dy, pixels } => {
                let view = ViewInfo {
                    camera: &self.view.camera,
                    viewport: self.view.viewport(),
                };
                self.board.nudge(dx, dy, pixels, view)
            }
            FlipHorizontal => self.board.flip(true),
            FlipVertical => self.board.flip(false),
            ResetTransform => self.board.reset_transform(),
            MatchWidth => self.board.match_size(rizref_core::SizeAxis::Width),
            MatchHeight => self.board.match_size(rizref_core::SizeAxis::Height),
            GroupSelection => self.board.group_selection(),
            UngroupSelection => self.board.ungroup_selection(),
            RaiseSelection => self.board.raise_selection(),
            LowerSelection => self.board.lower_selection(),
            RaiseToTop => self.board.front_selection(),
            LowerToBottom => self.board.back_selection(),
            ArrangeGrid => self
                .board
                .arrange_selection(rizref_core::ArrangeMode::Grid { columns: None }),
            ArrangeHierarchy => self
                .board
                .arrange_selection(rizref_core::ArrangeMode::Hierarchy),
            ArrangeSpread => self
                .board
                .arrange_selection(rizref_core::ArrangeMode::Spread),
            ArrangeStack => self
                .board
                .arrange_selection(rizref_core::ArrangeMode::Stack),
            AlignLeft => self.board.align_selection(rizref_core::Align::Left),
            AlignRight => self.board.align_selection(rizref_core::Align::Right),
            AlignTop => self.board.align_selection(rizref_core::Align::Top),
            AlignBottom => self.board.align_selection(rizref_core::Align::Bottom),
            DistributeHorizontal => self
                .board
                .distribute_selection(rizref_core::Orientation::Horizontal),
            DistributeVertical => self
                .board
                .distribute_selection(rizref_core::Orientation::Vertical),
            CreateNote => {
                let id = self.board.create_note(self.drop_world());
                self.begin_note_edit(id);
                true
            }
            _ => false,
        };

        if changed {
            self.after_board();
            self.redraw();
        }
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

    /// The HUD readout line: zoom, rotation, cursor, selection size.
    fn readout_text(&self) -> String {
        let zoom = self.view.camera.zoom * 100.0;
        let zoom = if (zoom - zoom.round()).abs() < 0.05 {
            format!("{}", zoom.round() as i64)
        } else {
            format!("{zoom:.1}")
        };
        let rotation = self.view.camera.rotation.to_degrees();
        let mut notes = Vec::new();
        if let Some(note) = self.selected_animation_note() {
            notes.push(note);
        }
        let selection = self.board.selection();
        if !selection.is_empty() {
            match self.board.selection_bounds() {
                Some(bounds) => notes.push(format!(
                    "{} selected · {:.0} × {:.0}",
                    selection.len(),
                    bounds.width(),
                    bounds.height()
                )),
                None => notes.push(format!("{} selected", selection.len())),
            }
        }
        let extra = if notes.is_empty() {
            String::new()
        } else {
            format!(" · {}", notes.join(" · "))
        };
        if self.view.has_cursor() {
            let world = self.view.cursor_world();
            format!(
                "{zoom}% · {rotation:.0}° · ({:.1}, {:.1}){extra}",
                world.x, world.y
            )
        } else {
            format!("{zoom}% · {rotation:.0}°{extra}")
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
        let (asset_id, was_new) = self.board.scene_mut().insert_asset(asset);
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
            .board
            .scene()
            .items()
            .iter()
            .map(Item::world_rect)
            .collect();
        let center = placement::find_spot(
            outcome.drop_world,
            Vec2::new(width, height),
            &occupied,
            placement::ITEM_GAP,
        );
        let Some(item_id) =
            self.board
                .scene_mut()
                .insert_image(asset_id, center, Vec2::new(width, height))
        else {
            tracing::error!(asset = %asset_id, "asset vanished between intern and insert");
            return;
        };
        self.board.refresh_rects();

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

    /// Renders any note whose content or size changed, and uploads it.
    fn ensure_note_textures(&mut self) {
        if self.renderer.is_none() {
            return;
        }
        let active = self.note_session.as_ref().map(NoteSession::item);
        let gesture = self.board.is_gesture_active();
        let notes: Vec<(ItemId, String, Vec2)> = self
            .board
            .scene()
            .items()
            .iter()
            .filter_map(|item| {
                item.text()
                    .map(|text| (item.id(), text.to_owned(), item.size()))
            })
            .collect();

        for (id, text, size) in notes {
            let wanted = note::texture_id(&text, size);
            let current = self.note_textures.get(&id).copied();
            let resident = self
                .renderer
                .as_ref()
                .is_some_and(|renderer| renderer.has_texture(wanted));
            let stale = current.is_none_or(|entry| entry.id != wanted || entry.world_size != size)
                || self.dirty_notes.contains(&id)
                || !resident;
            if !stale {
                continue;
            }
            // During a drag a note keeps its existing texture stretched; it is
            // re-rendered once at the final size instead of every mouse move.
            if gesture && active != Some(id) && current.is_some() {
                continue;
            }

            let session = self
                .note_session
                .as_mut()
                .filter(|session| session.item() == id);
            let Some(image) = note::render(&mut self.text, &text, size, session) else {
                continue;
            };
            self.dirty_notes.remove(&id);
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.upload_texture(wanted, &image, false);
            }
            self.note_textures.insert(
                id,
                NoteTexture {
                    id: wanted,
                    world_size: size,
                },
            );
        }
    }

    /// Builds this frame's drawable items in scene z-order.
    fn build_items(&self) -> Vec<ItemQuad> {
        self.board
            .scene()
            .items()
            .iter()
            .filter_map(|item| {
                let asset = match item.body() {
                    ItemBody::Image { asset } => *asset,
                    ItemBody::Note { .. } => self.note_textures.get(&item.id())?.id,
                };
                let center = item.center();
                let size = item.size();
                let mut instance =
                    QuadInstance::new([center.x, center.y], [size.x, size.y], item.rotation());
                instance.uv = match (item.flip_x(), item.flip_y()) {
                    (false, false) => [0.0, 0.0, 1.0, 1.0],
                    (true, false) => [1.0, 0.0, 0.0, 1.0],
                    (false, true) => [0.0, 1.0, 1.0, 0.0],
                    (true, true) => [1.0, 1.0, 0.0, 0.0],
                };
                instance.tint = [1.0, 1.0, 1.0, item.opacity()];
                Some(ItemQuad { instance, asset })
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
                if let Some(asset) = self.board.scene().asset(item.asset) {
                    renderer.upload_texture(item.asset, asset.preview().image(), true);
                }
            } else if renderer.texture_is_preview(item.asset)
                && let Some(full) = self.decode_cache.get(item.asset)
            {
                renderer.upload_texture(item.asset, &full, false);
            }
        }
    }

    /// Builds the frame's overlay triangles.
    fn build_overlay(&self) -> Vec<OverlayVertex> {
        let state = OverlayState {
            camera: &self.view.camera,
            viewport: self.view.viewport(),
            scene: self.board.scene(),
            selection: self.board.selection(),
            hover: self.board.hover(),
            marquee: self.board.marquee(),
            guides: self.board.guides(),
        };
        overlay::build(&state)
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
            self.board.scene(),
            self.board.selection(),
            &mut self.playbacks,
            visible,
            now,
        );

        if !uploads.is_empty()
            && let Some(renderer) = self.renderer.as_mut()
        {
            for (asset, frame) in uploads {
                if let Some(animation) = self.board.scene().asset(asset).and_then(Asset::animation)
                {
                    renderer.upload_texture(asset, animation.frame(frame).image(), false);
                }
            }
        }

        self.next_frame_at = next;
        next
    }

    /// The HUD note for a frozen animation, when one is selected.
    fn selected_animation_note(&self) -> Option<String> {
        let id = self.board.selection().iter().next()?;
        let item = self.board.scene().item(id)?;
        let asset = self.board.scene().asset(item.asset()?)?;
        let animation = asset.animation()?;
        let frame = self
            .playbacks
            .get(&item.asset()?)
            .map_or(0, |playback| playback.frame);
        Some(format!("GIF frozen {}/{}", frame + 1, animation.len()))
    }

    // -----------------------------------------------------------------------
    // Notes
    // -----------------------------------------------------------------------

    /// Starts editing `id` and selects it.
    fn begin_note_edit(&mut self, id: ItemId) {
        let Some(item) = self.board.scene().item(id) else {
            return;
        };
        self.note_session = NoteSession::begin(&mut self.text, item);
        if self.note_session.is_some() {
            // The space bar types a space while editing; make sure a pan
            // armed before the double-click does not stay held.
            self.view.set_space_held(false);
            self.board.select_only(id);
            self.dirty_notes.insert(id);
            self.redraw();
        }
    }

    /// Ends the edit session, committing the text as one undo step.
    fn commit_note_edit(&mut self) {
        let Some(session) = self.note_session.take() else {
            return;
        };
        let id = session.item();
        let original = session.original().to_owned();
        let text = session.text();
        self.board.commit_note_text(id, &original, &text);
        self.dirty_notes.insert(id);
        self.after_board();
    }

    /// Copies the editor's text into the scene so the live preview updates.
    fn sync_note_text(&mut self) {
        let Some(session) = self.note_session.as_ref() else {
            return;
        };
        let id = session.item();
        let text = session.text();
        self.board.set_note_text_live(id, &text);
        self.dirty_notes.insert(id);
        self.after_board();
    }

    /// Applies one editor action and redraws.
    fn note_action(&mut self, action: cosmic_text::Action) {
        let Some(session) = self.note_session.as_mut() else {
            return;
        };
        session.action(&mut self.text, action);
        self.sync_note_text();
    }

    /// Inserts text at the caret.
    fn note_insert(&mut self, text: &str) {
        let Some(session) = self.note_session.as_mut() else {
            return;
        };
        session.insert_text(&mut self.text, text);
        self.sync_note_text();
    }

    /// Places the caret at a world point inside the edited note.
    fn note_click(&mut self, world: Vec2) {
        let Some(session) = self.note_session.as_mut() else {
            return;
        };
        let Some(item) = self.board.scene().item(session.item()) else {
            return;
        };
        let local = item.transform().to_local(world);
        // A mirrored note shows mirrored text, so the caret's horizontal and
        // vertical position has to be mirrored back into the texture.
        let local = Vec2::new(
            if item.flip_x() { -local.x } else { local.x },
            if item.flip_y() { -local.y } else { local.y },
        );
        let pixels_per_world = session.pixels_per_world();
        let x = (local.x + item.size().x * 0.5 - note::NOTE_PADDING) * pixels_per_world;
        let y = (local.y + item.size().y * 0.5 - note::NOTE_PADDING) * pixels_per_world;
        session.click(&mut self.text, x, y);
        self.sync_note_text();
    }

    /// Routes one key event to the note editor. Returns whether it consumed it.
    fn handle_note_key(&mut self, event: &KeyEvent, pressed: bool) -> bool {
        use cosmic_text::{Action, Motion};
        if !pressed {
            return false;
        }
        let ctrl = self.modifiers.control_key();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.commit_note_edit();
                true
            }
            Key::Named(NamedKey::Backspace) => {
                self.note_action(Action::Backspace);
                true
            }
            Key::Named(NamedKey::Delete) => {
                self.note_action(Action::Delete);
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.note_action(Action::Enter);
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.note_action(Action::Motion(Motion::Left));
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.note_action(Action::Motion(Motion::Right));
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.note_action(Action::Motion(Motion::Up));
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.note_action(Action::Motion(Motion::Down));
                true
            }
            Key::Named(NamedKey::Home) => {
                self.note_action(Action::Motion(Motion::Home));
                true
            }
            Key::Named(NamedKey::End) => {
                self.note_action(Action::Motion(Motion::End));
                true
            }
            Key::Character(text) if ctrl => {
                match text.to_ascii_lowercase().as_str() {
                    "a" => {
                        if let Some(session) = self.note_session.as_mut() {
                            session.select_all();
                            let id = session.item();
                            self.dirty_notes.insert(id);
                            self.redraw();
                        }
                    }
                    "v" => {
                        if let Ok(text) = clipboard::read_text() {
                            self.note_insert(&text);
                        }
                    }
                    _ => {}
                }
                true
            }
            Key::Character(_) => {
                if let Some(text) = &event.text {
                    self.note_insert(text);
                }
                true
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Cursor
    // -----------------------------------------------------------------------

    /// Updates the window cursor icon from the board's hint.
    fn update_cursor_icon(&mut self, window: &Window, screen: DVec2) {
        let hint = if self.note_session.is_some() {
            CursorHint::Text
        } else {
            self.board
                .cursor(screen, self.view_info(), self.current_mods())
        };
        let icon = match hint {
            CursorHint::Default => CursorIcon::Default,
            CursorHint::Move => CursorIcon::Move,
            CursorHint::Grab => CursorIcon::Grab,
            CursorHint::Grabbing => CursorIcon::Grabbing,
            CursorHint::ResizeEw => CursorIcon::EwResize,
            CursorHint::ResizeNs => CursorIcon::NsResize,
            CursorHint::ResizeNwSe => CursorIcon::NwseResize,
            CursorHint::ResizeNeSw => CursorIcon::NeswResize,
            CursorHint::Rotate => CursorIcon::Crosshair,
            CursorHint::Text => CursorIcon::Text,
            CursorHint::Crosshair => CursorIcon::Crosshair,
        };
        if self.cursor_icon != Some(icon) {
            window.set_cursor(icon);
            self.cursor_icon = Some(icon);
        }
    }

    /// Handles a plain left press: note editing, double-click, then selection.
    fn left_press(&mut self, window: &Window) {
        let Some(screen) = self.view.cursor_position() else {
            return;
        };
        let world = self
            .view
            .camera
            .screen_to_world(screen, self.view.viewport())
            .as_vec2();
        let item = self.board.item_at(world);

        let now = Instant::now();
        let double_click = self.last_click.is_some_and(|(then, last)| {
            last == item && item.is_some() && now.duration_since(then) < DOUBLE_CLICK
        });
        self.last_click = Some((now, item));

        if let Some(session_item) = self.note_session.as_ref().map(NoteSession::item) {
            if item == Some(session_item) {
                self.note_click(world);
                return;
            }
            self.commit_note_edit();
        }

        if double_click && let Some(id) = item {
            if self.board.scene().item(id).is_some_and(Item::is_note) {
                self.begin_note_edit(id);
                self.note_click(world);
                return;
            }
            if let Some(item) = self.board.scene().item(id) {
                let rect = item.world_rect();
                self.view
                    .camera
                    .frame(rect, self.view.viewport(), FRAME_PADDING);
                self.board.select_only(id);
                self.redraw();
                return;
            }
        }

        let mods = self.current_mods();
        let view = ViewInfo {
            camera: &self.view.camera,
            viewport: self.view.viewport(),
        };
        if self.board.pointer_down(screen, view, mods) {
            self.after_board();
        }
        self.update_cursor_icon(window, screen);
        self.redraw();
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
                if self.view.pan_active() {
                    if let Some(previous) = previous {
                        self.view.drag_pan(logical - previous);
                    }
                } else {
                    let view = ViewInfo {
                        camera: &self.view.camera,
                        viewport: self.view.viewport(),
                    };
                    if self.board.pointer_move(logical, view, self.current_mods()) {
                        self.after_board();
                    }
                }
                self.update_cursor_icon(&window, logical);
                self.redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                if self.board.pointer_leave() {
                    self.redraw();
                }
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
                    return;
                }
                if button == InputButton::Left {
                    if pressed {
                        if !self.view.space_held() {
                            self.left_press(&window);
                        }
                    } else if let Some(screen) = self.view.cursor_position() {
                        let view = ViewInfo {
                            camera: &self.view.camera,
                            viewport: self.view.viewport(),
                        };
                        if self.board.pointer_up(screen, view, self.current_mods()) {
                            // A drag/scale/rotate is not the first half of a
                            // double-click on the item it ended over.
                            self.last_click = None;
                            self.after_board();
                        }
                        self.update_cursor_icon(&window, screen);
                        self.redraw();
                    }
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
                if self.note_session.is_some() {
                    if self.handle_note_key(&event, pressed) {
                        self.redraw();
                    }
                    return;
                }
                if matches!(&event.logical_key, Key::Named(NamedKey::Space)) {
                    self.view.set_space_held(pressed);
                    return;
                }
                if !pressed {
                    return;
                }
                if self.modifiers.control_key()
                    && let Key::Character(text) = &event.logical_key
                {
                    match text.to_ascii_lowercase().as_str() {
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
                if let Some(key) = input_key(&event.logical_key)
                    && let Some(command) = input::resolve(&Trigger::Key {
                        key,
                        pressed,
                        mods: self.current_mods(),
                    })
                {
                    self.apply(command);
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
                self.ensure_note_textures();
                let items = self.build_items();
                self.ensure_textures(&items);
                let overlay = self.build_overlay();
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
                    scene_rect: self.board.scene_rect(),
                    items: &items,
                    overlay: &overlay,
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
                self.commit_note_edit();
                if self.should_confirm_close() {
                    // Phase 05 asks "save changes?" here.
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

/// Maps a winit logical key to the input table's vocabulary.
fn input_key(key: &Key) -> Option<InputKey> {
    match key {
        Key::Character(text) => match text.to_ascii_lowercase().as_str() {
            "0" => Some(InputKey::Digit0),
            "1" => Some(InputKey::Digit1),
            "2" => Some(InputKey::Digit2),
            "3" => Some(InputKey::Digit3),
            "4" => Some(InputKey::Digit4),
            "5" => Some(InputKey::Digit5),
            "6" => Some(InputKey::Digit6),
            "a" => Some(InputKey::A),
            "d" => Some(InputKey::D),
            "f" => Some(InputKey::F),
            "g" => Some(InputKey::G),
            "h" => Some(InputKey::H),
            "n" => Some(InputKey::N),
            "r" => Some(InputKey::R),
            "v" => Some(InputKey::V),
            "w" => Some(InputKey::W),
            "y" => Some(InputKey::Y),
            "z" => Some(InputKey::Z),
            "," => Some(InputKey::Comma),
            "." => Some(InputKey::Period),
            _ => None,
        },
        Key::Named(named) => match named {
            NamedKey::ArrowLeft => Some(InputKey::Left),
            NamedKey::ArrowRight => Some(InputKey::Right),
            NamedKey::ArrowUp => Some(InputKey::Up),
            NamedKey::ArrowDown => Some(InputKey::Down),
            NamedKey::PageUp => Some(InputKey::PageUp),
            NamedKey::PageDown => Some(InputKey::PageDown),
            NamedKey::Home => Some(InputKey::Home),
            NamedKey::End => Some(InputKey::End),
            NamedKey::Delete => Some(InputKey::Delete),
            NamedKey::Backspace => Some(InputKey::Backspace),
            NamedKey::Escape => Some(InputKey::Escape),
            _ => None,
        },
        _ => None,
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
        let Some(asset_id) = item.asset() else {
            continue;
        };
        let Some(asset) = scene.asset(asset_id) else {
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

        let playback = playbacks.entry(asset_id).or_insert(Playback {
            frame: 0,
            next_at: now,
        });
        if now >= playback.next_at {
            let frame = (playback.frame + 1) % animation.len();
            playback.frame = frame;
            playback.next_at = now + animation.delay_after(frame);
            uploads.push((asset_id, frame));
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

    println!("  heic:         heic-rs (pure Rust; all platforms)");
    println!("  avif:         Windows codec (needs the AV1 Video Extension)");
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
        CANVAS_BACKGROUND, LOG_FILES_KEPT, Playback, WindowPlan, input_key, looks_like_image,
        parse_args, stale_logs, tick_animations, version_line,
    };
    use glam::Vec2;
    use rizref_codecs::ImageBuffer;
    use rizref_core::{
        Animation, AnimationFrame, Asset, AssetId, Item, ItemId, Rect, Scene, SceneId, Selection,
    };
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use winit::keyboard::{Key, NamedKey};

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

    #[test]
    fn logical_keys_map_to_the_input_table() {
        assert_eq!(
            input_key(&Key::Character("Z".into())),
            Some(super::InputKey::Z)
        );
        assert_eq!(
            input_key(&Key::Named(NamedKey::ArrowLeft)),
            Some(super::InputKey::Left)
        );
        assert_eq!(input_key(&Key::Named(NamedKey::Tab)), None);
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
    fn notes_never_get_a_playback_clock() {
        let mut scene = Scene::new(SceneId::from_bytes([5; 16]));
        scene.insert_note("a note", Vec2::ZERO, Vec2::splat(10.0));
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
        assert!(playbacks.is_empty());
    }

    #[test]
    fn a_plain_click_selects_the_topmost_item_under_the_cursor() {
        let (mut scene, asset, first) = animated_scene();
        let second = scene
            .insert_image(asset, Vec2::ZERO, Vec2::splat(10.0))
            .expect("inserted");

        // Both items cover the origin; the later one is drawn on top.
        let hit = scene
            .items()
            .iter()
            .rev()
            .find(|item| item.hit_test(Vec2::ZERO))
            .map(Item::id)
            .expect("something is under the cursor");

        assert_eq!(hit, second);
        assert_ne!(hit, first);
    }
}
