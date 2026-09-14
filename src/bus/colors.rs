//! Stable, readable agent identity colors. Allocation runs only on creation/load.
use std::sync::OnceLock;

pub(crate) const YOU_COLOR: [u8; 3] = [102, 255, 102];
const BACKGROUND: [u8; 3] = [24, 24, 28];

/// Typical vision plus each simulated color blindness.
const VIEWS: usize = 1 + COLOR_BLINDNESS.len();

/// Full-severity protanopia, deuteranopia and tritanopia in linear sRGB, from
/// Machado, Oliveira and Fernandes (2009):
/// https://www.inf.ufrgs.br/~oliveira/pubs_files/CVD_Simulation/CVD_Simulation.html
const COLOR_BLINDNESS: [[[f64; 3]; 3]; 3] = [
    [
        [0.152286, 1.052583, -0.204868],
        [0.114503, 0.786281, 0.099216],
        [-0.003882, -0.048116, 1.051998],
    ],
    [
        [0.367322, 0.860646, -0.227968],
        [0.280085, 0.672501, 0.047413],
        [-0.011820, 0.042940, 0.968881],
    ],
    [
        [1.255528, -0.076749, -0.178779],
        [-0.078411, 0.930809, 0.147602],
        [0.004733, 0.691367, 0.303900],
    ],
];

struct Candidate {
    rgb: [u8; 3],
    views: [[f64; 3]; VIEWS],
    accessible: bool,
}

/// Maximize the minimum Oklab distance to You and every existing room agent.
/// The search is exhaustive over a deterministic 17-step sRGB grid whose text
/// contrast is at least 4.5:1 and which excludes recognizable green shades; it is
/// not a continuous-gamut optimum. RGB order breaks ties, so an unchanged set of
/// occupied colors gives the same result.
pub(crate) fn next_agent_color(existing: impl IntoIterator<Item = [u8; 3]>) -> [u8; 3] {
    farthest(existing, false)
}

/// Color blind mode's allocation: the same search, restricted to colors that
/// stay readable under every simulated color blindness, maximizing the minimum
/// distance seen with typical vision and with each of those simulations.
pub(crate) fn next_accessible_agent_color(existing: impl IntoIterator<Item = [u8; 3]>) -> [u8; 3] {
    farthest(existing, true)
}

fn farthest(existing: impl IntoIterator<Item = [u8; 3]>, accessible: bool) -> [u8; 3] {
    let views = if accessible { VIEWS } else { 1 };
    let occupied: Vec<_> = std::iter::once(YOU_COLOR)
        .chain(existing)
        .map(simulated_views)
        .collect();
    let mut best = ([255, 255, 255], f64::NEG_INFINITY);
    for candidate in candidates()
        .iter()
        .filter(|candidate| candidate.accessible || !accessible)
    {
        let minimum_distance = occupied
            .iter()
            .flat_map(|other| candidate.views[..views].iter().zip(&other[..views]))
            .map(|(a, b)| squared_distance(*a, *b))
            .fold(f64::INFINITY, f64::min);
        if minimum_distance > best.1 {
            best = (candidate.rgb, minimum_distance);
        }
    }
    best.0
}

pub(crate) fn is_agent_color(rgb: [u8; 3]) -> bool {
    if !readable(linear_rgb(rgb), linear_rgb(BACKGROUND)) {
        return false;
    }
    let [_, a, b] = oklab(linear_rgb(rgb));
    let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
    // Reserve yellow-green through mint for You. Chroma below 0.04 is near
    // neutral; retaining those shades keeps whites/grays available to agents.
    let recognizable_green = a.hypot(b) >= 0.04 && (115.0..=185.0).contains(&hue);
    !recognizable_green
}

pub(crate) fn is_accessible_agent_color(rgb: [u8; 3]) -> bool {
    is_agent_color(rgb)
        && COLOR_BLINDNESS.iter().all(|matrix| {
            readable(
                simulate(matrix, linear_rgb(rgb)),
                simulate(matrix, linear_rgb(BACKGROUND)),
            )
        })
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
                            views: simulated_views(rgb),
                            accessible: is_accessible_agent_color(rgb),
                        });
                    }
                }
            }
        }
        candidates
    })
}

fn simulated_views(rgb: [u8; 3]) -> [[f64; 3]; VIEWS] {
    let linear = linear_rgb(rgb);
    let mut views = [oklab(linear); VIEWS];
    for (view, matrix) in views[1..].iter_mut().zip(&COLOR_BLINDNESS) {
        *view = oklab(simulate(matrix, linear));
    }
    views
}

fn simulate(matrix: &[[f64; 3]; 3], [r, g, b]: [f64; 3]) -> [f64; 3] {
    matrix.map(|[red, green, blue]| (red * r + green * g + blue * b).clamp(0.0, 1.0))
}

fn squared_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

fn readable(text: [f64; 3], background: [f64; 3]) -> bool {
    (luminance(text) + 0.05) / (luminance(background) + 0.05) >= 4.5
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

fn luminance([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

// Björn Ottosson's published linear-sRGB transform, updated 2021-01-25:
// https://bottosson.github.io/posts/oklab/#converting-from-linear-srgb-to-oklab
fn oklab([r, g, b]: [f64; 3]) -> [f64; 3] {
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

    /// Smallest Oklab distance between any two of You and `agents`, taken over
    /// typical vision and every simulated color blindness.
    fn worst_separation(agents: &[[u8; 3]]) -> f64 {
        let seen: Vec<_> = std::iter::once(YOU_COLOR)
            .chain(agents.iter().copied())
            .map(simulated_views)
            .collect();
        let mut worst = f64::INFINITY;
        for (index, a) in seen.iter().enumerate() {
            for b in &seen[index + 1..] {
                for (a, b) in a.iter().zip(b) {
                    worst = worst.min(squared_distance(*a, *b).sqrt());
                }
            }
        }
        worst
    }

    #[test]
    fn accessible_agent_colors_stay_distinct_with_color_blindness() {
        let (mut standard, mut accessible) = (Vec::new(), Vec::new());
        for count in 1..=8 {
            standard.push(next_agent_color(standard.clone()));
            accessible.push(next_accessible_agent_color(accessible.clone()));
            let color = *accessible.last().unwrap();
            assert!(is_accessible_agent_color(color), "unreadable {color:?}");
            // A lone agent is far from You either way. From two agents on, the
            // standard palette's worst pair under some color blindness falls
            // to 0.035 by the fourth agent; this mode keeps every pair apart.
            if count >= 2 {
                assert!(
                    worst_separation(&accessible) > worst_separation(&standard),
                    "{count} agents"
                );
            }
            assert!(
                worst_separation(&accessible) >= 0.09,
                "{count} agents: {accessible:?}"
            );
        }
    }
}
