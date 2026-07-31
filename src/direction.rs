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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_reverse_matches_backward_directions() {
        assert!(is_reverse(RgbDirection::CounterClockwise));
        assert!(is_reverse(RgbDirection::Down));
        assert!(is_reverse(RgbDirection::Gather));
        assert!(!is_reverse(RgbDirection::Clockwise));
        assert!(!is_reverse(RgbDirection::Up));
        assert!(!is_reverse(RgbDirection::Spread));
    }

    #[test]
    fn effective_reverse_xors_with_invert_flag() {
        assert!(!effective_reverse(RgbDirection::Clockwise, false));
        assert!(effective_reverse(RgbDirection::Clockwise, true));
        assert!(effective_reverse(RgbDirection::CounterClockwise, false));
        assert!(!effective_reverse(RgbDirection::CounterClockwise, true));
    }

    #[test]
    fn direction_label_cw_ccw_are_not_translated() {
        assert_eq!(direction_label(RgbDirection::Clockwise, Lang::En), "CW");
        assert_eq!(direction_label(RgbDirection::Clockwise, Lang::PtBr), "CW");
        assert_eq!(direction_label(RgbDirection::CounterClockwise, Lang::En), "CCW");
    }

    #[test]
    fn direction_label_covers_every_direction_in_both_languages() {
        for &d in ALL_DIRECTIONS.iter() {
            assert!(!direction_label(d, Lang::En).is_empty(), "missing En label for {d:?}");
            assert!(!direction_label(d, Lang::PtBr).is_empty(), "missing PtBr label for {d:?}");
        }
    }

    #[test]
    fn wave_directions_is_a_subset_of_all_directions() {
        for &d in WAVE_DIRECTIONS.iter() {
            assert!(ALL_DIRECTIONS.contains(&d));
        }
    }
}
