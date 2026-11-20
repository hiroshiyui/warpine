// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX arithmetic (ADR-0083): decimal numbers of any size, rounded to
// NUMERIC DIGITS significant digits, as the language defines them.
//
// A number is a sign, a coefficient (decimal digits, most significant
// first) and an exponent: `coefficient × 10^exponent`. Operations work on
// the exact values, round the result to `digits` significant digits
// (half up, away from zero) and keep the coefficient's trailing zeros
// except for division, so that `1.50 + 1` is `2.50` and `6 / 2` is `3`.

use std::cmp::Ordering;

/// The most NUMERIC DIGITS a script may ask for: the cost of an operation
/// grows with it.
pub const MAX_DIGITS: usize = 1000;

/// Why an arithmetic operation failed (the REXX error it raises).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithError {
    /// Error 42: arithmetic overflow or underflow (an exponent out of range).
    Overflow,
    /// Error 42: division by zero.
    DivideByZero,
    /// Error 26: a whole number was needed (power's exponent, `%` result).
    NotWhole,
}

/// The largest exponent a result may have; REXX's limit is about 10^9.
const MAX_EXPONENT: i64 = 999_999_999;

/// A REXX number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Num {
    neg: bool,
    /// Decimal digits 0–9, most significant first; never empty, no
    /// leading zeros except the single `0` of zero.
    coef: Vec<u8>,
    exp: i64,
}

impl Num {
    pub fn zero() -> Num { Num { neg: false, coef: vec![0], exp: 0 } }

    pub fn from_i64(n: i64) -> Num {
        let neg = n < 0;
        let coef = n.unsigned_abs().to_string().bytes().map(|b| b - b'0').collect();
        Num { neg, coef, exp: 0 }.normalized()
    }

    pub fn is_zero(&self) -> bool { self.coef == [0] }

    pub fn is_negative(&self) -> bool { self.neg && !self.is_zero() }

    /// Parse REXX's number syntax: blanks, an optional sign (blanks may
    /// follow it), digits with an optional point (at least one digit), an
    /// optional exponent `E[+|-]digits`, blanks. `None` for anything else.
    pub fn parse(s: &[u8]) -> Option<Num> {
        let s = trim_blanks(s);
        let (neg, rest) = match s.first() {
            Some(b'-') => (true, trim_blanks(&s[1..])),
            Some(b'+') => (false, trim_blanks(&s[1..])),
            _ => (false, s),
        };
        let mut coef = Vec::new();
        let mut exp: i64 = 0;
        let mut seen_point = false;
        let mut digits = 0usize;
        let mut i = 0;
        while i < rest.len() {
            match rest[i] {
                b'0'..=b'9' => {
                    coef.push(rest[i] - b'0');
                    digits += 1;
                    if seen_point { exp -= 1; }
                }
                b'.' if !seen_point => seen_point = true,
                _ => break,
            }
            i += 1;
        }
        if digits == 0 { return None; }
        if i < rest.len() {
            if !matches!(rest[i], b'e' | b'E') { return None; }
            i += 1;
            let mut eneg = false;
            if i < rest.len() && matches!(rest[i], b'+' | b'-') {
                eneg = rest[i] == b'-';
                i += 1;
            }
            let start = i;
            let mut e: i64 = 0;
            while i < rest.len() && rest[i].is_ascii_digit() {
                e = e.checked_mul(10)?.checked_add((rest[i] - b'0') as i64)?;
                if e > MAX_EXPONENT * 10 { return None; }
                i += 1;
            }
            if i == start || i != rest.len() { return None; }
            exp += if eneg { -e } else { e };
        }
        Some(Num { neg, coef, exp }.normalized())
    }

    /// Leading zeros removed; zero made positive with exponent 0.
    fn normalized(mut self) -> Num {
        let first = self.coef.iter().position(|&d| d != 0);
        match first {
            None => Num::zero(),
            Some(i) => {
                self.coef.drain(..i);
                self
            }
        }
    }

    /// Rounded to `digits` significant digits, half up (away from zero).
    pub fn round(mut self, digits: usize) -> Num {
        if self.coef.len() <= digits { return self; }
        let drop = self.coef.len() - digits;
        let round_up = self.coef[digits] >= 5;
        self.coef.truncate(digits);
        self.exp += drop as i64;
        if round_up {
            let mut i = digits;
            loop {
                if i == 0 {
                    self.coef.insert(0, 1);
                    self.coef.pop();
                    self.exp += 1;
                    break;
                }
                i -= 1;
                if self.coef[i] == 9 {
                    self.coef[i] = 0;
                } else {
                    self.coef[i] += 1;
                    break;
                }
            }
        }
        self.normalized()
    }

