//! Fixed-point money amounts: `Amount` and `Balance`, strict parsing, exact formatting (ADR-0007).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.
//!
//! Both types count units of `0.0001` ([`SCALE`] fractional digits). An
//! [`Amount`] is the strictly positive value of one `deposit` or `withdrawal`
//! and is only built by [`Amount::parse`], which enforces the grammar and range
//! of ADR-0007 M3 to M5. A [`Balance`] is a signed account balance that changes
//! only through checked arithmetic (M7) and prints in the fixed layout of
//! ADR-0009 O5 and O6. No floating-point type is used anywhere (M8).

use std::fmt::{self, Display, Formatter};
use std::num::NonZeroU64;

/// Number of fractional digits in every amount and balance (ADR-0007 M1).
pub const SCALE: usize = 4;

/// Units in one whole currency unit: `10^SCALE`.
const UNITS_PER_WHOLE: u128 = 10_000;

/// The value of one `deposit` or `withdrawal`: a non-zero count of `0.0001`
/// units, from `0.0001` to `1844674407370955.1615` (ADR-0007 M1, M5).
///
/// Zero and negative amounts cannot be represented, so no code that receives
/// an `Amount` has to check for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount(NonZeroU64);

/// Why an amount field was rejected (ADR-0007 M3 to M5).
///
/// Every variant makes the row invalid (M6). The input adapter reports all of
/// them under one diagnostic reason (ADR-0010); the variants keep the cause
/// precise for tests and for the diagnostic text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AmountError {
    /// The field is empty or holds only whitespace.
    #[error("amount is empty")]
    Empty,
    /// The field is not `digits [ "." digits ]` in ASCII (M3).
    #[error("amount is not a plain decimal number")]
    Syntax,
    /// A non-zero digit follows the fourth fractional digit (M4).
    #[error("amount has more than 4 non-zero fractional digits")]
    TooPrecise,
    /// The value is zero (M5).
    #[error("amount is zero")]
    Zero,
    /// The value is above `1844674407370955.1615` (M5).
    #[error("amount is above the maximum of 1844674407370955.1615")]
    TooLarge,
}

impl Amount {
    /// The smallest amount, `0.0001`.
    pub const MIN: Self = Self(NonZeroU64::MIN);

    /// The largest amount, `1844674407370955.1615` (`u64::MAX` units).
    pub const MAX: Self = Self(NonZeroU64::MAX);

    /// Parses one amount field (ADR-0007 M3 to M5).
    ///
    /// Surrounding ASCII whitespace is trimmed. What remains must be ASCII
    /// digits, optionally followed by `.` and at least one more digit.
    /// Fractional digits after the fourth are accepted only if they are all
    /// `0`. Leading zeros are accepted. The value is never rounded or
    /// truncated.
    ///
    /// The work is linear in the length of the field, and nothing is
    /// allocated.
    ///
    /// # Errors
    ///
    /// Returns the first rule the field breaks, checked in this order:
    /// [`AmountError::Empty`], [`AmountError::Syntax`],
    /// [`AmountError::TooPrecise`], [`AmountError::TooLarge`], and
    /// [`AmountError::Zero`].
    pub fn parse(field: &[u8]) -> Result<Self, AmountError> {
        let field = field.trim_ascii();
        if field.is_empty() {
            return Err(AmountError::Empty);
        }

        let mut parts = field.splitn(2, |&byte| byte == b'.');
        let integer = parts.next().unwrap_or_default();
        let fraction = parts.next();
        if !is_digits(integer) || fraction.is_some_and(|digits| !is_digits(digits)) {
            return Err(AmountError::Syntax);
        }

        let fraction = fraction.unwrap_or_default();
        let (kept, excess) = fraction.split_at_checked(SCALE).unwrap_or((fraction, &[]));
        if excess.iter().any(|&byte| byte != b'0') {
            return Err(AmountError::TooPrecise);
        }

        // Every digit was checked above, so `None` from `push_digit` can only
        // mean the value no longer fits in `u64` units.
        let mut units: u64 = 0;
        for &byte in integer {
            units = push_digit(units, byte).ok_or(AmountError::TooLarge)?;
        }
        for position in 0..SCALE {
            let byte = kept.get(position).copied().unwrap_or(b'0');
            units = push_digit(units, byte).ok_or(AmountError::TooLarge)?;
        }

        NonZeroU64::new(units).map(Self).ok_or(AmountError::Zero)
    }
}

