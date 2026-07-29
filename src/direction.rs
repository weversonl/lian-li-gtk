//! Shared `RgbDirection` labels/helpers for the RGB Editor and Global Effects.

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

pub fn direction_label(d: RgbDirection, lang: Lang) -> &'static str {
    match d {
        RgbDirection::Clockwise => "CW",
        RgbDirection::CounterClockwise => "CCW",
        RgbDirection::Up => crate::i18n::t(lang, "direction.up"),
        RgbDirection::Down => crate::i18n::t(lang, "direction.down"),
        RgbDirection::Spread => crate::i18n::t(lang, "direction.spread"),
        RgbDirection::Gather => crate::i18n::t(lang, "direction.gather"),
    }
}

/// Whether `d` runs the wave backwards relative to the default flow — the
/// 6 labels are all just one bit under the hood since host-rendered frames
/// have no real hardware direction to interpret.
pub fn is_reverse(d: RgbDirection) -> bool {
    matches!(d, RgbDirection::CounterClockwise | RgbDirection::Down | RgbDirection::Gather)
}

/// `is_reverse` with a per-device correction on top — see
/// `DeviceRgbPrefs::invert_direction`.
pub fn effective_reverse(d: RgbDirection, invert: bool) -> bool {
    is_reverse(d) ^ invert
}

pub const WAVE_DIRECTIONS: [RgbDirection; 4] = [
    RgbDirection::Up,
    RgbDirection::Down,
    RgbDirection::CounterClockwise,
    RgbDirection::Clockwise,
];

pub fn wave_direction_label(d: RgbDirection, lang: Lang) -> &'static str {
    direction_label(d, lang)
}