    fn check(self) -> Result<Num, ArithError> {
        let top = self.exp + self.coef.len() as i64 - 1;
        if !self.is_zero() && !(-MAX_EXPONENT..=MAX_EXPONENT).contains(&top) {
            return Err(ArithError::Overflow);
        }
        Ok(self)
    }

    /// Trailing zeros of the coefficient removed (the exponent grows).
    fn stripped(mut self) -> Num {
        while self.coef.len() > 1 && *self.coef.last().unwrap() == 0 {
            self.coef.pop();
            self.exp += 1;
        }
        self
    }

    /// The magnitudes of `a` and `b` as coefficients on a common exponent.
    fn aligned(a: &Num, b: &Num) -> (Vec<u8>, Vec<u8>, i64) {
        let exp = a.exp.min(b.exp);
        let pad = |n: &Num| {
            let mut c = n.coef.clone();
            c.extend(std::iter::repeat_n(0, (n.exp - exp) as usize));
            c
        };
        (pad(a), pad(b), exp)
    }

    /// `a + b`, rounded to `digits`.
    pub fn add(&self, b: &Num, digits: usize) -> Result<Num, ArithError> {
        if self.is_zero() { return b.clone().round(digits).check(); }
        if b.is_zero() { return self.clone().round(digits).check(); }
        // An operand far below the other's precision only rounds it: keep a
        // tiny stand-in for it so that rounding still sees it, without
        // aligning across a huge exponent gap.
        let (a, b) = (self.limited_against(b, digits), b.limited_against(self, digits));
        let (x, y, exp) = Num::aligned(&a, &b);
        let (neg, coef) = if a.neg == b.neg {
            (a.neg, add_mag(&x, &y))
        } else {
            match cmp_mag(&x, &y) {
                Ordering::Equal => return Ok(Num::zero()),
                Ordering::Greater => (a.neg, sub_mag(&x, &y)),
                Ordering::Less => (b.neg, sub_mag(&y, &x)),
            }
        };
        Num { neg, coef, exp }.normalized().round(digits).check()
    }

    /// `self`, or when it lies wholly below what `other` can show at
    /// `digits` digits, a single digit just below that point with the same
    /// sign (it can only round the sum).
    fn limited_against(&self, other: &Num, digits: usize) -> Num {
        let other_top = other.exp + other.coef.len() as i64;
        let floor = other_top - digits as i64 - 2;
        let self_top = self.exp + self.coef.len() as i64;
        if self_top < floor {
            Num { neg: self.neg, coef: vec![1], exp: floor - 1 }
        } else {
            self.clone()
        }
    }

    pub fn sub(&self, b: &Num, digits: usize) -> Result<Num, ArithError> {
        self.add(&b.negated(), digits)
    }

    pub fn negated(&self) -> Num {
        if self.is_zero() { return Num::zero(); }
        Num { neg: !self.neg, ..self.clone() }
    }

    pub fn abs(&self) -> Num { Num { neg: false, ..self.clone() } }

    /// `a * b`, rounded to `digits`.
    pub fn mul(&self, b: &Num, digits: usize) -> Result<Num, ArithError> {
        // Operands longer than the result can show are rounded first (a
        // few guard digits keep the result's rounding right).
        let a = self.clone().round(digits + 2);
        let b = b.clone().round(digits + 2);
        let mut out = vec![0u32; a.coef.len() + b.coef.len()];
        for (i, &x) in a.coef.iter().enumerate().rev() {
            for (j, &y) in b.coef.iter().enumerate().rev() {
                out[i + j + 1] += (x as u32) * (y as u32);
            }
        }
        for k in (1..out.len()).rev() {
            let carry = out[k] / 10;
            out[k] %= 10;
            out[k - 1] += carry;
        }
        let coef = out.into_iter().map(|d| d as u8).collect();
        Num { neg: a.neg != b.neg, coef, exp: a.exp + b.exp }.normalized().round(digits).check()
    }