impl Display for Amount {
    /// Prints the amount in the layout of ADR-0009 O5, such as `1.5000`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Balance::from(*self).fmt(f)
    }
}

/// A signed account balance in `0.0001` units (ADR-0007 M2).
///
/// Balances may be negative (ADR-0006 R2). The `i128` range cannot be reached
/// by valid input (ADR-0007, Range), but every change is still checked and
/// returns `None` instead of wrapping, saturating, or panicking (M7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Balance(i128);

impl Balance {
    /// A balance of `0.0000`.
    pub const ZERO: Self = Self(0);

    /// Returns `self + rhs`, or `None` if the result would overflow.
    #[must_use]
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    /// Returns `self - rhs`, or `None` if the result would overflow.
    #[must_use]
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }
}

impl From<Amount> for Balance {
    /// Widens an amount to a balance. The conversion is lossless (M7).
    fn from(amount: Amount) -> Self {
        Self(i128::from(amount.0.get()))
    }
}

impl Display for Balance {
    /// Prints the balance in the layout of ADR-0009 O5 and O6.
    ///
    /// The output is an optional `-`, the integer part without leading zeros,
    /// `.`, and exactly four fractional digits: `0.0000`, `-0.5000`,
    /// `2.0001`. The sign comes from the signed unit count and the digits from
    /// its unsigned magnitude, so values between `-1` and `0` keep their sign,
    /// zero never prints as `-0.0000`, and `i128::MIN` cannot overflow.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let magnitude = self.0.unsigned_abs();
        let sign = if self.0 < 0 { "-" } else { "" };
        let whole = magnitude / UNITS_PER_WHOLE;
        let fraction = magnitude % UNITS_PER_WHOLE;
        write!(f, "{sign}{whole}.{fraction:0SCALE$}")
    }
}

/// Whether `bytes` is one or more ASCII digits.
fn is_digits(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)
}

