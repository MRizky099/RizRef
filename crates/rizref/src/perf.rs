//! Performance instrumentation: the startup clock and the frame-time window.
//!
//! Phase 07 verifies the phase-00 budgets, and a budget is only as good as the
//! number behind it. This module holds the two pieces of measurement the
//! application carries in every build:
//!
//! - [`StartupClock`] stamps the launch milestones (process start, logging up,
//!   event loop built, window created, GPU surface ready, first frame
//!   presented). The first present is logged once at `info` with the whole
//!   waterfall, and `--exit-after-first-frame` prints it and exits — that is
//!   what `scripts/bench-startup.ps1` runs repeatedly.
//! - [`FrameStats`] keeps a rolling window of frame times for the `F3`
//!   developer overlay and for the `--bench-frames` soak, which reports the
//!   median, 95th percentile and worst frame.
//!
//! Both are pure data with no window or GPU dependency, so their arithmetic is
//! unit-tested here.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// When the process started, as close to `main` as Rust allows.
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Stamps the process start. Called first thing in `main`; later calls keep
/// the first stamp.
pub fn mark_process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

/// The process start stamp (stamped now if `main` never did).
pub fn process_start() -> Instant {
    mark_process_start()
}

/// A launch milestone, in the order a launch passes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Milestone {
    /// The log subscriber is installed.
    Logging,
    /// The single-instance handshake is settled and the event loop exists.
    EventLoop,
    /// The native window exists (still hidden).
    Window,
    /// The wgpu device (built on a background thread) is attached to the
    /// window and its surface configured.
    Gpu,
    /// The first frame was presented: the window shows the canvas or start
    /// view and takes input. Before the GPU is ready it is a CPU frame.
    FirstFrame,
    /// The first frame presented by the GPU: images draw from here on.
    GpuFrame,
}

impl Milestone {
    /// Every milestone, in launch order.
    pub const ALL: [Self; 6] = [
        Self::Logging,
        Self::EventLoop,
        Self::Window,
        Self::Gpu,
        Self::FirstFrame,
        Self::GpuFrame,
    ];

    /// Stable lower-case name, used in the log line and the JSON report.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Logging => "logging",
            Self::EventLoop => "event_loop",
            Self::Window => "window",
            Self::Gpu => "gpu",
            Self::FirstFrame => "first_frame",
            Self::GpuFrame => "gpu_frame",
        }
    }
}

/// The launch waterfall: one stamp per milestone, relative to process start.
#[derive(Debug, Clone)]
pub struct StartupClock {
    start: Instant,
    stamps: [Option<Duration>; Milestone::ALL.len()],
}

impl StartupClock {
    /// A clock measuring from `start`.
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            stamps: [None; Milestone::ALL.len()],
        }
    }

    /// Stamps `milestone` at `now`. A second stamp of the same milestone is
    /// ignored, so "first frame" stays the first one. Returns whether this
    /// call recorded it.
    pub fn mark_at(&mut self, milestone: Milestone, now: Instant) -> bool {
        let slot = &mut self.stamps[milestone as usize];
        if slot.is_some() {
            return false;
        }
        *slot = Some(now.saturating_duration_since(self.start));
        true
    }

    /// Stamps `milestone` now.
    pub fn mark(&mut self, milestone: Milestone) -> bool {
        self.mark_at(milestone, Instant::now())
    }

    /// Time from process start to `milestone`, if it was reached.
    pub fn elapsed(&self, milestone: Milestone) -> Option<Duration> {
        self.stamps[milestone as usize]
    }

    /// One line for the log: `first_frame 212.4 ms (logging 3.1, …)`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = Milestone::ALL
            .iter()
            .filter_map(|&m| {
                self.elapsed(m)
                    .map(|d| format!("{} {:.1}", m.name(), ms(d)))
            })
            .collect();
        format!("startup ms: {}", parts.join(", "))
    }

    /// A flat JSON object of milestone → milliseconds, for the bench script.
    pub fn to_json(&self) -> String {
        let parts: Vec<String> = Milestone::ALL
            .iter()
            .filter_map(|&m| {
                self.elapsed(m)
                    .map(|d| format!("\"{}_ms\":{:.3}", m.name(), ms(d)))
            })
            .collect();
        format!("{{{}}}", parts.join(","))
    }
}

/// How many frames the rolling window keeps (two seconds at 60 Hz).
pub const FRAME_WINDOW: usize = 120;

/// A rolling window of frame times with percentile queries.
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    /// Ring buffer of the most recent frame times.
    samples: Vec<Duration>,
    /// Next slot to overwrite once the ring is full.
    next: usize,
    /// Frames recorded in total, including those that left the window.
    total: u64,
}

