//! Japanese to Hepburn romaji. Words are read with the IPADIC dictionary
//! (lindera, the same dictionary kuromoji uses) by their pronunciation, so
//! particles read as spoken (は wa, を o) and long vowels come out doubled
//! (東京 toukyou). Known heteronyms are read from a fixed table first, and a
//! kanji the dictionary cannot read falls back to its usual reading, then
//! to pinyin, so no raw glyph is left on screen.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use lindera::dictionary::load_fs_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use unicode_normalization::UnicodeNormalization;

const HOMOGRAPHS: &str = include_str!("homographs.tsv");
const KANJI_READINGS: &str = include_str!("kanji_readings.tsv");

/// Multi-kanji words and their romaji, longest first so a compound wins
/// over any word inside it.
static WORDS: LazyLock<Vec<(Vec<char>, String)>> = LazyLock::new(|| {
    let mut words: Vec<(Vec<char>, String)> = HOMOGRAPHS
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(word, kana)| (word.chars().collect(), kana_to_romaji(kana)))
        .collect();
    words.sort_by_key(|(word, _)| std::cmp::Reverse(word.len()));
    words
});

static KANJI: LazyLock<HashMap<char, &'static str>> = LazyLock::new(|| {
    KANJI_READINGS
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(kanji, reading)| Some((kanji.chars().next()?, reading)))
        .collect()
});

pub struct Japanese {
    segmenter: Segmenter,
}

impl Japanese {
    /// Loads the dictionary unpacked in `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let dictionary = load_fs_dictionary(dir).context("cannot load the Japanese dictionary")?;
        Ok(Self {
            segmenter: Segmenter::new(Mode::Normal, dictionary, None),
        })
    }

    pub fn romanize(&self, text: &str) -> String {
        let text: String = text.nfkc().collect();
        let text = strip_furigana(&text);
        let mut words: Vec<String> = Vec::new();
        for part in split_known_words(&text) {
            match part {
                Part::Known(romaji) => words.push(romaji),
                Part::Text(text) => self.read(&text, &mut words),
            }
        }
        join(&words)
    }

    fn read(&self, text: &str, words: &mut Vec<String>) {
        let Ok(mut tokens) = self.segmenter.segment(Cow::Borrowed(text)) else {
            words.push(fallback(text));
            return;
        };
        for token in tokens.iter_mut() {
            let surface = token.surface.to_string();
            if !surface.chars().any(is_japanese) {
                words.push(surface);
                continue;
            }
            // Pronunciation first (は reads wa), then the dictionary reading.
            let reading = [8, 7]
                .into_iter()
                .filter_map(|index| token.get_detail(index).map(str::to_string))
                .find(|reading| reading != "*" && !reading.is_empty() && reading.chars().all(is_kana));
            words.push(match reading {
                Some(reading) => kana_to_romaji(&reading),
                None => fallback(&surface),
            });
        }
    }
}

enum Part {
    Text(String),
    Known(String),
}

fn split_known_words(text: &str) -> Vec<Part> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    'scan: while index < chars.len() {
        if is_kanji(chars[index]) {
            for (word, romaji) in WORDS.iter() {
                if chars[index..].starts_with(word) {
                    if !plain.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut plain)));
                    }
                    parts.push(Part::Known(romaji.clone()));
                    index += word.len();
                    continue 'scan;
                }
            }
        }
        plain.push(chars[index]);
        index += 1;
    }
    if !plain.is_empty() {
        parts.push(Part::Text(plain));
    }
    parts
}

