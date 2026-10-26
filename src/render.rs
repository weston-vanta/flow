//! Compact text rendering for read commands.

use jiff::Timestamp;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// How long before `now` `ts` was, in its largest whole unit: `40s`, `3m`, `2h`, `5d`.
/// Timestamps after `now` (clock skew between hosts) read as `0s`.
pub fn relative(ts: Timestamp, now: Timestamp) -> String {
    let seconds = (now.as_second() - ts.as_second()).max(0);
    match seconds {
        s if s < MINUTE => format!("{s}s"),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s => format!("{}d", s / DAY),
    }
}

/// Left-aligned columns separated by two spaces, one line per row, no borders. The last cell
/// of a row is not padded, so lines carry no trailing spaces.
pub fn table(rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = Vec::new();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let width = cell.chars().count();
            match widths.get_mut(i) {
                Some(w) => *w = (*w).max(width),
                None => widths.push(width),
            }
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i + 1 == row.len() {
                line.push_str(cell);
            } else {
                let pad = widths[i] - cell.chars().count();
                line.push_str(cell);
                line.push_str(&" ".repeat(pad + 2));
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The first line of `s`, cut to at most `max` characters. Anything dropped is marked with a
/// trailing `…`, which counts toward `max`.
pub fn truncate(s: &str, max: usize) -> String {
    let mut lines = s.lines();
    let first = lines.next().unwrap_or("");
    let more_lines = lines.next().is_some();
    if !more_lines && first.chars().count() <= max {
        return first.to_owned();
    }
    let mut out: String = first.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn relative_uses_largest_whole_unit() {
        let now = at("2026-10-26T12:00:00Z");
        assert_eq!(relative(at("2026-10-26T11:59:20Z"), now), "40s");
        assert_eq!(relative(at("2026-10-26T11:57:00Z"), now), "3m");
        assert_eq!(relative(at("2026-10-26T09:30:00Z"), now), "2h");
        assert_eq!(relative(at("2026-10-21T11:00:00Z"), now), "5d");
    }

    #[test]
    fn relative_clamps_future_to_zero() {
        let now = at("2026-10-26T12:00:00Z");
        assert_eq!(relative(at("2026-10-26T12:00:05Z"), now), "0s");
    }

    #[test]
    fn table_aligns_columns_without_trailing_spaces() {
        let rows = vec![
            vec!["a".to_owned(), "pending".to_owned(), "x".to_owned()],
            vec![
                "bbb".to_owned(),
                "done".to_owned(),
                "longer title".to_owned(),
            ],
        ];
        assert_eq!(
            table(&rows),
            "a    pending  x\nbbb  done     longer title\n"
        );
    }

    #[test]
    fn table_of_no_rows_is_empty() {
        assert_eq!(table(&[]), "");
    }

    #[test]
    fn truncate_marks_cut_text() {
        assert_eq!(truncate("short", 80), "short");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("first\nsecond", 80), "first…");
        assert_eq!(truncate("line\n", 80), "line");
        assert_eq!(truncate("ééééé", 3), "éé…");
    }
}
