//! Reads the few facts flow takes from a Claude Code transcript. The transcript's format is
//! internal to Claude Code and changes without notice, so all knowledge of it lives here, and
//! anything this module cannot read is treated as absent.
//!
//! A transcript is JSONL, appended to as the Session runs, and grows to tens of MB. Only a
//! fixed-size tail is read, so the hook's cost stays flat. Claude Code re-appends its title
//! and `last-prompt` records as the Session goes on, so they stay near the end.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use serde::Deserialize;

/// How much of the transcript's end is read. Title and `last-prompt` records have been seen
/// up to ~34 KB from the end.
const TAIL: u64 = 128 * 1024;

/// What a transcript's tail says about its Session.
#[derive(Debug, Default)]
pub struct Facts {
    /// The latest `custom-title`, else the latest `ai-title`, else the first prompt the user
    /// typed in the tail (the Session's first prompt while the transcript fits in the tail).
    pub title: Option<String>,
    /// The latest `last-prompt` record's prompt.
    pub last_prompt: Option<String>,
}

/// The facts in the transcript at `path`; empty if it is missing or unreadable.
pub fn read(path: &Path) -> Facts {
    read_tail(path).unwrap_or_default()
}

fn read_tail(path: &Path) -> io::Result<Facts> {
    let mut file = File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(TAIL).read_to_end(&mut tail)?;

    let (mut custom_title, mut ai_title, mut first_prompt, mut last_prompt) =
        (None, None, None, None);
    // A first line cut mid-record, or a last line still being written, fails to parse and is
    // skipped.
    for line in tail.split(|&b| b == b'\n') {
        match serde_json::from_slice(line) {
            Ok(Record::CustomTitle { custom_title: t }) => custom_title = Some(t),
            Ok(Record::AiTitle { ai_title: t }) => ai_title = Some(t),
            Ok(Record::LastPrompt { last_prompt: p }) => last_prompt = Some(p),
            Ok(Record::User(user)) if first_prompt.is_none() => first_prompt = user.typed_prompt(),
            _ => {}
        }
    }
    Ok(Facts {
        title: custom_title.or(ai_title).or(first_prompt),
        last_prompt,
    })
}

/// The transcript records flow reads; every other record is `Other`.
#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum Record {
    CustomTitle {
        custom_title: String,
    },
    AiTitle {
        ai_title: String,
    },
    LastPrompt {
        last_prompt: String,
    },
    User(UserRecord),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserRecord {
    #[serde(default)]
    is_meta: bool,
    #[serde(default)]
    is_compact_summary: bool,
    origin: Option<Origin>,
    message: Message,
}

#[derive(Deserialize)]
struct Origin {
    kind: String,
}

#[derive(Deserialize)]
struct Message {
    content: serde_json::Value,
}

impl UserRecord {
    /// The prompt, if the user typed it: not a tool result (its content is a list), a message
    /// Claude Code injected (meta, compaction summaries, task notifications), or a slash
    /// command's `<command-name>` wrapper.
    fn typed_prompt(self) -> Option<String> {
        let by_human = self.origin.is_none_or(|o| o.kind == "human");
        if self.is_meta || self.is_compact_summary || !by_human {
            return None;
        }
        let text = self.message.content.as_str()?.trim();
        (!text.is_empty() && !text.starts_with('<')).then(|| text.to_owned())
    }
}