    /// `a / b`, rounded to `digits`, trailing zeros removed.
    pub fn div(&self, b: &Num, digits: usize) -> Result<Num, ArithError> {
        if b.is_zero() { return Err(ArithError::DivideByZero); }
        if self.is_zero() { return Ok(Num::zero()); }
        let a = self.clone().round(digits + 2);
        let b = b.clone().round(digits + 2);
        // Scale the dividend so the quotient has digits + 1 digits.
        let shift = (digits as i64 + 1) - (a.coef.len() as i64 - b.coef.len() as i64) + 1;
        let mut dividend = a.coef.clone();
        if shift > 0 { dividend.extend(std::iter::repeat_n(0, shift as usize)); }
        let (q, r) = divmod_mag(&dividend, &b.coef);
        let mut coef = q;
        // A non-zero remainder only matters past the rounding digit; a
        // final sticky 1 keeps "exactly half" from rounding wrongly.
        if r.iter().any(|&d| d != 0) { coef.push(1); }
        let extra = if r.iter().any(|&d| d != 0) { 1 } else { 0 };
        let exp = a.exp - b.exp - shift.max(0) - extra;
        let n = Num { neg: a.neg != b.neg, coef, exp }.normalized().round(digits);
        n.stripped().check()
    }

    /// `a % b`: the integer part of the quotient (Error 26 when it needs
    /// more than `digits` digits).
    pub fn int_div(&self, b: &Num, digits: usize) -> Result<Num, ArithError> {
        let (q, _) = self.div_rem(b, digits)?;
        Ok(q)
    }

    /// `a // b`: the remainder, with the sign of `a`.
    pub fn rem(&self, b: &Num, digits: usize) -> Result<Num, ArithError> {
        let (_, r) = self.div_rem(b, digits)?;
        Ok(r)
    }

    fn div_rem(&self, b: &Num, digits: usize) -> Result<(Num, Num), ArithError> {
        if b.is_zero() { return Err(ArithError::DivideByZero); }
        let a = self.clone().round(digits);
        let b = b.clone().round(digits);
        let (x, y, exp) = Num::aligned(&a, &b);
        let (q, r) = divmod_mag(&x, &y);
        let q = Num { neg: a.neg != b.neg, coef: q, exp: 0 }.normalized();
        if q.coef.len() > digits { return Err(ArithError::NotWhole); }
        let r = Num { neg: a.neg, coef: r, exp }.normalized();
        Ok((q, r.round(digits).check()?))
    }

    /// `a ** n` for a whole `n` (Error 26 otherwise).
    pub fn pow(&self, n: &Num, digits: usize) -> Result<Num, ArithError> {
        let n = n.to_whole(digits).ok_or(ArithError::NotWhole)?;
        if n == 0 { return Ok(Num::from_i64(1)); }
        let work = digits + n.unsigned_abs().to_string().len() + 1;
        let mut result = Num::from_i64(1);
        let mut base = self.clone();
        let mut e = n.unsigned_abs();
        while e > 0 {
            if e & 1 == 1 { result = result.mul(&base, work)?; }
            e >>= 1;
            if e > 0 { base = base.mul(&base, work)?; }
        }
        if n < 0 {
            Num::from_i64(1).div(&result, digits)
        } else {
            result.round(digits).check()
        }
    }

    /// The value as a whole number, if it is one that fits `digits` digits
    /// and an `i64`.
    pub fn to_whole(&self, digits: usize) -> Option<i64> {
        let n = self.clone().round(digits);
        if n.exp < 0 {
            // Whole only if the fraction digits are zeros.
            let cut = (n.coef.len() as i64 + n.exp).max(0) as usize;
            if n.coef[cut..].iter().any(|&d| d != 0) { return None; }
            let int: i64 = n.coef[..cut].iter().try_fold(0i64, |acc, &d| acc.checked_mul(10)?.checked_add(d as i64))?;
            return Some(if n.neg { -int } else { int });
        }
        let mut int: i64 = n.coef.iter().try_fold(0i64, |acc, &d| acc.checked_mul(10)?.checked_add(d as i64))?;
        for _ in 0..n.exp { int = int.checked_mul(10)?; }
        Some(if n.neg { -int } else { int })
    }

    /// The integer part, the fraction cut off (TRUNC, `%`).
    pub fn trunc(&self) -> Num {
        if self.exp >= 0 { return self.clone(); }
        let keep = self.coef.len() as i64 + self.exp;
        if keep <= 0 { return Num::zero(); }
        Num { neg: self.neg, coef: self.coef[..keep as usize].to_vec(), exp: 0 }.normalized()
    }

