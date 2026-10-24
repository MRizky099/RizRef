//! The GPU renderer: backends, and where the surface will live.
//!
//! Phase 00 pins the backend selection and nothing else. The surface, the clear
//! pass, the item pipeline and the frame pacing are phase 01 and phase 02 work;
//! what matters now is that the choice of backends is written down in one place
//! and is visible in the startup summary, because "which backend did it pick?"
//! is the first question in every rendering bug report.

/// Backends this build tries, in preference order.
///
/// wgpu takes the flags as a set and picks the first backend that produces an
/// adapter, so ordering here is documentation rather than control: Vulkan first
/// because it is the thinnest path on Windows, DX12 second because it is the one
/// guaranteed to be present on Windows 10 and later.
///
/// GL is deliberately absent. Enabling `wgpu/gles` pulls `glutin_wgl_sys`, a
/// `-sys` crate outside the accepted Windows FFI set, which the pure-Rust rule
/// does not allow. Phase 01 has to settle that — either accept that crate (it
/// declares WGL and links nothing beyond the system `opengl32`) or drop the GL
/// fallback and rely on Vulkan and DX12. See the dependency audit in the
/// repository README.
pub fn preferred_backends() -> wgpu::Backends {
    wgpu::Backends::VULKAN | wgpu::Backends::DX12
}

/// Renders a backend set as the arrow-joined names a bug report should quote.
pub fn describe_backends(backends: wgpu::Backends) -> String {
    let ordered = [
        (wgpu::Backends::VULKAN, "vulkan"),
        (wgpu::Backends::DX12, "dx12"),
        (wgpu::Backends::GL, "gl"),
        (wgpu::Backends::METAL, "metal"),
        (wgpu::Backends::BROWSER_WEBGPU, "webgpu"),
    ];

    let names: Vec<&str> = ordered
        .iter()
        .filter(|(flag, _)| backends.contains(*flag))
        .map(|(_, name)| *name)
        .collect();

    if names.is_empty() {
        // wgpu treats an empty set as "everything this platform offers".
        "auto".to_owned()
    } else {
        names.join(" -> ")
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_backends, preferred_backends};

    #[test]
    fn this_build_prefers_vulkan_then_dx12() {
        assert_eq!(describe_backends(preferred_backends()), "vulkan -> dx12");
    }

    #[test]
    fn the_gl_fallback_is_not_compiled_in() {
        assert!(
            !preferred_backends().contains(wgpu::Backends::GL),
            "GL requires a disallowed -sys crate; see the dependency audit"
        );
    }

    #[test]
    fn an_empty_set_reads_as_auto() {
        assert_eq!(describe_backends(wgpu::Backends::empty()), "auto");
    }

    #[test]
    fn unknown_combinations_still_describe_themselves() {
        assert_eq!(
            describe_backends(wgpu::Backends::METAL),
            "metal",
            "a backend this build does not use must still be nameable"
        );
    }
}
