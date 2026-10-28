//! Animated images on the board.
//!
//! An animation is a list of composed frames with their own timings, shared by
//! every item that shows the same asset. Playback is a *query* —
//! [`Animation::frame_at`] answers "which frame is due at elapsed time `t`?" —
//! so the model holds no clock, no timer and no mutable state, and the
//! application decides when (or whether) time moves.

use std::time::Duration;

use rizref_codecs::ImageBuffer;

/// One frame and how long it stays on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationFrame {
    image: ImageBuffer,
    delay: Duration,
}

impl AnimationFrame {
    /// A frame of `image` shown for `delay`.
    pub fn new(image: ImageBuffer, delay: Duration) -> Self {
        Self { image, delay }
    }

    /// The frame's pixels.
    pub fn image(&self) -> &ImageBuffer {
        &self.image
    }

    /// How long the frame stays on screen.
    pub fn delay(&self) -> Duration {
        self.delay
    }
}

/// A composed, timed animation shared through an [`Asset`](crate::Asset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    frames: Vec<AnimationFrame>,
    total: Duration,
}

impl Animation {
    /// Builds an animation from its frames, or `None` when there are none.
    ///
    /// An animation with no frames has no first frame, so it cannot stand in
    /// for the asset's still image and must be refused at construction.
    pub fn new(frames: Vec<AnimationFrame>) -> Option<Self> {
        if frames.is_empty() {
            return None;
        }
        let total = frames.iter().map(AnimationFrame::delay).sum();
        Some(Self { frames, total })
    }

    /// The frames in playback order.
    pub fn frames(&self) -> &[AnimationFrame] {
        &self.frames
    }

    /// Number of frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the animation has no frames; never true for a constructed value.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// One full pass through every frame.
    pub fn total_duration(&self) -> Duration {
        self.total
    }

    /// The frame shown at `elapsed` into playback, looping forever.
    ///
    /// Animations on a reference board loop: a GIF that plays once and stops
    /// would be useless to study, and every viewer shows them looping.
    pub fn frame_at(&self, elapsed: Duration) -> usize {
        if self.total.is_zero() {
            return 0;
        }
        // `Duration` has no `%`, so work in nanoseconds. Playback spans hours
        // at most, and 64-bit nanoseconds cover 584 years.
        let total_ns = self.total.as_nanos().max(1);
        let mut remaining = elapsed.as_nanos() % total_ns;
        for (index, frame) in self.frames.iter().enumerate() {
            let delay = frame.delay().as_nanos();
            if remaining < delay {
                return index;
            }
            remaining -= delay;
        }
        // Only reachable when rounding left `remaining` at the total.
        self.frames.len() - 1
    }

    /// The frame with `index`, wrapping into range.
    pub fn frame(&self, index: usize) -> &AnimationFrame {
        &self.frames[index % self.frames.len()]
    }

    /// When the frame after `index` becomes due.
    pub fn delay_after(&self, index: usize) -> Duration {
        self.frame(index).delay()
    }
}

#[cfg(test)]
mod tests {
    use super::{Animation, AnimationFrame};
    use rizref_codecs::ImageBuffer;
    use std::time::Duration;

    fn frame(value: u8, delay_ms: u64) -> AnimationFrame {
        AnimationFrame::new(
            ImageBuffer::filled(1, 1, [value, value, value, 255]),
            Duration::from_millis(delay_ms),
        )
    }

    fn animation() -> Animation {
        // 100 + 200 + 300 ms.
        Animation::new(vec![frame(1, 100), frame(2, 200), frame(3, 300)]).expect("three frames")
    }

    #[test]
    fn an_animation_needs_at_least_one_frame() {
        assert!(Animation::new(Vec::new()).is_none());
    }

    #[test]
    fn the_total_duration_is_the_sum_of_the_frames() {
        let animation = animation();

        assert_eq!(animation.len(), 3);
        assert!(!animation.is_empty());
        assert_eq!(animation.total_duration(), Duration::from_millis(600));
    }

    #[test]
    fn frames_are_due_at_their_own_offsets() {
        let animation = animation();

        assert_eq!(animation.frame_at(Duration::ZERO), 0);
        assert_eq!(animation.frame_at(Duration::from_millis(99)), 0);
        assert_eq!(animation.frame_at(Duration::from_millis(100)), 1);
        assert_eq!(animation.frame_at(Duration::from_millis(299)), 1);
        assert_eq!(animation.frame_at(Duration::from_millis(300)), 2);
        assert_eq!(animation.frame_at(Duration::from_millis(599)), 2);
    }

    #[test]
    fn playback_loops() {
        let animation = animation();

        assert_eq!(animation.frame_at(Duration::from_millis(600)), 0);
        assert_eq!(animation.frame_at(Duration::from_millis(700)), 1);
        // 2 100 ms is 300 ms into the second pass, which is the third frame.
        assert_eq!(animation.frame_at(Duration::from_millis(2_100)), 2);
    }

    #[test]
    fn frame_lookup_wraps_instead_of_panicking() {
        let animation = animation();

        assert_eq!(animation.frame(4).delay(), Duration::from_millis(200));
        assert_eq!(animation.frame(3).delay(), Duration::from_millis(100));
    }

    #[test]
    fn a_zero_length_animation_stays_on_its_first_frame() {
        let still = Animation::new(vec![frame(7, 0)]).expect("one frame");

        assert_eq!(still.frame_at(Duration::from_secs(10)), 0);
        assert_eq!(still.total_duration(), Duration::ZERO);
    }
}
