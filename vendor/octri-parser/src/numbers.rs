//! Exact decimal operations. Exponents stay digit strings; never expand an
//! attacker-controlled exponent into zeroes or round a value through binary64.
use serde_json::{Number, Value};
use std::cmp::Ordering;

#[derive(Debug, Clone)]
struct SignedDigits {
    negative: bool,
    digits: String,
}

impl SignedDigits {
    fn new(raw: &str) -> Self {
        let negative = raw.starts_with('-');
        let digits = raw.trim_start_matches(['+', '-']).trim_start_matches('0');
        Self {
            negative: negative && !digits.is_empty(),
            digits: if digits.is_empty() {
                "0".into()
            } else {
                digits.into()
            },
        }
    }

    fn shifted(mut self, amount: i64) -> Self {
        if amount == 0 {
            return self;
        }
        let other = amount.unsigned_abs().to_string();
        let negative = amount < 0;
        if self.negative == negative {
            self.digits = add(&self.digits, &other);
        } else {
            match magnitude(&self.digits, &other) {
                Ordering::Less => {
                    self.digits = subtract(&other, &self.digits);
                    self.negative = negative;
                }
                Ordering::Equal => {
                    self.digits = "0".into();
                    self.negative = false;
                }
                Ordering::Greater => self.digits = subtract(&self.digits, &other),
            }
        }
        self
    }

    fn compare(&self, other: &Self) -> Ordering {
        if self.negative != other.negative {
            return self.negative.cmp(&other.negative).reverse();
        }
        let order = magnitude(&self.digits, &other.digits);
        if self.negative {
            order.reverse()
        } else {
            order
        }
    }

    fn text(&self) -> String {
        format!("{}{}", if self.negative { "-" } else { "" }, self.digits)
    }
}

