//! Stable, readable agent identity colors. Allocation runs only on creation/load.
use std::sync::OnceLock;

pub(crate) const YOU_COLOR: [u8; 3] = [102, 255, 102];
const BACKGROUND: [u8; 3] = [24, 24, 28];

struct Candidate {
    rgb: [u8; 3],
    lab: [f64; 3],
}

/// Maximize the minimum Oklab distance to You and every existing room agent.
/// The search is exhaustive over a deterministic 17-step sRGB grid whose text
/// contrast is at least 4.5:1 and which excludes recognizable green shades; it is
/// not a continuous-gamut optimum. RGB order breaks ties, so an unchanged set of
/// occupied colors gives the same result.
pub(crate) fn next_agent_color(existing: impl IntoIterator<Item = [u8; 3]>) -> [u8; 3] {
    let occupied: Vec<_> = std::iter::once(YOU_COLOR)
        .chain(existing)
        .map(oklab)
        .collect();
    let mut best = ([255, 255, 255], f64::NEG_INFINITY);
    for candidate in candidates() {
        let minimum_distance = occupied
            .iter()
            .map(|other| {
                candidate
                    .lab
                    .iter()
                    .zip(other)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
            })
            .fold(f64::INFINITY, f64::min);
        if minimum_distance > best.1 {
            best = (candidate.rgb, minimum_distance);
        }
    }
    best.0
}

pub(crate) fn is_agent_color(rgb: [u8; 3]) -> bool {
    if (luminance(rgb) + 0.05) / (luminance(BACKGROUND) + 0.05) < 4.5 {
        return false;
    }
    let [_, a, b] = oklab(rgb);
    let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
    // Reserve yellow-green through mint for You. Chroma below 0.04 is near
    // neutral; retaining those shades keeps whites/grays available to agents.
    let recognizable_green = a.hypot(b) >= 0.04 && (115.0..=185.0).contains(&hue);
    !recognizable_green
}

fn candidates() -> &'static [Candidate] {
    static CANDIDATES: OnceLock<Vec<Candidate>> = OnceLock::new();
    CANDIDATES.get_or_init(|| {
        let mut candidates = Vec::new();
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let rgb = [r, g, b];
                    if is_agent_color(rgb) {
                        candidates.push(Candidate {
                            rgb,
                            lab: oklab(rgb),
                        });
                    }
                }
            }
        }
        candidates
    })
}

// WCAG 2.2 relative luminance and sRGB transfer function:
// https://www.w3.org/TR/WCAG22/#dfn-relative-luminance
fn linear_rgb(rgb: [u8; 3]) -> [f64; 3] {
    rgb.map(|channel| {
        let channel = f64::from(channel) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let [r, g, b] = linear_rgb(rgb);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

// Björn Ottosson's published linear-sRGB transform, updated 2021-01-25:
// https://bottosson.github.io/posts/oklab/#converting-from-linear-srgb-to-oklab
fn oklab(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = linear_rgb(rgb);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_colors_reserve_recognizable_green_shades_for_you() {
        for green in [[0, 170, 0], [119, 204, 85], [153, 255, 204]] {
            assert!(!is_agent_color(green), "green belongs to You: {green:?}");
        }
        for distinct in [[255, 255, 0], [0, 204, 204], [221, 0, 255], [255, 255, 255]] {
            assert!(is_agent_color(distinct), "keep distinct color {distinct:?}");
        }
    }
}
