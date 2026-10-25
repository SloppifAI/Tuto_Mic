//! Deterministische Pseudonymisierung *vor* jedem LLM-Aufruf. Lokale Modelle haben im Test
//! Schülernamen trotz Verbot im Prompt ausgegeben – deshalb nicht dem LLM überlassen.

use regex::{Regex, RegexBuilder};

/// Platzhalter für einen Namensteil, der zu mehreren Personen der Klassenliste passt.
const AMBIGUOUS: &str = "[Schüler]";

pub struct Pseudonymizer {
    rules: Vec<(Regex, String)>,
}

impl Pseudonymizer {
    /// `class_list`: ein Eintrag pro Person, z. B. "Lena Wagner".
    ///
    /// - Der volle Name wird unabhängig von Groß-/Kleinschreibung ersetzt.
    /// - Vor- und Nachname werden zusätzlich einzeln ersetzt, aber nur großgeschrieben, damit „Lukas Klein“
    ///   nicht jedes „klein“ im Unterricht verschluckt.
    /// - Ein Namensteil, den mehrere Personen tragen, wird zu `[Schüler]` statt einer falschen Zuordnung.
    /// - Ein angehängtes Genitiv-s („Lenas“) wird mit ersetzt.
    pub fn new(class_list: &[String]) -> Self {
        // Kontaktdaten zuerst, sonst zerlegen die Namensregeln z. B. „lena.wagner@schule.de“.
        let mut rules: Vec<(Regex, String)> = [
            (r"[\w.+-]+@[\w-]+\.[\w.-]+", "[E-Mail]"),
            // Vorwahl mit 0 oder +Ländercode, danach mindestens 5 Ziffern, höchstens ein Trennzeichen zwischen
            // zwei Ziffern – trifft so keine Jahreszahlen („1939 - 1945“) oder Rechnungen.
            (r"(?:\+\d{2}[ /-]?|\b0)\d{2,5}(?:[ /-]?\d){5,}\b", "[Telefon]"),
            (
                r"\b(?:Herrn?|Frau)\s+(?:(?:Dr|Prof)\.?\s+)*(?:(?:von|van|vom|zu|de)\s+(?:(?:der|dem|den|la)\s+)?)?\p{Lu}[\w-]*",
                "[Lehrkraft]",
            ),
        ]
        .into_iter()
        .map(|(pattern, label)| (Regex::new(pattern).expect("festes, gültiges Muster"), label.to_string()))
        .collect();

        let people: Vec<&str> = class_list.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        let mut names: Vec<(String, bool, String)> = Vec::new(); // (Text, nur großgeschrieben?, Platzhalter)
        for (i, person) in people.iter().enumerate() {
            let label = format!("[Schüler {}]", letters(i));
            names.push(((*person).to_string(), false, label.clone()));
            for part in person.split_whitespace().filter(|p| p.chars().count() > 1) {
                let shared = people
                    .iter()
                    .enumerate()
                    .any(|(j, other)| j != i && other.split_whitespace().any(|o| o.eq_ignore_ascii_case(part)));
                let label = if shared { AMBIGUOUS.to_string() } else { label.clone() };
                names.push((capitalize(part), true, label));
            }
        }
        // Längste zuerst, damit "Lena Wagner" vor "Lena" ersetzt wird.
        names.sort_by_key(|(text, _, _)| std::cmp::Reverse(text.chars().count()));
        names.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

        rules.extend(names.into_iter().map(|(text, case_sensitive, label)| {
            let re = RegexBuilder::new(&format!(r"\b{}s?\b", regex::escape(&text)))
                .case_insensitive(!case_sensitive)
                .build()
                .expect("maskierter Name ist ein gültiges Muster");
            (re, label)
        }));
        Self { rules }
    }