fn magnitude(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn add(left: &str, right: &str) -> String {
    let length = left.len().max(right.len());
    let mut digits = Vec::with_capacity(length + 1);
    let mut carry = 0;
    for (left, right) in left
        .bytes()
        .rev()
        .chain(std::iter::repeat(b'0'))
        .zip(right.bytes().rev().chain(std::iter::repeat(b'0')))
        .take(length)
    {
        let sum = left - b'0' + right - b'0' + carry;
        digits.push(b'0' + sum % 10);
        carry = sum / 10;
    }
    if carry != 0 {
        digits.push(b'0' + carry);
    }
    digits.into_iter().rev().map(char::from).collect()
}

/// Subtract nonnegative decimal integers with left >= right.
fn subtract(left: &str, right: &str) -> String {
    let mut digits = Vec::with_capacity(left.len());
    let mut borrow = 0i16;
    for (left, right) in left
        .bytes()
        .rev()
        .zip(right.bytes().rev().chain(std::iter::repeat(b'0')))
    {
        let mut digit = i16::from(left) - i16::from(right) - borrow;
        borrow = i16::from(digit < 0);
        if digit < 0 {
            digit += 10;
        }
        digits.push(b'0' + digit as u8);
    }
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    digits.into_iter().rev().map(char::from).collect()
}

#[derive(Debug)]
struct Decimal {
    negative: bool,
    digits: String,
    scale: SignedDigits,
}

impl Decimal {
    /// Input must already have JSON/YAML decimal grammar checked.
    fn new(raw: &str) -> Self {
        let negative = raw.starts_with('-');
        let raw = raw.trim_start_matches(['+', '-']);
        let (mantissa, exponent) = raw.split_once(['e', 'E']).unwrap_or((raw, "0"));
        let fraction = mantissa
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        let digits = mantissa.replace('.', "");
        let significant = digits.trim_start_matches('0');
        if significant.is_empty() {
            return Self {
                negative: false,
                digits: "0".into(),
                scale: SignedDigits::new("0"),
            };
        }
        let digits = significant.trim_end_matches('0').to_owned();
        let trailing = significant.len() - digits.len();
        let scale = SignedDigits::new(exponent).shifted(trailing as i64 - fraction as i64);
        Self {
            negative,
            digits,
            scale,
        }
    }

    fn zero(&self) -> bool {
        self.digits == "0"
    }

    fn token(&self) -> String {
        if self.zero() {
            return "0".into();
        }
        let sign = if self.negative { "-" } else { "" };
        // Keep ordinary values readable. Only a small, fixed exponent window
        // can allocate zeroes; large exponents retain scientific notation.
        if let Ok(scale) = self.scale.text().parse::<i64>()
            && (-32..=32).contains(&scale)
        {
            if scale >= 0 {
                return format!("{sign}{}{}", self.digits, "0".repeat(scale as usize));
            }
            let places = scale.unsigned_abs() as usize;
            if places < self.digits.len() {
                let split = self.digits.len() - places;
                return format!("{sign}{}.{}", &self.digits[..split], &self.digits[split..]);
            }
            return format!(
                "{sign}0.{}{}",
                "0".repeat(places - self.digits.len()),
                self.digits
            );
        }
        format!("{sign}{}e{}", self.digits, self.scale.text())
    }
}

/// Compare two validated decimal spellings without floating-point conversion.
pub fn compare_text(left: &str, right: &str) -> Ordering {
    let left = Decimal::new(left);
    let right = Decimal::new(right);
    if left.zero() || right.zero() {
        return match (left.zero(), right.zero()) {
            (true, true) => Ordering::Equal,
            (true, false) => {
                if right.negative {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            _ => {
                if left.negative {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
        };
    }
    if left.negative != right.negative {
        return if left.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let left_order = left.scale.clone().shifted(left.digits.len() as i64);
    let right_order = right.scale.clone().shifted(right.digits.len() as i64);
    let order = left_order.compare(&right_order).then_with(|| {
        let length = left.digits.len().max(right.digits.len());
        left.digits
            .bytes()
            .chain(std::iter::repeat(b'0'))
            .take(length)
            .cmp(
                right
                    .digits
                    .bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(length),
            )
    });
    if left.negative {
        order.reverse()
    } else {
        order
    }
}

pub fn compare(left: &Number, right: &Number) -> Ordering {
    compare_text(&left.to_string(), &right.to_string())
}

pub fn is_integer(number: &Number) -> bool {
    let value = Decimal::new(&number.to_string());
    value.zero() || !value.scale.negative
}

/// A stable exact spelling, independent of redundant zeroes and exponent style.
pub fn canonical(number: &Number) -> Number {
    // Decimal::token produces valid JSON grammar from a validated Number.
    // Keep the original exact value if a future serde version rejects a form.
    Decimal::new(&number.to_string())
        .token()
        .parse()
        .unwrap_or_else(|_| number.clone())
}

pub fn canonicalize(value: &mut Value) {
    match value {
        Value::Number(number) => *number = canonical(number),
        Value::Array(values) => values.iter_mut().for_each(canonicalize),
        Value::Object(values) => values.values_mut().for_each(canonicalize),
        _ => {}
    }
}

/// Compare JSON trees by exact decimal value while retaining key and array order.
pub fn equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => compare(left, right).is_eq(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| equivalent(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, left)| right.get(key).is_some_and(|right| equivalent(left, right)))
        }
        _ => left == right,
    }
}

/// Recognize YAML 1.2 decimal forms, including leading plus and `.5`/`5.`.
pub fn parse_decimal(raw: &str) -> Option<Number> {
    let unsigned = raw.strip_prefix(['+', '-']).unwrap_or(raw);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    if exponent.is_empty() || !exponent.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut digits = 0;
    let mut dots = 0;
    for byte in mantissa.bytes() {
        if byte.is_ascii_digit() {
            digits += 1;
        } else if byte == b'.' {
            dots += 1;
        } else {
            return None;
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    Decimal::new(raw).token().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_comparison_and_canonical_forms_do_not_expand_large_exponents() {
        for (left, right, order) in [
            ("0.100000000000000005", "0.1", Ordering::Greater),
            (
                "1e9223372036854775808",
                "1e9223372036854775807",
                Ordering::Greater,
            ),
            (
                "1e-9223372036854775808",
                "1e-9223372036854775809",
                Ordering::Greater,
            ),
            ("-1e400", "-1e399", Ordering::Less),
            ("-0.0", "0e99999999999999999999", Ordering::Equal),
            ("12300e-2", "123", Ordering::Equal),
        ] {
            assert_eq!(compare_text(left, right), order);
        }
        assert_eq!(Decimal::new("001.2300e+0002").token(), "123");
        assert_eq!(
            Decimal::new("1e99999999999999999999999999").token(),
            "1e99999999999999999999999999"
        );
    }
}
