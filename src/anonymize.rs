//! Deterministische Pseudonymisierung *vor* jedem LLM-Aufruf. Lokale Modelle haben im Test
//! Schülernamen trotz Verbot im Prompt ausgegeben – deshalb nicht dem LLM überlassen.

use regex::{Regex, RegexBuilder};
use std::sync::LazyLock;

/// Platzhalter für einen Namensteil, der zu mehreren Personen der Klassenliste passt.
const AMBIGUOUS: &str = "[Schüler]";

/// Kürzere Namensteile und Wörter werden nicht unscharf verglichen: Bei drei Buchstaben („Tom“, „Ben“) sind
/// Kölner Code und Ähnlichkeit zu grob und träfen viele gewöhnliche Wörter.
const MIN_FUZZY_LEN: usize = 4;

/// Mindest-Jaro-Winkler-Ähnlichkeit der normalisierten Schreibweisen. 0.9 lässt typische Whisper-Varianten
/// durch („Mehmed“, „Ösdemir“, „Jörk“), weist aber Wörter mit gleichem Kölner Code ab, die nur ähnlich klingen
/// („Sofa“ zu „Sophie“: 0.88, „Becher“ zu „Becker“: 0.89, „Lehne“ zu „Lena“: 0.83).
const MIN_SIMILARITY: f64 = 0.9;

/// Namensteil der Klassenliste für den unscharfen Vergleich, vorberechnet.
struct FuzzyName {
    normalized: String,
    code: String,
    label: String,
}

pub struct Pseudonymizer {
    rules: Vec<(Regex, String)>,
    fuzzy: Vec<FuzzyName>,
}

impl Pseudonymizer {
    /// `class_list`: ein Eintrag pro Person, z. B. "Lena Wagner".
    ///
    /// - Der volle Name wird unabhängig von Groß-/Kleinschreibung ersetzt.
    /// - Vor- und Nachname werden zusätzlich einzeln ersetzt, aber nur großgeschrieben, damit „Lukas Klein“
    ///   nicht jedes „klein“ im Unterricht verschluckt.
    /// - Ein Namensteil, den mehrere Personen tragen, wird zu `[Schüler]` statt einer falschen Zuordnung.
    /// - Ein angehängtes Genitiv-s („Lenas“) wird mit ersetzt.
    /// - Danach werden großgeschriebene Wörter ersetzt, die Whisper falsch geschrieben hat („Nicky“ für „Niki“,
    ///   „Müler“ für „Müller“): gleicher Kölner Code *und* ähnliche Schreibweise, siehe [`MIN_SIMILARITY`].
    ///   Passt ein Wort zu mehreren Personen, wird es zu `[Schüler]`.
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
        let mut fuzzy = Vec::new();
        for (i, person) in people.iter().enumerate() {
            let label = format!("[Schüler {}]", letters(i));
            names.push(((*person).to_string(), false, label.clone()));
            for part in person.split_whitespace().filter(|p| p.chars().count() > 1) {
                let shared = people
                    .iter()
                    .enumerate()
                    .any(|(j, other)| j != i && other.split_whitespace().any(|o| o.eq_ignore_ascii_case(part)));
                let label = if shared { AMBIGUOUS.to_string() } else { label.clone() };
                // Doppelnamen einzeln, weil Whisper auch nur eine Hälfte falsch schreiben kann.
                for piece in part.split('-').filter(|p| p.chars().count() >= MIN_FUZZY_LEN) {
                    fuzzy.push(FuzzyName {
                        normalized: normalize(piece),
                        code: cologne_phonetic(piece),
                        label: label.clone(),
                    });
                }
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
        Self { rules, fuzzy }
    }

    pub fn apply(&self, text: &str) -> String {
        let exact = self.rules.iter().fold(text.to_string(), |acc, (re, label)| {
            re.replace_all(&acc, label.as_str()).into_owned()
        });
        self.replace_fuzzy(&exact)
    }

    /// Ersetzt großgeschriebene Wörter, die einem Namensteil ähneln. Läuft nach den exakten Regeln und überspringt
    /// deren Platzhalter, sonst würde z. B. das „Schüler“ in „[Schüler A]“ verglichen.
    fn replace_fuzzy(&self, text: &str) -> String {
        static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\[(?:Schüler(?: [A-Z]+)?|Lehrkraft|E-Mail|Telefon)\]|\b\p{Lu}\p{L}*\b")
                .expect("festes, gültiges Muster")
        });
        if self.fuzzy.is_empty() {
            return text.to_string();
        }
        TOKEN
            .replace_all(text, |caps: &regex::Captures| {
                let token = &caps[0];
                if token.starts_with('[') {
                    return token.to_string();
                }
                self.fuzzy_label(token).unwrap_or(token).to_string()
            })
            .into_owned()
    }

    /// Platzhalter für ein Wort, das einem oder mehreren Namensteilen ähnelt, auch mit Genitiv-s („Nickys“).
    fn fuzzy_label(&self, token: &str) -> Option<&str> {
        if token.chars().count() < MIN_FUZZY_LEN {
            return None;
        }
        let stem = token.strip_suffix('s').filter(|s| s.chars().count() >= MIN_FUZZY_LEN);
        let mut found: Option<&str> = None;
        for variant in std::iter::once(token).chain(stem) {
            let code = cologne_phonetic(variant);
            let normalized = normalize(variant);
            for name in &self.fuzzy {
                if name.code != code || strsim::jaro_winkler(&normalized, &name.normalized) < MIN_SIMILARITY {
                    continue;
                }
                match found {
                    Some(label) if label != name.label => return Some(AMBIGUOUS),
                    _ => found = Some(&name.label),
                }
            }
        }
        found
    }
}