    /// Compare two numbers by value.
    pub fn cmp_value(&self, b: &Num) -> Ordering {
        match (self.is_negative(), b.is_negative()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        if self.is_zero() && b.is_zero() { return Ordering::Equal; }
        if self.is_zero() { return if b.neg { Ordering::Greater } else { Ordering::Less }; }
        if b.is_zero() { return if self.neg { Ordering::Less } else { Ordering::Greater }; }
        let mag = {
            let (ta, tb) = (self.exp + self.coef.len() as i64, b.exp + b.coef.len() as i64);
            if ta != tb { ta.cmp(&tb) } else {
                let (x, y, _) = Num::aligned(self, b);
                cmp_mag(&x, &y)
            }
        };
        if self.neg { mag.reverse() } else { mag }
    }

    /// The number as REXX writes it, `digits` deciding when exponential
    /// notation is used: when it would need more than `digits` places
    /// before the point or more than twice `digits` after it.
    pub fn format(&self, digits: usize) -> Vec<u8> {
        if self.is_zero() { return b"0".to_vec(); }
        let mut out = Vec::new();
        if self.neg { out.push(b'-'); }
        let len = self.coef.len() as i64;
        let before = len + self.exp; // places before the point
        let after = -self.exp;       // places after the point
        let digit = |d: &u8| b'0' + d;
        if self.exp == 0 {
            out.extend(self.coef.iter().map(digit));
        } else if before <= digits as i64 && after <= 2 * digits as i64 {
            if self.exp > 0 {
                out.extend(self.coef.iter().map(digit));
                out.extend(std::iter::repeat_n(b'0', self.exp as usize));
            } else if before > 0 {
                let b = before as usize;
                out.extend(self.coef[..b].iter().map(digit));
                out.push(b'.');
                out.extend(self.coef[b..].iter().map(digit));
            } else {
                out.extend_from_slice(b"0.");
                out.extend(std::iter::repeat_n(b'0', (-before) as usize));
                out.extend(self.coef.iter().map(digit));
            }
        } else {
            // Scientific: one digit before the point.
            out.push(digit(&self.coef[0]));
            if self.coef.len() > 1 {
                out.push(b'.');
                out.extend(self.coef[1..].iter().map(digit));
            }
            let e = before - 1;
            out.push(b'E');
            out.push(if e < 0 { b'-' } else { b'+' });
            out.extend_from_slice(e.unsigned_abs().to_string().as_bytes());
        }
        out
    }
}

fn trim_blanks(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(s.len());
    let end = s.iter().rposition(|&b| b != b' ' && b != b'\t').map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

fn cmp_mag(a: &[u8], b: &[u8]) -> Ordering {
    let a = &a[a.iter().position(|&d| d != 0).unwrap_or(a.len())..];
    let b = &b[b.iter().position(|&d| d != 0).unwrap_or(b.len())..];
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn add_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    let n = a.len().max(b.len());
    let mut out = vec![0u8; n + 1];
    let mut carry = 0u8;
    for k in 0..n {
        let x = if k < a.len() { a[a.len() - 1 - k] } else { 0 };
        let y = if k < b.len() { b[b.len() - 1 - k] } else { 0 };
        let s = x + y + carry;
        out[n - k] = s % 10;
        carry = s / 10;
    }
    out[0] = carry;
    out
}

/// `a - b` for `a >= b`.
fn sub_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = a.to_vec();
    let mut borrow = 0i8;
    for k in 0..a.len() {
        let y = if k < b.len() { b[b.len() - 1 - k] as i8 } else { 0 };
        let i = a.len() - 1 - k;
        let mut d = a[i] as i8 - y - borrow;
        borrow = 0;
        if d < 0 { d += 10; borrow = 1; }
        out[i] = d as u8;
    }
    out
}

/// Quotient and remainder of magnitudes (long division).
fn divmod_mag(a: &[u8], b: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut q = Vec::with_capacity(a.len());
    let mut r: Vec<u8> = Vec::new();
    for &d in a {
        r.push(d);
        let lead = r.iter().position(|&x| x != 0).unwrap_or(r.len());
        r.drain(..lead);
        let mut count = 0u8;
        while cmp_mag(&r, b) != Ordering::Less {
            r = sub_mag(&r, b);
            let lead = r.iter().position(|&x| x != 0).unwrap_or(r.len());
            r.drain(..lead);
            count += 1;
        }
        q.push(count);
    }
    if r.is_empty() { r.push(0); }
    (q, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Num { Num::parse(s.as_bytes()).unwrap_or_else(|| panic!("{s} parses")) }
    fn f(x: Result<Num, ArithError>) -> String { String::from_utf8(x.unwrap().format(9)).unwrap() }

    #[test]
    fn numbers_parse_and_format() {
        for (s, want) in [("12", "12"), (" -3.50 ", "-3.50"), ("+ 7", "7"), ("0.0", "0"), (".5", "0.5"),
                          ("1E3", "1000"), ("1.5e-2", "0.015"), ("007", "7"), ("-0", "0")] {
            assert_eq!(String::from_utf8(n(s).format(9)).unwrap(), want, "{s}");
        }
        for bad in ["", "-", "1.2.3", "1e", "e5", "12abc", "1 2", "1E+", "."] {
            assert!(Num::parse(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[test]
    fn arithmetic_follows_rexx() {
        assert_eq!(f(n("1.50").add(&n("1"), 9)), "2.50");
        assert_eq!(f(n("12").sub(&n("7.00"), 9)), "5.00");
        assert_eq!(f(n("1.0").sub(&n("1.0"), 9)), "0");
        assert_eq!(f(n("1.5").mul(&n("2"), 9)), "3.0");
        assert_eq!(f(n("6").div(&n("2"), 9)), "3");
        assert_eq!(f(n("1").div(&n("3"), 9)), "0.333333333");
        assert_eq!(f(n("2").div(&n("3"), 9)), "0.666666667");
        assert_eq!(f(n("7").int_div(&n("2"), 9)), "3");
        assert_eq!(f(n("-7").int_div(&n("2"), 9)), "-3");
        assert_eq!(f(n("7").rem(&n("2"), 9)), "1");
        assert_eq!(f(n("-7").rem(&n("2"), 9)), "-1");
        assert_eq!(f(n("2").pow(&n("10"), 9)), "1024");
        assert_eq!(f(n("2").pow(&n("-2"), 9)), "0.25");
        assert_eq!(f(n("1.1").pow(&n("2"), 9)), "1.21");
        assert_eq!(n("2").pow(&n("0.5"), 9), Err(ArithError::NotWhole));
        assert_eq!(n("1").div(&n("0"), 9), Err(ArithError::DivideByZero));
        assert_eq!(f(n("123456789").add(&n("1"), 9)), "123456790");
        assert_eq!(f(n("999999999").add(&n("1"), 9)), "1.00000000E+9", "rounded, exponential");
        assert_eq!(f(n("1E-20").mul(&n("1"), 9)), "1E-20");
        assert_eq!(f(n("0.0000001").mul(&n("1"), 9)), "0.0000001");
        assert_eq!(f(n("1000000000").add(&n("0.4"), 9)), "1.00000000E+9");
        assert_eq!(f(n("1").add(&n("1E-30"), 9)), "1.00000000", "a tiny operand only rounds");
        assert_eq!(String::from_utf8(n("2").pow(&n("100"), 40).unwrap().format(40)).unwrap(),
                   "1267650600228229401496703205376");
    }

    #[test]
    fn rounding_is_half_up() {
        assert_eq!(f(n("1").div(&n("8"), 2).map(|x| x.round(2))), "0.13");
        assert_eq!(String::from_utf8(n("2.5").round(1).format(9)).unwrap(), "3");
        assert_eq!(String::from_utf8(n("-2.5").round(1).format(9)).unwrap(), "-3");
        assert_eq!(String::from_utf8(n("9.96").round(2).format(9)).unwrap(), "10");
    }

    #[test]
    fn comparisons_and_whole_numbers() {
        assert_eq!(n("1.0").cmp_value(&n("1")), Ordering::Equal);
        assert_eq!(n("-2").cmp_value(&n("1")), Ordering::Less);
        assert_eq!(n("10").cmp_value(&n("9.99")), Ordering::Greater);
        assert_eq!(n("-10").cmp_value(&n("-9")), Ordering::Less);
        assert_eq!(n("12.000").to_whole(9), Some(12));
        assert_eq!(n("12.5").to_whole(9), None);
        assert_eq!(n("1E3").to_whole(9), Some(1000));
        assert_eq!(String::from_utf8(n("-12.75").trunc().format(9)).unwrap(), "-12");
    }
}
