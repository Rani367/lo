//! Live captions — the spoken conversation rendered as light.
//!
//! Two layers float beneath the core: what you said (a quiet uppercase grotesk
//! line) and Lo's reply (a warm serif line below it). Ported from
//! `src/renderer/ui/captions.ts` + the `.cap-you` / `.cap-lo` rules in
//! `styles.css`. This is a plain display struct the orchestrator mutates; the
//! egui draw lives in [`draw`].

use egui::{Align, Color32, FontId, Layout, RichText};

/// Default screen offset (fraction of height) the caption block sits above the
/// bottom edge — mirrors the CSS `bottom: 13vh`.
const BOTTOM_FRACTION: f32 = 0.13;

/// The two caption lines plus a shared fade alpha. Word-by-word reveal is a
/// nice-to-have; v1 uses a clean cross-fade driven by `fade`.
#[derive(Debug, Clone)]
pub struct Captions {
