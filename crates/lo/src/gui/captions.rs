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
    /// What you said (rendered on top, uppercase grotesk).
    pub you: String,
    /// Lo's reply (rendered below, warm serif).
    pub lo: String,
    /// Shared opacity, 0..1.
    pub fade: f32,
}

impl Captions {
    /// An empty, fully-faded caption block.
    pub fn new() -> Self {
        Captions {
            you: String::new(),
            lo: String::new(),
            fade: 0.0,
        }
    }
}

impl Default for Captions {
    fn default() -> Self {
        Captions::new()
    }
}

/// Collapse runs of whitespace and trim, matching `captions.ts`'s `tokenize`
/// join — keeps the rendered text tidy regardless of streaming chunk boundaries.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Multiply a colour's alpha by `fade` (0..1) for the cross-fade.
fn faded(color: Color32, fade: f32) -> Color32 {
    let a = (color.a() as f32 * fade.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// Draw the captions centred near the bottom of the screen, fading with
/// `caps.fade`. No-op (other than reserving the area) when both lines are empty.
pub fn draw(ctx: &egui::Context, caps: &Captions) {
    if caps.fade <= 0.001 {
        return;
    }
    let you = normalize(&caps.you);
    let lo = normalize(&caps.lo);
    if you.is_empty() && lo.is_empty() {
        return;
    }

    let screen = ctx.screen_rect();
    // Width clamps to min(76ch, 84vw) in the CSS; approximate with 84% width capped.
    let max_width = (screen.width() * 0.84).min(760.0);

    // --ink-dim (#b9a9a6) for the "you" line; --ink (#f6efe9) for Lo's reply.
    let ink_dim = Color32::from_rgb(0xb9, 0xa9, 0xa6);
    let ink = Color32::from_rgb(0xf6, 0xef, 0xe9);

    // Lo's serif scales like clamp(22px, 3.6vw, 40px).
    let lo_size = (screen.width() * 0.036).clamp(22.0, 40.0);

    // Anchor the block 13vh above the screen bottom (CSS `bottom: 13vh`).
    egui::Area::new(egui::Id::new("lo-captions"))
        .anchor(
            egui::Align2::CENTER_BOTTOM,
            egui::vec2(0.0, -(screen.height() * BOTTOM_FRACTION)),
        )
        .interactable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(max_width);
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                if !you.is_empty() {
                    // Uppercase, letter-spaced grotesk; the spacing is faked by
                    // inserting thin spaces between characters of the upper text.
                    let upper = spaced_upper(&you);
                    ui.label(
                        RichText::new(upper)
                            .font(FontId::proportional(14.0))
                            .color(faded(ink_dim, caps.fade)),
                    );
                    ui.add_space(12.0);
                }
                if !lo.is_empty() {
                    ui.label(
                        RichText::new(lo)
                            .font(FontId::proportional(lo_size))
                            .color(faded(ink, caps.fade)),
                    );
                }
            });
        });
}
