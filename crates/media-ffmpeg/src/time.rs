use crate::error::{MediaFfmpegError, Result};

/// Rational value used as FFmpeg-like time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i32,
    pub den: i32,
}

impl Rational {
    /// Microsecond time base `(1 / 1_000_000)`.
    pub const MICROS: Self = Self {
        num: 1,
        den: 1_000_000,
    };

    /// Creates a normalized rational value.
    ///
    /// # Example
    /// ```
    /// use media_ffmpeg::Rational;
    ///
    /// let tb = Rational::new(1, 48_000).expect("valid");
    /// assert_eq!(tb.num, 1);
    /// assert_eq!(tb.den, 48_000);
    /// ```
    pub fn new(num: i32, den: i32) -> Result<Self> {
        if den <= 0 || num == 0 {
            return Err(MediaFfmpegError::InvalidRational { num, den });
        }

        Ok(Self { num, den })
    }

    /// Parses a `num/den` text into a rational.
    ///
    /// # Example
    /// ```
    /// use media_ffmpeg::Rational;
    ///
    /// let tb = Rational::parse("1/15360").expect("valid");
    /// assert_eq!(tb.den, 15360);
    /// ```
    pub fn parse(input: &str) -> Result<Self> {
        let (num, den) = input
            .split_once('/')
            .ok_or_else(|| MediaFfmpegError::Parse {
                context: "rational",
                value: input.to_string(),
            })?;
        let num = parse_i32(num, "rational num")?;
        let den = parse_i32(den, "rational den")?;
        Self::new(num, den)
    }
}

/// Rescales `ts` from one time base to another.
///
/// This mirrors FFmpeg-like integer timestamp conversion with nearest rounding.
///
/// # Example
/// ```
/// use media_ffmpeg::{rescale, Rational};
///
/// let src = Rational::new(1, 90_000).expect("valid");
/// assert_eq!(rescale(90_000, src, Rational::MICROS), 1_000_000);
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

fn parse_i32(value: &str, context: &'static str) -> Result<i32> {
    value
        .trim()
        .parse::<i32>()
        .map_err(|_| MediaFfmpegError::Parse {
            context,
            value: value.to_string(),
        })
}
