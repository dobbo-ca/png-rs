use super::*;

struct Xorshift(u64);

impl Xorshift {
    fn byte(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }
    fn fill(&mut self, n: usize, mode: u32) -> Vec<u8> {
        (0..n)
            .map(|_| match mode {
                0 => self.byte(),
                1 => self.byte() & 3, // small values: Paeth ties
                2 => 0xff,
                _ => self.byte() | 0xf0,
            })
            .collect()
    }
}

#[test]
fn tier_matches_hardware() {
    let forced = std::env::var("PNG_SIMD").is_ok_and(|v| v == "scalar");
    #[cfg(target_arch = "x86_64")]
    {
        assert!(is_x86_feature_detected!("avx2"));
        assert_eq!(tier(), if forced { "scalar" } else { "avx2" });
    }
    #[cfg(target_arch = "aarch64")]
    {
        assert!(std::arch::is_aarch64_feature_detected!("neon"));
        assert_eq!(tier(), if forced { "scalar" } else { "neon" });
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    assert_eq!(tier(), "scalar");
}

#[test]
fn tier_equals_scalar() {
    let mut rng = Xorshift(0x9e37_79b9_7f4a_7c15);
    for bpp in 1..=8usize {
        let mut lens = vec![0, 1, bpp - 1, bpp, 15, 16, 17, 31, 32, 33, 1000];
        lens.extend([2 * bpp, 5 * bpp, 5 * bpp + 1, 64, 65, 255, 1001]);
        for &len in &lens {
            for filter in 0..=4u8 {
                for mode in 0..4 {
                    for _ in 0..4 {
                        let cur = rng.fill(len, mode);
                        let prev = rng.fill(len, (mode + 1) % 4);
                        let (mut a, mut b) = (cur.clone(), cur.clone());
                        assert!(unfilter_scalar(filter, bpp, &mut a, &prev));
                        assert!(unfilter_simd(filter, bpp, &mut b, &prev));
                        assert_eq!(a, b, "filter {filter} bpp {bpp} len {len} mode {mode}");
                        let mut c = cur.clone();
                        assert!(unfilter(filter, bpp, &mut c, &prev));
                        assert_eq!(a, c);
                    }
                }
            }
        }
    }
}

/// Zeroed `prev` (the first row) is the same on every tier.
#[test]
fn first_row() {
    let mut rng = Xorshift(12345);
    for bpp in 1..=8usize {
        for filter in 0..=4u8 {
            let cur = rng.fill(777, 0);
            let prev = vec![0u8; 777];
            let (mut a, mut b) = (cur.clone(), cur);
            assert!(unfilter_scalar(filter, bpp, &mut a, &prev));
            assert!(unfilter_simd(filter, bpp, &mut b, &prev));
            assert_eq!(a, b);
        }
    }
}

#[test]
fn rejects_bad_arguments() {
    let mut row = [1u8, 2, 3, 4];
    let prev = [0u8; 4];
    for f in [unfilter_scalar, unfilter_simd, unfilter] {
        assert!(!f(5, 1, &mut row, &prev));
        assert!(!f(255, 1, &mut row, &prev));
        assert!(!f(1, 0, &mut row, &prev));
        assert!(!f(1, 9, &mut row, &prev));
        assert!(!f(1, 1, &mut row, &prev[..3]));
        assert_eq!(row, [1, 2, 3, 4]);
    }
}

#[test]
fn known_values() {
    // Sub, bpp 1: running sum.
    let mut r = [1u8, 1, 1, 255];
    assert!(unfilter(1, 1, &mut r, &[0; 4]));
    assert_eq!(r, [1, 2, 3, 2]);
    // Up.
    let mut r = [1u8, 2, 3];
    assert!(unfilter(2, 1, &mut r, &[10, 20, 255]));
    assert_eq!(r, [11, 22, 2]);
    // Average, bpp 1: [10, 10 + (10 + 4) / 2].
    let mut r = [10u8, 10];
    assert!(unfilter(3, 1, &mut r, &[0, 4]));
    assert_eq!(r, [10, 17]);
    // Paeth, bpp 1: a = 5, b = 5, c = 0 -> pa = 5, pb = 5, pc = 10: a.
    let mut r = [5u8, 1];
    assert!(unfilter(4, 1, &mut r, &[5, 5]));
    assert_eq!(r, [10, 11]);
}
