//! Finding what should not be shared: e-mail addresses, phone numbers,
//! IBANs, card numbers, IP addresses and secrets that look like API keys.
//!
//! Text comes from OCR with word positions; whatever matches is covered
//! with solid boxes (blurred or pixelated text can sometimes be read back).

use crate::geom::RectF;
use crate::ocr::{Line, Word};
use crate::output::Image;

/// Boxes (image pixels) over every sensitive item in `img`.
pub fn find(img: &Image) -> Result<Vec<RectF>, String> {
    let lines = crate::ocr::recognize(img)?;
    Ok(lines.iter().flat_map(|l| boxes_in_line(l)).collect())
}

/// Boxes over the sensitive parts of one recognized line.
pub fn boxes_in_line(words: &[Word]) -> Vec<RectF> {
    // Join the words like the line reads and remember where each one is.
    let mut text = String::new();
    let mut spans = Vec::with_capacity(words.len());
    for w in words {
        if !text.is_empty() {
            text.push(' ');
        }
        let start = text.chars().count();
        text.push_str(&w.text);
        spans.push((start, text.chars().count()));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    for (a, b) in sensitive_ranges(&chars) {
        let mut r: Option<RectF> = None;
        for (w, (s, e)) in words.iter().zip(&spans) {
            if *s < b && a < *e {
                r = Some(r.map_or(w.rect, |r| r.union(&w.rect)));
            }
        }
        if let Some(r) = r {
            // A little margin hides anti-aliased edges too.
            let pad = (r.h * 0.12).max(2.0);
            out.push(r.inflate(pad));
        }
    }
    out
}

fn is_sep(c: char) -> bool {
    matches!(c, ' ' | '-' | '.' | '/' | '(' | ')')
}

/// Character ranges (end exclusive) of sensitive items in a line.
pub fn sensitive_ranges(c: &[char]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let tokens = tokens(c);
    for &(a, b) in &tokens {
        let t: String = c[a..b].iter().collect();
        let t = t.trim_matches(|ch: char| ",;:!?\"'<>[]{}".contains(ch));
        if is_email(t) || is_secret(t) || is_ipv4(t) {
            out.push((a, b));
        }
    }
    // Numbers may contain spaces ("+49 151 2345 678"), so scan characters.
    let mut i = 0;
    while i < c.len() {
        let start_ok = c[i].is_ascii_digit()
            || (c[i] == '+' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
            || (c[i] == '(' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()));
        let after_word = i == 0 || !c[i - 1].is_alphanumeric();
        if !(start_ok && after_word) {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut digits = 0;
        let mut last_digit = i;
        while j < c.len() {
            if c[j].is_ascii_digit() {
                digits += 1;
                last_digit = j;
            } else {
                let separator = is_sep(c[j]) || (j == i && c[j] == '+');
                // A space only continues a number when more of it follows.
                let gap_ends = c[j] == ' '
                    && c.get(j + 1)
                        .is_some_and(|n| !n.is_ascii_digit() && *n != '(' && *n != '+');
                if !separator || gap_ends {
                    break;
                }
            }
            j += 1;
        }
        let end = last_digit + 1;
        let piece: String = c[i..end].iter().collect();
        if (7..=19).contains(&digits) && !looks_like_date_or_time(&piece) {
            out.push((i, end));
        }
        i = j.max(i + 1);
    }
    // IBAN: two letters, two digits, then 11 to 30 letters or digits.
    for (a, _) in tokens {
        if let Some(e) = iban_at(c, a) {
            out.push((a, e));
        }
    }
    // One box per item, even when several rules found (parts of) it.
    out.sort();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(out.len());
    for (a, b) in out {
        match merged.last_mut() {
            Some(last) if a < last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// Whitespace-separated tokens as character ranges.
fn tokens(c: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i].is_whitespace() {
            i += 1;
            continue;
        }
        let a = i;
        while i < c.len() && !c[i].is_whitespace() {
            i += 1;
        }
        out.push((a, i));
    }
    out
}

fn is_email(t: &str) -> bool {
    let Some((user, host)) = t.split_once('@') else {
        return false;
    };
    let host_ok = host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-');
    !user.is_empty() && user.chars().all(|c| !c.is_whitespace() && c != '@') && host_ok
}

/// Long tokens mixing letters and digits: API keys, tokens, hashes.
fn is_secret(t: &str) -> bool {
    t.chars().count() >= 20
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
}

fn is_ipv4(t: &str) -> bool {
    let parts: Vec<&str> = t.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.chars().all(|c| c.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|v| v <= 255)
        })
}

/// 2026-11-13, 13.11.2026, 11/13/2026, 12:30:00 - not worth hiding.
fn looks_like_date_or_time(s: &str) -> bool {
    let groups: Vec<usize> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .map(str::len)
        .collect();
    let seps: String = s.chars().filter(|c| !c.is_ascii_digit()).collect();
    let one_kind = seps.chars().all(|c| seps.starts_with(c));
    one_kind
        && matches!(
            groups.as_slice(),
            [4, 2, 2] | [2, 2, 4] | [1, 2, 4] | [2, 1, 4] | [1, 1, 4] | [2, 2, 2]
        )
        && (seps.starts_with('-') || seps.starts_with('.') || seps.starts_with('/'))
}

fn iban_at(c: &[char], a: usize) -> Option<usize> {
    let up = |i: usize| c.get(i).is_some_and(|ch| ch.is_ascii_uppercase());
    let dg = |i: usize| c.get(i).is_some_and(|ch| ch.is_ascii_digit());
    if !(up(a) && up(a + 1) && dg(a + 2) && dg(a + 3)) {
        return None;
    }
    if a > 0 && c[a - 1].is_alphanumeric() {
        return None;
    }
    let mut i = a + 4;
    let mut n = 4;
    let mut end = i;
    while i < c.len() {
        if c[i].is_ascii_alphanumeric() {
            n += 1;
            end = i + 1;
        } else if c[i] != ' ' {
            break;
        }
        i += 1;
    }
    (15..=34).contains(&n).then_some(end)
}

#[allow(dead_code)]
fn _line(words: &[(&str, f32)]) -> Line {
    let mut x = 0.0;
    words
        .iter()
        .map(|(t, w)| {
            let word = Word {
                text: t.to_string(),
                rect: RectF::new(x, 0.0, *w, 10.0),
            };
            x += w + 5.0;
            word
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(s: &str) -> Vec<String> {
        let c: Vec<char> = s.chars().collect();
        sensitive_ranges(&c)
            .into_iter()
            .map(|(a, b)| c[a..b].iter().collect())
            .collect()
    }

    #[test]
    fn finds_what_should_be_hidden() {
        assert_eq!(
            found("Mail me: jane.doe@example.com, thanks"),
            ["jane.doe@example.com,"]
        );
        assert_eq!(found("Call +49 151 2345 6789 today"), ["+49 151 2345 6789"]);
        assert_eq!(found("Tel. (030) 123-4567"), ["(030) 123-4567"]);
        assert_eq!(
            found("IBAN DE89 3704 0044 0532 0130 00"),
            ["DE89 3704 0044 0532 0130 00"]
        );
        assert_eq!(
            found("card 4111 1111 1111 1111 exp"),
            ["4111 1111 1111 1111"]
        );
        assert_eq!(found("server 192.168.10.24 is up"), ["192.168.10.24"]);
        assert_eq!(
            found("token sk_live_51Hc8abcdEFGH1234ijkl ok"),
            ["sk_live_51Hc8abcdEFGH1234ijkl"]
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        assert!(found("Meeting on 2026-11-13 at 12:30:00").is_empty());
        assert!(found("Release 13.11.2026, version 0.4.3").is_empty());
        assert!(found("Order 12345 shipped, 3 items").is_empty());
        assert!(found("someone@ said hi").is_empty());
        assert!(found("the quick brown fox").is_empty());
    }

    #[test]
    fn boxes_cover_all_words_of_a_match() {
        let line = _line(&[
            ("Call", 30.0),
            ("+49", 25.0),
            ("151", 25.0),
            ("2345678", 60.0),
            ("now", 30.0),
        ]);
        let boxes = boxes_in_line(&line);
        assert_eq!(boxes.len(), 1);
        let b = boxes[0];
        // From "+49" (x = 35) to the end of "2345678" (x = 95 + 60).
        assert!(b.x <= 35.0 && b.right() >= 155.0, "{b:?}");
        assert!(b.x > 30.0 - 3.0 && b.right() < 160.0 + 3.0, "{b:?}");
    }
}
