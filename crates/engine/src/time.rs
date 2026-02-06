use crate::error::{EngineError, Result};

/// FFmpeg-like rational number used as a time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i32,
    pub den: i32,
}

impl Rational {
    /// Timeline time base used by the editor: microseconds.
    pub const MICROS: Self = Self {
        num: 1,
        den: 1_000_000,
    };

    /// Creates a validated rational.
    ///
    /// # Example
    /// ```
    /// use engine::Rational;
    ///
    /// let tb = Rational::new(1, 90_000).expect("valid");
    /// assert_eq!(tb.den, 90_000);
    /// ```
    pub fn new(num: i32, den: i32) -> Result<Self> {
        if num <= 0 || den <= 0 {
            return Err(EngineError::InvalidRational { num, den });
        }
        Ok(Self { num, den })
    }
}

/// Timeline base `(1, 1_000_000)`.
pub const TIMELINE_TIME_BASE: Rational = Rational::MICROS;

/// Rescales `ts` from one time base to another with nearest rounding.
///
/// # Example
/// ```
/// use engine::{Rational, TIMELINE_TIME_BASE, rescale};
///
/// let src = Rational::new(1, 90_000).expect("valid");
/// assert_eq!(rescale(90_000, src, TIMELINE_TIME_BASE), 1_000_000);
/// ```
pub fn rescale(ts: i64, from: Rational, to: Rational) -> i64 {
    let numerator = i128::from(ts) * i128::from(from.num) * i128::from(to.den);
    let denominator = i128::from(from.den) * i128::from(to.num);
    let rounded = div_round_nearest(numerator, denominator);
    rounded.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn div_round_nearest(num: i128, den: i128) -> i128 {
    debug_assert!(den > 0);

    let abs_num = num.abs();
    let mut out = abs_num / den;
    let remainder = abs_num % den;
    if remainder.saturating_mul(2) >= den {
        out += 1;
    }

    if num < 0 { -out } else { out }
}

impl From<media_ffmpeg::Rational> for Rational {
    fn from(value: media_ffmpeg::Rational) -> Self {
        Self {
            num: value.num,
            den: value.den,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rational;

    #[test]
    fn rational_new_rejects_negative_numerator() {
        assert!(Rational::new(-1, 90_000).is_err());
    }
}