/// Vereinheitlicht Schreibweisen, die Whisper bei Namen verwechselt (ü/ue, ck/k, ph/f, y/i, ie/i, Doppelbuchstaben),
/// damit die Ähnlichkeit nur echte Unterschiede misst. „ch“ bleibt eigenständig, sonst würde „Becher“ zu „Becker“.
fn normalize(word: &str) -> String {
    let mut s = word.to_lowercase();
    for (from, to) in [
        ("ä", "ae"),
        ("ö", "oe"),
        ("ü", "ue"),
        ("ß", "ss"),
        ("ch", "x"),
        ("ck", "k"),
        ("ph", "f"),
        ("th", "t"),
        ("dt", "t"),
        ("c", "k"),
        ("y", "i"),
        ("ie", "i"),
    ] {
        s = s.replace(from, to);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// Kölner Phonetik: Ziffernfolge, die im Deutschen gleich klingende Wörter gleich kodiert
/// (z. B. „Müller“, „Mueller“, „Müler“ → „657“).
fn cologne_phonetic(word: &str) -> String {
    let raw = cologne_raw(word);
    let mut code = String::with_capacity(raw.len());
    for c in raw.chars() {
        if !code.ends_with(c) {
            code.push(c);
        }
    }
    // Nullen (Vokale) zählen nur am Wortanfang.
    code.char_indices()
        .filter(|&(i, c)| i == 0 || c != '0')
        .map(|(_, c)| c)
        .collect()
}

/// Kölner Phonetik vor dem Zusammenfassen gleicher Ziffern und dem Entfernen der Nullen.
fn cologne_raw(word: &str) -> String {
    // Großbuchstaben, „ß“ wird dabei zu „SS“. Andere Zeichen (Bindestrich, Akzente) tragen nichts bei.
    let letters: Vec<char> = word
        .chars()
        .flat_map(char::to_uppercase)
        .filter(|c| c.is_ascii_uppercase() || matches!(c, 'Ä' | 'Ö' | 'Ü'))
        .collect();
    let mut code = String::with_capacity(letters.len());
    for (i, &c) in letters.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| letters[j]);
        let next = letters.get(i + 1).copied();
        code.push_str(match c {
            'A' | 'E' | 'I' | 'J' | 'O' | 'U' | 'Y' | 'Ä' | 'Ö' | 'Ü' => "0",
            'P' if next == Some('H') => "3",
            'B' | 'P' => "1",
            'D' | 'T' if matches!(next, Some('C' | 'S' | 'Z')) => "8",
            'D' | 'T' => "2",
            'F' | 'V' | 'W' => "3",
            'G' | 'K' | 'Q' => "4",
            'C' if i == 0 => {
                if matches!(next, Some('A' | 'H' | 'K' | 'L' | 'O' | 'Q' | 'R' | 'U' | 'X')) {
                    "4"
                } else {
                    "8"
                }
            }
            'C' if matches!(prev, Some('S' | 'Z')) => "8",
            'C' if matches!(next, Some('A' | 'H' | 'K' | 'O' | 'Q' | 'U' | 'X')) => "4",
            'C' | 'S' | 'Z' => "8",
            'X' if matches!(prev, Some('C' | 'K' | 'Q')) => "8",
            'X' => "48",
            'L' => "5",
            'M' | 'N' => "6",
            'R' => "7",
            _ => "", // H
        });
    }
    code
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
    fn cologne_phonetic_reference_codes() {
        // Referenzwerte aus der Beschreibung des Verfahrens (Postel 1969, Wikipedia).
        assert_eq!(cologne_raw("Müller-Lüdenscheidt"), "60550750206880022");
        assert_eq!(cologne_phonetic("Müller-Lüdenscheidt"), "65752682");
        assert_eq!(cologne_phonetic("Wikipedia"), "3412");
        assert_eq!(cologne_phonetic("Breschnew"), "17863");
    }

    #[test]
    fn cologne_phonetic_rules() {
        // Vokale nur am Anfang, H stumm, Umlaute und ß.
        assert_eq!(cologne_phonetic("Anna"), "06");
        assert_eq!(cologne_phonetic("Hans"), "068");
        assert_eq!(cologne_phonetic("Strauß"), "8278");
        // C je nach Stellung, X als „ks“, PH als F, D/T vor Zischlaut.
        assert_eq!(cologne_phonetic("Carl"), "475");
        assert_eq!(cologne_phonetic("Celina"), "856");
        assert_eq!(cologne_phonetic("Max"), "648");
        assert_eq!(cologne_phonetic("Philipp"), "351");
        assert_eq!(cologne_phonetic("Schmitz"), "868");
        assert_eq!(cologne_phonetic(""), "");
        // Schreibvarianten, die Whisper liefert, fallen zusammen.
        for variant in ["Müller", "Mueller", "Müler", "Möller"] {
            assert_eq!(cologne_phonetic(variant), "657", "{variant}");
        }
        for variant in ["Niki", "Nicky", "Nikki", "Nicki"] {
            assert_eq!(cologne_phonetic(variant), "64", "{variant}");
        }
    }

    #[test]
    fn normalizes_spelling_variants() {
        assert_eq!(normalize("Müller"), normalize("Mueller"));
        assert_eq!(normalize("Müler"), normalize("Müller"));
        assert_eq!(normalize("Nicky"), normalize("Niki"));
        assert_eq!(normalize("Sofie"), normalize("Sophie"));
        assert_ne!(normalize("Becher"), normalize("Becker"));
    }

    #[test]
    fn replaces_whisper_misspellings() {
        let class = [
            "Niki Wagner",
            "Sophie Müller",
            "Niklas Özdemir",
            "Mehmet Yilmaz",
            "Jörg Schulz",
        ];
        assert_eq!(
            pseudonymize(
                &class,
                "Nicky und Nikki, dann Sofie Mueller. Müler fehlt. Nicklas, Ösdemir, Mehmed, Jörk und Schultz."
            ),
            "[Schüler A] und [Schüler A], dann [Schüler B] [Schüler B]. [Schüler B] fehlt. [Schüler C], [Schüler C], \
             [Schüler D], [Schüler E] und [Schüler E]."
        );
    }

    #[test]
    fn replaces_misspelled_genitive() {
        assert_eq!(
            pseudonymize(&["Niki Wagner", "Sophie Müller"], "Nickys Heft und Mülers Antwort"),
            "[Schüler A] Heft und [Schüler B] Antwort"
        );
    }

    #[test]
    fn misspelled_double_name_parts() {
        assert_eq!(
            pseudonymize(&["Lena Müller-Lüdenscheidt"], "Müler-Lüdenscheid kommt."),
            "[Schüler A]-[Schüler A] kommt."
        );
    }

    #[test]
    fn misspelling_close_to_several_people_is_ambiguous() {
        assert_eq!(
            pseudonymize(&["Niki Wagner", "Nicki Becker"], "Niki, Nicki und Nikki"),
            "[Schüler A], [Schüler B] und [Schüler]"
        );
    }

    #[test]
    fn fuzzy_matching_skips_lowercase_and_short_words() {
        // Kleingeschrieben ist es kein Name; „Tom“ ist zu kurz für den unscharfen Vergleich.
        assert_eq!(
            pseudonymize(&["Tom Becker", "Niki Wagner"], "nicky sagt, Thom und Tomm kommen."),
            "nicky sagt, Thom und Tomm kommen."
        );
    }

    #[test]
    fn keeps_placeholders_and_teachers() {
        assert_eq!(
            pseudonymize(&["Sophie Müller"], "Herr Müler fragt Sophie, Mail an a@schule.de"),
            "[Lehrkraft] fragt [Schüler A], Mail an [E-Mail]"
        );
    }

    #[test]
    fn ordinary_words_are_not_replaced() {
        let class = [
            "Lena Wagner",
            "Tom Becker",
            "Niklas Özdemir",
            "Mehmet Yilmaz",
            "Sophie Müller",
            "Jörg Schulz",
            "Emma Hartmann",
        ];
        let text = "Mehl, Meter und Methode. Der Wagen, die Wanne, Wasser und Wolken. Die Lehne, die Leine, \
                    die Linie, der Lehrer und die Lehrerin. Becher, Becken, Bücher. Nickel, Nichte, Nadel und \
                    Nacken. Die Mühle, der Müll, die Mauer, das Muster, die Mutter. Das Sofa, die Suppe, die Seife. \
                    Die Schule, die Schulter, der Schutz, die Schuld. Jahr, Jäger, Ernte, Erde, Hammer, Hartholz. \
                    Mehrheit, Messer, Mittag. Leider nicht. Nimm Ösen, Öl und Ozean. Jeder Erdteil hat Meere.";
        assert_eq!(pseudonymize(&class, text), text);
    }

    #[test]
    fn letters_sequence() {
        assert_eq!(letters(0), "A");
        assert_eq!(letters(25), "Z");
        assert_eq!(letters(26), "AA");
    }
}
