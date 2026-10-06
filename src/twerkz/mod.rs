//! Extras on top of upstream: more lyrics providers, song downloads,
//! YouTube and SoundCloud imports, and a user emoji font. Kept in one
//! folder so upstream merges stay small.

pub mod lyrics;
pub mod romanize;
pub mod tools;

/// Lowercased words without punctuation.
fn normalize(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut space = true;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    out
}

/// 1.0 for the same words, falling towards 0.0 with edit distance.
pub fn similarity(left: &str, right: &str) -> f64 {
    let a = normalize(left);
    let b = normalize(right);
    if a == b {
        return 1.0;
    }
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    1.0 - previous[b.len()] as f64 / longest as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_ignores_case_and_punctuation() {
        assert_eq!(similarity("Hello, World!", "hello world"), 1.0);
        assert!(similarity("Hello", "Goodbye") < 0.5);
    }
}
