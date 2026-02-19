//! Improved sigil generation: 8x32 canvas with dimension-blended character sets.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};

/// Character sets per compass dimension.
const DUTY_CHARS: &[char] = &['║', '╬', '█', '═', '╗', '╚', '╔', '╝'];
const VIRTUE_CHARS: &[char] = &['(', ')', 'o', '8', '@', '~', '*', '^'];
const OUTCOME_CHARS: &[char] = &['>', '/', '\\', '_', '|', '<', '-', '='];
const WELFARE_CHARS: &[char] = &['░', '▒', '▓', '♦', '●', '☼', '♥', '◊'];

/// Symmetry mode determined by primary dimension.
#[derive(Debug, Clone, Copy)]
enum Symmetry {
    VerticalMirror, // Duty: left→right mirror
    Radial,         // Virtue: rotational symmetry
    HorizontalFlow, // Outcome: top→bottom directional
    Quad,           // Welfare: 4-quadrant mirror
}

/// Generate a sigil from compass weights and entropy.
pub fn generate_sigil(
    duty: f64,
    virtue: f64,
    outcome: f64,
    welfare: f64,
    entropy: &[String],
    session_seed: u64,
    scenario_ids: &[String],
) -> (String, String) {
    // Build hash from all inputs
    let mut hasher = Sha256::new();
    hasher.update(format!("{:.6}{:.6}{:.6}{:.6}", duty, virtue, outcome, welfare).as_bytes());
    for e in entropy {
        hasher.update(e.as_bytes());
    }
    hasher.update(session_seed.to_be_bytes());
    for id in scenario_ids {
        hasher.update(id.as_bytes());
    }
    let result = hasher.finalize();
    let soul_hash = format!("{:x}", result);

    let seed_bytes: [u8; 8] = result[0..8].try_into().unwrap();
    let seed = u64::from_be_bytes(seed_bytes);
    let mut rng = StdRng::seed_from_u64(seed);

    // Determine primary and secondary dimensions
    let mut dims = [
        (duty, "duty"),
        (virtue, "virtue"),
        (outcome, "outcome"),
        (welfare, "welfare"),
    ];
    dims.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let primary = dims[0].1;
    let secondary = dims[1].1;

    // Blend character sets: 70% primary + 30% secondary
    let primary_chars = chars_for_dimension(primary);
    let secondary_chars = chars_for_dimension(secondary);

    let symmetry = symmetry_for_dimension(primary);

    // Generate 8x32 canvas
    let rows = 8;
    let cols = 32;
    let mut lines = Vec::new();

    for row in 0..rows {
        let half_cols = cols / 2;
        let mut half: Vec<char> = Vec::new();

        for _col in 0..half_cols {
            let ch = if rng.random_ratio(7, 10) {
                primary_chars[rng.random_range(0..primary_chars.len())]
            } else {
                secondary_chars[rng.random_range(0..secondary_chars.len())]
            };
            half.push(ch);
        }

        let full_line: String = match symmetry {
            Symmetry::VerticalMirror => {
                let mirrored: String = half.iter().rev().collect();
                let left: String = half.into_iter().collect();
                format!("{}{}", left, mirrored)
            }
            Symmetry::Radial => {
                if row < rows / 2 {
                    let mirrored: String = half.iter().rev().collect();
                    let left: String = half.into_iter().collect();
                    format!("{}{}", left, mirrored)
                } else {
                    // Bottom half mirrors top half (but we generate fresh + mirror)
                    let mirrored: String = half.iter().rev().collect();
                    let left: String = half.into_iter().collect();
                    format!("{}{}", mirrored, left)
                }
            }
            Symmetry::HorizontalFlow => {
                // No horizontal mirror — full width directional
                let mut full: Vec<char> = half;
                for _ in half_cols..cols {
                    let ch = if rng.random_ratio(7, 10) {
                        primary_chars[rng.random_range(0..primary_chars.len())]
                    } else {
                        secondary_chars[rng.random_range(0..secondary_chars.len())]
                    };
                    full.push(ch);
                }
                full.into_iter().collect()
            }
            Symmetry::Quad => {
                let mirrored: String = half.iter().rev().collect();
                let left: String = half.into_iter().collect();
                format!("{}{}", left, mirrored)
            }
        };

        lines.push(full_line);
    }

    // For quad symmetry, mirror bottom half from top
    if matches!(symmetry, Symmetry::Quad) {
        let top_half: Vec<String> = lines[..rows / 2].to_vec();
        for i in 0..rows / 2 {
            lines[rows - 1 - i] = top_half[i].clone();
        }
    }

    let sigil_art = lines.join("\n");
    (soul_hash, sigil_art)
}

fn chars_for_dimension(dim: &str) -> &'static [char] {
    match dim {
        "duty" => DUTY_CHARS,
        "virtue" => VIRTUE_CHARS,
        "outcome" => OUTCOME_CHARS,
        "welfare" => WELFARE_CHARS,
        _ => WELFARE_CHARS,
    }
}

fn symmetry_for_dimension(dim: &str) -> Symmetry {
    match dim {
        "duty" => Symmetry::VerticalMirror,
        "virtue" => Symmetry::Radial,
        "outcome" => Symmetry::HorizontalFlow,
        "welfare" => Symmetry::Quad,
        _ => Symmetry::VerticalMirror,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigil_deterministic() {
        let entropy = vec!["Follow Rules".to_string(), "Ruthless".to_string()];
        let ids = vec!["shortcut".to_string(), "critic".to_string()];

        let (hash1, sigil1) = generate_sigil(0.4, 0.3, 0.2, 0.1, &entropy, 42, &ids);
        let (hash2, sigil2) = generate_sigil(0.4, 0.3, 0.2, 0.1, &entropy, 42, &ids);

        assert_eq!(hash1, hash2);
        assert_eq!(sigil1, sigil2);
    }

    #[test]
    fn test_sigil_dimensions() {
        let (_, sigil) = generate_sigil(0.5, 0.2, 0.2, 0.1, &[], 0, &[]);
        let lines: Vec<&str> = sigil.lines().collect();
        assert_eq!(lines.len(), 8, "Expected 8 lines");
        // Each line should be 32 chars (Unicode width varies, but char count = 32)
        for line in &lines {
            assert_eq!(
                line.chars().count(),
                32,
                "Expected 32 chars per line, got {}",
                line.chars().count()
            );
        }
    }

    #[test]
    fn test_different_seeds_different_sigils() {
        let (h1, s1) = generate_sigil(0.4, 0.3, 0.2, 0.1, &[], 1, &[]);
        let (h2, s2) = generate_sigil(0.4, 0.3, 0.2, 0.1, &[], 2, &[]);
        assert_ne!(h1, h2);
        assert_ne!(s1, s2);
    }
}
