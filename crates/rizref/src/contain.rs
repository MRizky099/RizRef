//! Contained panics: `catch_unwind` that the panic hook knows about.
//!
//! The application's panic hook writes a report and exits the process, which
//! is right for a bug in the event loop and wrong for a decoder or the math
//! backend tripping over hostile input inside a deliberate `catch_unwind`.
//! Before phase 07 the hook exited unconditionally, so those guards never got
//! the chance to turn a bad file into an error message: the hook ran first.
//!
//! [`contain`] marks the current thread as inside a containment boundary for
//! the duration of the closure; the hook asks [`is_contained`] and, when it
//! is, logs the panic and returns so unwinding reaches the boundary.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    /// Containment depth on this thread: nested boundaries all count.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Restores the depth when the boundary is left, by return or by unwind.
struct DepthGuard;

impl DepthGuard {
    fn enter() -> Self {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Runs `body`, turning a panic into `Err(message)` instead of a crash.
///
/// The closure is asserted unwind-safe: every caller hands over owned input
/// and discards partial state on failure, which is the contract this wrapper
/// exists for.
pub fn contain<T>(body: impl FnOnce() -> T) -> Result<T, String> {
    let _guard = DepthGuard::enter();
    catch_unwind(AssertUnwindSafe(body)).map_err(|payload| panic_message(payload.as_ref()))
}

/// Whether the current thread is inside a [`contain`] boundary.
pub fn is_contained() -> bool {
    DEPTH.with(Cell::get) != 0
}

/// The human-readable part of a panic payload.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{contain, is_contained};

    #[test]
    fn a_panic_inside_the_boundary_becomes_an_error() {
        let outcome: Result<(), String> = contain(|| panic!("hostile input"));

        assert_eq!(outcome, Err("hostile input".to_owned()));
        assert!(!is_contained(), "the depth unwinds with the panic");
    }

    #[test]
    fn the_depth_is_visible_inside_and_nests() {
        assert!(!is_contained());
        let inner = contain(|| contain(is_contained).expect("no panic"));

        assert_eq!(inner, Ok(true));
        assert!(!is_contained());
    }

    #[test]
    fn a_value_passes_through() {
        assert_eq!(contain(|| 41 + 1), Ok(42));
    }
}