impl FrameStats {
    /// Records one frame's duration.
    pub fn record(&mut self, frame: Duration) {
        if self.samples.len() < FRAME_WINDOW {
            self.samples.push(frame);
        } else {
            self.samples[self.next] = frame;
        }
        self.next = (self.next + 1) % FRAME_WINDOW;
        self.total += 1;
    }

    #[cfg(test)]
    /// Frames recorded since the window was created.
    pub const fn total(&self) -> u64 {
        self.total
    }

    #[cfg(test)]
    /// Frames currently in the window.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[cfg(test)]
    /// Whether no frame has been recorded.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The most recently recorded frame.
    pub fn last(&self) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let index = (self.next + FRAME_WINDOW - 1) % FRAME_WINDOW;
        self.samples.get(index.min(self.samples.len() - 1)).copied()
    }

    /// The `p`-th percentile (0–100, nearest rank) of the window.
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        percentile(&self.samples, p)
    }

    /// The slowest frame in the window.
    pub fn max(&self) -> Option<Duration> {
        self.samples.iter().max().copied()
    }
}

/// Nearest-rank percentile of `samples` (`p` clamped to 0–100).
pub fn percentile(samples: &[Duration], p: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let p = p.clamp(0.0, 100.0);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// The scripted soak: `--bench-frames=N` drives the camera through a fixed
/// pan/zoom path for `N` frames and reports the frame-time distribution.
#[derive(Debug, Clone)]
pub struct FrameBench {
    /// Frames to record.
    pub frames: u32,
    /// The camera the path is relative to (the board's opening view).
    pub base: rizref_core::Camera,
    /// Per-frame work time: the frame minus its swapchain wait.
    pub work: Vec<Duration>,
    /// Per-frame total time, swapchain wait included.
    pub total: Vec<Duration>,
    /// Sum of items instanced across the recorded frames.
    pub drawn: u64,
}

impl FrameBench {
    /// A bench of `frames` frames around `base`.
    pub fn new(frames: u32, base: rizref_core::Camera) -> Self {
        Self {
            frames: frames.max(1),
            base,
            work: Vec::with_capacity(frames as usize),
            total: Vec::with_capacity(frames as usize),
            drawn: 0,
        }
    }

    /// The camera for frame `index`: one full circle of panning by half a
    /// viewport, while the zoom swings between a quarter and four times the
    /// base. Deterministic, so two runs draw the same frames.
    pub fn camera(&self, index: u32, viewport: glam::DVec2) -> rizref_core::Camera {
        let t = f64::from(index) / f64::from(self.frames);
        let angle = t * std::f64::consts::TAU;
        let mut camera = self.base;
        let zoom = self.base.zoom * 4.0_f64.powf((angle * 2.0).sin());
        camera.zoom = rizref_core::Camera::clamp_zoom(zoom);
        let radius = viewport * 0.5 / camera.zoom;
        camera.eye =
            self.base.eye + glam::DVec2::new(angle.cos() * radius.x, angle.sin() * radius.y);
        camera
    }

    /// Records one frame. Returns `true` once every frame is in.
    pub fn record(&mut self, total: Duration, acquire: Duration, drawn: usize) -> bool {
        self.total.push(total);
        self.work.push(total.saturating_sub(acquire));
        self.drawn += drawn as u64;
        self.is_done()
    }

    /// Whether the bench has recorded every frame.
    pub fn is_done(&self) -> bool {
        self.work.len() >= self.frames as usize
    }

    /// The JSON report: work and total p50/p95/max in ms, plus the mean
    /// number of items drawn per frame and the board-load time when known.
    pub fn to_json(&self, items: usize, load: Option<Duration>) -> String {
        let p = |samples: &[Duration], q: f64| percentile(samples, q).map_or(0.0, ms);
        let max = |samples: &[Duration]| samples.iter().max().copied().map_or(0.0, ms);
        let frames = self.work.len().max(1) as f64;
        format!(
            "{{\"frames\":{},\"items\":{items},\"drawn_mean\":{:.1},\"work_p50_ms\":{:.3},\"work_p95_ms\":{:.3},\"work_max_ms\":{:.3},\"total_p50_ms\":{:.3},\"total_p95_ms\":{:.3},\"total_max_ms\":{:.3},\"board_load_ms\":{}}}",
            self.work.len(),
            self.drawn as f64 / frames,
            p(&self.work, 50.0),
            p(&self.work, 95.0),
            max(&self.work),
            p(&self.total, 50.0),
            p(&self.total, 95.0),
            max(&self.total),
            load.map_or_else(|| "null".to_owned(), |d| format!("{:.3}", ms(d))),
        )
    }
}

/// Milliseconds as a float, for display.
pub fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The `F3` overlay line: frame times, the drawn/total item count, resident
/// textures and the decode queue.
pub fn dev_overlay_line(
    stats: &FrameStats,
    drawn: usize,
    items: usize,
    textures: usize,
    texture_bytes: u64,
    decode_queue: usize,
) -> String {
    let frame = |d: Option<Duration>| d.map_or_else(|| "–".to_owned(), |d| format!("{:.1}", ms(d)));
    format!(
        "frame {} ms · p95 {} · max {} · drawn {drawn}/{items} · tex {textures} {:.0} MB · decode {decode_queue}",
        frame(stats.last()),
        frame(stats.percentile(95.0)),
        frame(stats.max()),
        texture_bytes as f64 / (1024.0 * 1024.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms_(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn milestones_are_stamped_once_relative_to_start() {
        let start = Instant::now();
        let mut clock = StartupClock::new(start);

        assert!(clock.mark_at(Milestone::Window, start + ms_(40)));
        assert!(
            !clock.mark_at(Milestone::Window, start + ms_(90)),
            "the first stamp wins"
        );
        clock.mark_at(Milestone::FirstFrame, start + ms_(200));

        assert_eq!(clock.elapsed(Milestone::Window), Some(ms_(40)));
        assert_eq!(clock.elapsed(Milestone::Gpu), None);
        assert_eq!(
            clock.to_json(),
            "{\"window_ms\":40.000,\"first_frame_ms\":200.000}"
        );
        assert_eq!(
            clock.summary(),
            "startup ms: window 40.0, first_frame 200.0"
        );
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let samples: Vec<Duration> = (1..=100).map(ms_).collect();

        assert_eq!(percentile(&samples, 50.0), Some(ms_(50)));
        assert_eq!(percentile(&samples, 95.0), Some(ms_(95)));
        assert_eq!(percentile(&samples, 100.0), Some(ms_(100)));
        assert_eq!(percentile(&samples, 0.0), Some(ms_(1)));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn the_frame_window_rolls_and_remembers_the_last_frame() {
        let mut stats = FrameStats::default();
        assert!(stats.is_empty());
        assert_eq!(stats.last(), None);

        for n in 0..(FRAME_WINDOW as u64 + 10) {
            stats.record(ms_(n));
        }

        assert_eq!(stats.len(), FRAME_WINDOW);
        assert_eq!(stats.total(), FRAME_WINDOW as u64 + 10);
        assert_eq!(stats.last(), Some(ms_(FRAME_WINDOW as u64 + 9)));
        assert_eq!(stats.max(), Some(ms_(FRAME_WINDOW as u64 + 9)));
        // The ten oldest frames left the window.
        assert_eq!(percentile(&stats.samples, 0.0), Some(ms_(10)));
    }

    #[test]
    fn the_bench_path_is_deterministic_and_returns_home() {
        let base = rizref_core::Camera::new(glam::DVec2::new(10.0, 20.0), 1.0, 0.0);
        let bench = FrameBench::new(240, base);
        let viewport = glam::DVec2::new(800.0, 600.0);

        let a = bench.camera(60, viewport);
        let b = bench.camera(60, viewport);
        assert_eq!((a.eye, a.zoom), (b.eye, b.zoom));
        let home = bench.camera(0, viewport);
        assert!((home.zoom - 1.0).abs() < 1e-9);
        assert!((home.eye - glam::DVec2::new(410.0, 20.0)).length() < 1e-6);
        // The zoom swings to four times the base and back within the run.
        let peak = (0..240)
            .map(|i| bench.camera(i, viewport).zoom)
            .fold(0.0, f64::max);
        assert!((peak - 4.0).abs() < 0.05, "peak zoom {peak}");
    }

    #[test]
    fn the_bench_subtracts_swapchain_waits_and_reports_json() {
        let mut bench = FrameBench::new(2, rizref_core::Camera::default());
        assert!(!bench.record(ms_(16), ms_(14), 10));
        assert!(bench.record(ms_(17), ms_(13), 30));

        let json = bench.to_json(500, Some(ms_(250)));

        assert!(json.contains("\"frames\":2"), "{json}");
        assert!(json.contains("\"drawn_mean\":20.0"), "{json}");
        assert!(json.contains("\"work_max_ms\":4.000"), "{json}");
        assert!(json.contains("\"total_p50_ms\":16.000"), "{json}");
        assert!(json.contains("\"board_load_ms\":250.000"), "{json}");
    }

    #[test]
    fn the_overlay_line_names_every_counter() {
        let mut stats = FrameStats::default();
        stats.record(Duration::from_micros(2_500));

        let line = dev_overlay_line(&stats, 37, 500, 40, 96 * 1024 * 1024, 3);

        assert_eq!(
            line,
            "frame 2.5 ms · p95 2.5 · max 2.5 · drawn 37/500 · tex 40 96 MB · decode 3"
        );
        assert!(dev_overlay_line(&FrameStats::default(), 0, 0, 0, 0, 0).starts_with("frame – ms"));
    }
}
