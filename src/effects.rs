//! Client-rendered animated effects for wireless devices — the daemon only
//! ever pushes a solid color to them, so animation is host-rendered here
//! and streamed via `SetRgbFrames`.
//!
//! Speed and smoothness are independent: speed sets cycle duration
//! (`cycle_ms`); smoothness is a literal FPS value, and `frame_count`
//! follows from `fps * cycle_seconds`.

/// Percentage (0-100) → `RgbEffect.speed` (0-4), for wired firmware.
pub fn percent_to_speed4(percent: f64) -> u8 {
    ((percent / 100.0) * 4.0).round().clamp(0.0, 4.0) as u8
}

pub fn percent_to_brightness4(percent: f64) -> u8 {
    percent_to_speed4(percent)
}

/// Rainbow and Rainbow Morph auto-cycle the full hue wheel, so their color
/// picker(s) are ignored — everything else defaults to `true`.
pub fn mode_uses_color(mode: lianli_shared::rgb::RgbMode) -> bool {
    !matches!(mode, lianli_shared::rgb::RgbMode::Rainbow | lianli_shared::rgb::RgbMode::RainbowMorph)
}

/// Wireless `Static`/`Direct` pushes ignore `RgbEffect.brightness` on the
/// daemon side, so brightness has to be baked into the color here instead.
pub fn scale_color(color: [u8; 3], factor: f64) -> [u8; 3] {
    let factor = factor.clamp(0.0, 1.0);
    [
        (color[0] as f64 * factor).round() as u8,
        (color[1] as f64 * factor).round() as u8,
        (color[2] as f64 * factor).round() as u8,
    ]
}

/// Percentage (0-100) → cycle duration in ms. 0% ≈ 6s, 100% ≈ 1s.
pub fn percent_to_cycle_ms(percent: f64) -> f64 {
    let t = (percent / 100.0).clamp(0.0, 1.0);
    6000.0 - t * 5100.0
}

/// FPS → per-frame interval in ms, floored at 8ms (125fps).
pub fn fps_to_interval_ms(fps: f64) -> u16 {
    ((1000.0 / fps.max(1.0)).round() as u16).max(8)
}

/// Frames needed for `cycle_ms` at `fps`, clamped to bound payload size.
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

/// Moving rainbow gradient across the strip. `strip_count` is how many
/// physical LED strips this device's flat buffer is concatenated from —
/// see `strip_position`. `1` treats the buffer as one continuous strip.
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

/// Samples a gradient (`colors`, in order) at position `t` (wrapped into
/// `[0, 1)`). Not auto-looped — repeat the first color at the end yourself
/// for a seamless wrap.
fn sample_gradient(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    if colors.len() < 2 {
        return colors.first().copied().unwrap_or([0, 0, 0]);
    }
    let scaled = wrap01(t) * (colors.len() - 1) as f32;
    let i = (scaled.floor() as usize).min(colors.len() - 2);
    lerp_color(colors[i], colors[i + 1], scaled - i as f32)
}

/// A moving wave through a user-picked color list instead of the full hue
/// wheel. Same strip-count/direction handling as `rainbow_frames`.
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

/// `(start, end)` LED-index ranges for `strip_count` physical strips
/// concatenated in one flat buffer, distributed as evenly as possible.
fn strip_bounds(led_count: usize, strip_count: usize) -> Vec<usize> {
    let strip_count = strip_count.max(1).min(led_count.max(1));
    (0..=strip_count).map(|i| i * led_count / strip_count).collect()
}

/// Where LED `led` sits along its own strip's length, as a 0..1 fraction —
/// not its position in the flat buffer. Keeps every strip in sync instead
/// of each one playing its own independent slice of the gradient.
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

/// One continuous rainbow gradient spanning every device's combined LED
/// count (in the given order), sliced back into per-device frame
/// sequences — so the band flows from one device's strip into the next
/// instead of each device looping its own independent gradient.
///
/// `reverse` sets the global travel direction; `device_reversed` lets
/// individual devices flip their own slice on top of that (for hardware
/// mounted backwards relative to the rest). `device_strip_counts` is each
/// device's physical strip count (see `strip_position`).
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

/// Wraps a float into `[0, 1)`, staying positive for negative inputs.
fn wrap01(x: f32) -> f32 {
    ((x % 1.0) + 1.0) % 1.0
}

/// Every LED the same color, cycling through the full hue wheel together.
pub fn rainbow_morph_frames(led_count: usize, frame_count: usize, brightness: f32) -> Vec<Vec<[u8; 3]>> {
    (0..frame_count)
        .map(|frame| {
            let hue = frame as f32 / frame_count as f32;
            vec![hsv_to_rgb(hue, 1.0, brightness); led_count]
        })
        .collect()
}

/// The chosen color pulsing from dim to `brightness` and back.
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

/// Milliseconds per on/off step of the "Identify" blink.
pub const IDENTIFY_STEP_MS: u16 = 220;

/// 3 blinks at 25% brightness, ending off.
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
