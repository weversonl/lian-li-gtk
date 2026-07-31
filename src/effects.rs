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

/// How many of the 8 color swatches are meaningful for `mode` — others only
/// ever read `colors[0]`, so showing all 8 for those would be misleading.
pub fn color_count_for_mode(mode: lianli_shared::rgb::RgbMode) -> usize {
    use lianli_shared::rgb::RgbMode;
    match mode {
        RgbMode::ColorCycle => 8,
        RgbMode::Meteor | RgbMode::Runway => 2,
        _ => 1,
    }
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

/// How many tail-only frames a Meteor effect should hold once it reaches
/// the far tip, at `interval_ms` per frame, before looping — L-Connect
/// pauses a few seconds dark between laps instead of restarting instantly.
/// `pause_secs <= 0` means no pause at all (an instant restart).
pub fn meteor_pause_frames(interval_ms: u16, pause_secs: f64) -> usize {
    if pause_secs <= 0.0 {
        return 0;
    }
    ((pause_secs * 1000.0 / interval_ms.max(1) as f64).round() as usize).max(1)
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

/// Classic ARGB "Meteor": a bright head sweeps once across the strip with a
/// fading trail. `circular` picks topology — `false` is a linear one-way
/// sweep (tip to tip, then snaps back); `true` wraps the trail seamlessly
/// (e.g. LEDs around a fan hub). `rainbow_head` cycles the head's hue
/// instead of `head_color`. Every physical strip (`strip_bounds`) plays in
/// sync so a multi-strip cable reads as one wave, not a relay — `strip_count`
/// must match the device's real physical strip count.
///
/// `pause_frames` holds the tail dark between laps. `exit_frames` lets a
/// linear sweep's tail run off the far tip and fully fade before the pause,
/// instead of getting cut off mid-fade (a closed ring needs no extra frames
/// since the trail already fades out naturally by lap's end).
#[allow(clippy::too_many_arguments)]
pub fn meteor_frames(
    led_count: usize,
    frame_count: usize,
    pause_frames: usize,
    head_color: [u8; 3],
    tail_color: [u8; 3],
    rainbow_head: bool,
    reverse: bool,
    strip_count: usize,
    circular: bool,
    brightness: f32,
) -> Vec<Vec<[u8; 3]>> {
    const TAIL_LEN: f32 = 0.35;
    let bounds = strip_bounds(led_count, strip_count);
    let tail = scale_color(tail_color, brightness as f64);
    let exit_frames = if circular { 0 } else { (frame_count as f32 * TAIL_LEN).round() as usize };
    let mut frames: Vec<Vec<[u8; 3]>> = (0..frame_count + exit_frames)
        .map(|frame| {
            let t = frame as f32 / frame_count.max(1) as f32;
            let head_pos = if reverse { 1.0 - t } else { t };
            let head = if rainbow_head {
                hsv_to_rgb(t.min(1.0), 1.0, brightness)
            } else {
                scale_color(head_color, brightness as f64)
            };
            (0..led_count)
                .map(|led| {
                    let pos = strip_position(&bounds, led);
                    let distance = if circular {
                        // Wraps from the last LED back to the first — correct
                        // for a closed ring, where they're physically adjacent.
                        if reverse { wrap01(pos - head_pos) } else { wrap01(head_pos - pos) }
                    } else {
                        // Positive only on the side the head has already swept
                        // past — never wraps to the other tip of the strip.
                        if reverse { pos - head_pos } else { head_pos - pos }
                    };
                    if (0.0..TAIL_LEN).contains(&distance) {
                        lerp_color(tail, head, 1.0 - distance / TAIL_LEN)
                    } else {
                        tail
                    }
                })
                .collect()
        })
        .collect();
    frames.extend((0..pause_frames).map(|_| vec![tail; led_count]));
    frames
}

/// Strip-by-strip Meteor relay: each physical strip (`strip_bounds`) takes
/// its turn sweeping a full lap in sequence, unlike `meteor_frames` where
/// every strip sweeps together. Same tail-exit/`circular` handling as
/// `meteor_frames`, applied per strip's turn.
///
/// `frame_count_per_strip` is each strip's own share of the lap — callers
/// relaying across several strips divide their target turn length by strip
/// count first, so a 3-fan hub's whole turn still matches a 1-strip
/// device's. `reverse` flips both the in-strip sweep direction and which
/// end of the assembly the relay starts from.
#[allow(clippy::too_many_arguments)]
pub fn meteor_chase_frames(
    led_count: usize,
    frame_count_per_strip: usize,
    pause_frames: usize,
    head_color: [u8; 3],
    tail_color: [u8; 3],
    rainbow_head: bool,
    reverse: bool,
    strip_count: usize,
    circular: bool,
    brightness: f32,
) -> Vec<Vec<[u8; 3]>> {
    const TAIL_LEN: f32 = 0.35;
    let bounds = strip_bounds(led_count, strip_count);
    let tail = scale_color(tail_color, brightness as f64);
    let n_strips = bounds.len().saturating_sub(1).max(1);
    let exit_frames = if circular { 0 } else { (frame_count_per_strip as f32 * TAIL_LEN).round() as usize };
    let mut frames: Vec<Vec<[u8; 3]>> =
        Vec::with_capacity(n_strips * (frame_count_per_strip + exit_frames) + pause_frames);
    let windows: Vec<[usize; 2]> = bounds.windows(2).map(|w| [w[0], w[1]]).collect();
    let ordered: Vec<[usize; 2]> = if reverse { windows.into_iter().rev().collect() } else { windows };
    for w in ordered {
        let (start, end) = (w[0], w[1]);
        let strip_len = (end - start).max(1);
        for frame in 0..frame_count_per_strip + exit_frames {
            let t = frame as f32 / frame_count_per_strip.max(1) as f32;
            let head_pos = if reverse { 1.0 - t } else { t };
            let head = if rainbow_head {
                hsv_to_rgb(t.min(1.0), 1.0, brightness)
            } else {
                scale_color(head_color, brightness as f64)
            };
            let mut led_frame = vec![tail; led_count];
            for local in 0..strip_len {
                let pos = local as f32 / strip_len as f32;
                let distance = if circular {
                    if reverse { wrap01(pos - head_pos) } else { wrap01(head_pos - pos) }
                } else if reverse {
                    pos - head_pos
                } else {
                    head_pos - pos
                };
                if (0.0..TAIL_LEN).contains(&distance) {
                    led_frame[start + local] = lerp_color(tail, head, 1.0 - distance / TAIL_LEN);
                }
            }
            frames.push(led_frame);
        }
    }
    frames.extend((0..pause_frames).map(|_| vec![tail; led_count]));
    frames
}

/// Horizontal position of LED `local_index` on its fan ring, as a fraction
/// of the fan's width (`0.0`=left, `1.0`=right). `offset_deg` is the real
/// clock position of `local_index == 0` (0 = 12 o'clock) — fans can be
/// mounted at different rotations, so this is per-device, not assumed
/// (see `DeviceRgbPrefs::ring_offset_deg`).
fn ring_horizontal_fraction(local_index: usize, led_count: usize, offset_deg: f32) -> f32 {
    let angle = std::f32::consts::TAU * local_index as f32 / led_count.max(1) as f32 + offset_deg.to_radians();
    (angle.sin() + 1.0) / 2.0
}

/// Each LED's position on the whole assembly's horizontal axis, in
/// fan-widths. `None` for LEDs past the last real fan (an unpopulated hub
/// port with no physical LEDs).
fn band_positions(total_led_count: usize, zone_led_counts: &[usize], ring_offset_deg: f32) -> Vec<Option<f32>> {
    let mut positions = vec![None; total_led_count];
    let mut offset = 0usize;
    for (fan_i, &n) in zone_led_counts.iter().enumerate() {
        for j in 0..n {
            if offset + j >= total_led_count {
                break;
            }
            positions[offset + j] = Some(fan_i as f32 + ring_horizontal_fraction(j, n, ring_offset_deg));
        }
        offset += n;
    }
    positions
}

/// Fraction of one fan's width the leading (not-yet-reached) side of the
/// band spans before fading to nothing — short and sharp, per the "entrada
/// mais curta e definida" requirement.
const BAND_FRONT_WIDTH: f32 = 0.35;
/// Fraction of one fan's width the trailing (already-passed) tail spans —
/// long and gradual, per "saída gradual, sem apagar repentinamente".
const BAND_BACK_WIDTH: f32 = 0.85;

/// A wide, physically continuous band of light sweeping once across a
/// whole multi-fan assembly, crossing fan-to-fan rather than restarting
/// per fan like `meteor_chase_frames` — every fan's ring is treated as one
/// horizontal surface (`band_positions`), so brightness follows real
/// physical position, not flat buffer index.
///
/// Peaks at exactly `band_color`, no forced-white core; asymmetric falloff
/// (short sharp leading edge, long gradual tail), unlike `meteor_frames`'
/// symmetric tail. `zone_led_counts` must be only the real physical fans —
/// see `band_positions`.
#[allow(clippy::too_many_arguments)]
pub fn meteor_band_frames(
    total_led_count: usize,
    zone_led_counts: &[usize],
    ring_offset_deg: f32,
    frame_count: usize,
    pause_frames: usize,
    head_color: [u8; 3],
    background_color: [u8; 3],
    reverse: bool,
    brightness: f32,
) -> Vec<Vec<[u8; 3]>> {
    let positions = band_positions(total_led_count, zone_led_counts, ring_offset_deg);
    let num_fans = zone_led_counts.len().max(1) as f32;
    let head = scale_color(head_color, brightness as f64);
    let background = scale_color(background_color, brightness as f64);

    let (start_pos, end_pos) = if reverse {
        (num_fans + BAND_FRONT_WIDTH, -BAND_BACK_WIDTH)
    } else {
        (-BAND_FRONT_WIDTH, num_fans + BAND_BACK_WIDTH)
    };

    let mut frames: Vec<Vec<[u8; 3]>> = (0..frame_count)
        .map(|frame| {
            let t = frame as f32 / (frame_count.max(2) - 1) as f32;
            let band_center = start_pos + (end_pos - start_pos) * t;
            (0..total_led_count)
                .map(|led| {
                    let Some(pos) = positions[led] else { return background };
                    let delta = pos - band_center;
                    let body = if (0.0..BAND_FRONT_WIDTH).contains(&delta) {
                        (1.0 - delta / BAND_FRONT_WIDTH).powi(3)
                    } else if (-BAND_BACK_WIDTH..0.0).contains(&delta) {
                        (1.0 - (-delta) / BAND_BACK_WIDTH).powf(1.5)
                    } else {
                        0.0
                    };
                    lerp_color(background, head, body)
                })
                .collect()
        })
        .collect();
    frames.extend((0..pause_frames).map(|_| vec![background; total_led_count]));
    frames
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

/// Meteor relay carried across *devices*: each takes its full turn, in
/// order, before handing off to the next; `pause_frames` holds dark before
/// looping back to the first.
///
/// A device's turn uses `meteor_band_frames` when `device_chase[i]` is set
/// (multi-fan hubs — band sweeps fan-to-fan). Otherwise (e.g. a Strimer
/// cable) uses `meteor_frames`, since chasing strand-by-strand there looked
/// like lighting one thin wire at a time. Pass `has_fan` for `device_chase`.
///
/// All sequences share the same total length so every device stays in
/// step with one `interval_ms` — outside its own turn, a device's sequence
/// is just solid `tail_color`.
#[allow(clippy::too_many_arguments)]
pub fn meteor_relay_across_devices(
    device_led_counts: &[usize],
    device_zone_led_counts: &[Vec<usize>],
    frame_count_per_device: usize,
    pause_frames: usize,
    head_color: [u8; 3],
    tail_color: [u8; 3],
    rainbow_head: bool,
    device_reversed: &[bool],
    device_strip_counts: &[usize],
    device_circular: &[bool],
    device_chase: &[bool],
    device_ring_offset_deg: &[f32],
    brightness: f32,
) -> Vec<Vec<Vec<[u8; 3]>>> {
    let n = device_led_counts.len();
    let tail = scale_color(tail_color, brightness as f64);

    let own_turns: Vec<Vec<Vec<[u8; 3]>>> = (0..n)
        .map(|i| {
            let strip_count = device_strip_counts.get(i).copied().unwrap_or(1).max(1);
            let reverse = device_reversed.get(i).copied().unwrap_or(false);
            let circular = device_circular.get(i).copied().unwrap_or(false);
            if device_chase.get(i).copied().unwrap_or(false) {
                let zones = device_zone_led_counts.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
                let ring_offset_deg = device_ring_offset_deg.get(i).copied().unwrap_or(0.0);
                meteor_band_frames(
                    device_led_counts[i],
                    zones,
                    ring_offset_deg,
                    frame_count_per_device,
                    0,
                    head_color,
                    tail_color,
                    reverse,
                    brightness,
                )
            } else {
                meteor_frames(
                    device_led_counts[i],
                    frame_count_per_device,
                    0,
                    head_color,
                    tail_color,
                    rainbow_head,
                    reverse,
                    strip_count,
                    circular,
                    brightness,
                )
            }
        })
        .collect();

    stagger_across_devices(&own_turns, device_led_counts, pause_frames, tail)
}

/// Interleaves an already-rendered "own turn" frame sequence per device
/// into a device-relay timeline, generalizing `meteor_relay_across_devices`
/// to any effect: each device's sequence is padded with `off_color` frames
/// everywhere except its own turn window, so every device can be sent with
/// the same `interval_ms` and stay in step, one device lighting up at a
/// time in the given order, looping back to the first once every device
/// has had a turn (with `pause_frames` of `off_color` held in between).
pub fn stagger_across_devices(
    own_turns: &[Vec<Vec<[u8; 3]>>],
    device_led_counts: &[usize],
    pause_frames: usize,
    off_color: [u8; 3],
) -> Vec<Vec<Vec<[u8; 3]>>> {
    (0..own_turns.len())
        .map(|i| {
            let led_count = device_led_counts.get(i).copied().unwrap_or(0);
            let mut seq: Vec<Vec<[u8; 3]>> = Vec::new();
            for (j, turn) in own_turns.iter().enumerate() {
                if j == i {
                    seq.extend(turn.iter().cloned());
                } else {
                    seq.extend((0..turn.len()).map(|_| vec![off_color; led_count]));
                }
            }
            seq.extend((0..pause_frames).map(|_| vec![off_color; led_count]));
            seq
        })
        .collect()
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

/// Colors the first `edge_count` LEDs (if `color_start`) and/or last
/// `edge_count` LEDs (if `color_end`) of every physical strip (see
/// `strip_bounds`) by copying from `edge_source` (another frame of the same
/// length) instead of a flat color — lets the edges run their own animated
/// effect, independent of the middle. This colors along each strip's own
/// length — e.g. the two tips of a single strip glued around a case edge.
pub fn apply_edge_frame(
    frame: &mut [[u8; 3]],
    edge_source: &[[u8; 3]],
    strip_count: usize,
    edge_count: usize,
    color_start: bool,
    color_end: bool,
) {
    let bounds = strip_bounds(frame.len(), strip_count);
    for w in bounds.windows(2) {
        let (start, end) = (w[0], w[1]);
        let n = edge_count.min(end - start);
        if color_start {
            frame[start..start + n].copy_from_slice(&edge_source[start..start + n]);
        }
        if color_end {
            frame[end - n..end].copy_from_slice(&edge_source[end - n..end]);
        }
    }
}

/// Same as `apply_edge_frame`, but selects whole physical strips (see
/// `strip_bounds`) instead of a LED count within each — e.g. the outer
/// strands of a Strimer cable, leaving the inner strands to the middle.
pub fn apply_edge_strips_frame(
    frame: &mut [[u8; 3]],
    edge_source: &[[u8; 3]],
    strip_count: usize,
    edge_strips: usize,
    color_start: bool,
    color_end: bool,
) {
    let bounds = strip_bounds(frame.len(), strip_count);
    let n_strips = bounds.len().saturating_sub(1);
    let n = edge_strips.min(n_strips);
    if color_start {
        for i in 0..n {
            frame[bounds[i]..bounds[i + 1]].copy_from_slice(&edge_source[bounds[i]..bounds[i + 1]]);
        }
    }
    if color_end {
        for i in 0..n {
            frame[bounds[n_strips - 1 - i]..bounds[n_strips - i]]
                .copy_from_slice(&edge_source[bounds[n_strips - 1 - i]..bounds[n_strips - i]]);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_to_speed4_clamps_to_0_4() {
        assert_eq!(percent_to_speed4(0.0), 0);
        assert_eq!(percent_to_speed4(100.0), 4);
        assert_eq!(percent_to_speed4(50.0), 2);
        assert_eq!(percent_to_speed4(-10.0), 0);
        assert_eq!(percent_to_speed4(1000.0), 4);
    }

    #[test]
    fn percent_to_cycle_ms_bounds() {
        assert_eq!(percent_to_cycle_ms(0.0), 6000.0);
        assert_eq!(percent_to_cycle_ms(100.0), 900.0);
        assert!(percent_to_cycle_ms(-50.0) == 6000.0); // clamped
        assert!(percent_to_cycle_ms(150.0) == 900.0); // clamped
    }

    #[test]
    fn fps_to_interval_ms_floors_at_8ms() {
        assert_eq!(fps_to_interval_ms(1000.0), 8);
        assert_eq!(fps_to_interval_ms(10.0), 100);
    }

    #[test]
    fn frame_count_for_is_clamped() {
        assert_eq!(frame_count_for(1.0, 100.0), 4); // would be < 4 unclamped
        assert_eq!(frame_count_for(1000.0, 100_000.0), 200); // would be > 200 unclamped
    }

    #[test]
    fn meteor_pause_frames_zero_when_no_pause() {
        assert_eq!(meteor_pause_frames(20, 0.0), 0);
        assert_eq!(meteor_pause_frames(20, -1.0), 0);
        assert_eq!(meteor_pause_frames(1000, 2.0), 2);
    }

    #[test]
    fn scale_color_clamps_factor() {
        assert_eq!(scale_color([200, 100, 50], 0.5), [100, 50, 25]);
        assert_eq!(scale_color([10, 20, 30], -1.0), [0, 0, 0]);
        assert_eq!(scale_color([10, 20, 30], 2.0), [10, 20, 30]);
    }

    #[test]
    fn hsv_to_rgb_primary_hues() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), [0, 0, 255]);
    }

    #[test]
    fn wrap01_stays_in_range() {
        assert_eq!(wrap01(0.5), 0.5);
        assert!((wrap01(1.5) - 0.5).abs() < 1e-6);
        assert!((wrap01(-0.25) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn strip_bounds_splits_evenly() {
        assert_eq!(strip_bounds(90, 3), vec![0, 30, 60, 90]);
        assert_eq!(strip_bounds(10, 1), vec![0, 10]);
        // strip_count clamped to at most led_count.
        assert_eq!(strip_bounds(2, 5).len(), 3);
    }

    #[test]
    fn strip_position_is_fraction_within_own_strip() {
        let bounds = strip_bounds(30, 3); // [0, 10, 20, 30]
        assert_eq!(strip_position(&bounds, 0), 0.0);
        assert_eq!(strip_position(&bounds, 10), 0.0); // start of 2nd strip
        assert!((strip_position(&bounds, 15) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ring_horizontal_fraction_at_default_offset() {
        // local_index 0 with no offset sits at angle 0 (12 o'clock),
        // whose horizontal (sin) position is the ring's midline.
        assert!((ring_horizontal_fraction(0, 4, 0.0) - 0.5).abs() < 1e-6);
        // A quarter turn (index 1 of 4) sits at the horizontal extreme.
        assert!((ring_horizontal_fraction(1, 4, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn band_positions_marks_unpopulated_leds_as_none() {
        // 2 fans of 3 LEDs each, but the buffer has room for a 3rd
        // (unpopulated) fan's worth of LEDs.
        let positions = band_positions(9, &[3, 3], 0.0);
        assert!(positions[0].is_some());
        assert!(positions[5].is_some());
        assert!(positions[6].is_none());
        assert!(positions[8].is_none());
        // Second fan's LEDs land in [1.0, 2.0).
        assert!(positions[3].unwrap() >= 1.0 && positions[3].unwrap() < 2.0);
    }

    #[test]
    fn meteor_frames_head_is_at_start_on_first_frame() {
        let frames = meteor_frames(10, 20, 5, [255, 0, 0], [0, 0, 0], false, false, 1, false, 1.0);
        // frame_count (+ exit_frames for a non-circular strip) + pause_frames.
        assert!(frames.len() > 20 + 5);
        // At t=0 the head sits exactly on LED 0.
        assert_eq!(frames[0][0], [255, 0, 0]);
        // The last `pause_frames` frames are solid tail color.
        let last = frames.last().unwrap();
        assert!(last.iter().all(|&c| c == [0, 0, 0]));
    }

    #[test]
    fn meteor_frames_pause_frames_are_appended_exactly() {
        let with_pause = meteor_frames(5, 10, 7, [255, 255, 255], [0, 0, 0], false, false, 1, true, 1.0);
        let without_pause = meteor_frames(5, 10, 0, [255, 255, 255], [0, 0, 0], false, false, 1, true, 1.0);
        assert_eq!(with_pause.len(), without_pause.len() + 7);
    }

    #[test]
    fn meteor_band_frames_length_includes_pause() {
        let frames = meteor_band_frames(9, &[3, 3, 3], 0.0, 16, 4, [255, 255, 255], [0, 0, 0], false, 1.0);
        assert_eq!(frames.len(), 16 + 4);
        // Pause tail is solid background.
        assert!(frames.last().unwrap().iter().all(|&c| c == [0, 0, 0]));
    }

    #[test]
    fn stagger_across_devices_pads_other_devices_dark() {
        let own_turns = vec![vec![vec![[1, 1, 1]; 2]; 3], vec![vec![[2, 2, 2]; 2]; 3]];
        let led_counts = [2, 2];
        let staggered = stagger_across_devices(&own_turns, &led_counts, 2, [0, 0, 0]);
        assert_eq!(staggered.len(), 2);
        // Device 0's own turn (frames 0..3) is its real color; device 1's
        // window (frames 3..6) is padded dark; plus 2 pause frames.
        assert_eq!(staggered[0].len(), 3 + 3 + 2);
        assert_eq!(staggered[0][0], vec![[1, 1, 1]; 2]);
        assert_eq!(staggered[0][3], vec![[0, 0, 0]; 2]);
        assert_eq!(staggered[0][6], vec![[0, 0, 0]; 2]); // pause frame
    }

    #[test]
    fn rainbow_wave_frames_handles_zero_total_leds() {
        let result = rainbow_wave_frames(&[0, 0], 10, false, &[false, false], &[1, 1], 1.0);
        assert_eq!(result, vec![Vec::<Vec<[u8; 3]>>::new(), Vec::new()]);
    }

    #[test]
    fn mode_uses_color_excludes_rainbow_modes() {
        use lianli_shared::rgb::RgbMode;
        assert!(!mode_uses_color(RgbMode::Rainbow));
        assert!(!mode_uses_color(RgbMode::RainbowMorph));
        assert!(mode_uses_color(RgbMode::Static));
        assert!(mode_uses_color(RgbMode::Meteor));
    }

    #[test]
    fn color_count_for_mode_matches_ui_swatches() {
        use lianli_shared::rgb::RgbMode;
        assert_eq!(color_count_for_mode(RgbMode::ColorCycle), 8);
        assert_eq!(color_count_for_mode(RgbMode::Meteor), 2);
        assert_eq!(color_count_for_mode(RgbMode::Runway), 2);
        assert_eq!(color_count_for_mode(RgbMode::Static), 1);
    }

    #[test]
    fn percent_to_brightness4_matches_speed4() {
        assert_eq!(percent_to_brightness4(0.0), percent_to_speed4(0.0));
        assert_eq!(percent_to_brightness4(75.0), percent_to_speed4(75.0));
    }

    #[test]
    fn lerp_color_endpoints_and_midpoint() {
        assert_eq!(lerp_color([0, 0, 0], [200, 100, 50], 0.0), [0, 0, 0]);
        assert_eq!(lerp_color([0, 0, 0], [200, 100, 50], 1.0), [200, 100, 50]);
        assert_eq!(lerp_color([0, 0, 0], [200, 100, 50], 0.5), [100, 50, 25]);
        // Out-of-range t is clamped.
        assert_eq!(lerp_color([0, 0, 0], [200, 100, 50], 2.0), [200, 100, 50]);
    }

    #[test]
    fn sample_gradient_picks_exact_stops_and_blends_between() {
        let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        assert_eq!(sample_gradient(&colors, 0.0), [255, 0, 0]);
        // t wraps into [0, 1) — 1.0 wraps back to the first stop, not the last.
        assert_eq!(sample_gradient(&colors, 1.0), [255, 0, 0]);
        // Just under 1.0 is still essentially the last stop.
        assert_eq!(sample_gradient(&colors, 0.999), [0, 1, 254]);
        // Halfway between stop 0 and stop 1 (t=0.25 of the whole gradient).
        assert_eq!(sample_gradient(&colors, 0.25), [128, 128, 0]);
    }

    #[test]
    fn sample_gradient_single_color_is_constant() {
        assert_eq!(sample_gradient(&[[1, 2, 3]], 0.5), [1, 2, 3]);
        assert_eq!(sample_gradient(&[], 0.5), [0, 0, 0]);
    }

    #[test]
    fn rainbow_frames_frame_count_and_led_count() {
        let frames = rainbow_frames(12, 8, false, 1, 1.0);
        assert_eq!(frames.len(), 8);
        assert_eq!(frames[0].len(), 12);
    }

    #[test]
    fn custom_gradient_wave_frames_uses_gradient_colors() {
        let frames = custom_gradient_wave_frames(4, 5, &[[255, 0, 0], [0, 0, 255]], false, 1, 1.0);
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].len(), 4);
    }

    #[test]
    fn meteor_chase_frames_relays_one_strip_at_a_time() {
        // 2 strips of 5 LEDs; only one strip should ever be lit per frame.
        let frames = meteor_chase_frames(10, 8, 0, [255, 255, 255], [0, 0, 0], false, false, 2, false, 1.0);
        for frame in &frames {
            let first_half_lit = frame[0..5].iter().any(|&c| c != [0, 0, 0]);
            let second_half_lit = frame[5..10].iter().any(|&c| c != [0, 0, 0]);
            assert!(!(first_half_lit && second_half_lit), "both strips lit in the same frame");
        }
    }

    #[test]
    fn meteor_chase_frames_pause_frames_are_appended() {
        let with_pause = meteor_chase_frames(6, 5, 3, [255, 255, 255], [0, 0, 0], false, false, 1, true, 1.0);
        let without_pause = meteor_chase_frames(6, 5, 0, [255, 255, 255], [0, 0, 0], false, false, 1, true, 1.0);
        assert_eq!(with_pause.len(), without_pause.len() + 3);
    }

    #[test]
    fn meteor_relay_across_devices_shares_total_length() {
        let led_counts = [6, 6];
        let zone_counts = vec![vec![3, 3], vec![6]];
        let frames = meteor_relay_across_devices(
            &led_counts,
            &zone_counts,
            10,
            2,
            [255, 255, 255],
            [0, 0, 0],
            false,
            &[false, false],
            &[1, 1],
            &[false, false],
            &[true, false],
            &[0.0, 0.0],
            1.0,
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].len(), frames[1].len());
    }

    #[test]
    fn apply_edge_frame_colors_only_the_edges() {
        let mut frame = [[0, 0, 0]; 10];
        let source = [[9, 9, 9]; 10];
        apply_edge_frame(&mut frame, &source, 1, 2, true, true);
        assert_eq!(frame[0], [9, 9, 9]);
        assert_eq!(frame[1], [9, 9, 9]);
        assert_eq!(frame[2], [0, 0, 0]); // middle untouched
        assert_eq!(frame[9], [9, 9, 9]);
        assert_eq!(frame[8], [9, 9, 9]);
    }

    #[test]
    fn apply_edge_strips_frame_colors_whole_outer_strips() {
        // 4 strips of 3 LEDs each; color the outermost strip on each end.
        let mut frame = [[0, 0, 0]; 12];
        let source = [[9, 9, 9]; 12];
        apply_edge_strips_frame(&mut frame, &source, 4, 1, true, true);
        assert!(frame[0..3].iter().all(|&c| c == [9, 9, 9]));
        assert!(frame[3..9].iter().all(|&c| c == [0, 0, 0])); // untouched middle strips
        assert!(frame[9..12].iter().all(|&c| c == [9, 9, 9]));
    }

    #[test]
    fn identify_frames_blinks_3_times_ending_off() {
        let frames = identify_frames(5);
        assert_eq!(frames.len(), 7);
        assert_eq!(frames[0][0], [0, 0, 0]);
        assert_eq!(frames.last().unwrap()[0], [0, 0, 0]);
        let yellow = frames[1][0];
        assert_eq!(yellow, [63, 63, 0]);
        assert!(frames.iter().all(|f| f.len() == 5));
    }
}
