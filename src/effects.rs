//! Client-rendered animated effects for wireless devices.
//!
//! `RgbController::set_effect` (daemon side) only ever pushes a *solid*
//! color to wireless devices regardless of `RgbEffect.mode` — real
//! animation has to be host-rendered here and streamed via `SetRgbFrames`.
//! `GetRgbCapabilities` reflects that honestly (wireless devices report
//! `["Static", "Direct"]` only), so anything beyond those two modes for a
//! wireless device is *this app* generating frames, not a hardware effect
//! being unlocked.
//!
//! Animation "speed" and "smoothness" are two independent knobs, not one:
//! speed is how long a full cycle takes (`cycle_ms`); smoothness is a literal
//! **frames-per-second** value (30fps, 60fps, ...) rather than an abstract
//! percentage — `frame_count` for a given cycle then falls out of
//! `fps * cycle_seconds`. The original version hardcoded a fixed 24-frame
//! cycle, so turning up "speed" only shortened the per-frame interval
//! without adding frames — the wave still jumped in the same 24 big steps,
//! just faster, which reads as choppy compared to L-Connect's finer-grained
//! animation at the same perceived speed.

/// Percentage (0-100) → `RgbEffect.speed` (0-4), for wired devices whose
/// firmware expects the old discrete scale.
pub fn percent_to_speed4(percent: f64) -> u8 {
    ((percent / 100.0) * 4.0).round().clamp(0.0, 4.0) as u8
}

pub fn percent_to_brightness4(percent: f64) -> u8 {
    percent_to_speed4(percent)
}

/// Whether a mode's color picker(s) actually mean anything. Rainbow and
/// Rainbow Morph auto-cycle through the full hue wheel on their own — any
/// color picked for them is ignored, so showing the picker just invited
/// picking a color that visibly does nothing. Everything not listed here
/// defaults to `true` (safer to show an unused picker than to hide one that
/// was actually needed — this only covers the modes this app knows for
/// certain don't use color).
pub fn mode_uses_color(mode: lianli_shared::rgb::RgbMode) -> bool {
    !matches!(mode, lianli_shared::rgb::RgbMode::Rainbow | lianli_shared::rgb::RgbMode::RainbowMorph)
}

/// Scales an RGB color's channels by `factor` (0-1). Used for wireless
/// `Static`/`Direct` pushes, where the daemon just forwards `RgbEffect`'s raw
/// color bytes straight to the device with no brightness scaling of its own
/// — `RgbEffect.brightness` is silently ignored for these, so a 20%
/// brightness pick still lit the LEDs at full raw white. Every other
/// wireless effect (Rainbow, Breathing, ...) already bakes brightness into
/// its frame colors the same way (see `rainbow_wave_frames`'s
/// `brightness_factor` and `breathing_frames`), so this brings Static in
/// line with them instead of trusting a field the firmware doesn't read.
pub fn scale_color(color: [u8; 3], factor: f64) -> [u8; 3] {
    let factor = factor.clamp(0.0, 1.0);
    [
        (color[0] as f64 * factor).round() as u8,
        (color[1] as f64 * factor).round() as u8,
        (color[2] as f64 * factor).round() as u8,
    ]
}

/// Percentage (0-100) → total duration of one full animation cycle, in ms.
/// 0% is a slow 6s crawl; 100% is just under 1s.
pub fn percent_to_cycle_ms(percent: f64) -> f64 {
    let t = (percent / 100.0).clamp(0.0, 1.0);
    6000.0 - t * 5100.0
}

/// A literal FPS value → per-frame interval in ms (just `1000 / fps`),
/// floored at 8ms. Empirically checked against the real daemon/hardware
/// (`SetRgbFrames` on a 176-LED device): 120 frames at 15ms/frame (~66fps)
/// round-trips in ~240ms with no error, so an 8ms floor (125fps) leaves
/// headroom without ever having been pushed past what was actually tested.
pub fn fps_to_interval_ms(fps: f64) -> u16 {
    ((1000.0 / fps.max(1.0)).round() as u16).max(8)
}

/// How many frames a cycle of `cycle_ms` needs at `fps`, so the wave
/// completes the same lap in the same wall-clock time regardless of how
/// many frames that's split into. Clamped to keep the `SetRgbFrames`
/// payload (and upload time) bounded even for a slow cycle at high fps.
pub fn frame_count_for(fps: f64, cycle_ms: f64) -> usize {
    let interval_ms = fps_to_interval_ms(fps) as f64;
    ((cycle_ms / interval_ms).round() as usize).clamp(4, 200)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// A moving rainbow gradient across the strip — the classic "Rainbow" mode.
/// `reverse` flips which end of the strip the wave travels toward.
/// `strip_count` is how many physical LED strips this device's flat LED
/// buffer is actually made of, concatenated strip-by-strip — see
/// `strip_position` for why this matters and how it was confirmed against
/// real hardware. `1` (the default) treats the whole buffer as a single
/// strip, i.e. today's plain continuous gradient.
pub fn rainbow_frames(
    led_count: usize,
    frame_count: usize,
    reverse: bool,
    strip_count: usize,
    brightness: f32,
) -> Vec<Vec<[u8; 3]>> {
    let sign = if reverse { -1.0 } else { 1.0 };
    let bounds = strip_bounds(led_count, strip_count);
    (0..frame_count)
        .map(|frame| {
            let frame_offset = sign * (frame as f32 / frame_count as f32);
            (0..led_count)
                .map(|led| {
                    let pos = strip_position(&bounds, led);
                    let hue = wrap01(pos + frame_offset);
                    hsv_to_rgb(hue, 1.0, brightness)
                })
                .collect()
        })
        .collect()
}

fn lerp_color(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round() as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round() as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round() as u8,
    ]
}

