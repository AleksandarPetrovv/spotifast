//! Hangul to Latin letters by the Revised Romanization transliteration
//! rules, letter for letter as aromanize's `rr-translit` writes them.

const CHO: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "l", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];
const JUNG: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "oa", "oae", "oi", "yo", "u", "ueo", "ue",
    "ui", "yu", "eu", "eui", "i",
];
/// Final consonants, 1-based as in the syllable block (0 is none).
const JONG: [&str; 28] = [
    "", "g", "kk", "gs", "n", "nj", "nh", "d", "l", "lg", "lm", "lb", "ls", "lt", "lp", "lh", "m",
    "b", "bs", "s", "ss", "ng", "j", "ch", "k", "t", "p", "h",
];

/// A syllable split into its letters: initial, vowel and final (0 for none).
#[derive(Clone, Copy)]
struct Syllable {
    cho: usize,
    jung: usize,
    jong: usize,
}

fn split(c: char) -> Option<Syllable> {
    let code = (c as u32).checked_sub(0xAC00)?;
    (code <= 11171).then(|| Syllable {
        cho: (code / 588) as usize,
        jung: (code % 588 / 28) as usize,
        jong: (code % 28) as usize,
    })
}

const CHO_IEUNG: usize = 11;
const CHO_SIOS: usize = 9;
const CHO_JIEUJ: usize = 12;
const CHO_SSANGSIOS: usize = 10;

/// What a final consonant becomes before the next syllable's initial, when
/// the pair has its own spelling; `true` when that spelling swallows the
/// initial too.
fn joined_final(jong: usize, next_cho: usize) -> Option<&'static str> {
    // Before ㅇ every final keeps its letters and marks the break with "-".
    if next_cho == CHO_IEUNG {
        return Some(match jong {
            1 => "g-",
            2 => "kk-",
            3 => "gs-",
            4 => "n-",
            5 => "nj-",
            6 => "nh-",
            7 => "d-",
            8 => "l-",
            9 => "lg-",
            10 => "lm-",
            11 => "lb-",
            12 => "ls-",
            13 => "lt-",
            14 => "lp-",
            15 => "lh-",
            16 => "m-",
            17 => "b-",
            18 => "bs-",
            19 => "s-",
            20 => "ss-",
            21 => "ng-",
            22 => "j-",
            23 => "ch-",
            24 => "k-",
            25 => "t-",
            26 => "p-",
            27 => "h-",
            _ => return None,
        });
    }
    Some(match (jong, next_cho) {
        (3, CHO_SIOS) => "gs-s",
        (5, CHO_JIEUJ) => "nj-j",
        (12, CHO_SIOS) => "ls-s",
        (18, CHO_SIOS) => "bs-s",
        (19, CHO_SSANGSIOS) => "s-ss",
        (20, CHO_SIOS) => "ss-s",
        (22, CHO_JIEUJ) => "j-j",
        _ => return None,
    })
}

pub fn romanize(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut skip_initial = false;
    for (index, &c) in chars.iter().enumerate() {
        let Some(syllable) = split(c) else {
            out.push(c);
            skip_initial = false;
            continue;
        };
        if !skip_initial {
            out.push_str(CHO[syllable.cho]);
        }
        skip_initial = false;
        out.push_str(JUNG[syllable.jung]);
        if syllable.jong > 0 {
            let next = chars.get(index + 1).and_then(|&c| split(c));
            match next.and_then(|next| joined_final(syllable.jong, next.cho)) {
                Some(joined) => {
                    out.push_str(joined);
                    skip_initial = true;
                }
                None => out.push_str(JONG[syllable.jong]),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_aromanize_rr_translit() {
        assert_eq!(romanize("안녕하세요"), "annyeonghaseyo");
        assert_eq!(romanize("사랑해"), "salanghae");
        assert_eq!(romanize("먹어"), "meog-eo");
        assert_eq!(romanize("너를 사랑해!"), "neoleul salanghae!");
        assert_eq!(romanize("읽어"), "ilg-eo");
        assert_eq!(romanize("없어"), "eobs-eo");
        assert_eq!(romanize("값이"), "gabs-i");
        assert_eq!(romanize("앉자"), "anj-ja");
    }
}
