//! Randomised keypad layout, to blunt shoulder-surfing and smudge reading.

pub const STANDARD: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 0];

/// Fisher–Yates shuffle of the ten digits. `next_u32` must come from a
/// CSPRNG (the UI uses `crypto.getRandomValues`); it's injected so the
/// shuffle itself is deterministic and testable.
pub fn shuffled_digits(mut next_u32: impl FnMut() -> u32) -> [u8; 10] {
    let mut d = STANDARD;
    for i in (1..d.len()).rev() {
        let j = uniform_below(&mut next_u32, (i + 1) as u32) as usize;
        d.swap(i, j);
    }
    d
}

/// Unbiased integer in `0..n` by rejection sampling (no modulo bias).
fn uniform_below(next: &mut impl FnMut() -> u32, n: u32) -> u32 {
    debug_assert!(n > 0);
    let zone = u32::MAX - (u32::MAX % n); // largest multiple of n that fits
    loop {
        let r = next();
        if r < zone {
            return r % n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut d: [u8; 10]) -> [u8; 10] {
        d.sort();
        d
    }

    #[test]
    fn is_a_permutation() {
        let mut x = 12345u32;
        let d = shuffled_digits(|| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        });
        assert_eq!(sorted(d), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn rejects_biased_values() {
        // u32::MAX is always outside the zone for n = 10, so it must be skipped.
        let mut vals = [u32::MAX, 3].into_iter();
        assert_eq!(uniform_below(&mut || vals.next().unwrap(), 10), 3);
    }

    #[test]
    fn zero_source_still_gives_valid_layout() {
        assert_eq!(
            sorted(shuffled_digits(|| 0)),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
    }
}