    pub fn apply(&self, text: &str) -> String {
        self.rules.iter().fold(text.to_string(), |acc, (re, label)| {
            re.replace_all(&acc, label.as_str()).into_owned()
        })
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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

    fn pseudonymize(class_list: &[&str], text: &str) -> String {
        let list: Vec<String> = class_list.iter().map(|s| (*s).to_string()).collect();
        Pseudonymizer::new(&list).apply(text)
    }

    #[test]
    fn replaces_names_consistently() {
        assert_eq!(
            pseudonymize(
                &["Lena Wagner", "Tom Becker"],
                "Oder hier, Lena? Ja, Tom? Danke, Lena Wagner. Und Becker?"
            ),
            "Oder hier, [Schüler A]? Ja, [Schüler B]? Danke, [Schüler A]. Und [Schüler B]?"
        );
    }

    #[test]
    fn full_name_ignores_case() {
        assert_eq!(pseudonymize(&["Lena Wagner"], "LENA WAGNER fehlt"), "[Schüler A] fehlt");
    }

    #[test]
    fn replaces_genitive() {
        assert_eq!(
            pseudonymize(&["Lena Wagner"], "Lenas Antwort und Wagners Idee"),
            "[Schüler A] Antwort und [Schüler A] Idee"
        );
    }

    #[test]
    fn does_not_replace_inside_words() {
        assert_eq!(
            pseudonymize(&["Ben"], "Benjamin und Ben benutzen"),
            "Benjamin und [Schüler A] benutzen"
        );
    }

    #[test]
    fn surnames_that_are_words_only_match_capitalized() {
        assert_eq!(
            pseudonymize(
                &["Lukas Klein", "Mia Weiß"],
                "Das ist zu klein, ich weiß. Klein, Weiß, bitte."
            ),
            "Das ist zu klein, ich weiß. [Schüler A], [Schüler B], bitte."
        );
    }

    #[test]
    fn shared_name_parts_are_not_attributed_to_one_person() {
        assert_eq!(
            pseudonymize(&["Lena Wagner", "Lena Schmidt"], "Lena? Lena Schmidt! Wagner."),
            "[Schüler]? [Schüler B]! [Schüler A]."
        );
    }

    #[test]
    fn handles_umlauts() {
        assert_eq!(
            pseudonymize(&["Jörg Özdemir"], "Jörg und Özdemir"),
            "[Schüler A] und [Schüler A]"
        );
    }

    #[test]
    fn masks_contact_data() {
        assert_eq!(
            pseudonymize(&[], "Mail an a.b@schule.de oder 0171 1234567 bzw. +49 6621 123456"),
            "Mail an [E-Mail] oder [Telefon] bzw. [Telefon]"
        );
    }

    #[test]
    fn email_with_student_name_stays_one_placeholder() {
        assert_eq!(
            pseudonymize(&["Lena Wagner"], "Schreib an lena.wagner@schule.de"),
            "Schreib an [E-Mail]"
        );
    }

    #[test]
    fn numbers_in_lessons_are_not_phone_numbers() {
        let text = "Von 1939 - 1945, also 1 000 000 Menschen; 15 - 7 - 3 - 2 = 3; 0,25 = 25 %";
        assert_eq!(pseudonymize(&[], text), text);
    }

    #[test]
    fn masks_teachers_in_all_forms() {
        assert_eq!(
            pseudonymize(
                &[],
                "Herr Kirchner fragt Herrn Müller, Frau Dr. Schmidt und Frau von der Heide."
            ),
            "[Lehrkraft] fragt [Lehrkraft], [Lehrkraft] und [Lehrkraft]."
        );
        // Werktitel mit kleingeschriebenem Artikel bleiben stehen.
        assert_eq!(pseudonymize(&[], "Der Herr der Ringe"), "Der Herr der Ringe");
    }

    #[test]
    fn letters_sequence() {
        assert_eq!(letters(0), "A");
        assert_eq!(letters(25), "Z");
        assert_eq!(letters(26), "AA");
    }
}