/// Returns `units * 10 + digit`, or `None` if `byte` is not an ASCII digit or
/// the result does not fit in `u64`.
fn push_digit(units: u64, byte: u8) -> Option<u64> {
    let digit = char::from(byte).to_digit(10)?;
    units.checked_mul(10)?.checked_add(u64::from(digit))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{Amount, AmountError, Balance};

    /// Parses a field given as text.
    fn parse(field: &str) -> Result<Amount, AmountError> {
        Amount::parse(field.as_bytes())
    }

    /// The unit count of an amount parsed from `field`.
    fn units(field: &str) -> u64 {
        parse(field).unwrap().0.get()
    }

    /// A balance of `units` units.
    fn balance(units: i128) -> Balance {
        Balance(units)
    }

    /// Decodes the ADR-0009 O5 layout back into a signed unit count, with
    /// checked integer arithmetic only (ADR-0009, Confirmation).
    ///
    /// Returns `None` for any string that does not match
    /// `-?(0|[1-9][0-9]*)\.[0-9]{4}` or whose value does not fit in `i128`.
    /// This is a test oracle for the balance formatter and is separate from
    /// the amount parser, which rejects zero, negative values, and values
    /// above `u64::MAX` units.
    fn decode_balance(text: &str) -> Option<i128> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, fraction) = digits.split_once('.')?;
        let whole_is_canonical = whole == "0" || !whole.starts_with('0');
        let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !whole_is_canonical || !all_digits(whole) || !all_digits(fraction) || fraction.len() != 4
        {
            return None;
        }
        let mut magnitude: u128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            let digit = u128::from(char::from(byte).to_digit(10)?);
            magnitude = magnitude.checked_mul(10)?.checked_add(digit)?;
        }
        if negative {
            0_i128.checked_sub_unsigned(magnitude)
        } else {
            i128::try_from(magnitude).ok()
        }
    }

    // ---- Accepted forms (ADR-0007 M3, M4) ----

    #[test]
    fn integer_and_decimal_forms_are_accepted() {
        assert_eq!(units("1"), 10_000);
        assert_eq!(units("1.0"), 10_000);
        assert_eq!(units("1.5"), 15_000);
        assert_eq!(units("1.23"), 12_300);
        assert_eq!(units("1.2345"), 12_345);
        assert_eq!(units("0.5"), 5_000);
        assert_eq!(units("42"), 420_000);
    }

    /// Covers: A19, I7.
    #[test]
    fn a19_extra_zero_fractional_digits_are_accepted_exactly() {
        assert_eq!(units("1.23450"), 12_345);
        assert_eq!(units("1.2345000000"), 12_345);
        assert_eq!(units("2.00000"), 20_000);
    }

    #[test]
    fn leading_zeros_are_accepted() {
        assert_eq!(units("007.5"), 75_000);
        assert_eq!(units("00.0001"), 1);
        let many_zeros = format!("{}1", "0".repeat(4_000));
        assert_eq!(units(&many_zeros), 10_000);
    }

    /// Covers: A22.
    #[test]
    fn a22_surrounding_ascii_whitespace_is_trimmed() {
        assert_eq!(units(" 2.5 "), 25_000);
        assert_eq!(units("\t2.5\r\n"), 25_000);
        assert_eq!(units("  1"), 10_000);
    }

    // ---- Range boundaries (ADR-0007 M5) ----

    /// Covers: I7.
    #[test]
    fn i7_smallest_amount_is_one_unit() {
        assert_eq!(parse("0.0001"), Ok(Amount::MIN));
        assert_eq!(Amount::MIN.0.get(), 1);
    }

    /// Covers: A21, I7.
    #[test]
    fn a21_largest_amount_is_u64_max_units() {
        assert_eq!(parse("1844674407370955.1615"), Ok(Amount::MAX));
        assert_eq!(Amount::MAX.0.get(), u64::MAX);
    }

    /// Covers: A21.
    #[test]
    fn a21_one_unit_above_the_maximum_is_too_large() {
        assert_eq!(parse("1844674407370955.1616"), Err(AmountError::TooLarge));
        assert_eq!(parse("1844674407370956"), Err(AmountError::TooLarge));
        assert_eq!(parse("18446744073709551615"), Err(AmountError::TooLarge));
        assert_eq!(
            parse("99999999999999999999999999999999999999999"),
            Err(AmountError::TooLarge)
        );
    }

    /// Covers: A20.
    #[test]
    fn a20_zero_is_rejected_in_every_form() {
        for field in ["0", "0.0", "0.0000", "000.00000", "0.000000000"] {
            assert_eq!(parse(field), Err(AmountError::Zero), "{field:?}");
        }
    }

    // ---- Rejected forms (ADR-0007 M3, M4) ----

    #[test]
    fn empty_and_blank_fields_are_rejected() {
        for field in ["", " ", "\t", " \r\n "] {
            assert_eq!(parse(field), Err(AmountError::Empty), "{field:?}");
        }
    }

    /// Covers: A17, A18.
    #[test]
    fn a17_a18_signed_amounts_are_rejected() {
        for field in ["-5", "+5", "-0.5", "+0.0001", "- 5", "5-"] {
            assert_eq!(parse(field), Err(AmountError::Syntax), "{field:?}");
        }
    }

    /// Covers: A22.
    #[test]
    fn a22_non_canonical_decimal_forms_are_rejected() {
        let fields = [
            ".5", "5.", ".", "1.2.3", "1..2", "1e3", "1E3", "1.5e2", "1_000", "1,000", "1,5",
            "0x10", "1 000", "1. 5", "1 .5",
        ];
        for field in fields {
            assert_eq!(parse(field), Err(AmountError::Syntax), "{field:?}");
        }
    }

    #[test]
    fn named_non_numbers_are_rejected() {
        for field in ["NaN", "nan", "inf", "-inf", "Infinity", "abc"] {
            assert_eq!(parse(field), Err(AmountError::Syntax), "{field:?}");
        }
    }

    #[test]
    fn non_ascii_digits_and_whitespace_are_rejected() {
        // Fullwidth five, Arabic-Indic five, and a no-break space before 5.
        for field in ["\u{ff15}", "\u{0665}", "\u{a0}5", "5\u{2009}"] {
            assert_eq!(parse(field), Err(AmountError::Syntax), "{field:?}");
        }
        assert_eq!(Amount::parse(b"\xff5"), Err(AmountError::Syntax));
        assert_eq!(Amount::parse(b"5\x00"), Err(AmountError::Syntax));
    }

    /// Covers: A19, I7.
    #[test]
    fn a19_non_zero_digits_after_the_fourth_are_too_precise() {
        for field in ["1.00005", "0.00001", "1.23456", "1.000000001", "0.00015"] {
            assert_eq!(parse(field), Err(AmountError::TooPrecise), "{field:?}");
        }
    }

    #[test]
    fn syntax_is_checked_before_precision_and_range() {
        assert_eq!(parse("1.00005x"), Err(AmountError::Syntax));
        assert_eq!(parse("-99999999999999999999"), Err(AmountError::Syntax));
    }

    // ---- Formatting (ADR-0009 O5, O6) ----

    /// The formatter cases listed in ADR-0009, Confirmation.
    #[test]
    fn balance_formatter_cases_of_adr_0009() {
        let cases = [
            (0, "0.0000"),
            (1, "0.0001"),
            (-1, "-0.0001"),
            (-5_000, "-0.5000"),
            (-9_999, "-0.9999"),
            (-10_000, "-1.0000"),
            (15_000, "1.5000"),
            (
                i128::from(u64::MAX).checked_add(1).unwrap(),
                "1844674407370955.1616",
            ),
            (36_893_488_147_419_103_230, "3689348814741910.3230"),
            (i128::MIN, "-17014118346046923173168730371588410.5728"),
            (i128::MAX, "17014118346046923173168730371588410.5727"),
        ];
        for (units, expected) in cases {
            assert_eq!(balance(units).to_string(), expected, "{units}");
        }
    }

    #[test]
    fn zero_balance_is_never_negative_zero() {
        assert_eq!(Balance::ZERO.to_string(), "0.0000");
        assert_eq!(Balance::default(), Balance::ZERO);
    }

    #[test]
    fn amount_prints_with_four_fractional_digits() {
        assert_eq!(Amount::MIN.to_string(), "0.0001");
        assert_eq!(Amount::MAX.to_string(), "1844674407370955.1615");
        assert_eq!(parse("2").unwrap().to_string(), "2.0000");
        assert_eq!(parse("007.50").unwrap().to_string(), "7.5000");
    }

    // ---- Balance arithmetic (ADR-0007 M2, M7) ----

    #[test]
    fn amount_widens_to_an_equal_balance() {
        assert_eq!(Balance::from(Amount::MIN), balance(1));
        assert_eq!(Balance::from(Amount::MAX), balance(i128::from(u64::MAX)));
    }

    #[test]
    fn balances_add_and_subtract_exactly() {
        let deposit = Balance::from(parse("1.5").unwrap());
        let withdrawal = Balance::from(parse("2.0001").unwrap());
        let after = Balance::ZERO.checked_add(deposit).unwrap();
        assert_eq!(after, balance(15_000));
        let after = after.checked_sub(withdrawal).unwrap();
        assert_eq!(after, balance(-5_001));
        assert_eq!(after.to_string(), "-0.5001");
    }

    /// Covers: A21, I6.
    #[test]
    fn a21_two_maximum_deposits_do_not_overflow_a_balance() {
        let max = Balance::from(Amount::MAX);
        let total = Balance::ZERO
            .checked_add(max)
            .unwrap()
            .checked_add(max)
            .unwrap();
        assert_eq!(total.to_string(), "3689348814741910.3230");
    }

    /// Covers: I6.
    #[test]
    fn i6_balance_overflow_returns_none() {
        assert_eq!(balance(i128::MAX).checked_add(balance(1)), None);
        assert_eq!(balance(i128::MIN).checked_sub(balance(1)), None);
        assert_eq!(balance(i128::MIN).checked_add(balance(i128::MIN)), None);
        assert_eq!(balance(i128::MAX).checked_sub(balance(i128::MIN)), None);
        assert_eq!(
            balance(i128::MAX).checked_sub(balance(1)),
            Some(balance(i128::MAX.checked_sub(1).unwrap()))
        );
    }

    #[test]
    fn balances_compare_by_value() {
        let one = Balance::from(parse("1").unwrap());
        assert!(balance(-1) < Balance::ZERO);
        assert!(Balance::ZERO < one);
        assert!(Balance::from(parse("0.9999").unwrap()) < one);
    }

    // ---- Property tests (ADR-0007 and ADR-0009, Confirmation) ----

    /// Bytes that reach every branch of the parser.
    fn amount_like_byte() -> impl Strategy<Value = u8> {
        prop::sample::select(vec![
            b'0', b'0', b'1', b'5', b'9', b'.', b'.', b' ', b'\t', b'-', b'+', b'e', b'_', b',',
            0xc3, 0xa9, 0xff,
        ])
    }

    /// Balances that always include zero, values between `-1` and `0`, and
    /// values above `u64::MAX` units, besides any `i128`.
    fn any_balance_units() -> impl Strategy<Value = i128> {
        prop_oneof![
            Just(0_i128),
            -9_999_i128..=-1,
            -20_000_i128..=20_000,
            i128::from(u64::MAX)..,
            Just(i128::MIN),
            Just(i128::MAX),
            any::<i128>(),
        ]
    }

    proptest! {
        /// ADR-0012 S12: the parser returns a result on any input, never panics.
        #[test]
        fn parse_never_panics_on_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            let _ = Amount::parse(&bytes);
        }

        /// ADR-0012 S12, with bytes chosen to reach the deeper branches.
        #[test]
        fn parse_never_panics_on_amount_like_bytes(
            bytes in prop::collection::vec(amount_like_byte(), 0..48),
        ) {
            if let Ok(amount) = Amount::parse(&bytes) {
                prop_assert!(amount >= Amount::MIN);
            }
        }

        /// Covers: I7. Formatting then parsing returns the same amount.
        #[test]
        fn i7_amount_round_trips_through_display(units in 1..=u64::MAX) {
            let amount = Amount(std::num::NonZeroU64::new(units).unwrap());
            prop_assert_eq!(Amount::parse(amount.to_string().as_bytes()), Ok(amount));
        }

        /// Covers: I7. Any well-formed field parses to exactly its value,
        /// computed independently in `u128`, or to the matching range error.
        #[test]
        fn i7_well_formed_fields_parse_to_their_exact_value(
            leading_zeros in 0_usize..4,
            whole in any::<u64>(),
            fraction in prop::option::of(0_u32..10_000),
            fraction_width in 1_usize..=4,
            trailing_zeros in 0_usize..4,
            padding in prop::sample::select(vec!["", " ", "\t", " \r\n"]),
        ) {
            let mut field = format!("{padding}{}{whole}", "0".repeat(leading_zeros));
            let mut fraction_units = 0_u128;
            if let Some(fraction) = fraction {
                // Print `fraction` as exactly four digits, then drop up to
                // `4 - fraction_width` trailing zeros so short forms appear.
                let digits = format!("{fraction:04}");
                let trimmed = digits.trim_end_matches('0');
                let shown = if trimmed.len() >= fraction_width {
                    trimmed
                } else {
                    digits.get(..fraction_width).unwrap()
                };
                field.push('.');
                field.push_str(shown);
                field.push_str(&"0".repeat(trailing_zeros));
                fraction_units = u128::from(fraction);
            }
            field.push_str(padding);

            let expected = u128::from(whole)
                .checked_mul(10_000)
                .unwrap()
                .checked_add(fraction_units)
                .unwrap();
            let result = Amount::parse(field.as_bytes());
            match u64::try_from(expected) {
                Ok(0) => prop_assert_eq!(result, Err(AmountError::Zero)),
                Ok(units) => prop_assert_eq!(result.map(|a| a.0.get()), Ok(units)),
                Err(_) => prop_assert_eq!(result, Err(AmountError::TooLarge)),
            }
        }

        /// ADR-0009 Confirmation: every balance prints in the O5 layout, with
        /// a `-` exactly when it is negative, and decodes to the same value.
        #[test]
        fn balance_format_is_canonical_signed_and_exact(units in any_balance_units()) {
            let text = balance(units).to_string();
            prop_assert_eq!(text.starts_with('-'), units < 0);
            prop_assert_eq!(decode_balance(&text), Some(units), "{}", text);
        }
    }

    #[test]
    fn decode_balance_rejects_non_canonical_text() {
        for text in [
            "-0.000", "00.0000", "+1.0000", "1.00000", "1", "1.", ".0000", "1e3.0000",
        ] {
            assert_eq!(decode_balance(text), None, "{text:?}");
        }
        assert_eq!(decode_balance("-0.5000"), Some(-5_000));
    }
}
