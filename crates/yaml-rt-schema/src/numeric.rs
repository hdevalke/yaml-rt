use std::cmp::Ordering;

use crate::Value;

/// Exact decimal representation using arbitrary-precision digits and an i64
/// base-10 exponent.
struct Decimal {
    digits: Box<[u8]>,
    exponent: i64,
    negative: bool,
}

impl Decimal {
    fn parse(value: &Value) -> Option<Self> {
        let text = value.as_number()?.to_string();
        let (mantissa, exponent) = text
            .split_once(['e', 'E'])
            .map_or((text.as_str(), "0"), |(a, b)| (a, b));
        let negative = mantissa.starts_with('-');
        let scale = match mantissa.split_once('.') {
            Some((_, fraction)) => i64::try_from(fraction.len()).ok()?,
            None => 0,
        };
        let mut digits: Vec<u8> = mantissa
            .bytes()
            .filter(|b| b.is_ascii_digit())
            .map(|b| b - b'0')
            .collect();
        let first = digits
            .iter()
            .position(|digit| *digit != 0)
            .unwrap_or(digits.len());
        if first == digits.len() {
            return Some(Self {
                negative: false,
                digits: Box::new([0]),
                exponent: 0,
            });
        }
        digits.drain(..first);
        let mut exponent = exponent.parse::<i64>().ok()?.checked_sub(scale)?;
        while digits.len() > 1 && digits.last() == Some(&0) {
            digits.pop();
            exponent = exponent.checked_add(1)?;
        }
        Some(Self {
            negative,
            digits: digits.into_boxed_slice(),
            exponent,
        })
    }

