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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glam::{DVec2, Vec2};
use rizref_core::{
    Asset, AssetId, Camera, Item, ItemBody, ItemId, Rect, Scene, SceneId, Selection,
};
use rizref_format::{AssetProvenance, SaveMode, SaveRequest};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

use crate::accessibility::Accessibility;
use crate::autosave::{self, AutosavePolicy, AutosaveTracker, PreviousSession, Recovery};
use crate::board::{Board, CursorHint, Modifiers, ViewInfo};
use crate::canvas::{FrameFlags, HudUpload, ItemQuad, QuadInstance, UiUpload};
use crate::clipboard::{self, ClipboardContent};
use crate::dialogs;
use crate::document::Document;
use crate::hud::HudSurface;
use crate::import::{ImportOutcome, ImportPurpose, ImportRequest, ImportSource, Importer};
use crate::input::{self, Button as InputButton, Command, Trigger};
use crate::location::{self, LocationPlan};
use crate::memory::DecodeCache;
use crate::note::{self, NoteRenderer, NoteSession};
use crate::note_chrome::{
    NoteChrome, PillButton, PillLayout, ScreenRect, ToolbarButton, ToolbarLayout,
};
use crate::overlay::{self, OverlayState, OverlayVertex};
use crate::perf::{self, FrameBench, FrameStats, Milestone, StartupClock};
use crate::placement;
use crate::platform::launch;
use crate::platform::window_modes::WindowModes;
use crate::platform::{
    self, DropEvent, DropRegistration, SingleInstance, forward_args_to_primary,
    serve_forwarded_args,
};
use crate::provenance;
use crate::recents::{Recents, now_unix};
use crate::renderer::{FrameError, FrameParams, GpuDevice, GpuSurface};
use crate::settings::Settings;
use crate::shortcuts::{Key as ShortcutKey, KeyChord, ShortcutId, Shortcuts};
use crate::text::TextEngine;
use crate::ui::image_info::{ImageInfo, ImageInfoPanel, InfoButton, InfoResult};
use crate::ui::menu::{self, Action, MenuContext, MenuEntry, MenuResult};
use crate::ui::note_color::{NoteColorPopover, PopoverResult};
use crate::ui::panels::{self, CloseChoice, PanelResult, SettingId};
use crate::ui::start::{self, StartLayout};
use crate::ui::titlebar::{self, TitleBarButton};
use crate::ui::{Mode, Shell, UiLayer};
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