/// Samples a user-defined gradient (`colors`, in order) at position `t`
/// (wrapped into `[0, 1)`) — the same "OpenRGB Custom Gradient Wave"
/// convention: colors are NOT auto-looped, so repeat the first color at the
/// end of the list yourself for a seamless wrap (that's why a 7-color
/// "Unicorn Vomit" preset lists Red...Red — the last Red closes the loop).
fn sample_gradient(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    if colors.len() < 2 {
        return colors.first().copied().unwrap_or([0, 0, 0]);
    }
    let scaled = wrap01(t) * (colors.len() - 1) as f32;
    let i = (scaled.floor() as usize).min(colors.len() - 2);
    lerp_color(colors[i], colors[i + 1], scaled - i as f32)
}

/// A moving wave through a user-picked color list instead of the full hue
/// wheel — e.g. a "Red → Pink → Dark Blue → Light Blue" band cycling across
/// the strip, matching OpenRGB's "Custom Gradient Wave" effect (its "Unicorn
/// Vomit" preset is the same idea with more stops). Same
/// strip-count/direction handling as `rainbow_frames`.
pub fn custom_gradient_wave_frames(
    led_count: usize,
    frame_count: usize,
    colors: &[[u8; 3]],
    reverse: bool,
    strip_count: usize,
    brightness: f32,
) -> Vec<Vec<[u8; 3]>> {
    let sign = if reverse { -1.0 } else { 1.0 };
    let bounds = strip_bounds(led_count, strip_count);
    (0..frame_count)
        .map(|frame| {
            let frame_offset = sign * (frame as f32 / frame_count as f32);
            (0..led_count)
                .map(|led| {
                    let pos = strip_position(&bounds, led);
                    let base = sample_gradient(colors, pos + frame_offset);
                    [
                        (base[0] as f32 * brightness) as u8,
                        (base[1] as f32 * brightness) as u8,
                        (base[2] as f32 * brightness) as u8,
                    ]
                })
                .collect()
        })
        .collect()
}

/// `(start, end)` LED-index ranges for `strip_count` physical strips,
/// distributed as evenly as the LED count allows (e.g. 116 LEDs / 8 strips
/// comes out as an alternating 14/15/14/15/... split — confirmed against
/// real hardware: coloring the buffer in 4 equal quarters showed up as 2
/// solid-colored physical strips per quarter, i.e. this device's buffer is
/// 8 strips concatenated one after another, not one continuous run of 116
/// individually-addressable positions).
fn strip_bounds(led_count: usize, strip_count: usize) -> Vec<usize> {
    let strip_count = strip_count.max(1).min(led_count.max(1));
    (0..=strip_count).map(|i| i * led_count / strip_count).collect()
}

/// Where LED `led` sits along its *own strip's* length, as a 0..1 fraction —
/// not its position in the flat buffer. Every strip reaching the same
/// fraction at the same time is what makes the gradient move in sync across
/// all of them instead of each strip playing its own independent slice of
/// the rainbow (which is what treating the buffer as one continuous run —
/// `strip_count = 1` — does, and is why a merged cable looked "chopped up"
/// rather than one clean band sweeping its full length).
fn strip_position(bounds: &[usize], led: usize) -> f32 {
    for w in bounds.windows(2) {
        let (start, end) = (w[0], w[1]);
        if led >= start && led < end {
            let len = (end - start).max(1);
            return (led - start) as f32 / len as f32;
        }
    }
    0.0
}