    fn compare(&self, other: &Self) -> Ordering {
        if self.is_zero() || other.is_zero() {
            return match (self.is_zero(), other.is_zero()) {
                (true, true) => Ordering::Equal,
                (true, false) => {
                    if other.negative {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                (false, true) => {
                    if self.negative {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                _ => unreachable!(),
            };
        }
        if self.negative != other.negative {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let magnitude = self.compare_magnitude(other);
        if self.negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }

    fn is_zero(&self) -> bool {
        self.digits.as_ref() == [0]
    }

    fn compare_magnitude(&self, other: &Self) -> Ordering {
        let left_order = self.digits.len() as i128 + self.exponent as i128;
        let right_order = other.digits.len() as i128 + other.exponent as i128;
        left_order.cmp(&right_order).then_with(|| {
            let len = self.digits.len().max(other.digits.len());
            (0..len)
                .map(|index| {
                    self.digits
                        .get(index)
                        .copied()
                        .unwrap_or(0)
                        .cmp(&other.digits.get(index).copied().unwrap_or(0))
                })
                .find(|result| *result != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        })
    }

    fn is_integer(&self) -> bool {
        self.exponent >= 0
    }

    fn multiple_of(&self, other: &Self) -> bool {
        if other.is_zero() {
            return false;
        }
        if self.is_zero() {
            return true;
        }
        if self.exponent < other.exponent {
            return false;
        }
        let exponent = (self.exponent as i128 - other.exponent as i128) as u64;
        let remainder = remainder_digits(&self.digits, &other.digits);
        pow10_mul_mod(remainder, exponent, &other.digits) == [0]
    }
}

fn pow10_mul_mod(mut value: Vec<u8>, mut exponent: u64, modulus: &[u8]) -> Vec<u8> {
    let mut power = remainder_digits(&[1, 0], modulus);
    while exponent != 0 && value != [0] {
        if exponent & 1 != 0 {
            value = multiply_mod(&value, &power, modulus);
        }
        exponent >>= 1;
        if exponent != 0 {
            power = multiply_mod(&power, &power, modulus);
        }
    }
    value
}

fn remainder_digits(digits: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut remainder = vec![0];
    for &digit in digits {
        if remainder == [0] {
            remainder[0] = digit;
        } else {
            remainder.push(digit);
        }
        while compare_digits(&remainder, divisor) != Ordering::Less {
            subtract_digits(&mut remainder, divisor);
        }
    }
    remainder
}

fn multiply_mod(left: &[u8], right: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut product = vec![0; left.len() + right.len()];
    for i in (0..left.len()).rev() {
        let mut carry = 0u16;
        for j in (0..right.len()).rev() {
            let index = i + j + 1;
            let value = left[i] as u16 * right[j] as u16 + product[index] as u16 + carry;
            product[index] = (value % 10) as u8;
            carry = value / 10;
        }
        product[i] = carry as u8;
    }
    remainder_digits(&product, divisor)
}

fn compare_digits(left: &[u8], right: &[u8]) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn subtract_digits(left: &mut Vec<u8>, right: &[u8]) {
    let mut borrow = 0;
    for index in 0..left.len() {
        let offset = left.len() - 1 - index;
        let digit = left[offset] as i16
            - right
                .len()
                .checked_sub(index + 1)
                .map_or(0, |offset| right[offset]) as i16
            - borrow;
        left[offset] = digit.rem_euclid(10) as u8;
        borrow = if digit < 0 { 1 } else { 0 };
    }
    let first = left
        .iter()
        .position(|digit| *digit != 0)
        .unwrap_or(left.len() - 1);
    left.drain(..first);
}

pub(crate) fn is_integer(value: &Value) -> bool {
    Decimal::parse(value).is_some_and(|value| value.is_integer())
}
pub(crate) fn equal(left: &Value, right: &Value) -> Option<bool> {
    Some(Decimal::parse(left)?.compare(&Decimal::parse(right)?) == Ordering::Equal)
}
pub(crate) fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    Some(Decimal::parse(left)?.compare(&Decimal::parse(right)?))
}
pub(crate) fn multiple_of(left: &Value, right: &Value) -> Option<bool> {
    Some(Decimal::parse(left)?.multiple_of(&Decimal::parse(right)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_large_and_fractional_arithmetic() {
        let number = |text| Value::parse(text).unwrap();
        assert_eq!(equal(&number("1e100"), &number("10e99")), Some(true));
        assert_eq!(multiple_of(&number("0.3"), &number("0.1")), Some(true));
        assert_eq!(multiple_of(&number("0.31"), &number("0.1")), Some(false));
        assert_eq!(
            compare(&number("0"), &number("0.0001")),
            Some(Ordering::Less)
        );
        assert!(is_integer(&number("100.000")));
    }

    #[test]
    fn canonical_values_and_exponent_extremes() {
        let number = |text| Value::parse(text).unwrap();
        assert_eq!(equal(&number("100.000"), &number("1e2")), Some(true));
        assert_eq!(equal(&number("-0.000"), &number("0e100")), Some(true));
        assert_eq!(
            compare(&number("-1e1000000"), &number("-1e-1000000")),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare(&number("1e1000000"), &number("1e-1000000")),
            Some(Ordering::Greater)
        );
        assert_eq!(
            equal(&number("9007199254740993"), &number("9007199254740992")),
            Some(false)
        );
    }

    #[test]
    fn fractional_and_large_scale_multiples() {
        let number = |text| Value::parse(text).unwrap();
        assert_eq!(multiple_of(&number("-0.3"), &number("0.1")), Some(true));
        assert_eq!(multiple_of(&number("0.3"), &number("-0.1")), Some(true));
        assert_eq!(multiple_of(&number("0.01"), &number("0.1")), Some(false));
        assert_eq!(multiple_of(&number("0"), &number("0.1")), Some(true));
        assert_eq!(multiple_of(&number("-0"), &number("0")), Some(false));
        assert_eq!(multiple_of(&number("1e100001"), &number("2")), Some(true));
        assert_eq!(multiple_of(&number("1e100001"), &number("3")), Some(false));
        assert_eq!(
            multiple_of(&number("1e1000000000"), &number("7")),
            Some(false)
        );
        assert_eq!(
            multiple_of(&number("7e1000000000"), &number("7")),
            Some(true)
        );
        assert_eq!(
            multiple_of(
                &number("1e9223372036854775807"),
                &number("3e-9223372036854775808")
            ),
            Some(false)
        );
        for numerator in 1..30u128 {
            for divisor in 1..30u128 {
                for exponent in 0..12u32 {
                    let value = Value::parse(&format!("{numerator}e{exponent}")).unwrap();
                    let bound = Value::parse(&divisor.to_string()).unwrap();
                    assert_eq!(
                        multiple_of(&value, &bound),
                        Some(numerator * 10u128.pow(exponent) % divisor == 0),
                        "{numerator}e{exponent} / {divisor}"
                    );
                }
            }
        }
    }
}
