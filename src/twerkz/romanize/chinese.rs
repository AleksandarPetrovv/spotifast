//! Hanzi to pinyin with tone marks, one syllable per character, spaced.

use pinyin::ToPinyin;

/// Pinyin for every Han character; anything else passes through.
pub fn romanize(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    let mut last_was_syllable = false;
    for c in text.chars() {
        match c.to_pinyin() {
            Some(syllable) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(syllable.with_tone());
                last_was_syllable = true;
            }
            None => {
                if last_was_syllable && c.is_alphanumeric() {
                    out.push(' ');
                }
                out.push(c);
                last_was_syllable = false;
            }
        }
    }
    out
}

/// Toneless pinyin, for the last-resort reading of a stray kanji.
pub fn plain(c: char) -> Option<&'static str> {
    c.to_pinyin().map(|syllable| syllable.plain())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hanzi_become_spaced_pinyin() {
        assert_eq!(romanize("你好"), "nǐ hǎo");
        assert_eq!(romanize("我爱你!"), "wǒ ài nǐ!");
        assert_eq!(romanize("爱 you"), "ài you");
    }
}
