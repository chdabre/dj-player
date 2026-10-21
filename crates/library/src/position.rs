//! Fractional positions: strings that sort (bytewise) in list order, so an insert or a move
//! writes one row, and inserts made apart (later: two devices) interleave without renumbering.
//!
//! A position is a base-36 fraction (`0-9a-z`) that never ends in `0`, so there is always
//! room before it. [`between`] makes a key strictly between two neighbours; [`after`] makes
//! one past the last, growing by one character every 35 appends rather than every few, so
//! a playlist built by appending keeps short keys.

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const BASE: usize = 36;

fn value(c: u8) -> usize {
    DIGITS.iter().position(|&d| d == c).unwrap_or(0)
}

fn digit(v: usize) -> char {
    DIGITS[v] as char
}

/// The first position of an empty list.
pub const FIRST: &str = "i";

/// A position strictly after `last` (or [`FIRST`] for an empty list): `last` with its final
/// digit stepped up, or with a `1` added when that digit is already `z`.
pub fn after(last: Option<&str>) -> String {
    let Some(last) = last.filter(|s| !s.is_empty()) else {
        return FIRST.into();
    };
    let mut bytes = last.as_bytes().to_vec();
    let end = bytes.len() - 1;
    let v = value(bytes[end]);
    if v + 1 < BASE {
        bytes[end] = DIGITS[v + 1];
    } else {
        bytes.push(b'1');
    }
    String::from_utf8(bytes).expect("ascii")
}

/// A position strictly between `a` and `b` (`None` = the start or the end of the list).
/// `a` must sort before `b`.
pub fn between(a: Option<&str>, b: Option<&str>) -> String {
    match (a, b) {
        (a, None) => after(a),
        (a, Some(b)) => midpoint(a.unwrap_or("").as_bytes(), b.as_bytes()),
    }
}

/// The fraction halfway (roughly) between `a` and `b`, with `a < b`.
fn midpoint(a: &[u8], b: &[u8]) -> String {
    // Shared leading digits stay.
    let mut n = 0;
    while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
        n += 1;
    }
    if n > 0 {
        let head = String::from_utf8(b[..n].to_vec()).expect("ascii");
        return head + &midpoint(a.get(n..).unwrap_or(&[]), &b[n..]);
    }
    let da = a.first().map_or(0, |&c| value(c));
    let db = b.first().map_or(BASE, |&c| value(c));
    if db - da > 1 {
        return digit((da + db) / 2).to_string();
    }
    // Adjacent digits: a longer `b` has room under its first digit; else go a digit deeper
    // after `a`'s.
    if b.len() > 1 {
        return digit(db).to_string();
    }
    digit(da).to_string() + &midpoint(a.get(1..).unwrap_or(&[]), &[])
}

/// `n` positions in order after `last`, for adding several entries at the end.
pub fn after_n(last: Option<&str>, n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    let mut prev = last.map(str::to_string);
    for _ in 0..n {
        let p = after(prev.as_deref());
        prev = Some(p.clone());
        out.push(p);
    }
    out
}

/// `n` positions in order strictly between `a` and `b`.
pub fn between_n(a: Option<&str>, b: Option<&str>, n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    let mut prev = a.map(str::to_string);
    for _ in 0..n {
        let p = between(prev.as_deref(), b);
        prev = Some(p.clone());
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(p: &str) {
        assert!(!p.is_empty() && !p.ends_with('0'), "{p:?}");
        assert!(p.bytes().all(|c| DIGITS.contains(&c)), "{p:?}");
    }

    #[test]
    fn appending_stays_ordered_and_short() {
        let keys = after_n(None, 1000);
        for w in keys.windows(2) {
            assert!(w[0] < w[1], "{w:?}");
        }
        keys.iter().for_each(|k| valid(k));
        let longest = keys.iter().map(String::len).max().unwrap();
        assert!(longest <= 31, "1000 appends need {longest} characters");
    }

    #[test]
    fn between_is_strictly_between() {
        let cases = [
            (None, Some("i")),
            (Some("i"), Some("j")),
            (Some("i"), Some("i1")),
            (Some("iz"), Some("j")),
            (Some("a"), None),
            (Some("z"), Some("z1")),
            (Some("1"), Some("2")),
            (None, Some("1")),
            (None, Some("01")),
        ];
        for (a, b) in cases {
            let m = between(a, b);
            valid(&m);
            assert!(a.is_none_or(|a| a < m.as_str()), "{a:?} < {m:?}");
            assert!(b.is_none_or(|b| m.as_str() < b), "{m:?} < {b:?}");
        }
    }

    #[test]
    fn repeated_inserts_at_one_place_stay_ordered() {
        // Always inserting just after the first entry, as a DJ dropping tracks at the top.
        let mut list = vec![after(None), after(Some("i"))];
        for _ in 0..200 {
            let m = between(Some(&list[0]), Some(&list[1]));
            list.insert(1, m);
        }
        for w in list.windows(2) {
            assert!(w[0] < w[1], "{w:?}");
        }
        list.iter().for_each(|k| valid(k));
        // And at the very front.
        let mut front = vec!["i".to_string()];
        for _ in 0..200 {
            front.insert(0, between(None, Some(&front[0])));
        }
        for w in front.windows(2) {
            assert!(w[0] < w[1], "{w:?}");
        }
        front.iter().for_each(|k| valid(k));
        let n = between_n(Some("a"), Some("b"), 50);
        assert!(n.windows(2).all(|w| w[0] < w[1]) && n[0].as_str() > "a" && n[49].as_str() < "b");
    }
}
