//! Homograph attack detection
//!
//! Detects when characters from non-Latin scripts (Cyrillic, Greek, etc.)
//! are used to create lookalike domains.

use super::types::DomainWarning;
use unicode_script::{Script, UnicodeScript};

/// Check a domain string for homograph attacks
pub fn detect_homographs(domain: &str) -> Vec<DomainWarning> {
    let mut warnings = Vec::new();

    for (pos, ch) in domain.chars().enumerate() {
        if let Some(lookalike) = get_lookalike(ch) {
            warnings.push(DomainWarning::HomographDetected {
                suspicious_char: ch,
                position: pos,
                looks_like: lookalike,
            });
        }
    }

    warnings
}

/// Check if the domain mixes scripts suspiciously
pub fn has_mixed_scripts(domain: &str) -> bool {
    let mut has_latin = false;
    let mut has_other = false;

    for ch in domain.chars() {
        if ch == '.' || ch == '-' || ch.is_ascii_digit() {
            continue;
        }

        let script = ch.script();
        match script {
            Script::Latin => has_latin = true,
            Script::Common => {} // Ignore common characters
            _ => has_other = true,
        }
    }

    has_latin && has_other
}

/// Map of characters that look like Latin letters but are from other scripts
fn get_lookalike(ch: char) -> Option<char> {
    // Cyrillic lookalikes
    let lookalikes: &[(char, char)] = &[
        // Cyrillic
        ('а', 'a'), // Cyrillic small a
        ('е', 'e'), // Cyrillic small ie
        ('о', 'o'), // Cyrillic small o
        ('р', 'p'), // Cyrillic small er
        ('с', 'c'), // Cyrillic small es
        ('у', 'y'), // Cyrillic small u
        ('х', 'x'), // Cyrillic small ha
        ('А', 'A'), // Cyrillic capital A
        ('В', 'B'), // Cyrillic capital Ve
        ('Е', 'E'), // Cyrillic capital Ie
        ('К', 'K'), // Cyrillic capital Ka
        ('М', 'M'), // Cyrillic capital Em
        ('Н', 'H'), // Cyrillic capital En
        ('О', 'O'), // Cyrillic capital O
        ('Р', 'P'), // Cyrillic capital Er
        ('С', 'C'), // Cyrillic capital Es
        ('Т', 'T'), // Cyrillic capital Te
        ('У', 'Y'), // Cyrillic capital U
        ('Х', 'X'), // Cyrillic capital Ha
        ('і', 'i'), // Ukrainian i
        ('ј', 'j'), // Cyrillic je
        // Greek
        ('α', 'a'), // Greek small alpha
        ('ο', 'o'), // Greek small omicron
        ('ε', 'e'), // Greek small epsilon (sort of)
        ('Α', 'A'), // Greek capital alpha
        ('Β', 'B'), // Greek capital beta
        ('Ε', 'E'), // Greek capital epsilon
        ('Η', 'H'), // Greek capital eta
        ('Ι', 'I'), // Greek capital iota
        ('Κ', 'K'), // Greek capital kappa
        ('Μ', 'M'), // Greek capital mu
        ('Ν', 'N'), // Greek capital nu
        ('Ο', 'O'), // Greek capital omicron
        ('Ρ', 'P'), // Greek capital rho
        ('Τ', 'T'), // Greek capital tau
        ('Υ', 'Y'), // Greek capital upsilon
        ('Χ', 'X'), // Greek capital chi
        ('Ζ', 'Z'), // Greek capital zeta
        // Other confusables
        ('ı', 'i'),  // Turkish dotless i
        ('ɡ', 'g'),  // Latin small script g
        ('ɑ', 'a'),  // Latin small alpha
        ('ß', 'B'),  // German sharp s (looks like B)
        ('ℓ', 'l'),  // Script small l
        ('ｏ', 'o'), // Fullwidth o
        ('ａ', 'a'), // Fullwidth a
    ];

    for (suspicious, latin) in lookalikes {
        if ch == *suspicious {
            return Some(*latin);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cyrillic_a() {
        let warnings = detect_homographs("pаypal.com"); // Cyrillic 'а' in paypal
        assert!(!warnings.is_empty());
    }

    #[test]
    fn test_clean_domain() {
        let warnings = detect_homographs("paypal.com");
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_mixed_scripts() {
        assert!(has_mixed_scripts("gооgle.com")); // Cyrillic o's mixed with Latin
        assert!(!has_mixed_scripts("google.com")); // All Latin
    }
}