/// 漢字(かな) and 漢字（かな）: the reading replaces the kanji it annotates.
fn strip_furigana(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let start = index;
        while index < chars.len() && (is_kanji(chars[index]) || matches!(chars[index], '々' | '〆' | 'ヶ')) {
            index += 1;
        }
        if index > start
            && index < chars.len()
            && matches!(chars[index], '(' | '（')
        {
            let open = index + 1;
            let mut close = open;
            while close < chars.len() && is_kana(chars[close]) {
                close += 1;
            }
            if close > open && close < chars.len() && matches!(chars[close], ')' | '）') {
                out.extend(&chars[open..close]);
                index = close + 1;
                continue;
            }
        }
        out.extend(&chars[start..index]);
        if index < chars.len() && index == start {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

/// A word the dictionary has no reading for: its kana as romaji, its kanji
/// by their usual reading, then by pinyin.
fn fallback(text: &str) -> String {
    let mut out = String::new();
    let mut kana = String::new();
    for c in text.chars() {
        if is_kana(c) {
            kana.push(c);
            continue;
        }
        if !kana.is_empty() {
            out.push_str(&kana_to_romaji(&std::mem::take(&mut kana)));
        }
        if let Some(reading) = KANJI.get(&c) {
            out.push_str(reading);
        } else if let Some(reading) = super::chinese::plain(c) {
            out.push_str(reading);
        } else {
            out.push(c);
        }
    }
    if !kana.is_empty() {
        out.push_str(&kana_to_romaji(&kana));
    }
    out
}

/// Words separated by spaces; punctuation sticks to the word before it.
fn join(words: &[String]) -> String {
    let mut out = String::new();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        let punctuation = !word.chars().any(char::is_alphanumeric);
        if !out.is_empty() && !punctuation && !out.ends_with(['(', '「', '『', '"']) {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}')
}

fn is_kanji(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '々')
}

fn is_japanese(c: char) -> bool {
    is_kana(c) || is_kanji(c)
}

/// Kana to Hepburn romaji: long vowels doubled (おう ou, ー repeats the
/// vowel, with ō written ou), っ doubling the next consonant.
pub fn kana_to_romaji(kana: &str) -> String {
    let hira: Vec<char> = kana
        .chars()
        .map(|c| match c {
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            other => other,
        })
        .collect();
    let mut out = String::with_capacity(hira.len() * 2);
    let mut sokuon = false;
    let mut index = 0;
    while index < hira.len() {
        let c = hira[index];
        if c == 'っ' {
            sokuon = true;
            index += 1;
            continue;
        }
        if c == 'ー' {
            match out.chars().last() {
                Some('o') => out.push('u'),
                Some(vowel @ ('a' | 'i' | 'u' | 'e')) => out.push(vowel),
                _ => {}
            }
            index += 1;
            continue;
        }
        let pair = hira.get(index + 1).and_then(|&next| combined(c, next));
        let (romaji, used) = match pair {
            Some(romaji) => (Some(romaji), 2),
            None => (single(c), 1),
        };
        match romaji {
            Some(romaji) => {
                if sokuon {
                    out.push_str(if romaji.starts_with("ch") { "t" } else { &romaji[..1] });
                }
                out.push_str(romaji);
            }
            None => out.push(c),
        }
        sokuon = false;
        index += used;
    }
    out
}

fn combined(c: char, next: char) -> Option<&'static str> {
    Some(match (c, next) {
        ('き', 'ゃ') => "kya", ('き', 'ゅ') => "kyu", ('き', 'ょ') => "kyo",
        ('ぎ', 'ゃ') => "gya", ('ぎ', 'ゅ') => "gyu", ('ぎ', 'ょ') => "gyo",
        ('し', 'ゃ') => "sha", ('し', 'ゅ') => "shu", ('し', 'ょ') => "sho", ('し', 'ぇ') => "she",
        ('じ', 'ゃ') => "ja", ('じ', 'ゅ') => "ju", ('じ', 'ょ') => "jo", ('じ', 'ぇ') => "je",
        ('ち', 'ゃ') => "cha", ('ち', 'ゅ') => "chu", ('ち', 'ょ') => "cho", ('ち', 'ぇ') => "che",
        ('に', 'ゃ') => "nya", ('に', 'ゅ') => "nyu", ('に', 'ょ') => "nyo",
        ('ひ', 'ゃ') => "hya", ('ひ', 'ゅ') => "hyu", ('ひ', 'ょ') => "hyo",
        ('び', 'ゃ') => "bya", ('び', 'ゅ') => "byu", ('び', 'ょ') => "byo",
        ('ぴ', 'ゃ') => "pya", ('ぴ', 'ゅ') => "pyu", ('ぴ', 'ょ') => "pyo",
        ('み', 'ゃ') => "mya", ('み', 'ゅ') => "myu", ('み', 'ょ') => "myo",
        ('り', 'ゃ') => "rya", ('り', 'ゅ') => "ryu", ('り', 'ょ') => "ryo",
        ('ふ', 'ぁ') => "fa", ('ふ', 'ぃ') => "fi", ('ふ', 'ぇ') => "fe", ('ふ', 'ぉ') => "fo",
        ('て', 'ぃ') => "ti", ('で', 'ぃ') => "di", ('と', 'ぅ') => "tu", ('ど', 'ぅ') => "du",
        ('う', 'ぃ') => "wi", ('う', 'ぇ') => "we", ('う', 'ぉ') => "wo",
        ('ゔ', 'ぁ') => "va", ('ゔ', 'ぃ') => "vi", ('ゔ', 'ぇ') => "ve", ('ゔ', 'ぉ') => "vo",
        ('つ', 'ぁ') => "tsa", ('い', 'ぇ') => "ye",
        _ => return None,
    })
}

fn single(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "i", 'ゑ' => "e", 'を' => "o", 'ん' => "n",
        'ぁ' => "a", 'ぃ' => "i", 'ぅ' => "u", 'ぇ' => "e", 'ぉ' => "o",
        'ゃ' => "ya", 'ゅ' => "yu", 'ょ' => "yo", 'ゔ' => "vu",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana_reads_as_hepburn_with_doubled_vowels() {
        assert_eq!(kana_to_romaji("トーキョー"), "toukyou");
        assert_eq!(kana_to_romaji("がっこう"), "gakkou");
        assert_eq!(kana_to_romaji("マッチ"), "matchi");
        assert_eq!(kana_to_romaji("ファン"), "fan");
        assert_eq!(kana_to_romaji("パーティー"), "paatii");
    }

    #[test]
    fn furigana_replaces_the_kanji() {
        assert_eq!(strip_furigana("連(かさ)なる"), "かさなる");
        assert_eq!(strip_furigana("君の名は"), "君の名は");
    }

    #[test]
    fn known_words_win_before_the_dictionary() {
        let parts = split_known_words("今日は晴れ");
        assert!(matches!(&parts[0], Part::Known(romaji) if romaji == "kyou"));
    }
}
