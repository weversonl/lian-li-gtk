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

/// How many of the 8 color swatches are actually meaningful for `mode`.
/// Every other color-using mode (Static, Breathing, Direct, ...) only ever
/// reads `colors[0]` — showing all 8 swatches for those wasn't just visual
/// clutter, it was misleading, since picking swatches 2-8 silently did
/// nothing. `ColorCycle` (Gradient Wave) uses all 8; `Meteor` uses 2 (head,
/// tail); `MeteorShower` (repurposed as a rainbow-headed meteor for wireless
/// devices — see `apply_edge_frame`'s callers) only needs the tail color,
/// since the head cycles the hue wheel on its own.
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

/// A bright "head" sweeping once across the strip, dragging a fading trail
/// behind it and leaving everything else off (or `tail_color`, if not
/// black) — the classic ARGB "Meteor" effect. `circular` picks the strip's
/// topology: `false` (a cable with two distinct physical ends) is a one-way
/// linear sweep — the head starts at one tip, travels to the other, and
/// only *then* snaps back to the start for the next lap, since blending the
/// trail across the seam of a strip with real ends (as an earlier version
/// of this function did for every strip) made the tail flicker at the
/// wrong end every lap. `true` (a closed ring, e.g. the LEDs around a fan's
/// hub) wraps the trail seamlessly from the last LED back to the first,
/// since there they really are physically adjacent. `rainbow_head` cycles
/// the head's hue over the lap instead of using `head_color` (used to
/// repurpose `RgbMode::MeteorShower` as a rainbow-headed variant for
/// wireless devices, same trick as `ColorCycle` → "Gradient Wave").
///
/// Every physical strip (see `strip_bounds`) plays the identical sweep in
/// sync, all moving together — this is what actually reads as "one pulse
/// traveling the cable" for a cable made of several physical strips stacked
/// end to end: each strip's own point lights up at the same relative
/// height at the same instant, so the whole set moves as one wave. `strip_count`
/// must match the device's real physical strip count — setting it to the
/// wrong number (e.g. 2 on an 8-strip cable) makes only that many
/// simultaneous points appear instead of matching the real segmentation.
/// Not a relay where only one strip animates while the rest sit dark (see
/// `meteor_chase_frames`, "Meteor (Split)").
///
/// `pause_frames` appends that many tail-only frames after the sweep
/// completes, before the sequence loops — L-Connect holds ~5s dark between
/// laps instead of restarting instantly.
///
/// For a linear strip, the head only travels to `t = 1.0` (the far tip),
/// but the trail behind it is still lit at that point — cutting straight to
/// `pause_frames` there would chop the fading tail off abruptly instead of
/// letting it finish fading out. `exit_frames` extra frames let `t` keep
/// going past 1.0 (or below 0.0 in reverse) at the same speed, so the tail
/// runs off the far tip and fully fades to `tail_color` before the pause,
/// instead of ending mid-fade. A closed ring has no such tip to run off of
/// — by the time the head completes the lap, the trail behind it has
/// already faded out naturally, so no extra frames are needed there.
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

/// The strip-by-strip Meteor relay: each physical strip (see `strip_bounds`)
/// takes its turn sweeping a full lap, one at a time, in sequence — a
/// purple thread passing through one LED strip, then another, then another,
/// until the last, then restarting from the first — not every strip
/// sweeping together merged as one like `meteor_frames`. Once the last
/// strip finishes, `pause_frames` tail-only frames hold before the relay
/// loops back to the first strip.
///
/// Same tail-exit handling as `meteor_frames` — each strip's own turn gets
/// extra frames past `t = 1.0` so its trail fades out fully before handing
/// off to the next strip, instead of cutting the fade off abruptly. `circular`
/// has the same meaning as in `meteor_frames` (a closed ring vs. a strip
/// with two real ends), applied to each strip's own turn.
///
/// `reverse` only flips the sweep direction *within* each strip's turn —
/// the relay always visits strips in the same buffer order regardless, so
/// which physical strip goes first stays fixed while just the motion
/// inside each turn flips (a wiring-order fix shouldn't also reorder which
/// strip the relay starts from).
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
    for w in bounds.windows(2) {
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

/// The L-Connect-style Meteor relay carried across *devices* instead of
/// physical strips within one device (see `meteor_chase_frames` for the
/// within-device version): each device takes its full turn, in the order
/// given, before handing off to the next — fans first, then the GPU
/// Strimer, then the motherboard Strimer, then back to the fans, matching
/// whatever device order the user set on this page. Once every device has
/// had a turn, `pause_frames` holds everything dark before the relay loops
/// back to the first device.
///
/// All returned sequences share the same total length so every device can
/// be sent with the same `interval_ms` and stay in step — while it's not
/// this device's turn, its sequence is just solid `tail_color`.
/// `device_reversed`/`device_strip_counts`/`device_circular` are each
/// device's own settings (from `DeviceRgbPrefs`), applied only to that
/// device's own turn — they don't affect which device goes first.
#[allow(clippy::too_many_arguments)]
pub fn meteor_relay_across_devices(
    device_led_counts: &[usize],
    frame_count_per_device: usize,
    pause_frames: usize,
    head_color: [u8; 3],
    tail_color: [u8; 3],
    rainbow_head: bool,
    device_reversed: &[bool],
    device_strip_counts: &[usize],
    device_circular: &[bool],
    brightness: f32,
) -> Vec<Vec<Vec<[u8; 3]>>> {
    let n = device_led_counts.len();
    let tail = scale_color(tail_color, brightness as f64);

    let own_turns: Vec<Vec<Vec<[u8; 3]>>> = (0..n)
        .map(|i| {
            meteor_frames(
                device_led_counts[i],
                frame_count_per_device,
                0,
                head_color,
                tail_color,
                rainbow_head,
                device_reversed.get(i).copied().unwrap_or(false),
                device_strip_counts.get(i).copied().unwrap_or(1),
                device_circular.get(i).copied().unwrap_or(false),
                brightness,
            )
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