/// Thickness of the frameless window's edge-resize band, in logical pixels.
const RESIZE_EDGE: f64 = 5.0;

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
    /// Raise an access violation after the crash handler is installed (the
    /// minidump check).
    crash_test: bool,
    /// Print the startup waterfall as JSON once the first frame is presented,
    /// then exit (the cold-start bench, `scripts/bench-startup.ps1`).
    exit_after_first_frame: bool,
    /// Drive the scripted pan/zoom soak for this many frames once the board is
    /// on screen, print the frame-time report as JSON, then exit.
    bench_frames: Option<u32>,
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
            "--crash-test" => parsed.crash_test = true,
            "--exit-after-first-frame" => parsed.exit_after_first_frame = true,
            flag if flag.starts_with("--bench-frames=") => {
                let count = &flag["--bench-frames=".len()..];
                match count.parse::<u32>() {
                    Ok(frames) if frames > 0 => parsed.bench_frames = Some(frames),
                    _ => return Err(flag.to_owned()),
                }
            }
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
    if args.first().map(String::as_str) == Some("inspect") {
        return crate::inspect::run(&args[1..]);
    }
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
    let mut startup = StartupClock::new(perf::process_start());
    startup.mark(Milestone::Logging);
    install_panic_hook(log_dir);
    let dump = platform::crash::install(&platform::crash::crashes_dir());
    if parsed.crash_test {
        tracing::warn!(dump = %dump.display(), "--crash-test: raising an access violation");
        platform::crash::raise_access_violation();
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), ?parsed, "starting");

    if parsed.check_dialogs {
        check_dialogs();
        return ExitCode::SUCCESS;
    }
    if parsed.build_info {
        print_startup_summary();
        return ExitCode::SUCCESS;
    }

    // The benches measure a launch of their own: they neither hand their
    // arguments to a running RizRef nor touch its session marker.
    if parsed.exit_after_first_frame || parsed.bench_frames.is_some() {
        return run_window(None, parsed, startup);
    }

    match SingleInstance::acquire() {
        Ok(guard) => run_window(Some(guard), parsed, startup),
        Err(()) => {
            // Either another instance owns the mutex, or the mutex could not
            // be created. Handing the arguments over is correct in both
            // cases; when nobody answers, the guard was stale and this
            // process becomes the instance.
            if forward_args_to_primary(&parsed.paths) {
                ExitCode::SUCCESS
            } else {
                tracing::warn!("no running instance answered; continuing as the instance");
                run_window(None, parsed, startup)
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
    println!("    rizref inspect <FILE> [--verify]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help            print this help and exit");
    println!("    -V, --version         print the version and exit");
    println!("    -v, --verbose         log at debug level");
    println!("    --build-info          print what this build contains and exit");
    println!("    --check-dialogs       open the native file dialog and print the chosen paths");
    println!("    --exit-after-first-frame");
    println!(
        "                          print the startup timings as JSON after the first frame and exit"
    );
    println!(
        "    --bench-frames=N      pan/zoom the opened board for N frames, print frame times, exit"
    );
    println!();
    println!("SUBCOMMANDS:");
    println!("    inspect <FILE>        print a board's header, chunks and document summary");
    println!("                          (--verify recomputes every digest)");
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
        // A panic inside a containment boundary (a decoder, the math
        // backend) is reported by the boundary as an ordinary error; exiting
        // here would defeat it, which is what happened before phase 07.
        if crate::contain::is_contained() {
            tracing::warn!(%info, "contained panic");
            return;
        }
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
pub(crate) enum AppEvent {
    /// A second launch handed its arguments to this instance.
    ForwardedArgs(Vec<String>),
    /// An import worker finished one request.
    Import(Box<ImportOutcome>),
    /// The OLE drop target received files, in client-area physical pixels.
    Dropped(Box<DropEvent>),
    /// An accessibility adapter event (tree request, action, deactivation).
    Accessibility(accesskit_winit::Event),
    /// The background GPU initialisation finished (successfully or not).
    GpuReady,
}

impl From<accesskit_winit::Event> for AppEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
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
    /// Assets whose full-resolution texture has been requested from the pool.
    pending_textures: BTreeSet<AssetId>,
    /// When the next animation frame is due, for the event loop's timer.
    next_frame_at: Option<Instant>,
    /// The active note edit session, if any.
    note_session: Option<NoteSession>,
    /// Markdown layout and math caches shared by every note.
    note_renderer: NoteRenderer,
    /// The note whose hover toolbar is showing.
    note_hover: Option<ItemId>,
    /// The note the eyedropper was armed for from its popover.
    pick_for_note: Option<ItemId>,
    /// Resident note textures.
    note_textures: HashMap<ItemId, NoteTexture>,
    /// Notes whose texture must be re-rendered on the next frame.
    dirty_notes: BTreeSet<ItemId>,
    /// The last click, for double-click detection.
    last_click: Option<(Instant, Option<ItemId>)>,
    /// The last press on the title bar's drag area, for double-click maximize.
    last_title_click: Option<Instant>,
    /// The cursor currently set on the window.
    cursor_icon: Option<CursorIcon>,
    /// Start view, menus, overlays and toasts.
    shell: Shell,
    /// The open document, absent on the start view before any board exists.
    document: Option<Document>,
    /// The next untitled document number.
    next_document: u32,
    /// The cached, dirty-tracked screen-space shell layer.
    ui_layer: UiLayer,
    /// Persisted settings.
    settings: Settings,
    /// The rebindable keyboard table built from the settings.
    shortcuts: Shortcuts,
    /// The recent-board list.
    recents: Recents,
    /// Provenance hints for assets, persisted in and restored from a board.
    provenance: BTreeMap<AssetId, AssetProvenance>,
    /// The document last read from disk, for unknown-field and link preservation.
    loaded_document: Option<rizref_format::SceneDocument>,
    /// What to do after the user confirms discarding unsaved changes.
    pending_action: Option<PendingAction>,
    /// The desired window modes (topmost, opacity, click-through, ghost).
    window_modes: WindowModes,
    /// The UI Automation adapter, present on Windows once the window exists.
    accessibility: Option<Accessibility>,
    /// Accessibility tree revision already published.
    published_accessibility: u64,
    /// Pending colour-picker readback, in logical cursor coordinates.
    pending_pick: Option<DVec2>,
    /// Latch for the polled ghost hotkey.
    ghost_hotkey_was_down: bool,
    /// Latch for the polled click-through hotkey.
    click_through_hotkey_was_down: bool,
    /// Launch milestones, reported once the first frame is presented.
    startup: StartupClock,
    /// Rolling frame times for the `F3` developer overlay.
    frame_stats: FrameStats,
    /// Whether the `F3` developer overlay replaces the readout.
    dev_overlay: bool,
    /// Items instanced by the last frame, for the developer overlay.
    last_drawn: usize,
    /// Exit after the first presented frame, printing the startup JSON.
    exit_after_first_frame: bool,
    /// Frames the scripted soak should run once a board is on screen.
    bench_frames: Option<u32>,
    /// The running scripted soak.
    bench: Option<FrameBench>,
    /// When the last board open started, until its first frame is presented.
    board_open_started: Option<Instant>,
    /// Open-to-first-frame time of the last opened board.
    last_board_load: Option<Duration>,
    /// The device being created on a background thread, until it arrives.
    gpu_init: Option<std::sync::mpsc::Receiver<Option<GpuDevice>>>,
    /// Whether the device thread was started (after the first CPU frame).
    gpu_started: bool,
    /// Changes since the last autosave.
    autosave: AutosaveTracker,
    /// When autosaves are due.
    autosave_policy: AutosavePolicy,
    /// Where recovery copies live.
    autosave_dir: PathBuf,
    /// Recovery copies found at launch, offered once the window is up.
    pending_recoveries: Vec<Recovery>,
    /// Whether the previous session ended without a clean exit.
    previous_session: PreviousSession,
}

/// One asset's place in its animation.
#[derive(Debug, Clone, Copy)]
struct Playback {
    /// Frame currently uploaded to the GPU.
    frame: usize,
    /// When the next frame becomes due.
    next_at: Instant,
}

/// A destructive action waiting on the unsaved-changes guard.
#[derive(Debug, Clone)]
enum PendingAction {
    /// Close the application.
    Quit,
    /// Replace the board with a new empty one.
    NewBoard,
    /// Replace the board with the file at this path.
    OpenBoard(PathBuf),
}

impl App {
    fn new(plan: WindowPlan, proxy: EventLoopProxy<AppEvent>, start_on_launch: bool) -> Self {
        // Every session starts with its own identity; a board saved from the
        // start view must not share a scene id with the next launch.
        let scene = Scene::new(SceneId::random());
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

        // The camera belongs to the board document; a session with no board
        // starts at the default view.
        let camera = Camera::default();
        let settings = Settings::load();
        let mut shortcuts = Shortcuts::new();
        shortcuts.apply_overrides(&settings.shortcuts);
        let mut board = Board::new(scene);
        board.set_snap_enabled(settings.snap_enabled);
        let window_modes = WindowModes::from_settings(&settings);
        let mut recents = Recents::load();
        let pruned = recents.retain_existing();
        if pruned > 0
            && let Err(error) = recents.save()
        {
            tracing::warn!(%error, "cannot save the recent-board list");
        }

        Self {
            drop_target: None,
            proxy,
            view: ViewState::new(camera, DVec2::new(plan.size.width, plan.size.height)),
            plan,
            window: None,
            renderer: None,
            board,
            decode_cache: DecodeCache::new(settings.max_decode_bytes),
            importer: Some(importer),
            pending_paths: Vec::new(),
            text: TextEngine::new(),
            hud: None,
            modifiers: ModifiersState::default(),
            playbacks: HashMap::new(),
            pending_textures: BTreeSet::new(),
            next_frame_at: None,
            note_session: None,
            note_renderer: NoteRenderer::new(),
            note_hover: None,
            pick_for_note: None,
            note_textures: HashMap::new(),
            dirty_notes: BTreeSet::new(),
            last_click: None,
            last_title_click: None,
            cursor_icon: None,
            shell: Shell::new(if start_on_launch {
                Mode::Start
            } else {
                Mode::Canvas
            }),
            ui_layer: UiLayer::new(),
            document: None,
            next_document: 1,
            settings,
            shortcuts,
            recents,
            provenance: BTreeMap::new(),
            loaded_document: None,
            pending_action: None,
            window_modes,
            accessibility: None,
            published_accessibility: 0,
            pending_pick: None,
            ghost_hotkey_was_down: false,
            click_through_hotkey_was_down: false,
            startup: StartupClock::new(perf::process_start()),
            frame_stats: FrameStats::default(),
            dev_overlay: false,
            last_drawn: 0,
            exit_after_first_frame: false,
            bench_frames: None,
            bench: None,
            board_open_started: None,
            last_board_load: None,
            gpu_init: None,
            gpu_started: false,
            autosave: AutosaveTracker::default(),
            autosave_policy: AutosavePolicy::default(),
            autosave_dir: autosave::autosave_dir(),
            pending_recoveries: Vec::new(),
            previous_session: PreviousSession::Clean,
        }
    }

    /// Whether closing needs a confirmation dialog.
    fn should_confirm_close(&self) -> bool {
        self.document.as_ref().is_some_and(Document::is_dirty)
    }

    /// Closes the window, asking first when the document has unsaved changes.
    ///
    /// This is the single close path for the title-bar button, Ctrl+Q, the menu
    /// and the native `CloseRequested` message, so no route can bypass the
    /// guard.
    fn request_close(&mut self, event_loop: &ActiveEventLoop) {
        self.commit_note_edit();
        if self.should_confirm_close() {
            let title = self.document.as_ref().map(Document::display_title);
            if let Some(title) = title {
                self.pending_action = Some(PendingAction::Quit);
                self.shell.title_bar.hide();
                self.shell.open_confirm_close(title);
            }
            self.redraw();
            return;
        }
        self.exit(event_loop);
    }

    /// Leaves the event loop immediately; callers must have cleared the close
    /// guard or confirmed that there is nothing to lose.
    fn exit(&mut self, event_loop: &ActiveEventLoop) {
        tracing::info!("closing the window");
        // Exit is reached only after a save or an explicit discard, so the
        // recovery copy has nothing left to protect.
        self.discard_autosave();
        event_loop.exit();
    }

    /// Deletes the current board's recovery copy and forgets pending changes.
    fn discard_autosave(&mut self) {
        autosave::discard(&self.autosave_dir, self.board.scene().id);
        self.autosave.reset();
    }

    /// Writes the recovery copy now, if the document has unsaved changes.
    ///
    /// The copy is an ordinary board in the autosave folder: the first write
    /// is a fresh save, every later one an incremental append, so a periodic
    /// autosave costs the changed chunks plus a new index. The user's own
    /// file is never touched.
    fn autosave_now(&mut self) {
        self.autosave.reset();
        let Some(document) = self.document.as_ref() else {
            return;
        };
        if !document.is_dirty() {
            return;
        }
        let started = Instant::now();
        let scene_id = self.board.scene().id;
        let path = autosave::board_path(&self.autosave_dir, scene_id);
        if let Err(error) = std::fs::create_dir_all(&self.autosave_dir) {
            tracing::warn!(%error, "cannot create the autosave folder");
            return;
        }
        let name = document.stem().to_owned();
        let info = autosave::RecoveryInfo {
            name: document.name().to_owned(),
            original: document.path().map(Path::to_path_buf),
            saved_unix: now_unix(),
            items: self.board.scene().items().len(),
        };
        let result = {
            let mut request = SaveRequest::new(self.board.scene(), self.view.camera, &name);
            request.provenance = &self.provenance;
            request.background = self.settings.canvas_theme.rgba();
            request.old_document = self.loaded_document.as_ref();
            rizref_format::save(&path, &request)
        };
        match result {
            Ok(report) => {
                if let Err(error) = autosave::write_sidecar(&self.autosave_dir, scene_id, &info) {
                    tracing::warn!(%error, "cannot write the autosave sidecar");
                }
                tracing::info!(
                    path = %path.display(),
                    mode = ?report.mode,
                    bytes = report.bytes_written,
                    ms = perf::ms(started.elapsed()),
                    "autosaved"
                );
            }
            Err(error) => tracing::warn!(path = %path.display(), %error, "autosave failed"),
        }
    }

    /// Offers the newest recovery copy found at launch, then reports an
    /// unclean previous exit that left nothing to recover.
    fn offer_recovery(&mut self) {
        let recoveries = std::mem::take(&mut self.pending_recoveries);
        if let Some(recovery) = recoveries.first() {
            let text = autosave::prompt_text(recovery, now_unix());
            if platform::confirm("RizRef recovered", &text) {
                self.restore_recovery(recovery);
            } else {
                tracing::info!(board = %recovery.board.display(), "recovery declined");
                let _ = std::fs::remove_file(&recovery.board);
                let _ = std::fs::remove_file(&recovery.sidecar);
            }
            return;
        }
        if self.previous_session == PreviousSession::Crashed {
            // Nothing unsaved survived (the board was clean or never edited),
            // so put the user back where they were: the last board, if any.
            let last = self
                .recents
                .entries
                .first()
                .map(|entry| PathBuf::from(&entry.path));
            let message = match last {
                Some(path) if self.pending_paths.is_empty() => {
                    self.open_board_now(&path);
                    "RizRef recovered from an unexpected exit; your last board was reopened"
                }
                _ => "RizRef recovered from an unexpected exit",
            };
            self.shell.toast(message, Instant::now());
            self.redraw();
        }
    }

    /// Loads a recovery copy as the open document: named after the board it
    /// belongs to, marked unsaved, and still autosaving to the same copy.
    fn restore_recovery(&mut self, recovery: &Recovery) {
        match rizref_format::load(&recovery.board) {
            Ok(loaded) => {
                let item_count = loaded.scene.items().len();
                let provenance: BTreeMap<AssetId, AssetProvenance> = loaded
                    .document
                    .assets
                    .iter()
                    .filter_map(|entry| {
                        AssetId::from_hex(&entry.hash).map(|id| {
                            (
                                id,
                                AssetProvenance {
                                    origin: entry.origin,
                                    source_path: entry.source_path.clone(),
                                    source_url: entry.source_url.clone(),
                                },
                            )
                        })
                    })
                    .collect();
                self.reset_board(loaded.scene);
                self.provenance = provenance;
                // Unknown-field preservation follows the user's own file when
                // it still exists; the copy's document is the fallback.
                self.loaded_document = recovery
                    .info
                    .original
                    .as_ref()
                    .and_then(|path| rizref_format::BoardReader::open_path(path).ok())
                    .and_then(|mut reader| reader.document().ok())
                    .or(Some(loaded.document));
                self.view.camera = loaded.camera;
                let mut document = match &recovery.info.original {
                    Some(path) => Document::from_path(path),
                    None => self.new_document(),
                };
                document.mark_dirty();
                self.document = Some(document);
                self.shell.set_mode(Mode::Canvas);
                self.refresh_window_title();
                self.upload_asset_previews();
                self.shell.toast(
                    format!("Restored unsaved work ({item_count} items) — save to keep it"),
                    Instant::now(),
                );
                tracing::info!(board = %recovery.board.display(), items = item_count, "recovery restored");
                self.redraw();
            }
            Err(error) => {
                tracing::warn!(board = %recovery.board.display(), %error, "cannot restore the recovery copy");
                self.shell.toast(
                    format!("Could not restore unsaved work: {error}"),
                    Instant::now(),
                );
                self.redraw();
            }
        }
    }

    /// Confirms the unsaved-changes guard and runs the pending action.
    fn confirm_discard(&mut self, event_loop: &ActiveEventLoop) {
        self.shell.confirm_close = None;
        self.shell.touch();
        match self.pending_action.take() {
            Some(PendingAction::NewBoard) => self.new_board_now(),
            Some(PendingAction::OpenBoard(path)) => {
                self.open_board_now(&path);
            }
            Some(PendingAction::Quit) | None => self.exit(event_loop),
        }
    }

    /// Asks before replacing a dirty board, then runs `action` on confirmation.
    fn request_destructive(&mut self, action: PendingAction, discard_label: &str, note: &str) {
        self.commit_note_edit();
        if !self.should_confirm_close() {
            match action {
                PendingAction::NewBoard => self.new_board_now(),
                PendingAction::OpenBoard(path) => {
                    self.open_board_now(&path);
                }
                PendingAction::Quit => {}
            }
            return;
        }
        if let Some(title) = self.document.as_ref().map(Document::display_title) {
            self.pending_action = Some(action);
            self.shell.title_bar.hide();
            self.shell.open_confirm(title, discard_label, note);
            self.redraw();
        }
    }

    /// Allocates the next untitled document name.
    fn new_document(&mut self) -> Document {
        let number = self.next_document;
        self.next_document = self.next_document.saturating_add(1);
        Document::untitled(number)
    }

    /// Drops the old board's GPU textures and resets per-board caches.
    fn reset_board(&mut self, scene: Scene) {
        // Replacing the board happens only after a save or an explicit
        // discard (the close guard), so its recovery copy goes with it.
        self.discard_autosave();
        let old_assets: Vec<AssetId> = self
            .board
            .scene()
            .assets()
            .iter()
            .map(|(id, _)| *id)
            .collect();
        if let Some(renderer) = self.renderer.as_mut() {
            for id in old_assets {
                renderer.remove_texture(id);
            }
        }
        self.board = Board::new(scene);
        self.board.set_snap_enabled(self.settings.snap_enabled);
        self.decode_cache = DecodeCache::new(self.settings.max_decode_bytes);
        self.playbacks.clear();
        self.pending_textures.clear();
        self.note_textures.clear();
        self.dirty_notes.clear();
        self.provenance.clear();
        self.loaded_document = None;
    }

    /// Replaces the board with a new empty one, without asking.
    fn new_board_now(&mut self) {
        self.reset_board(Scene::new(SceneId::random()));
        self.view.camera = Camera::default();
        self.shell.set_mode(Mode::Canvas);
        self.document = Some(self.new_document());
        self.refresh_window_title();
        let cursor = self.view.cursor_position();
        self.update_title_bar_pointer(cursor);
        self.shell.toast("New board", Instant::now());
        self.redraw();
    }

    /// Opens `path`, replacing the board. Errors become a toast, never a panic.
    fn open_board_now(&mut self, path: &Path) {
        let open_started = Instant::now();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("board")
            .to_owned();
        match rizref_format::load(path) {
            Ok(loaded) => {
                let rizref_format::LoadedBoard {
                    document,
                    scene,
                    camera,
                    name,
                    thumbnail,
                    missing,
                    warnings,
                    recovered,
                    ..
                } = loaded;
                for warning in &warnings {
                    tracing::warn!(path = %path.display(), "{warning}");
                }
                let item_count = scene.items().len();
                let provenance: BTreeMap<AssetId, AssetProvenance> = document
                    .assets
                    .iter()
                    .filter_map(|entry| {
                        AssetId::from_hex(&entry.hash).map(|id| {
                            (
                                id,
                                AssetProvenance {
                                    origin: entry.origin,
                                    source_path: entry.source_path.clone(),
                                    source_url: entry.source_url.clone(),
                                },
                            )
                        })
                    })
                    .collect();
                self.reset_board(scene);
                self.provenance = provenance;
                self.loaded_document = Some(document);
                self.view.camera = camera;
                self.document = Some(Document::from_path(path));
                self.shell.set_mode(Mode::Canvas);
                self.refresh_window_title();
                self.upload_asset_previews();
                self.record_recent(path, item_count, thumbnail.as_ref());
                let message = if recovered {
                    format!("Recovered {name} (interrupted save)")
                } else {
                    format!("Opened {name}")
                };
                let message = if missing.is_empty() {
                    message
                } else {
                    format!("{message} · {} linked image(s) missing", missing.len())
                };
                self.shell.toast(message, Instant::now());
                self.board_open_started = Some(open_started);
                tracing::info!(
                    path = %path.display(),
                    items = item_count,
                    read_ms = perf::ms(open_started.elapsed()),
                    missing = missing.len(),
                    recovered,
                    "board opened"
                );
                self.redraw();
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot open board");
                self.shell.toast(
                    format!("Could not open {file_name}: {error}"),
                    Instant::now(),
                );
                self.redraw();
            }
        }
    }

    /// Uploads every loaded asset's preview, so a board draws before any full
    /// decode starts.
    fn upload_asset_previews(&mut self) {
        let scene = self.board.scene();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for (id, asset) in scene.assets().iter() {
            if !renderer.has_texture(*id) {
                renderer.upload_texture(*id, asset.preview().image(), true);
            }
        }
    }

    /// Records a board in the recent list and persists the list.
    fn record_recent(
        &mut self,
        path: &Path,
        item_count: usize,
        thumbnail: Option<&rizref_format::Preview>,
    ) {
        self.recents.record(
            path.to_string_lossy().into_owned(),
            u32::try_from(item_count).unwrap_or(u32::MAX),
            thumbnail.map(|preview| preview.bytes.clone()),
            now_unix(),
        );
        if let Err(error) = self.recents.save() {
            tracing::warn!(%error, "cannot save the recent-board list");
        }
        launch::add_to_recent_documents(path);
    }

    /// Saves the board to its file, or asks for one first.
    fn save_document(&mut self, save_as: bool) {
        let now = Instant::now();
        if self.document.is_none() {
            self.document = Some(self.new_document());
        }
        let existing = !save_as && self.document.as_ref().is_some_and(|d| d.path().is_some());
        let path = if existing {
            self.document
                .as_ref()
                .and_then(Document::path)
                .expect("checked above")
                .to_path_buf()
        } else {
            let default = self
                .document
                .as_ref()
                .map(Document::name)
                .unwrap_or("Board.rizref")
                .to_owned();
            let Some(path) = dialogs::save_board(&default) else {
                return;
            };
            path
        };
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("Board")
            .to_owned();

        let result = {
            let mut request = SaveRequest::new(self.board.scene(), self.view.camera, &name);
            request.provenance = &self.provenance;
            request.background = self.settings.canvas_theme.rgba();
            request.save_as = !existing;
            request.old_document = self.loaded_document.as_ref();
            rizref_format::save(&path, &request)
        };

        match result {
            Ok(report) => {
                if let Some(document) = self.document.as_mut() {
                    document.set_path(&path);
                    document.mark_saved();
                }
                self.discard_autosave();
                self.refresh_window_title();
                let thumbnail = rizref_format::read_summary(&path)
                    .ok()
                    .and_then(|summary| summary.thumbnail);
                if let Ok(mut reader) = rizref_format::BoardReader::open_path(&path) {
                    self.loaded_document = reader.document().ok();
                }
                self.record_recent(&path, self.board.scene().items().len(), thumbnail.as_ref());
                let verb = match report.mode {
                    SaveMode::Fresh => "Saved",
                    SaveMode::Appended => "Saved (incremental)",
                    SaveMode::Compacted => "Saved (compacted)",
                };
                self.shell.toast(
                    format!(
                        "{verb} {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    now,
                );
                tracing::info!(
                    path = %path.display(),
                    mode = ?report.mode,
                    bytes = report.bytes_written,
                    file_len = report.file_len,
                    generation = report.generation,
                    "board saved"
                );
            }
            Err(error) if error.is_denied() && existing => {
                // Read-only file, read-only share, or a lock another program
                // holds: retrying cannot help, saving elsewhere can.
                tracing::warn!(path = %path.display(), %error, "save refused; offering Save As");
                self.shell.toast(
                    format!(
                        "{} is read-only or in use — choose where to save a copy",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    now,
                );
                self.save_document(true);
                return;
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot save board");
                self.shell.toast(format!("Could not save: {error}"), now);
            }
        }
        self.redraw();
    }

    /// Exports the board as a PNG image.
    fn export_board(&mut self) {
        let now = Instant::now();
        let default = format!(
            "{}.png",
            self.document
                .as_ref()
                .map(Document::stem)
                .unwrap_or("Board")
        );
        let Some(path) = dialogs::export_png(&default) else {
            return;
        };
        match rizref_format::thumbnail::export_png(
            &path,
            self.board.scene(),
            self.settings.canvas_theme.rgba(),
            2048,
        ) {
            Ok(()) => {
                self.shell.toast(
                    format!(
                        "Exported {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    now,
                );
                tracing::info!(path = %path.display(), "board exported");
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot export board");
                self.shell.toast(format!("Could not export: {error}"), now);
            }
        }
        self.redraw();
    }

    /// Ensures a document exists for content being added, and refreshes the
    /// title (the mode may have just switched from the start view).
    fn ensure_document(&mut self) {
        if self.document.is_none() {
            self.document = Some(self.new_document());
        }
        self.refresh_window_title();
    }

    /// Marks the open document dirty. Returns whether the asterisk changed.
    fn mark_document_dirty(&mut self) -> bool {
        if self.document.is_some() {
            self.autosave.note_change(Instant::now());
        }
        self.document.as_mut().is_some_and(Document::mark_dirty)
    }

    /// Mirrors the document title into the title bar and the native window.
    ///
    /// The start view stays blank: the title bar only names a document while
    /// its canvas is on screen.
    fn refresh_window_title(&mut self) {
        let title = if self.shell.mode() == Mode::Canvas {
            self.document.as_ref().map(Document::display_title)
        } else {
            None
        };
        if self.shell.title_bar.set_title(title.clone()) {
            self.shell.touch();
        }
        if let Some(window) = self.window.as_ref() {
            let native = title.unwrap_or_else(|| self.plan.title.to_owned());
            if window.title() != native {
                window.set_title(&native);
            }
        }
    }

    /// Asks for one more frame; the loop draws only when something changed.
    fn redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Whether the title bar may appear in the current window mode.
    fn title_bar_allowed(&self) -> bool {
        !self.window_modes.effective_click_through()
            && !self.window_modes.borderless
            && !self.shell.picker
    }

    /// Mirrors always-on-top and maximized into the title bar's icon state.
    fn sync_title_bar_state(&mut self) {
        let pinned = self.window_modes.always_on_top;
        let maximized = self
            .window
            .as_ref()
            .is_some_and(|window| window.is_maximized());
        if self.shell.title_bar.set_state(pinned, maximized) {
            self.shell.touch();
        }
    }

    /// Updates title-bar visibility and hover for the current pointer.
    fn update_title_bar_pointer(&mut self, cursor: Option<DVec2>) {
        let allowed = self.title_bar_allowed();
        let viewport = self.viewport();
        if self
            .shell
            .title_bar
            .pointer_moved(cursor, viewport, allowed)
        {
            self.shell.touch();
            self.redraw();
        }
    }

    /// Handles a left press on the title bar.
    fn title_bar_press(&mut self, window: &Window, position: DVec2, event_loop: &ActiveEventLoop) {
        let layout = titlebar::TitleBarLayout::new(self.viewport());
        if let Some(button) = layout.button_at(position) {
            self.last_title_click = None;
            match button {
                TitleBarButton::Menu => self.execute(Action::OpenMainMenu, event_loop),
                TitleBarButton::Pin => self.execute(Action::ToggleAlwaysOnTop, event_loop),
                TitleBarButton::Minimize => self.execute(Action::WindowMinimize, event_loop),
                TitleBarButton::Maximize => self.execute(Action::WindowToggleMaximize, event_loop),
                TitleBarButton::Close => self.execute(Action::Quit, event_loop),
            }
            return;
        }

        let now = Instant::now();
        let double = self
            .last_title_click
            .is_some_and(|last| now.duration_since(last) < DOUBLE_CLICK);
        self.last_title_click = Some(now);
        if double {
            self.last_title_click = None;
            self.execute(Action::WindowToggleMaximize, event_loop);
        } else if let Err(error) = window.drag_window() {
            tracing::warn!(%error, "cannot start a window drag");
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
        if self.mark_document_dirty() {
            self.refresh_window_title();
        }
        for id in &effects.content {
            self.dirty_notes.insert(*id);
        }
        if effects.structural() {
            let gone: Vec<AssetId> = self
                .note_textures
                .iter()
                .filter(|(id, _)| !self.board.scene().contains(**id))
                .map(|(_, entry)| entry.id)
                .collect();
            self.note_textures
                .retain(|id, _| self.board.scene().contains(*id));
            for texture in gone {
                self.release_note_texture(texture);
            }
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
            ResetTransform, ResetView, SelectAll, Undo, UngroupSelection, ZoomHundred,
        };

        if self.shell.locked && command_mutates_items(command) {
            self.shell
                .toast("Canvas is locked (L to unlock)", Instant::now());
            self.redraw();
            return;
        }

        if matches!(
            command,
            Command::BeginPan(_)
                | Command::EndPan
                | Command::ZoomNotch(_)
                | Command::RotateNotch(_)
                | FitContent
                | ZoomHundred
                | ResetRotation
                | ResetView
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
                let id = self.board.create_provisional_note(self.drop_world());
                self.after_board();
                self.begin_note_edit(id, true);
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

    /// The viewport size in logical pixels.
    fn viewport(&self) -> DVec2 {
        self.view.viewport()
    }

    /// Builds the command menu for the current state.
    fn menu_tree(&self) -> Vec<MenuEntry> {
        menu::build(&self.menu_context())
    }

    /// The state the menus are built from.
    fn menu_context(&self) -> MenuContext<'_> {
        MenuContext {
            settings: &self.settings,
            shortcuts: &self.shortcuts,
            recents: &self.recents,
            locked: self.shell.locked,
            ghost: self.window_modes.ghost(),
            click_through: self.window_modes.click_through,
            always_on_top: self.window_modes.always_on_top,
            opacity_percent: self.window_modes.opacity_percent,
            note_selected: self.board.selected_note().is_some(),
            selected_image: self
                .selected_image()
                .map(|(_, provenance)| provenance.and_then(location::location_kind)),
        }
    }

    /// The one selected image and its provenance record, if any.
    fn selected_image(&self) -> Option<(rizref_core::ItemId, Option<&AssetProvenance>)> {
        let item = self.board.selected_image()?;
        Some((item, self.image_provenance(item)))
    }

    /// The provenance of image `item`'s asset.
    fn image_provenance(&self, item: rizref_core::ItemId) -> Option<&AssetProvenance> {
        let asset = self.board.scene().item(item)?.asset()?;
        self.provenance.get(&asset)
    }

    /// Opens the information dialog for the one selected image.
    fn show_image_info(&mut self) {
        let Some((item, provenance)) = self.selected_image() else {
            return;
        };
        let scene = self.board.scene();
        let Some(info) = ImageInfo::from_scene(scene, item, provenance) else {
            return;
        };
        // The stored preview, never a fresh decode of the original.
        let preview = scene
            .item(item)
            .and_then(|entry| entry.asset())
            .and_then(|asset| scene.asset(asset))
            .map(|asset| asset.preview().image())
            .map(|image| (image.width(), image.height(), image.pixels()));
        let panel = ImageInfoPanel::new(info, preview);
        self.commit_note_edit();
        self.shell.open_image_info(panel);
        self.redraw();
    }

    /// Opens the location of the image in the info dialog, or of the one
    /// selected image. Never mutates the board, its dirty flag or history.
    fn open_image_location(&mut self) {
        let item = self
            .shell
            .image_info
            .as_ref()
            .map(|panel| panel.info.item)
            .or_else(|| self.board.selected_image());
        let Some(item) = item else {
            return;
        };
        let provenance = self.image_provenance(item).cloned().unwrap_or_default();
        let board_dir = self
            .document
            .as_ref()
            .and_then(Document::path)
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        let plan = location::plan(&provenance, board_dir.as_deref(), location::probe);
        let result = match &plan {
            LocationPlan::OpenUrl(url) => launch::open_url(url),
            LocationPlan::SelectFile(path) => launch::select_in_explorer(path),
            LocationPlan::OpenParent(path) => launch::open_folder(path),
            LocationPlan::Gone | LocationPlan::Nothing => Ok(()),
        };
        let now = Instant::now();
        match result {
            Ok(()) => self.shell.toast(plan.message(), now),
            Err(error) => {
                tracing::warn!(?error, ?plan, "cannot open the image location");
                let message = if matches!(plan, LocationPlan::OpenUrl(_)) {
                    "Could not open the image URL"
                } else {
                    "Could not open Explorer"
                };
                self.shell.toast(message, now);
            }
        }
        self.redraw();
    }

    /// Copies a value from the open info dialog to the clipboard.
    fn copy_image_info(&mut self, what: InfoButton) {
        let Some(info) = self.shell.image_info.as_ref().map(|panel| &panel.info) else {
            return;
        };
        let (text, done) = match what {
            InfoButton::CopyLocation => match &info.location {
                Some(location) if info.location_is_url => (location.clone(), "URL copied"),
                Some(location) => (location.clone(), "Path copied"),
                None => return,
            },
            InfoButton::CopyId => (info.asset_id.clone(), "Asset ID copied"),
            InfoButton::Open | InfoButton::Close => return,
        };
        let now = Instant::now();
        match clipboard::write_text(&text) {
            Ok(()) => self.shell.toast(done, now),
            Err(error) => {
                tracing::warn!(%error, "cannot copy from the image info dialog");
                self.shell.toast("Could not write the clipboard", now);
            }
        }
        self.redraw();
    }

    /// Carries out a choice made in the info dialog.
    fn apply_image_info_result(&mut self, result: InfoResult) {
        match result {
            InfoResult::None => {}
            InfoResult::Changed => {
                self.shell.touch();
                self.redraw();
            }
            InfoResult::Activate(InfoButton::Close) => {
                self.shell.close_image_info();
                self.redraw();
            }
            InfoResult::Activate(InfoButton::Open) => self.open_image_location(),
            InfoResult::Activate(button) => self.copy_image_info(button),
        }
    }

    /// Opens the right-click menu at a logical point: the note commands (or
    /// `New note`) first, then the full command tree.
    fn open_context_menu(&mut self, position: DVec2) {
        let tree = menu::build_context(&self.menu_context());
        self.shell.tooltip = None;
        self.shell.open_menu(tree, position, self.viewport());
        self.redraw();
    }

    /// Opens the full command menu under the title bar's ☰ button.
    fn open_main_menu(&mut self) {
        let tree = self.menu_tree();
        let anchor = DVec2::new(0.0, f64::from(titlebar::HEIGHT));
        self.shell.tooltip = None;
        self.shell.open_menu(tree, anchor, self.viewport());
        self.redraw();
    }

    /// Opens the small context menu for a start-view card.
    fn open_card_menu(&mut self, path: std::path::PathBuf, position: DVec2) {
        let tree = vec![
            MenuEntry::item("Open board…", Action::OpenRecent(path.clone())),
            MenuEntry::item(
                "Open containing folder",
                Action::OpenContainingFolder(path.clone()),
            ),
            MenuEntry::separator(),
            MenuEntry::item("Remove from recents", Action::RemoveRecent(path)),
        ];
        self.shell.open_menu(tree, position, self.viewport());
        self.redraw();
    }

    /// Saves settings and the shortcut overrides.
    fn save_settings(&mut self) {
        self.settings.shortcuts = self.shortcuts.overrides();
        if let Err(error) = self.settings.save() {
            tracing::warn!(%error, "cannot save settings");
            self.shell
                .toast("Could not write settings.json", Instant::now());
        }
    }

    /// Runs one shell or board action.
    fn execute(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        if self.shell.confirm_close.is_some() {
            match action {
                Action::ConfirmCloseCancel => {
                    self.shell.close_confirm_close();
                    self.redraw();
                }
                Action::ConfirmCloseDiscard => self.confirm_discard(event_loop),
                // The close guard is modal: nothing behind it runs.
                _ => {}
            }
            return;
        }
        if self.shell.image_info.is_some() {
            match action {
                Action::OpenImageLocation => self.open_image_location(),
                Action::CopyImageLocation => self.copy_image_info(InfoButton::CopyLocation),
                Action::CopyImageAssetId => self.copy_image_info(InfoButton::CopyId),
                Action::CloseImageInfo => {
                    self.shell.close_image_info();
                    self.redraw();
                }
                // The info dialog is modal: nothing behind it runs.
                _ => {}
            }
            return;
        }
        let now = Instant::now();
        match action {
            Action::Command(command) => self.apply(command),
            Action::ShowImageInfo => self.show_image_info(),
            Action::OpenImageLocation => self.open_image_location(),
            Action::CopyImageLocation | Action::CopyImageAssetId | Action::CloseImageInfo => {}
            Action::NewBoard => {
                self.request_destructive(
                    PendingAction::NewBoard,
                    "Discard changes",
                    "Creating a new board will discard them.",
                );
            }
            Action::OpenImages => {
                self.open_images();
                self.redraw();
            }
            Action::Paste => {
                self.paste();
                self.redraw();
            }
            Action::OpenBoard => {
                if let Some(path) = dialogs::pick_board() {
                    self.request_destructive(
                        PendingAction::OpenBoard(path),
                        "Discard changes",
                        "Opening another board will discard them.",
                    );
                }
            }
            Action::OpenRecent(path) => {
                self.request_destructive(
                    PendingAction::OpenBoard(path),
                    "Discard changes",
                    "Opening another board will discard them.",
                );
            }
            Action::RemoveRecent(path) => {
                let path = path.to_string_lossy().into_owned();
                if self.recents.remove(&path) {
                    if let Err(error) = self.recents.save() {
                        tracing::warn!(%error, "cannot save the recent-board list");
                    }
                    self.shell.toast("Removed from recents", now);
                    self.shell.touch();
                    self.redraw();
                }
            }
            Action::OpenContainingFolder(path) => {
                let argument = format!("/select,{}", path.display());
                match std::process::Command::new("explorer.exe")
                    .arg(argument)
                    .spawn()
                {
                    Ok(_) => self.shell.toast("Opened in Explorer", now),
                    Err(error) => {
                        tracing::warn!(%error, "cannot open Explorer");
                        self.shell.toast("Could not open Explorer", now);
                    }
                }
            }
            Action::Save => self.save_document(false),
            Action::SaveAs => self.save_document(true),
            Action::Export => self.export_board(),
            Action::ShowStart => {
                self.commit_note_edit();
                self.shell.set_mode(Mode::Start);
                self.refresh_window_title();
                self.redraw();
            }
            Action::ShowShortcuts => {
                self.commit_note_edit();
                self.shell.toggle_shortcuts();
                self.redraw();
            }
            Action::ShowSettings => {
                self.commit_note_edit();
                self.shell.toggle_settings();
                self.redraw();
            }
            Action::OpenLogsFolder => {
                let dir = logs_dir();
                let _ = std::fs::create_dir_all(&dir);
                if let Err(error) = launch::open_folder(&dir) {
                    tracing::warn!(dir = %dir.display(), ?error, "cannot open the logs folder");
                    self.shell
                        .toast(format!("Could not open {}", dir.display()), now);
                }
            }
            Action::About => {
                self.shell.toast(
                    format!(
                        "{} · pure-Rust reference board · math fonts: KaTeX (SIL OFL 1.1) · F1 for shortcuts",
                        version_line()
                    ),
                    now,
                );
            }
            Action::Quit => self.request_close(event_loop),
            Action::ToggleAlwaysOnTop => {
                self.window_modes.always_on_top = !self.window_modes.always_on_top;
                self.settings.always_on_top = self.window_modes.always_on_top;
                self.apply_window_modes();
                self.save_settings();
                self.sync_title_bar_state();
                self.shell.toast(
                    if self.window_modes.always_on_top {
                        "Always on top"
                    } else {
                        "No longer always on top"
                    },
                    now,
                );
                self.redraw();
            }
            Action::WindowMinimize => {
                if let Some(window) = self.window.as_ref() {
                    window.set_minimized(true);
                }
                self.shell.title_bar.hide();
                self.redraw();
            }
            Action::WindowToggleMaximize => {
                if let Some(window) = self.window.as_ref() {
                    window.set_maximized(!window.is_maximized());
                }
                self.sync_title_bar_state();
                self.redraw();
            }
            Action::SetOpacity(percent) => {
                self.window_modes.set_opacity(percent);
                self.settings.opacity_percent = self.window_modes.opacity_percent;
                self.apply_window_modes();
                self.save_settings();
                self.shell.toast(
                    format!("Window opacity {}%", self.window_modes.opacity_percent),
                    now,
                );
                self.redraw();
            }
            Action::CycleOpacity => {
                self.window_modes.cycle_opacity();
                self.settings.opacity_percent = self.window_modes.opacity_percent;
                self.apply_window_modes();
                self.save_settings();
                self.shell.toast(
                    format!("Window opacity {}%", self.window_modes.opacity_percent),
                    now,
                );
                self.redraw();
            }
            Action::ToggleClickThrough => {
                self.window_modes.toggle_click_through();
                self.click_through_hotkey_was_down = true;
                self.apply_window_modes();
                self.shell.title_bar.hide();
                let message = if self.window_modes.effective_click_through() {
                    "Click-through armed · Ctrl+Shift+X restores input"
                } else {
                    "Click-through off"
                };
                self.shell.toast(message, now);
                self.redraw();
            }
            Action::ToggleGhostMode => {
                self.window_modes.toggle_ghost();
                self.ghost_hotkey_was_down = true;
                self.apply_window_modes();
                self.shell.title_bar.hide();
                let message = if self.window_modes.ghost() {
                    "Ghost mode · Ctrl+Shift+G restores the window"
                } else {
                    "Ghost mode off"
                };
                self.shell.toast(message, now);
                self.redraw();
            }
            Action::ToggleBorderless => {
                self.window_modes.borderless = !self.window_modes.borderless;
                if let Some(window) = self.window.as_ref() {
                    platform::window_modes::set_borderless(window, self.window_modes.borderless);
                }
                self.shell.title_bar.hide();
                self.shell.toast(
                    if self.window_modes.borderless {
                        "Borderless fullscreen"
                    } else {
                        "Windowed"
                    },
                    now,
                );
                self.redraw();
            }
            Action::ToggleCanvasLock => {
                self.shell.locked = !self.shell.locked;
                let message = if self.shell.locked {
                    "Canvas locked · navigation still works"
                } else {
                    "Canvas unlocked"
                };
                self.shell.toast(message, now);
                self.shell.touch();
                self.redraw();
            }
            Action::ToggleGrid => self.toggle_bool_setting(SettingId::Grid),
            Action::ToggleSnap => self.toggle_bool_setting(SettingId::Snap),
            Action::ToggleHud => self.toggle_bool_setting(SettingId::Hud),
            Action::ToggleBorder => self.toggle_bool_setting(SettingId::Border),
            Action::ToggleCheckerboard => self.toggle_bool_setting(SettingId::Checkerboard),
            Action::ToggleGrayscale => self.toggle_bool_setting(SettingId::Grayscale),
            Action::SetTheme(theme) => {
                self.settings.canvas_theme = theme;
                self.save_settings();
                self.shell.touch();
                self.redraw();
            }
            Action::OpenMainMenu => {
                if self.shell.mode() == Mode::Canvas {
                    self.commit_note_edit();
                }
                self.open_main_menu();
            }
            Action::EditNote => {
                if let Some(id) = self.board.selected_note() {
                    self.begin_note_edit(id, false);
                }
            }
            Action::NoteBackground => {
                self.commit_note_edit();
                if let Some(id) = self.board.selected_note() {
                    let anchor = self.note_color_anchor(id);
                    self.open_note_color(id, anchor);
                }
            }
            Action::ReflowNote => {
                self.commit_note_edit();
                if let Some(id) = self.board.selected_note()
                    && self.board.reflow_note(id)
                {
                    self.dirty_notes.insert(id);
                    self.after_board();
                }
                self.redraw();
            }
            Action::FitNote => {
                self.commit_note_edit();
                self.fit_selected_note();
                self.redraw();
            }
            Action::DeleteNote => {
                self.cancel_provisional_or_commit();
                if self.board.selected_note().is_some() && self.board.delete_selection() {
                    self.note_hover = None;
                    self.shell.tooltip = None;
                    self.after_board();
                }
                self.redraw();
            }
            Action::ConfirmNoteEdit => self.commit_note_edit(),
            Action::CancelNoteEdit => self.cancel_note_edit(),
            Action::ColorPicker => {
                self.pick_for_note = None;
                self.shell.picker = true;
                self.shell.title_bar.hide();
                self.shell
                    .toast("Click a pixel to copy its hex colour", now);
                self.shell.touch();
                self.redraw();
            }
            // Handled by the modal guard above; unreachable with no dialog open.
            Action::ConfirmCloseCancel | Action::ConfirmCloseDiscard => {}
        }
        self.shell.touch();
    }

    /// Applies the desired window modes to the real window.
    fn apply_window_modes(&mut self) {
        if let Some(window) = self.window.as_ref() {
            self.window_modes.apply(window);
        }
    }

    /// Runs a rebindable shortcut.
    fn execute_shortcut(&mut self, id: ShortcutId, event_loop: &ActiveEventLoop) {
        if self.shell.mode() == Mode::Start && id.command().is_some() {
            return;
        }
        self.command_cancels_note_edit(id);
        self.execute(Action::from_shortcut(id), event_loop);
    }

    /// Ends a note edit before a shortcut runs (so typed text becomes history).
    fn command_cancels_note_edit(&mut self, id: ShortcutId) {
        if !matches!(
            id,
            ShortcutId::ShowShortcuts | ShortcutId::ShowSettings | ShortcutId::ShowStart
        ) {
            self.commit_note_edit();
        }
    }

    /// Handles a result from the F1 overlay.
    fn apply_shortcuts_panel_result(&mut self, result: PanelResult) {
        match result {
            PanelResult::None => {}
            PanelResult::Changed => {
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Close => {
                self.shell.shortcuts = None;
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Bind(id, chord) => {
                match self.shortcuts.bind(id, chord) {
                    Ok(()) => {
                        self.save_settings();
                        if let Some(panel) = self.shell.shortcuts.as_mut() {
                            panel.error = None;
                        }
                    }
                    Err(conflict) => {
                        if let Some(panel) = self.shell.shortcuts.as_mut() {
                            panel.error = Some(format!(
                                "{} is already used by {}",
                                chord,
                                conflict.action.label()
                            ));
                        }
                    }
                }
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Reset(id) => {
                self.shortcuts.reset(id);
                self.save_settings();
                if let Some(panel) = self.shell.shortcuts.as_mut() {
                    panel.error = None;
                }
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Adjust(_, _) => {}
        }
    }

    /// Handles a result from the settings dialog.
    fn apply_settings_panel_result(&mut self, result: PanelResult) {
        match result {
            PanelResult::None => {}
            PanelResult::Changed => {
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Close => {
                self.shell.settings = None;
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Adjust(id, direction) => {
                self.adjust_setting(id, direction);
                self.save_settings();
                self.shell.touch();
                self.redraw();
            }
            PanelResult::Bind(_, _) | PanelResult::Reset(_) => {}
        }
    }

    /// Toggles one boolean setting.
    fn toggle_bool_setting(&mut self, id: SettingId) {
        let now = Instant::now();
        match id {
            SettingId::Grid => self.settings.grid_visible = !self.settings.grid_visible,
            SettingId::Snap => {
                self.settings.snap_enabled = !self.settings.snap_enabled;
                self.board.set_snap_enabled(self.settings.snap_enabled);
            }
            SettingId::Hud => self.settings.hud_visible = !self.settings.hud_visible,
            SettingId::Border => self.settings.scene_border = !self.settings.scene_border,
            SettingId::Checkerboard => {
                self.settings.checkerboard = !self.settings.checkerboard;
            }
            SettingId::Grayscale => self.settings.grayscale = !self.settings.grayscale,
            SettingId::AlwaysOnTop => {
                self.settings.always_on_top = !self.settings.always_on_top;
                self.window_modes.always_on_top = self.settings.always_on_top;
                self.apply_window_modes();
                self.sync_title_bar_state();
            }
            SettingId::Opacity | SettingId::GhostOpacity | SettingId::DecodeCap => {}
        }
        self.save_settings();
        self.shell.touch();
        self.shell
            .toast(format!("{}: {}", id.label(), id.value(&self.settings)), now);
        self.redraw();
    }

    /// Adjusts one setting row from the settings dialog.
    fn adjust_setting(&mut self, id: SettingId, direction: i32) {
        match id {
            SettingId::Opacity => {
                let percent = panels::cycle_percent(
                    self.settings.opacity_percent,
                    direction,
                    crate::platform::window_modes::MIN_OPACITY_PERCENT,
                    100,
                    10,
                );
                self.settings.opacity_percent = percent;
                self.window_modes.set_opacity(percent);
                self.apply_window_modes();
            }
            SettingId::GhostOpacity => {
                let percent = panels::cycle_percent(
                    self.settings.ghost_opacity_percent,
                    direction,
                    10,
                    40,
                    5,
                );
                self.settings.ghost_opacity_percent = percent;
                self.window_modes.ghost_opacity_percent = percent;
                self.apply_window_modes();
            }
            SettingId::DecodeCap => {
                let cap = panels::cycle_decode_cap(self.settings.max_decode_bytes, direction);
                self.settings.max_decode_bytes = cap;
                self.decode_cache.set_max_bytes(cap);
            }
            other => self.toggle_bool_setting(other),
        }
    }

    /// Copies a picked pixel to the clipboard and reports it.
    fn finish_pick(&mut self, [r, g, b, a]: [u8; 4]) {
        let hex = format!("#{r:02X}{g:02X}{b:02X}");
        self.shell.picker = false;
        self.pending_pick = None;
        if let Some(id) = self.pick_for_note.take() {
            self.apply_note_background(id, [r, g, b]);
            self.shell
                .toast(format!("Note background set to {hex}"), Instant::now());
            self.shell.touch();
            self.redraw();
            return;
        }
        match clipboard::write_text(&hex) {
            Ok(()) => self
                .shell
                .toast(format!("{hex} copied to the clipboard"), Instant::now()),
            Err(error) => {
                tracing::warn!(%error, "cannot copy the picked colour");
                self.shell
                    .toast(format!("{hex} (clipboard unavailable)"), Instant::now());
            }
        }
        tracing::info!(r, g, b, a, %hex, "colour picked");
        self.shell.touch();
        self.redraw();
    }

    /// Publishes the current accessibility tree to the adapter.
    fn refresh_accessibility(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let tree = self.menu_tree();
        let notes = self.note_controls();
        let status = if self.shell.mode() == Mode::Start {
            "Start view: drop images or open a recent board".to_owned()
        } else {
            self.readout_text()
        };
        if let Some(accessibility) = self.accessibility.as_mut() {
            accessibility.update(
                &tree,
                &status,
                &window,
                &self.shell.title_bar,
                self.shell.confirm_close.as_ref(),
                self.shell.image_info.as_ref(),
                &notes,
            );
        }
        self.published_accessibility = self.shell.revision();
    }

    /// Handles a start-view click. Returns whether it changed anything.
    fn start_click(&mut self, position: DVec2, event_loop: &ActiveEventLoop) -> bool {
        let layout = StartLayout::new(self.viewport(), self.recents.entries.len());
        let Some(control) = layout.hit(position) else {
            return false;
        };
        if let Some(action) = start::action_for(control, &self.recents) {
            self.execute(action, event_loop);
            return true;
        }
        false
    }

    /// Handles a start-view right click on a card.
    fn start_right_click(&mut self, position: DVec2) -> bool {
        let layout = StartLayout::new(self.viewport(), self.recents.entries.len());
        let Some(index) = layout.card_at(position) else {
            return false;
        };
        let Some(entry) = self.recents.entries.get(index) else {
            return false;
        };
        let path = entry.path_buf();
        self.open_card_menu(path, position);
        true
    }

    /// Handles one menu result.
    fn apply_menu_result(&mut self, result: MenuResult, event_loop: &ActiveEventLoop) {
        match result {
            MenuResult::None => {}
            MenuResult::Changed => {
                self.shell.touch();
                self.redraw();
            }
            MenuResult::Closed => {
                self.shell.close_menu();
                self.redraw();
            }
            MenuResult::Action(action) => {
                self.shell.close_menu();
                self.execute(action, event_loop);
                self.redraw();
            }
        }
    }

    /// The HUD readout line: zoom, rotation, cursor, selection size.
    fn readout_text(&self) -> String {
        if self.dev_overlay {
            let (textures, texture_bytes) = self
                .renderer
                .as_ref()
                .map_or((0, 0), |r| (r.texture_count(), r.texture_bytes()));
            return perf::dev_overlay_line(
                &self.frame_stats,
                self.last_drawn,
                self.board.scene().items().len(),
                textures,
                texture_bytes,
                self.pending_textures.len(),
            );
        }
        let zoom = self.view.camera.zoom * 100.0;
        let zoom = if (zoom - zoom.round()).abs() < 0.05 {
            format!("{}", zoom.round() as i64)
        } else {
            format!("{zoom:.1}")
        };
        let rotation = self.view.camera.rotation.to_degrees();
        let mut notes = Vec::new();
        if self.shell.locked {
            notes.push("LOCKED".to_owned());
        }
        if self.window_modes.ghost() {
            notes.push("GHOST".to_owned());
        } else if self.window_modes.effective_click_through() {
            notes.push("CLICK-THROUGH".to_owned());
        }
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
        let submitted = self
            .importer
            .as_ref()
            .is_some_and(|importer| importer.submit(request));
        if submitted {
            // Content is arriving, so a document must exist even when the
            // start view was the entry point.
            self.ensure_document();
            tracing::info!(%label, ?drop_world, "import queued");
        } else {
            tracing::error!(%label, "the import pool is not running");
        }
    }

    /// Queues every path in `paths` as an image import around `drop_world`.
    fn import_paths(&mut self, paths: Vec<PathBuf>, drop_world: Vec2) {
        if paths.iter().any(|path| looks_like_image(path)) && self.shell.mode() == Mode::Start {
            self.shell.set_mode(Mode::Canvas);
        }
        for path in paths {
            if looks_like_image(&path) {
                self.submit_import(ImportRequest::path(path, drop_world));
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rizref"))
            {
                tracing::info!(path = %path.display(), "board file requested");
                self.request_destructive(
                    PendingAction::OpenBoard(path),
                    "Discard changes",
                    "Opening another board will discard them.",
                );
            } else {
                tracing::info!(path = %path.display(), "not an image; ignored");
            }
        }
    }

    /// Ctrl+V: files first, then a raw bitmap from the clipboard.
    fn paste(&mut self) {
        let drop_world = self.drop_world();
        match clipboard::read() {
            Ok(ClipboardContent::Files(paths)) => self.import_paths(paths, drop_world),
            Ok(ClipboardContent::Image {
                name,
                png,
                source_url,
            }) => {
                self.shell.set_mode(Mode::Canvas);
                let source = ImportSource::bytes(name, png).with_source_url(source_url);
                self.submit_import(ImportRequest::new(source, drop_world));
            }
            Err(clipboard::PasteError::LinkOnly) => {
                tracing::info!("the clipboard holds HTML without pixels; nothing downloaded");
                self.shell
                    .toast("The clipboard holds a link, not an image", Instant::now());
            }
            Err(reason) => {
                tracing::warn!(%reason, "nothing pasteable on the clipboard");
                self.shell
                    .toast("Nothing pasteable on the clipboard", Instant::now());
            }
        }
    }

    /// Ctrl+O: the native multi-select image dialog.
    fn open_images(&mut self) {
        if let Some(paths) = crate::dialogs::pick_images() {
            let drop_world = self.drop_world();
            // `import_paths` switches to the canvas only for actual images, so
            // cancelling or picking a non-image never strands the start view.
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
        // The drop target attaches a URL only to a single-image drop. A
        // browser drag that also wrote a cache file arrives as one path plus
        // the URL; the importer then records the URL, not the cache path.
        let source_url = event.source_url;
        if let (Some(url), [path]) = (&source_url, event.paths.as_slice()) {
            if looks_like_image(path) {
                tracing::info!(path = %path.display(), url = %url, "browser drag with a source URL");
                self.shell.set_mode(Mode::Canvas);
                let source = ImportSource::path(path.clone()).with_source_url(Some(url.clone()));
                self.submit_import(ImportRequest::new(source, drop_world));
            } else {
                self.import_paths(event.paths, drop_world);
            }
        } else if !event.paths.is_empty() {
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
            self.shell.set_mode(Mode::Canvas);
            let source =
                ImportSource::bytes(file.name, file.bytes).with_source_url(source_url.clone());
            self.submit_import(ImportRequest::new(source, drop_world));
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
        if let ImportPurpose::RefreshTexture(asset_id) = outcome.purpose {
            match outcome.result {
                Ok(imported) => {
                    self.pending_textures.remove(&asset_id);
                    self.apply_texture_refresh(asset_id, imported);
                }
                // Keep the request recorded: a corrupt original would
                // otherwise be decoded again on every visible frame.
                Err(error) => tracing::debug!(
                    asset = %asset_id,
                    reason = %error.reason,
                    "full-resolution decode failed; keeping the preview"
                ),
            }
            return;
        }

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
        let record = provenance::record_for(&provenance::ImportProvenance {
            source_path: imported
                .source_path
                .as_ref()
                .map(|path| path.display().to_string()),
            source_url: imported.source_url.clone(),
        });
        let (asset_id, was_new) = self.board.scene_mut().insert_asset(asset);
        let merged = provenance::merge(self.provenance.get(&asset_id), record);
        self.provenance.insert(asset_id, merged);
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
        if self.mark_document_dirty() {
            self.refresh_window_title();
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

    /// Uploads a refreshed full-resolution texture for an existing asset.
    fn apply_texture_refresh(
        &mut self,
        asset_id: AssetId,
        mut imported: crate::import::ImportedImage,
    ) {
        if self.board.scene().asset(asset_id).is_none() {
            return;
        }
        // A deferred decode is also where an animated GIF's frames arrive: the
        // board file stores only the original bytes, so the animation is
        // rebuilt here and attached to the existing content-addressed asset.
        if let Some(animation) = imported.animation.take()
            && let Some(preview) = self
                .board
                .scene()
                .asset(asset_id)
                .map(|asset| asset.preview().image().clone())
        {
            let replacement = Asset::from_parts(
                Arc::clone(&imported.encoded),
                imported.format,
                imported.checksum,
                &imported.image,
                preview,
            )
            .with_animation(animation);
            self.board.scene_mut().replace_asset(replacement);
        }
        let pixels = Arc::new(imported.image);
        let full_bytes = (pixels.width() as u64) * (pixels.height() as u64) * 4;
        let display = imported.display;
        for evicted in self.decode_cache.insert(asset_id, Arc::clone(&pixels)) {
            tracing::debug!(asset = %evicted, "decoded bitmap evicted from the memory cache");
        }
        if let Some(renderer) = self.renderer.as_mut() {
            let upload = display.as_ref().unwrap_or(&pixels);
            renderer.upload_texture(asset_id, upload, false);
        }
        tracing::debug!(
            asset = %asset_id,
            decoded_bytes = full_bytes,
            cache_bytes = self.decode_cache.bytes(),
            "full-resolution texture refreshed"
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
        let notes: Vec<(ItemId, rizref_core::NoteContent, Vec2)> = self
            .board
            .scene()
            .items()
            .iter()
            .filter_map(|item| {
                item.note_content()
                    .map(|content| (item.id(), content, item.size()))
            })
            .collect();

        for (id, content, size) in notes {
            let density = match self.note_session.as_ref().filter(|s| s.item() == id) {
                Some(session) => session.pixels_per_world(),
                None => note::pixels_per_world(size),
            };
            let wanted = note::texture_id(&content, size, density);
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
            let session = match session {
                Some(session) => {
                    // A resize changes the visible area, never the wrap width.
                    session.resize(size);
                    Some(session)
                }
                None => None,
            };
            let Some(image) = self
                .note_renderer
                .render(&mut self.text, &content, size, session)
            else {
                continue;
            };
            self.dirty_notes.remove(&id);
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.upload_texture(wanted, &image, false);
            }
            let previous = self.note_textures.insert(
                id,
                NoteTexture {
                    id: wanted,
                    world_size: size,
                },
            );
            // One resident texture per note: the superseded one is freed as
            // soon as no other note shares it, so typing or resizing never
            // accumulates GPU memory.
            if let Some(previous) = previous {
                self.release_note_texture(previous.id);
            }
        }
    }

    /// Frees a note texture once no note references it any more.
    fn release_note_texture(&mut self, texture: AssetId) {
        let in_use = self.note_textures.values().any(|entry| entry.id == texture);
        if !in_use && let Some(renderer) = self.renderer.as_mut() {
            renderer.remove_texture(texture);
        }
    }

    /// Builds this frame's drawable items in scene z-order.
    ///
    /// Only the visible set is instanced (phase 07): items whose world bounds
    /// miss the viewport are culled here, so neither their quads nor their
    /// textures cost anything while they are off screen.
    fn build_items(&self) -> Vec<ItemQuad> {
        let visible = self.view.camera.visible_world_rect(self.view.viewport());
        self.board
            .scene()
            .visible_items(&visible)
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
    /// upgrades a preview when the cache still has the full bitmap, and asks
    /// the worker pool to decode assets that have never been seen at full size.
    fn ensure_textures(&mut self, items: &[ItemQuad]) {
        let mut wanted = Vec::new();
        {
            let Some(renderer) = self.renderer.as_mut() else {
                return;
            };
            for item in items {
                renderer.touch_texture(item.asset);
                if !renderer.has_texture(item.asset) {
                    if let Some(asset) = self.board.scene().asset(item.asset) {
                        renderer.upload_texture(item.asset, asset.preview().image(), true);
                    }
                } else if renderer.texture_is_preview(item.asset) {
                    if let Some(full) = self.decode_cache.get(item.asset) {
                        renderer.upload_texture(item.asset, &full, false);
                    } else if !self.pending_textures.contains(&item.asset) {
                        wanted.push(item.asset);
                    }
                }
            }
        }
        for asset_id in wanted {
            let Some(asset) = self.board.scene().asset(asset_id) else {
                continue;
            };
            let request = ImportRequest::texture(asset_id, Arc::clone(asset.encoded()));
            if self
                .importer
                .as_ref()
                .is_some_and(|pool| pool.submit(request))
            {
                self.pending_textures.insert(asset_id);
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
            rotatable: self.board.selection_rotatable(),
            chrome: self.note_chrome(),
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

    /// Starts editing `id` and selects it. `provisional` marks a note created
    /// for this edit that is not in the history yet.
    fn begin_note_edit(&mut self, id: ItemId, provisional: bool) {
        if self.note_session.as_ref().is_some_and(|s| s.item() != id) {
            self.commit_note_edit();
        }
        if self.note_session.is_some() {
            return;
        }
        let was_clean = self
            .document
            .as_ref()
            .is_some_and(|document| !document.is_dirty());
        let Some(item) = self.board.scene().item(id) else {
            return;
        };
        self.note_session = NoteSession::begin(&mut self.text, item, provisional, was_clean);
        if self.note_session.is_some() {
            // The space bar types a space while editing; make sure a pan
            // armed before the double-click does not stay held.
            self.view.set_space_held(false);
            self.board.select_only(id);
            self.note_hover = None;
            self.shell.note_color = None;
            self.shell.tooltip = None;
            self.dirty_notes.insert(id);
            self.shell.touch();
            self.redraw();
        }
    }

    /// Ends the edit session, committing it as one undo step. A new note
    /// that is still empty is discarded and adds nothing.
    fn commit_note_edit(&mut self) {
        let Some(session) = self.note_session.take() else {
            return;
        };
        let id = session.item();
        let after = session.content();
        if session.is_provisional() {
            let added = self.board.commit_new_note(id, &after);
            self.after_board();
            if !added && session.was_clean() {
                self.restore_clean_title();
            }
        } else if after.text == session.original().text {
            // Nothing was typed (or it was typed and undone): a legacy plain
            // note stays plain, and the board keeps its saved state.
            self.board.restore_note(id, session.original());
            self.after_board();
            if session.was_clean() {
                self.restore_clean_title();
            }
        } else {
            self.board.commit_note(id, session.original(), &after);
            self.after_board();
        }
        self.dirty_notes.insert(id);
        self.shell.touch();
        self.redraw();
    }

    /// Ends the edit session without keeping anything: a new note is removed,
    /// an existing one gets its exact pre-edit content back, and a board that
    /// was saved before the edit is "not dirty" again.
    fn cancel_note_edit(&mut self) {
        let Some(session) = self.note_session.take() else {
            return;
        };
        let id = session.item();
        if session.is_provisional() {
            self.board.discard_provisional_note(id);
        } else {
            self.board.restore_note(id, session.original());
            self.dirty_notes.insert(id);
        }
        self.after_board();
        if session.was_clean() {
            self.restore_clean_title();
        }
        self.shell.touch();
        self.redraw();
    }

    /// Before deleting: a provisional note is simply discarded; any other
    /// edit is committed so the delete undoes back to the edited text.
    fn cancel_provisional_or_commit(&mut self) {
        if self
            .note_session
            .as_ref()
            .is_some_and(NoteSession::is_provisional)
        {
            self.cancel_note_edit();
        } else {
            self.commit_note_edit();
        }
    }

    /// Marks the document saved again after an edit that left no change.
    fn restore_clean_title(&mut self) {
        if self.document.as_mut().is_some_and(Document::mark_saved) {
            self.refresh_window_title();
        }
    }

    /// Copies the editor's text into the scene so the live preview updates.
    fn sync_note_text(&mut self) {
        let Some(session) = self.note_session.as_ref() else {
            return;
        };
        let id = session.item();
        let content = session.live_content();
        self.board.set_note_live(id, &content);
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
                self.cancel_note_edit();
                true
            }
            Key::Named(NamedKey::Enter) if ctrl => {
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
            Key::Named(NamedKey::Tab) => {
                self.note_insert("    ");
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
            Key::Character(_) | Key::Named(NamedKey::Space) => {
                if let Some(text) = &event.text {
                    self.note_insert(text);
                }
                true
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Note chrome: hover toolbar, edit pill, background popover
    // -----------------------------------------------------------------------

    /// The note's on-screen bounding box, in logical pixels.
    fn note_screen_rect(&self, id: ItemId) -> Option<ScreenRect> {
        let item = self.board.scene().item(id)?;
        let viewport = self.view.viewport();
        let corners = item.transform().corners().map(|corner| {
            self.view
                .camera
                .world_to_screen(corner.as_dvec2(), viewport)
                .as_vec2()
        });
        Some(ScreenRect::bounding(&corners))
    }

    /// The note whose hover toolbar should show, if any.
    fn toolbar_note(&self) -> Option<ItemId> {
        if self.shell.mode() != Mode::Canvas || self.shell.locked || self.note_session.is_some() {
            return None;
        }
        // While its popover is open the toolbar stays pinned to that note.
        if let Some(popover) = &self.shell.note_color {
            return Some(popover.note);
        }
        if self.shell.has_overlay() || self.shell.picker || self.board.is_gesture_active() {
            return None;
        }
        let id = self.note_hover?;
        let selection = self.board.selection();
        if selection.len() > 1 || (selection.len() == 1 && !selection.contains(id)) {
            return None;
        }
        self.board.scene().item(id).filter(|item| item.is_note())?;
        Some(id)
    }

    /// What the overlay draws for notes this frame.
    fn note_chrome(&self) -> Option<NoteChrome> {
        let cursor = self.view.cursor_position().map(|c| c.as_vec2());
        if let Some(session) = &self.note_session {
            if self.shell.mode() != Mode::Canvas {
                return None;
            }
            let layout = PillLayout::new(self.note_screen_rect(session.item())?);
            let hovered = cursor.and_then(|c| layout.hit(c));
            return Some(NoteChrome::Pill { layout, hovered });
        }
        let id = self.toolbar_note()?;
        let layout = ToolbarLayout::new(self.note_screen_rect(id)?);
        let background = self
            .board
            .scene()
            .item(id)
            .and_then(Item::note_content)?
            .background_or_default();
        let hovered = cursor.and_then(|c| layout.hit(c));
        Some(NoteChrome::Toolbar {
            layout,
            background,
            hovered,
        })
    }

    /// The note chrome for the accessibility tree: the edit pill while
    /// editing, otherwise the toolbar actions for a single selected or
    /// hovered note.
    fn note_controls(&self) -> crate::accessibility::NoteControls {
        if let Some(session) = &self.note_session {
            return crate::accessibility::NoteControls {
                buttons: vec![
                    (PillButton::Cancel.label(), Action::CancelNoteEdit),
                    (PillButton::Confirm.label(), Action::ConfirmNoteEdit),
                ],
                source: Some(session.text()),
            };
        }
        if self.shell.mode() == Mode::Canvas && self.board.selected_note().is_some() {
            return crate::accessibility::NoteControls {
                buttons: vec![
                    (ToolbarButton::Background.label(), Action::NoteBackground),
                    (ToolbarButton::Edit.label(), Action::EditNote),
                    (ToolbarButton::Delete.label(), Action::DeleteNote),
                ],
                source: None,
            };
        }
        crate::accessibility::NoteControls::default()
    }

    /// Whether `point` is on a chrome button.
    fn chrome_button_at(&self, point: DVec2) -> bool {
        let point = point.as_vec2();
        match self.note_chrome() {
            Some(NoteChrome::Pill { layout, .. }) => layout.hit(point).is_some(),
            Some(NoteChrome::Toolbar { layout, .. }) => layout.hit(point).is_some(),
            None => false,
        }
    }

    /// Tracks which note the toolbar belongs to and its tooltip. Returns
    /// whether anything visible changed.
    fn update_note_hover(&mut self, logical: DVec2) -> bool {
        let point = logical.as_vec2();
        let world = self
            .view
            .camera
            .screen_to_world(logical, self.view.viewport())
            .as_vec2();
        let under = self
            .board
            .item_at(world)
            .filter(|id| self.board.scene().item(*id).is_some_and(Item::is_note));
        // Moving from the note onto its toolbar keeps the toolbar.
        let on_toolbar = self.note_hover.filter(|id| {
            self.note_screen_rect(*id)
                .is_some_and(|rect| ToolbarLayout::new(rect).contains(point))
        });
        let hover = on_toolbar.or(under);
        let mut changed = hover != self.note_hover;
        self.note_hover = hover;

        let tooltip = match self.note_chrome() {
            Some(NoteChrome::Toolbar { layout, .. }) if self.shell.note_color.is_none() => {
                layout.hit(point).map(|button| {
                    let rect = layout.button(button);
                    (
                        button.label().to_owned(),
                        DVec2::new(f64::from(rect.min.x), f64::from(rect.max.y)),
                    )
                })
            }
            Some(NoteChrome::Pill { layout, .. }) => layout.hit(point).map(|button| {
                let rect = match button {
                    PillButton::Cancel => layout.cancel,
                    PillButton::Confirm => layout.confirm,
                };
                (
                    button.label().to_owned(),
                    DVec2::new(f64::from(rect.min.x), f64::from(rect.max.y)),
                )
            }),
            _ => None,
        };
        if tooltip != self.shell.tooltip {
            self.shell.tooltip = tooltip;
            self.shell.touch();
            changed = true;
        }
        changed
    }

    /// Handles a left press on the note chrome. Returns whether it hit a
    /// button (and so must not reach the board).
    fn chrome_press(&mut self, position: DVec2, event_loop: &ActiveEventLoop) -> bool {
        let point = position.as_vec2();
        match self.note_chrome() {
            Some(NoteChrome::Pill { layout, .. }) => match layout.hit(point) {
                Some(PillButton::Confirm) => {
                    self.execute(Action::ConfirmNoteEdit, event_loop);
                    true
                }
                Some(PillButton::Cancel) => {
                    self.execute(Action::CancelNoteEdit, event_loop);
                    true
                }
                None => false,
            },
            Some(NoteChrome::Toolbar { layout, .. }) => {
                let Some(button) = layout.hit(point) else {
                    return false;
                };
                let Some(id) = self.toolbar_note() else {
                    return false;
                };
                self.shell.tooltip = None;
                match button {
                    ToolbarButton::Background => {
                        let rect = layout.button(ToolbarButton::Background);
                        self.open_note_color(
                            id,
                            DVec2::new(f64::from(rect.min.x), f64::from(rect.max.y)),
                        );
                    }
                    ToolbarButton::Edit => self.begin_note_edit(id, false),
                    ToolbarButton::Delete => {
                        self.board.select_only(id);
                        self.execute(Action::DeleteNote, event_loop);
                    }
                }
                self.shell.touch();
                self.redraw();
                true
            }
            None => false,
        }
    }

    /// Opens the background popover for `id` under `anchor`.
    fn open_note_color(&mut self, id: ItemId, anchor: DVec2) {
        let Some(content) = self.board.scene().item(id).and_then(Item::note_content) else {
            return;
        };
        self.board.select_only(id);
        self.shell.menu = None;
        self.shell.note_color = Some(NoteColorPopover::open(
            id,
            content.background_or_default(),
            anchor,
            self.viewport(),
        ));
        self.shell.touch();
        self.redraw();
    }

    /// The anchor for a popover opened from a menu: under the toolbar
    /// swatch, where the hover toolbar would put it.
    fn note_color_anchor(&self, id: ItemId) -> DVec2 {
        self.note_screen_rect(id)
            .map_or(self.viewport() * 0.5, |rect| {
                let swatch = ToolbarLayout::new(rect).button(ToolbarButton::Background);
                DVec2::new(f64::from(swatch.min.x), f64::from(swatch.max.y))
            })
    }

    /// Runs a popover result.
    fn apply_popover_result(&mut self, result: PopoverResult) {
        match result {
            PopoverResult::None => {}
            PopoverResult::Changed => self.shell.touch(),
            PopoverResult::Closed => {
                self.shell.note_color = None;
                self.shell.touch();
            }
            PopoverResult::Apply(rgb) => {
                if let Some(popover) = self.shell.note_color.take() {
                    self.apply_note_background(popover.note, rgb);
                }
                self.shell.touch();
            }
            PopoverResult::Pick => {
                if let Some(popover) = self.shell.note_color.take() {
                    self.pick_for_note = Some(popover.note);
                    self.shell.picker = true;
                    self.shell.title_bar.hide();
                    self.shell.toast(
                        "Click a pixel to use it as the note background",
                        Instant::now(),
                    );
                }
                self.shell.touch();
            }
        }
        self.redraw();
    }

    /// Sets a note's background as one undo step.
    fn apply_note_background(&mut self, id: ItemId, rgb: [u8; 3]) {
        let Some(mut content) = self.board.scene().item(id).and_then(Item::note_content) else {
            return;
        };
        content.background = Some(rgb);
        if self.board.set_note_content(id, &content) {
            self.dirty_notes.insert(id);
            self.after_board();
        }
    }

    /// Sets the selected note's height to its laid-out content.
    fn fit_selected_note(&mut self) {
        let Some(id) = self.board.selected_note() else {
            return;
        };
        let Some(item) = self.board.scene().item(id) else {
            return;
        };
        let Some(content) = item.note_content() else {
            return;
        };
        let size = item.size();
        let height = self
            .note_renderer
            .content_height(&mut self.text, &content, size);
        if self.board.fit_note_height(id, height) {
            self.after_board();
        }
    }

    // -----------------------------------------------------------------------
    // Cursor
    // -----------------------------------------------------------------------

    /// Updates the window cursor icon from the active shell state or the
    /// board's hint.
    fn update_cursor_icon(&mut self, window: &Window, screen: DVec2) {
        let viewport = self.viewport();
        let title_bar = self
            .shell
            .title_bar
            .visible()
            .then(|| titlebar::TitleBarLayout::new(viewport));
        let icon = if let Some(dialog) = &self.shell.confirm_close {
            if dialog.hit_button(screen, viewport).is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            }
        } else if let Some(dialog) = &self.shell.image_info {
            if dialog.hit_button(screen, viewport).is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            }
        } else if self.shell.picker {
            CursorIcon::Crosshair
        } else if let Some(layout) = title_bar.filter(|layout| layout.contains(screen)) {
            if layout.button_at(screen).is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            }
        } else if self.shell.has_overlay() {
            CursorIcon::Default
        } else if let Some(direction) = resize_direction(viewport, screen) {
            resize_cursor(direction)
        } else if self.shell.mode() == Mode::Start {
            let layout = StartLayout::new(self.viewport(), self.recents.entries.len());
            if layout.hit(screen).is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            }
        } else if self.shell.locked {
            CursorIcon::Default
        } else if self.chrome_button_at(screen) {
            CursorIcon::Pointer
        } else {
            let hint = if self.note_session.is_some() {
                CursorHint::Text
            } else {
                self.board
                    .cursor(screen, self.view_info(), self.current_mods())
            };
            match hint {
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
            }
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

        if self.note_session.is_some() && self.board.handle_at(screen, self.view_info()) {
            // The box stays resizable while editing; a handle press scales
            // the frame and never moves the caret or ends the edit.
            let mods = self.current_mods();
            let view = ViewInfo {
                camera: &self.view.camera,
                viewport: self.view.viewport(),
            };
            if self.board.pointer_down(screen, view, mods) {
                self.after_board();
            }
            self.redraw();
            return;
        }
        if let Some(session_item) = self.note_session.as_ref().map(NoteSession::item) {
            if item == Some(session_item) {
                self.note_click(world);
                return;
            }
            self.commit_note_edit();
        }

        if double_click && let Some(id) = item {
            if self.board.scene().item(id).is_some_and(Item::is_note) {
                self.begin_note_edit(id, false);
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
    /// Takes the device from the background thread and attaches it to the
    /// window. `wait` blocks for it (the thread has announced it is done);
    /// otherwise a device that is not ready yet is left for `GpuReady`.
    fn attach_gpu(&mut self, wait: bool, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let (Some(receiver), Some(window)) = (self.gpu_init.as_ref(), self.window.clone()) else {
            return;
        };
        let device = if wait {
            receiver.recv().ok().flatten()
        } else {
            match receiver.try_recv() {
                Ok(device) => device,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            }
        };
        self.gpu_init = None;
        let size = window.inner_size();
        match device.and_then(|device| {
            GpuSurface::attach(device, window.clone(), (size.width, size.height))
        }) {
            Some(renderer) => {
                self.startup.mark(Milestone::Gpu);
                self.renderer = Some(renderer);
                // Anything opened during the CPU frames gets its previews now.
                self.upload_asset_previews();
                if self.bench_frames.is_some() {
                    self.open_pending_paths();
                }
                window.request_redraw();
            }
            None => {
                // No GPU device or surface at all. There is no CPU canvas for
                // images yet; be loud and honest rather than a grey box.
                platform::alert(
                    "RizRef",
                    "RizRef could not initialise a GPU surface (Vulkan or DirectX 12)\nand must close.",
                );
                event_loop.exit();
            }
        }
    }

    /// A frame presented without the GPU: the background and the CPU-painted
    /// shell layer (start view, title bar, menus, toasts) copied to the window
    /// with GDI. Items appear once the device is attached, a moment later.
    fn cpu_frame(&mut self, window: &Window, started: Instant, event_loop: &ActiveEventLoop) {
        self.sync_title_bar_state();
        let viewport = self.viewport();
        let scale = window.scale_factor();
        if self
            .ui_layer
            .needs_paint(viewport, scale, self.shell.revision())
        {
            let cursor = self.view.cursor_position();
            self.ui_layer.paint(
                &self.shell,
                &self.settings,
                &self.shortcuts,
                &self.recents,
                self.window_modes,
                viewport,
                scale,
                cursor,
                &mut self.text,
            );
        }
        let painted = started.elapsed();
        let size = window.inner_size();
        let layer = self.ui_layer.upload();
        let bgra = platform::gdi::compose_bgra(
            size.width.max(1),
            size.height.max(1),
            self.settings.canvas_theme.rgba(),
            layer
                .as_ref()
                .map(|(bytes, w, h)| (bytes.as_slice(), *w, *h)),
        );
        let composed = started.elapsed();
        let shown = platform::gdi::present(window, &bgra, size.width.max(1), size.height.max(1));
        tracing::debug!(
            paint_ms = perf::ms(painted),
            compose_ms = perf::ms(composed),
            present_ms = perf::ms(started.elapsed()),
            since_start_ms = perf::ms(perf::process_start().elapsed()),
            "cpu frame"
        );
        // The GPU device is the slow part of a launch (the driver may have to
        // wake a discrete GPU): it is built on its own thread, started only
        // once the first CPU frame is on screen. Started earlier — before or
        // right after window creation — the driver's start-up was measured to
        // stall showing the window by up to ~500 ms in one launch out of five,
        // and starting it after the window is shown gained nothing.
        if !self.gpu_started {
            self.gpu_started = true;
            self.gpu_init = Some(spawn_gpu_init(self.proxy.clone()));
        }
        if shown {
            self.last_drawn = 0;
            let canvas_mode = self.shell.mode() == Mode::Canvas;
            self.after_present(started, Duration::ZERO, canvas_mode, event_loop);
        }
        if self.published_accessibility != self.shell.revision() {
            self.refresh_accessibility();
        }
    }

    /// Opens or imports the paths held since launch.
    fn open_pending_paths(&mut self) {
        let pending = std::mem::take(&mut self.pending_paths);
        if !pending.is_empty() {
            let drop_world = self.drop_world();
            let paths = pending.into_iter().map(PathBuf::from).collect();
            self.import_paths(paths, drop_world);
        }
    }

    /// Book-keeping after a presented frame: frame statistics, the startup
    /// and board-load milestones, and the scripted soak.
    fn after_present(
        &mut self,
        frame_started: Instant,
        acquire_wait: Duration,
        canvas_mode: bool,
        event_loop: &ActiveEventLoop,
    ) {
        let now = Instant::now();
        let total = now.saturating_duration_since(frame_started);
        self.frame_stats.record(total.saturating_sub(acquire_wait));

        self.startup.mark_at(Milestone::FirstFrame, now);
        if self.renderer.is_some() && self.startup.mark_at(Milestone::GpuFrame, now) {
            tracing::info!("{}", self.startup.summary());
            if self.exit_after_first_frame {
                // Both milestones are in: the first (CPU) frame and the
                // first GPU frame, which also means the device thread is done.
                println!("{}", self.startup.to_json());
                event_loop.exit();
                return;
            }
        }
        // A board is interactive at its first GPU frame: that is when its
        // previews draw.
        if self.renderer.is_some()
            && let Some(started) = self.board_open_started.take()
        {
            let load = now.saturating_duration_since(started);
            self.last_board_load = Some(load);
            tracing::info!(ms = perf::ms(load), "board interactive");
        }

        if let Some(bench) = self.bench.as_mut() {
            if bench.record(total, acquire_wait, self.last_drawn) {
                let report = bench.to_json(self.board.scene().items().len(), self.last_board_load);
                tracing::info!(%report, "frame bench finished");
                println!("{report}");
                self.bench = None;
                event_loop.exit();
            }
        } else if let Some(frames) = self.bench_frames
            && canvas_mode
            && self.renderer.is_some()
            && self.pending_paths.is_empty()
            && !self.board.scene().items().is_empty()
        {
            self.bench_frames = None;
            tracing::info!(frames, "frame bench starting");
            self.bench = Some(FrameBench::new(frames, self.view.camera));
        }
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
            .with_inner_size(self.plan.size)
            // The window draws its own auto-hiding title bar; a native frame
            // would sit above the board permanently.
            .with_decorations(false)
            .with_resizable(true)
            // AccessKit must attach before the window is first shown.
            .with_visible(false);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => {
                self.startup.mark(Milestone::Window);
                Arc::new(window)
            }
            Err(error) => {
                tracing::error!(%error, "cannot create the window");
                platform::alert("RizRef", "RizRef could not open its window and must close.");
                event_loop.exit();
                return;
            }
        };

        self.accessibility = Accessibility::new(event_loop, &window, self.proxy.clone());
        window.set_visible(true);
        self.window_modes.apply(&window);

        let size = window.inner_size();
        let scale = window.scale_factor();
        self.view.set_viewport(DVec2::new(
            size.width as f64 / scale,
            size.height as f64 / scale,
        ));
        self.rebuild_hud(scale);
        tracing::info!(?size, scale, "window ready");
        self.window = Some(window);
        self.refresh_window_title();
        self.install_drop_target();
        // The GPU may already be ready (a fast driver); otherwise the first
        // frames are CPU frames and `AppEvent::GpuReady` attaches it later.
        self.attach_gpu(false, event_loop);
        self.window.as_ref().expect("just set").request_redraw();

        self.offer_recovery();

        // Command-line and forwarded paths were held until the window
        // (and therefore the camera and viewport) existed. The frame bench
        // holds them until the GPU is attached, so its board-load time is an
        // open on a running GPU, as a user's open is.
        if self.bench_frames.is_none() {
            self.open_pending_paths();
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
        if let Some(accessibility) = self.accessibility.as_mut() {
            accessibility.process_event(&window, &event);
        }
        match event {
            WindowEvent::Resized(size) => {
                // Before the GPU is ready the CPU frame follows the window size
                // by itself; the swapchain is created at the current size.
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                let scale = window.scale_factor();
                self.view.set_viewport(DVec2::new(
                    size.width as f64 / scale,
                    size.height as f64 / scale,
                ));
                self.sync_title_bar_state();
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
                if self.shell.image_info.is_some() {
                    let viewport = self.viewport();
                    let changed = self
                        .shell
                        .image_info
                        .as_mut()
                        .is_some_and(|dialog| dialog.pointer_move(logical, viewport));
                    if changed {
                        self.shell.touch();
                        self.redraw();
                    }
                    self.update_cursor_icon(&window, logical);
                    return;
                }
                if self.shell.confirm_close.is_some() {
                    let viewport = self.viewport();
                    let changed = self
                        .shell
                        .confirm_close
                        .as_mut()
                        .is_some_and(|dialog| dialog.pointer_move(logical, viewport));
                    if changed {
                        self.shell.touch();
                    }
                    self.update_cursor_icon(&window, logical);
                    self.redraw();
                    return;
                }
                self.update_title_bar_pointer(Some(logical));
                let mut menu_changed = false;
                let viewport = self.viewport();
                if let Some(menu) = self.shell.menu.as_mut() {
                    menu_changed = menu.pointer_move(logical, viewport);
                }
                if menu_changed {
                    self.shell.touch();
                }
                if let Some(popover) = self.shell.note_color.as_mut()
                    && popover.pointer_moved(logical)
                {
                    self.shell.touch();
                }
                if self.shell.mode() == Mode::Canvas
                    && self.shell.menu.is_none()
                    && self.update_note_hover(logical)
                {
                    self.redraw();
                }
                if self.shell.title_bar.visible()
                    && titlebar::TitleBarLayout::new(viewport).contains(logical)
                {
                    self.update_cursor_icon(&window, logical);
                    self.redraw();
                    return;
                }
                if self.shell.has_overlay() || self.shell.mode() == Mode::Start || self.shell.locked
                {
                    self.update_cursor_icon(&window, logical);
                    self.redraw();
                    return;
                }
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
                if self.shell.title_bar.pointer_left() {
                    self.shell.touch();
                    self.redraw();
                }
                if self.note_hover.take().is_some() || self.shell.tooltip.take().is_some() {
                    self.shell.touch();
                    self.redraw();
                }
                if self.board.pointer_leave() {
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                let position = self.view.cursor_position().unwrap_or(self.viewport() * 0.5);
                if self.shell.image_info.is_some() {
                    // A true modal: only a left press reaches the dialog,
                    // and nothing reaches the board behind it.
                    if pressed && button == MouseButton::Left {
                        let viewport = self.viewport();
                        let result = self
                            .shell
                            .image_info
                            .as_mut()
                            .map_or(InfoResult::None, |dialog| {
                                dialog.pointer_click(position, viewport)
                            });
                        self.apply_image_info_result(result);
                    }
                    return;
                }
                if self.shell.confirm_close.is_some() {
                    if pressed && button == MouseButton::Left {
                        let viewport = self.viewport();
                        let choice = self
                            .shell
                            .confirm_close
                            .as_mut()
                            .map(|dialog| dialog.pointer_click(position, viewport));
                        match choice {
                            Some(CloseChoice::Cancel) => {
                                self.shell.close_confirm_close();
                                self.redraw();
                            }
                            Some(CloseChoice::Discard) => self.confirm_discard(event_loop),
                            _ => {}
                        }
                    }
                    return;
                }
                if button == MouseButton::Right {
                    if !pressed {
                        return;
                    }
                    if self.shell.menu.is_some() {
                        self.shell.close_menu();
                        self.redraw();
                        return;
                    }
                    if self.shell.title_bar.visible()
                        && titlebar::TitleBarLayout::new(self.viewport()).contains(position)
                    {
                        return;
                    }
                    if self.shell.mode() == Mode::Start {
                        self.start_right_click(position);
                        return;
                    }
                    if self.shell.has_overlay() {
                        return;
                    }
                    if !self.shell.locked {
                        let world = self
                            .view
                            .camera
                            .screen_to_world(position, self.viewport())
                            .as_vec2();
                        if let Some(id) = self.board.item_at(world)
                            && !self.board.selection().contains(id)
                        {
                            self.board.select_only(id);
                            self.redraw();
                        }
                    }
                    self.open_context_menu(position);
                    return;
                }

                let viewport = self.viewport();
                if let Some(menu) = self.shell.menu.as_mut() {
                    let result = menu.pointer_click(position, viewport);
                    self.apply_menu_result(result, event_loop);
                    return;
                }
                if let Some(popover) = self.shell.note_color.as_mut() {
                    if pressed && button == MouseButton::Left {
                        let result = popover.click(position);
                        self.apply_popover_result(result);
                    } else if pressed {
                        self.apply_popover_result(PopoverResult::Closed);
                    }
                    return;
                }
                if self.shell.shortcuts.is_some() || self.shell.settings.is_some() {
                    if pressed
                        && !panels::shortcuts_contains(viewport, position)
                        && !panels::settings_contains(viewport, position)
                    {
                        self.shell.shortcuts = None;
                        self.shell.settings = None;
                        self.shell.touch();
                        self.redraw();
                    }
                    return;
                }
                if self.shell.picker && pressed && button == MouseButton::Left {
                    self.pending_pick = Some(position);
                    self.redraw();
                    return;
                }
                if pressed
                    && button == MouseButton::Left
                    && self.shell.title_bar.visible()
                    && titlebar::TitleBarLayout::new(viewport).contains(position)
                {
                    self.title_bar_press(&window, position, event_loop);
                    return;
                }
                if pressed && button == MouseButton::Left {
                    // A click anywhere else ends the title-bar double-click run.
                    self.last_title_click = None;
                }
                if pressed
                    && button == MouseButton::Left
                    && let Some(direction) = resize_direction(viewport, position)
                {
                    if let Err(error) = window.drag_resize_window(direction) {
                        tracing::warn!(%error, "cannot start a window resize");
                    }
                    return;
                }
                if self.shell.mode() == Mode::Start {
                    if pressed {
                        self.start_click(position, event_loop);
                    }
                    return;
                }
                // Note chrome is hit-tested before the board, so pressing a
                // button never starts a drag or changes the selection.
                if pressed
                    && button == MouseButton::Left
                    && !self.shell.locked
                    && !self.shell.picker
                    && self.chrome_press(position, event_loop)
                {
                    self.update_cursor_icon(&window, position);
                    return;
                }

                let button = match button {
                    MouseButton::Left => InputButton::Left,
                    MouseButton::Middle => InputButton::Middle,
                    _ => return,
                };
                let trigger = Trigger::Button {
                    button,
                    pressed,
                    space_held: self.view.space_held(),
                };
                if let Some(command) = input::resolve(&trigger) {
                    self.apply(command);
                    // A left release ends a pan *and* the board gesture its
                    // press began; returning here left items stuck to the
                    // cursor.
                    if !input::board_also_handles(&trigger) {
                        return;
                    }
                }
                if button == InputButton::Left {
                    if pressed {
                        if !self.view.space_held() && !self.shell.locked {
                            self.left_press(&window);
                        }
                    } else if !self.shell.locked
                        && let Some(screen) = self.view.cursor_position()
                    {
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
            WindowEvent::MouseWheel { delta, .. } => {
                if self.shell.confirm_close.is_some() || self.shell.image_info.is_some() {
                    return;
                }
                if self.shell.menu.is_some() || self.shell.mode() == Mode::Start {
                    return;
                }
                if self.shell.title_bar.visible()
                    && self.view.cursor_position().is_some_and(|position| {
                        titlebar::TitleBarLayout::new(self.viewport()).contains(position)
                    })
                {
                    return;
                }
                if self.shell.shortcuts.is_some() {
                    let (page, key) = match delta {
                        MouseScrollDelta::LineDelta(_, y) if y > 0.0 => {
                            (crate::shortcuts::Key::PageUp, true)
                        }
                        MouseScrollDelta::LineDelta(_, y) if y < 0.0 => {
                            (crate::shortcuts::Key::PageDown, true)
                        }
                        _ => (crate::shortcuts::Key::PageDown, false),
                    };
                    let viewport = self.viewport();
                    let result =
                        self.shell.shortcuts.as_mut().filter(|_| key).map(|panel| {
                            panel.key(KeyChord::plain(page), &self.shortcuts, viewport)
                        });
                    if let Some(result) = result {
                        self.apply_shortcuts_panel_result(result);
                    }
                    return;
                }
                match delta {
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
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // F3 is the developer overlay: outside the rebindable table on
                // purpose, so it works in every mode and can never be lost.
                if pressed && !event.repeat && event.logical_key == Key::Named(NamedKey::F3) {
                    self.dev_overlay = !self.dev_overlay;
                    tracing::info!(on = self.dev_overlay, "developer overlay");
                    self.redraw();
                    return;
                }
                let chord = shortcut_key(&event.logical_key).map(|key| KeyChord {
                    key,
                    ctrl: self.modifiers.control_key(),
                    shift: self.modifiers.shift_key(),
                    alt: self.modifiers.alt_key(),
                });

                // Overlays own the keyboard completely.
                let viewport = self.viewport();
                if self.shell.image_info.is_some() && self.shell.confirm_close.is_none() {
                    if pressed {
                        let result = chord.map_or(InfoResult::None, |chord| {
                            self.shell
                                .image_info
                                .as_mut()
                                .map_or(InfoResult::None, |dialog| dialog.key(chord))
                        });
                        self.apply_image_info_result(result);
                    }
                    return;
                }
                if self.shell.confirm_close.is_some() {
                    if pressed {
                        let choice = chord.and_then(|chord| {
                            self.shell
                                .confirm_close
                                .as_mut()
                                .map(|dialog| dialog.key(chord))
                        });
                        match choice {
                            Some(CloseChoice::Cancel) => {
                                self.shell.close_confirm_close();
                                self.redraw();
                            }
                            Some(CloseChoice::Discard) => self.confirm_discard(event_loop),
                            Some(CloseChoice::Changed) => {
                                self.shell.touch();
                                self.redraw();
                            }
                            _ => {}
                        }
                    }
                    return;
                }
                if self.shell.shortcuts.is_some() {
                    let result = chord.and_then(|chord| {
                        let shortcuts = &self.shortcuts;
                        self.shell
                            .shortcuts
                            .as_mut()
                            .map(|panel| panel.key(chord, shortcuts, viewport))
                    });
                    if let Some(result) = result {
                        self.apply_shortcuts_panel_result(result);
                    }
                    return;
                }
                if self.shell.settings.is_some() {
                    let result = if pressed {
                        chord.and_then(|chord| {
                            self.shell.settings.as_mut().map(|panel| panel.key(chord))
                        })
                    } else {
                        None
                    };
                    if let Some(result) = result {
                        self.apply_settings_panel_result(result);
                    }
                    return;
                }
                if self.shell.note_color.is_some() {
                    if pressed {
                        let ctrl = self.modifiers.control_key();
                        let result = match (&event.logical_key, chord) {
                            (Key::Character(_), _) if !ctrl => {
                                let typed = event.text.as_deref().unwrap_or_default();
                                let changed = self
                                    .shell
                                    .note_color
                                    .as_mut()
                                    .is_some_and(|popover| popover.type_text(typed));
                                if changed {
                                    PopoverResult::Changed
                                } else {
                                    PopoverResult::None
                                }
                            }
                            (_, Some(chord)) => self
                                .shell
                                .note_color
                                .as_mut()
                                .map_or(PopoverResult::None, |popover| popover.key(chord.key)),
                            _ => PopoverResult::None,
                        };
                        self.apply_popover_result(result);
                    }
                    return;
                }
                if self.shell.menu.is_some() {
                    let result = if pressed {
                        chord.and_then(|chord| {
                            self.shell
                                .menu
                                .as_mut()
                                .map(|menu| menu.key(chord.key, viewport))
                        })
                    } else {
                        None
                    };
                    if let Some(result) = result {
                        self.apply_menu_result(result, event_loop);
                    }
                    return;
                }

                if self.note_session.is_some() {
                    if self.handle_note_key(&event, pressed) {
                        self.redraw();
                        return;
                    }
                    if !pressed {
                        return;
                    }
                    // F1 and friends fall through; typed characters were
                    // already consumed by the editor above.
                } else {
                    if matches!(&event.logical_key, Key::Named(NamedKey::Space)) {
                        self.view.set_space_held(pressed);
                        return;
                    }
                    if !pressed {
                        return;
                    }
                }

                if matches!(&event.logical_key, Key::Named(NamedKey::Escape)) && self.shell.escape()
                {
                    self.redraw();
                    return;
                }
                if let Some(chord) = chord
                    && let Some(id) = self.shortcuts.resolve(chord)
                {
                    self.execute_shortcut(id, event_loop);
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
                let frame_started = Instant::now();
                if self.renderer.is_none() {
                    self.cpu_frame(&window, frame_started, event_loop);
                    return;
                }
                if let Some(bench) = &self.bench {
                    let index = bench.work.len() as u32;
                    self.view.camera = bench.camera(index, self.view.viewport());
                }
                self.sync_title_bar_state();
                // Advance the zoom animation first: this frame shows the new
                // state, and if it is still running the next frame is queued.
                // The HUD and the frame params are gathered before the
                // renderer is taken mutably, so the borrows never overlap.
                let canvas_mode = self.shell.mode() == Mode::Canvas;
                let animating = canvas_mode && self.view.advance(Instant::now());
                if canvas_mode {
                    let _next_frame = self.advance_animations(Instant::now());
                    self.ensure_note_textures();
                }
                let items = if canvas_mode {
                    self.build_items()
                } else {
                    Vec::new()
                };
                if canvas_mode {
                    self.ensure_textures(&items);
                }
                let overlay = if canvas_mode {
                    self.build_overlay()
                } else {
                    Vec::new()
                };
                let picking = self.pending_pick.is_some();
                // The picked frame must contain only the board: the cursor
                // crosshair and the HUD would otherwise contaminate the pixel.
                let hud_layer = if canvas_mode && self.settings.hud_visible && !picking {
                    let text = self.readout_text();
                    self.render_hud(&text)
                } else {
                    None
                };

                let viewport = self.viewport();
                let scale = window.scale_factor();
                let repaint_ui = self
                    .ui_layer
                    .needs_paint(viewport, scale, self.shell.revision());
                if repaint_ui {
                    let cursor = self.view.cursor_position();
                    self.ui_layer.paint(
                        &self.shell,
                        &self.settings,
                        &self.shortcuts,
                        &self.recents,
                        self.window_modes,
                        viewport,
                        scale,
                        cursor,
                        &mut self.text,
                    );
                }
                let ui_visible = !picking
                    && (self.shell.mode() == Mode::Start
                        || self.shell.has_overlay()
                        || self.shell.toast.is_some()
                        || self.shell.picker
                        || self.window_modes.effective_click_through()
                        || self.window_modes.effective_opacity_percent() < 100);
                let ui_bytes = if ui_visible && repaint_ui {
                    self.ui_layer.upload()
                } else {
                    None
                };
                let ui = if ui_visible {
                    match &ui_bytes {
                        Some((bytes, width, height)) => UiUpload::Layer {
                            bytes,
                            width: *width,
                            height: *height,
                        },
                        None if repaint_ui => UiUpload::None,
                        None => UiUpload::Keep,
                    }
                } else {
                    UiUpload::None
                };

                let physical_pick = self.pending_pick.map(|position| {
                    (
                        (position.x * scale).round().max(0.0) as u32,
                        (position.y * scale).round().max(0.0) as u32,
                    )
                });
                self.last_drawn = items.len();
                let acquire_wait;
                let mut presented = false;
                let picked = {
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
                        physical_scale: scale,
                        scene_rect: self.board.scene_rect(),
                        items: &items,
                        overlay: &overlay,
                        hud,
                        ui,
                        flags: FrameFlags {
                            grid: self.settings.grid_visible,
                            scene_border: self.settings.scene_border,
                            checkerboard: self.settings.checkerboard,
                            grayscale: self.settings.grayscale,
                        },
                        background: self.settings.canvas_theme.rgba(),
                        pick: physical_pick,
                    };
                    let result = renderer.render(&params);
                    acquire_wait = renderer.last_swapchain_wait();
                    match result {
                        Ok(pixel) => {
                            presented = true;
                            pixel
                        }
                        Err(FrameError::NeedsReconfigure) => None,
                        Err(FrameError::Fatal(why)) => {
                            // Log and stop: retrying into a lost surface would
                            // spin the loop. The CPU fallback (phase 02+) will
                            // take over cases a fresh adapter could have served.
                            tracing::error!(%why, "render failed; window will stay as-is");
                            None
                        }
                    }
                };
                if let Some(pixel) = picked {
                    self.finish_pick(pixel);
                }
                if presented {
                    self.after_present(frame_started, acquire_wait, canvas_mode, event_loop);
                }
                if animating || self.bench.is_some() || self.dev_overlay {
                    window.request_redraw();
                }
                if self.published_accessibility != self.shell.revision() {
                    self.refresh_accessibility();
                }
            }
            WindowEvent::Focused(false) => {
                if self.shell.title_bar.hide() {
                    self.shell.touch();
                }
                if self.shell.close_menu() {
                    self.redraw();
                }
            }
            WindowEvent::CloseRequested => self.request_close(event_loop),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::ForwardedArgs(args) => {
                tracing::info!(count = args.len(), "accepted forwarded args");
                self.pending_paths = args;
                let pending = std::mem::take(&mut self.pending_paths);
                let drop_world = self.drop_world();
                let paths = pending.into_iter().map(PathBuf::from).collect();
                self.import_paths(paths, drop_world);
            }
            AppEvent::GpuReady => self.attach_gpu(true, event_loop),
            AppEvent::Import(outcome) => self.apply_import(*outcome),
            AppEvent::Dropped(event) => self.apply_drop(*event),
            AppEvent::Accessibility(event) => match event.window_event {
                accesskit_winit::WindowEvent::InitialTreeRequested => {
                    self.refresh_accessibility();
                }
                accesskit_winit::WindowEvent::ActionRequested(request) => {
                    let action = self
                        .accessibility
                        .as_ref()
                        .and_then(|accessibility| accessibility.requested_action(&request));
                    if let Some(action) = action {
                        self.execute(action, event_loop);
                        self.redraw();
                    }
                }
                accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                    tracing::debug!("accessibility deactivated");
                }
            },
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The board idles at zero CPU unless something is animating: a playing
        // GIF arms a one-shot timer for its next frame, and everything else
        // waits for an event. Ghost/click-through modes add a 50 ms poll so the
        // window can be woken even though it receives no input.
        let now = Instant::now();
        if self.shell.expire_toast(now) {
            self.redraw();
        }

        let mut poll_deadline = None;
        if self.window_modes.ghost() {
            let down = platform::window_modes::ghost_chord_down();
            if down && !self.ghost_hotkey_was_down {
                self.ghost_hotkey_was_down = true;
                self.execute(Action::ToggleGhostMode, event_loop);
            } else if !down {
                self.ghost_hotkey_was_down = false;
            }
            poll_deadline = Some(now + Duration::from_millis(50));
        } else if self.window_modes.effective_click_through() {
            let down = platform::window_modes::click_through_chord_down();
            if down && !self.click_through_hotkey_was_down {
                self.click_through_hotkey_was_down = true;
                self.execute(Action::ToggleClickThrough, event_loop);
            } else if !down {
                self.click_through_hotkey_was_down = false;
            }
            poll_deadline = Some(now + Duration::from_millis(50));
        }

        if self.autosave.is_due(self.autosave_policy, now) {
            self.autosave_now();
        }
        let autosave_deadline = self.autosave.deadline(self.autosave_policy);

        let deadline = [
            self.next_frame_at,
            self.shell.next_wake(),
            poll_deadline,
            autosave_deadline,
        ]
        .into_iter()
        .flatten()
        .min();
        match deadline {
            Some(deadline) if deadline <= now => {
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

/// Maps a winit logical key to the shortcut vocabulary.
fn shortcut_key(key: &Key) -> Option<ShortcutKey> {
    match key {
        Key::Character(text) => ShortcutKey::from_name(text),
        Key::Named(named) => match named {
            NamedKey::F1 => Some(ShortcutKey::F1),
            NamedKey::F2 => Some(ShortcutKey::F2),
            NamedKey::F11 => Some(ShortcutKey::F11),
            NamedKey::ArrowLeft => Some(ShortcutKey::Left),
            NamedKey::ArrowRight => Some(ShortcutKey::Right),
            NamedKey::ArrowUp => Some(ShortcutKey::Up),
            NamedKey::ArrowDown => Some(ShortcutKey::Down),
            NamedKey::PageUp => Some(ShortcutKey::PageUp),
            NamedKey::PageDown => Some(ShortcutKey::PageDown),
            NamedKey::Home => Some(ShortcutKey::Home),
            NamedKey::End => Some(ShortcutKey::End),
            NamedKey::Delete => Some(ShortcutKey::Delete),
            NamedKey::Backspace => Some(ShortcutKey::Backspace),
            NamedKey::Escape => Some(ShortcutKey::Escape),
            NamedKey::Tab => Some(ShortcutKey::Tab),
            NamedKey::Space => Some(ShortcutKey::Space),
            NamedKey::Enter => Some(ShortcutKey::Enter),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a command changes scene content (and must be blocked by the canvas
/// lock). Navigation and selection stay available in a locked canvas.
fn command_mutates_items(command: Command) -> bool {
    use Command::{
        AlignBottom, AlignLeft, AlignRight, AlignTop, ArrangeGrid, ArrangeHierarchy, ArrangeSpread,
        ArrangeStack, CreateNote, DeleteSelection, DistributeHorizontal, DistributeVertical,
        DuplicateSelection, FlipHorizontal, FlipVertical, GroupSelection, LowerSelection,
        LowerToBottom, MatchHeight, MatchWidth, Nudge, RaiseSelection, RaiseToTop, Redo,
        ResetTransform, Undo, UngroupSelection,
    };
    matches!(
        command,
        Undo | Redo
            | DuplicateSelection
            | DeleteSelection
            | Nudge { .. }
            | FlipHorizontal
            | FlipVertical
            | ResetTransform
            | MatchWidth
            | MatchHeight
            | GroupSelection
            | UngroupSelection
            | RaiseSelection
            | LowerSelection
            | RaiseToTop
            | LowerToBottom
            | ArrangeGrid
            | ArrangeHierarchy
            | ArrangeSpread
            | ArrangeStack
            | CreateNote
            | AlignLeft
            | AlignRight
            | AlignTop
            | AlignBottom
            | DistributeHorizontal
            | DistributeVertical
    )
}

/// The edge-resize direction for a point near the frameless window's border.
///
/// The title bar owns the top strip, so only the left, right, bottom and bottom
/// corners are exposed: a pointer near the top reveals the bar instead.
fn resize_direction(viewport: DVec2, point: DVec2) -> Option<ResizeDirection> {
    let west = point.x <= RESIZE_EDGE;
    let east = point.x >= viewport.x - RESIZE_EDGE;
    let south = point.y >= viewport.y - RESIZE_EDGE;
    match (west, east, south) {
        (true, false, true) => Some(ResizeDirection::SouthWest),
        (false, true, true) => Some(ResizeDirection::SouthEast),
        (true, false, false) => Some(ResizeDirection::West),
        (false, true, false) => Some(ResizeDirection::East),
        (false, false, true) => Some(ResizeDirection::South),
        _ => None,
    }
}

/// The cursor matching a resize direction.
fn resize_cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
        ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
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
/// Starts building the GPU device on its own thread; the result arrives on
/// the returned channel, announced by `AppEvent::GpuReady`.
fn spawn_gpu_init(proxy: EventLoopProxy<AppEvent>) -> std::sync::mpsc::Receiver<Option<GpuDevice>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("rizref-gpu-init".to_owned())
        .spawn(move || {
            let device = GpuDevice::create();
            let _ = sender.send(device);
            let _ = proxy.send_event(AppEvent::GpuReady);
        });
    if let Err(error) = spawned {
        // The receiver reports a disconnect, which `attach_gpu` treats as
        // "no GPU": the same honest failure as a device that cannot open.
        tracing::error!(%error, "cannot start the GPU thread");
    }
    receiver
}

fn run_window(
    guard: Option<SingleInstance>,
    parsed: ParsedArgs,
    mut startup: StartupClock,
) -> ExitCode {
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
    let benchmark = parsed.exit_after_first_frame || parsed.bench_frames.is_some();
    let proxy = event_loop.create_proxy();
    let app_proxy = proxy.clone();
    if !benchmark {
        serve_forwarded_args(move |args| {
            let _ = proxy.send_event(AppEvent::ForwardedArgs(args));
        });
    }

    startup.mark(Milestone::EventLoop);
    let marker = autosave::session_marker();
    let previous_session = if benchmark {
        PreviousSession::Clean
    } else {
        autosave::claim_session(&marker)
    };
    let mut app = App::new(WindowPlan::default(), app_proxy, parsed.paths.is_empty());
    app.previous_session = previous_session;
    // The benches measure a launch, not a recovery prompt.
    if !benchmark {
        app.pending_recoveries = autosave::pending_recoveries(&app.autosave_dir);
    }
    if previous_session == PreviousSession::Crashed {
        tracing::warn!(
            recoveries = app.pending_recoveries.len(),
            "the previous session did not exit cleanly"
        );
    }
    app.startup = startup;
    app.exit_after_first_frame = parsed.exit_after_first_frame;
    app.bench_frames = parsed.bench_frames;
    app.pending_paths = parsed.paths;
    match event_loop.run_app(&mut app) {
        Ok(()) => {
            tracing::info!("exited cleanly");
            if !benchmark {
                autosave::release_session(&marker);
            }
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
        CANVAS_BACKGROUND, LOG_FILES_KEPT, Playback, ShortcutKey, WindowPlan, looks_like_image,
        parse_args, resize_cursor, resize_direction, shortcut_key, stale_logs, tick_animations,
        version_line,
    };
    use glam::{DVec2, Vec2};
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
    fn logical_keys_map_to_the_shortcut_vocabulary() {
        assert_eq!(
            shortcut_key(&Key::Character("Z".into())),
            Some(ShortcutKey::Z)
        );
        assert_eq!(
            shortcut_key(&Key::Named(NamedKey::ArrowLeft)),
            Some(ShortcutKey::Left)
        );
        assert_eq!(
            shortcut_key(&Key::Named(NamedKey::Tab)),
            Some(ShortcutKey::Tab)
        );
        assert_eq!(shortcut_key(&Key::Named(NamedKey::F3)), None);
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
    fn the_frameless_window_resizes_from_its_side_and_bottom_edges() {
        let viewport = DVec2::new(800.0, 600.0);
        assert_eq!(
            resize_direction(viewport, DVec2::new(2.0, 300.0)),
            Some(winit::window::ResizeDirection::West)
        );
        assert_eq!(
            resize_direction(viewport, DVec2::new(798.0, 300.0)),
            Some(winit::window::ResizeDirection::East)
        );
        assert_eq!(
            resize_direction(viewport, DVec2::new(400.0, 598.0)),
            Some(winit::window::ResizeDirection::South)
        );
        assert_eq!(
            resize_direction(viewport, DVec2::new(2.0, 598.0)),
            Some(winit::window::ResizeDirection::SouthWest)
        );
        assert_eq!(
            resize_direction(viewport, DVec2::new(798.0, 598.0)),
            Some(winit::window::ResizeDirection::SouthEast)
        );
        assert_eq!(resize_direction(viewport, DVec2::new(400.0, 300.0)), None);
        assert_eq!(
            resize_cursor(winit::window::ResizeDirection::South),
            winit::window::CursorIcon::NsResize
        );
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
