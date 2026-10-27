//! Dev-only persistence of the view transform.
//!
//! Phase 02's acceptance check is "the camera survives a restart". The real
//! answer is the `.rizref` container in phase 06, which stores the view
//! transform inside the scene; until then a small JSON file under the RizRef
//! data directory carries the camera across runs. This is a development
//! affordance, not a save format: nothing here is versioned, migrated or
//! validated beyond serde, and phase 06 deletes this module.

use std::path::PathBuf;

use rizref_core::Camera;
use serde::{Deserialize, Serialize};

/// The camera as it is written to disk.
#[derive(Debug, Serialize, Deserialize)]
struct CameraDump {
    eye: [f64; 2],
    zoom: f64,
    rotation: f64,
}

/// Where the dev state lives: `%LOCALAPPDATA%\RizRef\dev\view.json`, or the
/// temp directory when the environment does not provide app data (the same
/// fallback the logs use).
fn state_file() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rizref"))
        .join("RizRef")
        .join("dev")
        .join("view.json")
}

/// Writes the camera out. Failures are logged, never fatal: a dev checkpoint
/// that cannot be written must not take the window down with it.
pub fn store(camera: &Camera) {
    let dump = CameraDump {
        eye: [camera.eye.x, camera.eye.y],
        zoom: camera.zoom,
        rotation: camera.rotation,
    };
    let path = state_file();
    let body = match serde_json::to_string_pretty(&dump) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(%error, "cannot serialise the dev camera state");
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::debug!(%error, "cannot create the dev state directory");
        return;
    }
    match std::fs::write(&path, body) {
        Ok(()) => tracing::debug!(path = %path.display(), "stored the dev camera state"),
        Err(error) => tracing::debug!(%error, "cannot write the dev camera state"),
    }
}

/// Loads the camera from the last `store`, or `None` when there is none yet.
pub fn load() -> Option<Camera> {
    let body = std::fs::read_to_string(state_file()).ok()?;
    let dump: CameraDump = serde_json::from_str(&body).ok()?;
    Some(Camera::new(
        glam::DVec2::new(dump.eye[0], dump.eye[1]),
        dump.zoom,
        dump.rotation,
    ))
}

#[cfg(test)]
mod tests {
    use super::{CameraDump, state_file};
    use glam::DVec2;
    use rizref_core::Camera;

    #[test]
    fn the_dump_round_trips_through_json() {
        let camera = Camera::new(DVec2::new(-12.5, 340.0), 2.75, -0.523);
        let dump = CameraDump {
            eye: [camera.eye.x, camera.eye.y],
            zoom: camera.zoom,
            rotation: camera.rotation,
        };
        let body = serde_json::to_string(&dump).expect("serialise");
        let back: CameraDump = serde_json::from_str(&body).expect("parse");

        assert_eq!(back.eye, [-12.5, 340.0]);
        assert_eq!(back.zoom, 2.75);
        assert_eq!(back.rotation, -0.523);
    }

    #[test]
    fn the_state_file_lives_under_the_rizref_data_directory() {
        let path = state_file();
        assert!(path.ends_with("view.json"));
        assert!(path.components().any(|c| c.as_os_str() == "RizRef"));
    }
}
