//! Shared `RgbDirection` labels/helpers — used by both the per-device RGB
//! Editor and Global Effects' per-device orientation overrides.

use crate::app_prefs::Lang;
use lianli_shared::rgb::RgbDirection;

pub const ALL_DIRECTIONS: [RgbDirection; 6] = [
    RgbDirection::Clockwise,
    RgbDirection::CounterClockwise,
    RgbDirection::Up,
    RgbDirection::Down,
    RgbDirection::Spread,
    RgbDirection::Gather,
];

// Short labels on purpose: 6 buttons in one row already push a row's title
// column tight, and "Counter-Clockwise" was forcing "Direction" to
// hyphen-wrap into "Direc-tion" for lack of space.
pub fn direction_label(d: RgbDirection, lang: Lang) -> &'static str {
    match d {
        // CW/CCW are initialisms, not words — they don't have a distinct
        // Portuguese abbreviation in common use, so they stay as-is in
        // both languages (same treatment as e.g. "RGB" elsewhere).
        RgbDirection::Clockwise => "CW",
        RgbDirection::CounterClockwise => "CCW",
        RgbDirection::Up => crate::i18n::t(lang, "direction.up"),
        RgbDirection::Down => crate::i18n::t(lang, "direction.down"),
        RgbDirection::Spread => crate::i18n::t(lang, "direction.spread"),
        RgbDirection::Gather => crate::i18n::t(lang, "direction.gather"),
    }
}

/// Whether `d` means "run the wave backwards" relative to the default flow.
///
/// The wireless Rainbow wave is fundamentally a 1D gradient (`RgbDirection`
/// doesn't carry a real hardware meaning for host-rendered frames — there's
/// no firmware to interpret it), so all 6 labels collapse to one bit here:
/// the direction the gradient runs along a device's LEDs. Offering the full
/// 6-label set anyway (instead of a plain Forward/Reverse) matters because
/// the user picks whichever term matches how that specific piece of
/// hardware is physically mounted — a fan reads naturally as CW/CCW, a
/// Strimer cable as Up/Down — even though under the hood it's the same
/// forward/backward choice either way.
pub fn is_reverse(d: RgbDirection) -> bool {
    matches!(d, RgbDirection::CounterClockwise | RgbDirection::Down | RgbDirection::Gather)
}

/// Global Effects' per-device orientation override is honestly just one bit
/// (see `is_reverse`'s doc comment) — Up/Down plus the same CW/CCW pair the
/// individual RGB Editor uses (`ALL_DIRECTIONS`/`direction_label`), rather
/// than the earlier Left/Right relabeling: that read as inconsistent once
/// both pages were actually side by side (same underlying value, different
/// label depending which page you were on), including for fans, where
/// CW/CCW is the more natural way to describe it anyway.
pub const WAVE_DIRECTIONS: [RgbDirection; 4] = [
    RgbDirection::Up,
    RgbDirection::Down,
    RgbDirection::CounterClockwise,
    RgbDirection::Clockwise,
];

pub fn wave_direction_label(d: RgbDirection, lang: Lang) -> &'static str {
    direction_label(d, lang)
}
