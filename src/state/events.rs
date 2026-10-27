use std::fs;
use std::io;
use std::path::Path;

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// The file in a task directory holding its event log.
pub const EVENTS_FILE: &str = "events.jsonl";

/// One line of a task's `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: Timestamp,
    pub source: String,
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Appends `ev` as one line to `<dir>/events.jsonl`; see `fs::append_line`.
pub fn append(dir: &Path, ev: &Event) -> anyhow::Result<()> {
    let path = dir.join(EVENTS_FILE);
    let mut line = serde_json::to_vec(ev)?;
    line.push(b'\n');
    super::fs::append_line(&path, &line).with_context(|| format!("appending to {}", path.display()))
}

/// Every event in `<dir>/events.jsonl`, oldest first. A missing file has no events. A last line
/// that does not parse is a crashed writer's partial append and is skipped; any other bad line
/// is an error.
pub fn read_all(dir: &Path) -> anyhow::Result<Vec<Event>> {
    let path = dir.join(EVENTS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    // Split bytes rather than text: a crash can cut the last line inside a UTF-8 sequence.
    let lines: Vec<_> = bytes
        .split(|&b| b == b'\n')
        .enumerate()
        .filter(|(_, line)| !line.trim_ascii().is_empty())
        .collect();
    let mut events = Vec::with_capacity(lines.len());
    for (pos, (i, line)) in lines.iter().enumerate() {
        match serde_json::from_slice(line) {
            Ok(ev) => events.push(ev),
            Err(_) if pos + 1 == lines.len() => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("parsing {} line {}", path.display(), i + 1));
            }
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::OpenOptions;
    use std::io::Write;

    fn event(kind: &str) -> Event {
        Event {
            ts: "2026-10-26T12:00:00Z".parse().unwrap(),
            source: "test".to_owned(),
            kind: kind.to_owned(),
            data: json!({}),
        }
    }

    fn kinds(dir: &Path) -> Vec<String> {
        read_all(dir)
            .unwrap()
            .into_iter()
            .map(|ev| ev.kind)
            .collect()
    }

    #[test]
    fn read_all_skips_partial_trailing_line() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &event("first")).unwrap();
        append(dir.path(), &event("second")).unwrap();
        let path = dir.path().join(EVENTS_FILE);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"ts":"2026-10-26T12:"#).unwrap();

        assert_eq!(kinds(dir.path()), ["first", "second"]);
    }

    #[test]
    fn read_all_skips_trailing_line_cut_mid_character() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &event("first")).unwrap();
        let mut dashed = event("second");
        dashed.data = json!({"message": "retry \u{2014} then report"});
        let line = serde_json::to_vec(&dashed).unwrap();
        let dash = line.iter().position(|&b| b == 0xE2).unwrap();
        let path = dir.path().join(EVENTS_FILE);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&line[..=dash]).unwrap();

        assert_eq!(kinds(dir.path()), ["first"]);
    }

    #[test]
    fn read_all_rejects_malformed_line_before_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVENTS_FILE);
        let good = serde_json::to_string(&event("good")).unwrap();
        fs::write(&path, format!("{good}\nnot json\n{good}\n")).unwrap();

        let err = read_all(dir.path()).unwrap_err();

        assert!(format!("{err:#}").contains("line 2"), "{err:#}");
    }

    #[test]
    fn append_after_partial_line_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &event("first")).unwrap();
        let path = dir.path().join(EVENTS_FILE);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"ts":"2026-10-26T12:"#).unwrap();

        append(dir.path(), &event("second")).unwrap();

        assert_eq!(kinds(dir.path()), ["first", "second"]);
        assert!(fs::read_to_string(&path).unwrap().ends_with("}\n"));
    }
}
