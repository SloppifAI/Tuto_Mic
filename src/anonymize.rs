//! Deterministische Pseudonymisierung *vor* jedem LLM-Aufruf. Lokale Modelle haben im Test
//! Schülernamen trotz Verbot im Prompt ausgegeben – deshalb nicht dem LLM überlassen.

use regex::{Regex, RegexBuilder};

pub struct Pseudonymizer {
    rules: Vec<(Regex, String)>,
}

impl Pseudonymizer {
    /// `class_list`: ein Eintrag pro Person, z. B. "Lena Wagner". Vor- und Nachname werden
    /// zusätzlich einzeln ersetzt, alle Varianten einer Person erhalten dasselbe Pseudonym.
    pub fn new(class_list: &[String]) -> Self {
        let mut variants: Vec<(String, String)> = Vec::new();
        let people = class_list.iter().map(|s| s.trim()).filter(|s| !s.is_empty());
        for (i, person) in people.enumerate() {
            let label = format!("[Schüler {}]", letters(i));
            variants.push((person.to_string(), label.clone()));
            for part in person.split_whitespace().filter(|p| p.chars().count() > 1) {
                variants.push((part.to_string(), label.clone()));
            }
        }
        // Längste zuerst, damit "Lena Wagner" vor "Lena" ersetzt wird.
        variants.sort_by_key(|(v, _)| std::cmp::Reverse(v.chars().count()));
        variants.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));

        let mut rules: Vec<(Regex, String)> = variants
            .into_iter()
            .map(|(v, label)| {
                let re = RegexBuilder::new(&format!(r"\b{}\b", regex::escape(&v)))
                    .case_insensitive(true)
                    .build()
                    .expect("escaped regex");
                (re, label)
            })
            .collect();

        for (pattern, label) in [
            (r"[\w.+-]+@[\w-]+\.[\w.-]+", "[E-Mail]"),
            (r"\+?\d[\d /-]{7,}\d", "[Telefon]"),
            (r"\b(Herr|Frau)\s+[A-ZÄÖÜ][\wäöüß-]+", "[Lehrkraft]"),
        ] {
            rules.push((Regex::new(pattern).expect("festes, gültiges Muster"), label.to_string()));
        }
        Self { rules }
    }

    pub fn apply(&self, text: &str) -> String {
        self.rules.iter().fold(text.to_string(), |acc, (re, label)| {
            re.replace_all(&acc, label.as_str()).into_owned()
        })
    }
}

/// 0 → A, 25 → Z, 26 → AA, …
fn letters(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (i % 26) as u8) as char);
        if i < 26 {
            return s;
        }
        i = i / 26 - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_names_consistently() {
        let p = Pseudonymizer::new(&["Lena Wagner".into(), "Tom Becker".into()]);
        let out = p.apply("Oder hier, Lena? Ja, Tom? Danke, Lena Wagner. Frau Becker kommt gleich.");
        assert_eq!(
            out,
            "Oder hier, [Schüler A]? Ja, [Schüler B]? Danke, [Schüler A]. Frau [Schüler B] kommt gleich."
        );
    }

    #[test]
    fn does_not_replace_inside_words() {
        let p = Pseudonymizer::new(&["Ben".into()]);
        assert_eq!(
            p.apply("Benjamin und Ben benutzen"),
            "Benjamin und [Schüler A] benutzen"
        );
    }

    #[test]
    fn masks_contact_data() {
        let p = Pseudonymizer::new(&[]);
        assert_eq!(
            p.apply("Mail an a.b@schule.de oder 0171 1234567, sagt Herr Kirchner"),
            "Mail an [E-Mail] oder [Telefon], sagt [Lehrkraft]"
        );
    }

    #[test]
    fn letters_sequence() {
        assert_eq!(letters(0), "A");
        assert_eq!(letters(25), "Z");
        assert_eq!(letters(26), "AA");
    }
}
