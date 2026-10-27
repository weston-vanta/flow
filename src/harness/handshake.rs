//! Bringing up Claude Code after typing its command line into a herdr pane: waiting for it to
//! start and accepting the folder-trust dialog it shows in a directory it has not seen.

use anyhow::bail;

use crate::herdr::{AgentStatus, Herdr, KEY_DOWN, KEY_ENTER};
use crate::state::config::Host;

/// How long the agent gets to start, and again to settle after the trust dialog.
const START_TIMEOUT_MS: u64 = 60_000;
/// Enough of the pane to hold the whole trust dialog.
const DIALOG_LINES: u32 = 40;
/// The trust dialog lists two options, so reaching `Yes` never takes more moves than this.
const MAX_CURSOR_MOVES: usize = 3;

#[derive(Debug)]
pub enum AgentStart {
    /// herdr sees the agent idle or working.
    Running,
    /// The agent stopped on a prompt other than the trust dialog, showing `pane_text`; no keys
    /// were sent.
    BlockedAtStart { pane_text: String },
}

/// Waits for the agent just launched in `pane_id` to come up. Claude shows a folder-trust
/// dialog in a directory it has not seen, and herdr reports it `blocked`; keys sent before then
/// are lost. When the blocked pane shows that dialog, this selects its `Yes` option and waits
/// again.
pub fn await_start(
    herdr: &Herdr,
    host: &Host,
    workspace_id: &str,
    pane_id: &str,
) -> anyhow::Result<AgentStart> {
    let wait = |until: &[AgentStatus]| -> anyhow::Result<AgentStatus> {
        match herdr.wait(host, pane_id, until, Some(START_TIMEOUT_MS))? {
            Some(status) => Ok(status),
            None => bail!(
                "agent in pane {pane_id} on {} did not start within {} s; workspace \
                 {workspace_id} is left open for inspection",
                host.name,
                START_TIMEOUT_MS / 1000
            ),
        }
    };
    let status = wait(&[
        AgentStatus::Blocked,
        AgentStatus::Idle,
        AgentStatus::Working,
    ])?;
    if status != AgentStatus::Blocked {
        return Ok(AgentStart::Running);
    }
    let text = herdr.read(host, pane_id, DIALOG_LINES)?;
    let Some(keys) = trust_dialog_keys(&text) else {
        return Ok(AgentStart::BlockedAtStart { pane_text: text });
    };
    herdr.pane_send_keys(host, pane_id, &keys)?;
    wait(&[AgentStatus::Idle, AgentStatus::Working])?;
    Ok(AgentStart::Running)
}

/// The keys that accept Claude's folder-trust dialog shown in `text`: enough `down`s to move
/// the cursor (`❯` or `>` starting a line) onto the option containing `Yes`, then `enter`.
/// `None` when `text` is not that dialog or the cursor cannot reach `Yes` by moving down.
pub fn trust_dialog_keys(text: &str) -> Option<Vec<&'static str>> {
    if !text.to_lowercase().contains("trust") {
        return None;
    }
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // The pane holds earlier output too, so the dialog's cursor is the last one on screen.
    let cursor = lines.iter().rposition(|line| is_cursor_line(line))?;
    let yes = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("Yes"))
        .min_by_key(|(i, _)| i.abs_diff(cursor))?
        .0;
    if yes < cursor {
        return None;
    }
    let moves = lines[cursor + 1..=yes]
        .iter()
        .filter(|line| !line.is_empty())
        .count();
    if moves > MAX_CURSOR_MOVES {
        return None;
    }
    let mut keys = vec![KEY_DOWN; moves];
    keys.push(KEY_ENTER);
    Some(keys)
}

/// A selection cursor: `❯` or `>` alone or followed by a space, so a `>_ claude` prompt line
/// does not count.
fn is_cursor_line(line: &str) -> bool {
    ["❯", ">"].iter().any(|marker| {
        line.strip_prefix(marker)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIALOG_ON_NO: &str = "\
=> /workspaces/acme-worktrees/search-index
>_ claude
 Accessing workspace:

 /workspaces/acme-worktrees/search-index

 Quick safety check: Is this a project you created or one you trust?

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

    #[test]
    fn trust_dialog_keys_moves_cursor_to_yes() {
        assert_eq!(
            trust_dialog_keys(DIALOG_ON_NO),
            Some(vec![KEY_DOWN, KEY_ENTER])
        );
    }

    #[test]
    fn trust_dialog_keys_confirms_when_cursor_is_on_yes() {
        let text =
            " Do you trust the files in this folder?\n\n > 1. Yes, proceed\n   2. No, exit\n";
        assert_eq!(trust_dialog_keys(text), Some(vec![KEY_ENTER]));
    }

    #[test]
    fn trust_dialog_keys_ignores_other_prompts() {
        assert_eq!(
            trust_dialog_keys("Choose a theme:\n ❯ Dark\n   Light\n"),
            None
        );
        assert_eq!(
            trust_dialog_keys("Do you trust this folder?\n   Yes\n ❯ No\n"),
            None,
            "Yes above the cursor would need an up key"
        );
        assert_eq!(
            trust_dialog_keys("Do you trust this folder?\n>_ claude\n   Yes\n"),
            None,
            "a shell prompt is not a cursor"
        );
    }
}
