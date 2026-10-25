//! Das Markdown der Zusammenfassung für den Editor: welche Stellen wie aussehen (Überschrift, Fett, Liste) und die
//! Knöpfe „Überschrift“, „Fett“, „Liste“. Tuto bekommt weiter reines Markdown; formatiert wird nur die Anzeige.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Markdown-Zeichen (`## `, `**`), in der Vorschau ausgeblendet.
    Marker,
    /// Überschrift der Ebene 1–3.
    Heading(u8),
    Bold,
    /// Aufzählungszeichen `- ` bzw. `* `.
    Bullet,
}

/// Formatierter Abschnitt, `range` in Bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: Kind,
}

/// Formatierte Abschnitte, sortiert und ohne Überlappung. Text ohne Abschnitt ist normaler Fließtext.
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        line_spans(content, start, &mut out);
        start += line.len();
    }
    out
}

fn line_spans(line: &str, offset: usize, out: &mut Vec<Span>) {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if (1..=3).contains(&hashes) && line[hashes..].starts_with(' ') {
        let marker = hashes + 1;
        out.push(Span {
            range: offset..offset + marker,
            kind: Kind::Marker,
        });
        if marker < line.len() {
            out.push(Span {
                range: offset + marker..offset + line.len(),
                kind: Kind::Heading(u8::try_from(hashes).expect("höchstens 3")),
            });
        }
        return;
    }
    let indent = line.len() - line.trim_start_matches(' ').len();
    let mut rest = indent;
    if line[indent..].starts_with("- ") || line[indent..].starts_with("* ") {
        out.push(Span {
            range: offset + indent..offset + indent + 2,
            kind: Kind::Bullet,
        });
        rest = indent + 2;
    }
    bold_spans(&line[rest..], offset + rest, out);
}

/// `**fett**` innerhalb einer Zeile; ein einzelnes `**` ohne Gegenstück bleibt normaler Text.
fn bold_spans(text: &str, offset: usize, out: &mut Vec<Span>) {
    let mut pos = 0;
    while let Some(open) = text[pos..].find("**").map(|i| pos + i) {
        let Some(close) = text[open + 2..].find("**").map(|i| open + 2 + i) else {
            break;
        };
        if close == open + 2 {
            pos = close + 2;
            continue;
        }
        out.push(Span {
            range: offset + open..offset + open + 2,
            kind: Kind::Marker,
        });
        out.push(Span {
            range: offset + open + 2..offset + close,
            kind: Kind::Bold,
        });
        out.push(Span {
            range: offset + close..offset + close + 2,
            kind: Kind::Marker,
        });
        pos = close + 2;
    }
}

/// Auswahl in Zeichen (nicht Bytes), wie sie egui liefert: `start <= end`.
pub type Selection = Range<usize>;

fn byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices().nth(char_index).map_or(text.len(), |(i, _)| i)
}

fn char_index(text: &str, byte_index: usize) -> usize {
    text[..byte_index].chars().count()
}

/// Bytebereich der Zeilen, die die Auswahl berührt (ohne abschließenden Zeilenumbruch).
fn line_bounds(text: &str, sel: &Selection) -> Range<usize> {
    let (a, b) = (byte_index(text, sel.start), byte_index(text, sel.end));
    let start = text[..a].rfind('\n').map_or(0, |i| i + 1);
    let end = text[b..].find('\n').map_or(text.len(), |i| b + i);
    start..end
}

/// Ersetzt die berührten Zeilen durch `f(zeile)` und hält die Auswahl auf denselben Zeilen.
fn map_lines(text: &str, sel: &Selection, f: impl Fn(&str) -> String) -> (String, Selection) {
    let bounds = line_bounds(text, sel);
    let replaced: Vec<String> = text[bounds.clone()].split('\n').map(f).collect();
    let replaced = replaced.join("\n");
    let start = char_index(text, bounds.start);
    let end = start + replaced.chars().count();
    let new = format!("{}{replaced}{}", &text[..bounds.start], &text[bounds.end..]);
    (new, end..end)
}