/// Builds one continuous rainbow gradient across the combined LED count of
/// every device (in the given order), then slices it back into per-device
/// frame sequences. This is what makes the color band flow off the end of
/// one device's strip straight into the start of the next one — the L-Connect
/// "sync" look — instead of every device independently looping its own
/// gradient (which is what `rainbow_frames` does per-device, and why it
/// looked like unrelated devices spinning their own rainbows rather than
/// one wave crossing between them).
///
/// `reverse` flips which way the band travels (both along the strip and
/// forward/back in time) — the global default orientation. `device_reversed`
/// then lets individual devices flip their own slice's LED order on top of
/// that — physically, some cables/fans are mounted "backwards" relative to
/// the others (a GPU power cable running up rather than across, say), so the
/// same shared wave needs to visually run the other way on just that one
/// device to look continuous. `device_strip_counts` tells each device how
/// many physical LED strips its own LED buffer is made of (see
/// `strip_position`) — every strip on that device reaches the same point in
/// the gradient at the same time, instead of each strip playing its own
/// independent slice of it. `brightness` (0-1) scales the HSV value channel
/// directly.
pub fn rainbow_wave_frames(
    device_led_counts: &[usize],
    frame_count: usize,
    reverse: bool,
    device_reversed: &[bool],
    device_strip_counts: &[usize],
    brightness: f32,
) -> Vec<Vec<Vec<[u8; 3]>>> {
    let total: usize = device_led_counts.iter().sum();
    if total == 0 {
        return device_led_counts.iter().map(|_| Vec::new()).collect();
    }

    let sign = if reverse { -1.0 } else { 1.0 };

    let device_bounds: Vec<Vec<usize>> = device_led_counts
        .iter()
        .enumerate()
        .map(|(i, &count)| strip_bounds(count, device_strip_counts.get(i).copied().unwrap_or(1)))
        .collect();

    let mut device_offsets = Vec::with_capacity(device_led_counts.len());
    let mut acc = 0usize;
    for &count in device_led_counts {
        device_offsets.push(acc);
        acc += count;
    }

    let mut per_device: Vec<Vec<Vec<[u8; 3]>>> = device_led_counts
        .iter()
        .map(|_| Vec::with_capacity(frame_count))
        .collect();

    for frame in 0..frame_count {
        let frame_offset = sign * (frame as f32 / frame_count as f32);
        for (i, &count) in device_led_counts.iter().enumerate() {
            let bounds = &device_bounds[i];
            let device_offset = device_offsets[i] as f32;
            let mut slice: Vec<[u8; 3]> = (0..count)
                .map(|local_led| {
                    let local_pos = strip_position(bounds, local_led);
                    let global_pos = wrap01((device_offset + local_pos * count as f32) / total as f32 + frame_offset);
                    hsv_to_rgb(global_pos, 1.0, brightness)
                })
                .collect();
            if device_reversed.get(i).copied().unwrap_or(false) {
                slice.reverse();
            }
            per_device[i].push(slice);
        }
    }

    per_device
}

/// Wraps a float into `[0, 1)` — unlike `f32::fract`, this stays positive
/// for negative inputs, which the reversed wave direction produces.
fn wrap01(x: f32) -> f32 {
    ((x % 1.0) + 1.0) % 1.0
}

/// Every LED the same color, that color cycling through the full hue wheel
/// together — "Rainbow Morph".
pub fn rainbow_morph_frames(led_count: usize, frame_count: usize, brightness: f32) -> Vec<Vec<[u8; 3]>> {
    (0..frame_count)
        .map(|frame| {
            let hue = frame as f32 / frame_count as f32;
            vec![hsv_to_rgb(hue, 1.0, brightness); led_count]
        })
        .collect()
}

/// The chosen color pulsing from dim to `brightness` and back — "Breathing".
/// `brightness` (0-1) caps the pulse's peak, same knob every other wireless
/// effect already uses — this used to hardcode a 1.0 peak regardless of the
/// user's brightness slider.
pub fn breathing_frames(led_count: usize, frame_count: usize, color: [u8; 3], brightness: f32) -> Vec<Vec<[u8; 3]>> {
    (0..frame_count)
        .map(|frame| {
            let t = frame as f32 / frame_count as f32;
            // Smooth triangle 0..1..0, floor so it never fully goes black.
            let pulse = brightness * (0.15 + 0.85 * (1.0 - (2.0 * t - 1.0).abs()));
            let led = [
                (color[0] as f32 * pulse) as u8,
                (color[1] as f32 * pulse) as u8,
                (color[2] as f32 * pulse) as u8,
            ];
            vec![led; led_count]
        })
        .collect()
}

/// Milliseconds per on/off step of the "Identify" blink (see `identify.rs`)
/// — 3 blinks at a rate slow enough to clearly count, fast enough not to
/// feel like a wait.
pub const IDENTIFY_STEP_MS: u16 = 220;

/// Off/yellow/off/yellow/off/yellow — 3 blinks, ending off so the restore
/// step that follows doesn't have to fight a still-lit frame. 25% brightness
/// matches this project's default test brightness (bright yellow blinking
/// at full brightness is the kind of thing the user asked us to avoid).
pub fn identify_frames(led_count: usize) -> Vec<Vec<[u8; 3]>> {
    let yellow = [
        (255.0 * 0.25) as u8,
        (255.0 * 0.25) as u8,
        0,
    ];
    let off = [0u8, 0, 0];
    [off, yellow, off, yellow, off, yellow, off]
        .into_iter()
        .map(|color| vec![color; led_count])
        .collect()
}
