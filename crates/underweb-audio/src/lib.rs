//! Audio.
//!
//! A package renders sound in its own sandboxed process (the [`engine`]:
//! oscillators, Pure Data, or the package's own callback) and writes it
//! into an [`AudioRing`] in shared memory. The browser reads every
//! playing package's ring, mixes them ([`mixer`]) and plays the result.
//! Packages never touch the sound card.

pub mod ring;
pub mod ugens;

#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "engine")]
pub mod pd;

pub mod mixer;

pub use ring::AudioRing;

/// Frames per render block (Pd's block size).
pub const BLOCK: usize = 64;
/// All rings are stereo.
pub const CHANNELS: usize = 2;