/// Macht die Zeile(n) zur Überschrift (`## `) bzw. wieder zu Fließtext.
pub fn toggle_heading(text: &str, sel: &Selection) -> (String, Selection) {
    let all_headings = text[line_bounds(text, sel)].split('\n').all(|l| l.starts_with("## "));
    map_lines(text, sel, |line| {
        if all_headings {
            line["## ".len()..].to_string()
        } else {
            format!("## {}", line.trim_start_matches(['#', ' ']))
        }
    })
}

/// Macht die Zeile(n) zu Aufzählungspunkten (`- `) bzw. nimmt sie wieder heraus.
pub fn toggle_list(text: &str, sel: &Selection) -> (String, Selection) {
    let all_items = text[line_bounds(text, sel)].split('\n').all(|l| l.starts_with("- "));
    map_lines(text, sel, |line| {
        if all_items {
            line["- ".len()..].to_string()
        } else if line.is_empty() {
            line.to_string()
        } else {
            format!("- {line}")
        }
    })
}

/// Setzt die Auswahl fett (`**…**`) bzw. nimmt das wieder zurück. Ohne Auswahl entsteht `****` mit dem Cursor in
/// der Mitte, damit man gleich weiterschreiben kann.
pub fn toggle_bold(text: &str, sel: &Selection) -> (String, Selection) {
    let (a, b) = (byte_index(text, sel.start), byte_index(text, sel.end));
    if a >= 2 && text[..a].ends_with("**") && text[b..].starts_with("**") {
        let new = format!("{}{}{}", &text[..a - 2], &text[a..b], &text[b + 2..]);
        return (new, sel.start - 2..sel.end - 2);
    }
    let new = format!("{}**{}**{}", &text[..a], &text[a..b], &text[b..]);
    (new, sel.start + 2..sel.end + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, Kind)> {
        spans(text).into_iter().map(|s| (&text[s.range], s.kind)).collect()
    }

    #[test]
    fn finds_headings_lists_and_bold() {
        let text = "# Mathe 7b\n## Ablauf\n- **Einstieg:** Umfrage\nText mit ** Sternchen\n";
        assert_eq!(
            kinds(text),
            vec![
                ("# ", Kind::Marker),
                ("Mathe 7b", Kind::Heading(1)),
                ("## ", Kind::Marker),
                ("Ablauf", Kind::Heading(2)),
                ("- ", Kind::Bullet),
                ("**", Kind::Marker),
                ("Einstieg:", Kind::Bold),
                ("**", Kind::Marker),
            ]
        );
    }

    #[test]
    fn hashtag_without_space_and_level_four_are_text() {
        assert_eq!(spans("#7b\n#### tief\n"), Vec::new());
    }

    #[test]
    fn umlauts_keep_ranges_on_char_boundaries() {
        // Würde ein Bereich mitten in einem Umlaut enden, bräche das Slicing hier ab.
        let text = "## Übung\n- **Größe** messen";
        assert_eq!(kinds(text)[1], ("Übung", Kind::Heading(2)));
        assert_eq!(kinds(text)[4], ("Größe", Kind::Bold));
    }

    #[test]
    fn heading_toggles_on_cursor_line() {
        let text = "Titel\nAblauf";
        let (new, sel) = toggle_heading(text, &(8..8));
        assert_eq!(new, "Titel\n## Ablauf");
        assert_eq!(sel, 15..15);
        assert_eq!(toggle_heading(&new, &sel).0, text);
        // Eine # -Überschrift wird zur ##-Überschrift statt doppelt markiert.
        assert_eq!(toggle_heading("# Titel", &(0..0)).0, "## Titel");
    }

    #[test]
    fn list_toggles_all_selected_lines() {
        let text = "Einstieg\nÜbung\n\nSchluss";
        let (new, _) = toggle_list(text, &(0..11));
        assert_eq!(new, "- Einstieg\n- Übung\n\nSchluss");
        assert_eq!(toggle_list(&new, &(0..12)).0, text);
    }

    #[test]
    fn bold_wraps_and_unwraps_selection() {
        let text = "Größe messen";
        let (new, sel) = toggle_bold(text, &(0..5));
        assert_eq!(new, "**Größe** messen");
        assert_eq!(sel, 2..7);
        assert_eq!(toggle_bold(&new, &sel), (text.to_string(), 0..5));
        let (empty, sel) = toggle_bold("a ", &(2..2));
        assert_eq!((empty.as_str(), sel), ("a ****", 4..4));
    }
}
