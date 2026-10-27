//! The brief a worker starts from: the task, the ask, its completion criteria, and the commands
//! the worker reports through.

use std::path::Path;

use crate::state::task::TaskRecord;

const TEMPLATE: &str = include_str!("../../templates/brief.md");

/// `templates/brief.md` filled in for `task`, with `body` and `criteria` verbatim. Commands in
/// the brief call `flow_bin` by path, so it must be valid on the worker's host.
pub fn render(task: &TaskRecord, body: &str, criteria: &str, flow_bin: &Path) -> String {
    let worktree = task.worktree.display().to_string();
    let flow = flow_bin.display().to_string();
    fill(TEMPLATE, |name| match name {
        "id" => &task.id,
        "title" => &task.title,
        "worktree" => &worktree,
        "body" => body,
        "criteria" => criteria,
        "flow" => &flow,
        _ => panic!("brief template has unknown placeholder {{{{{name}}}}}"),
    })
}

/// `template` with each `{{name}}` replaced by `value(name)`. One pass, so a value that itself
/// contains `{{...}}` is kept as written.
fn fill<'a>(template: &str, value: impl Fn(&str) -> &'a str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(value(&rest[start + 2..start + 2 + len]));
        rest = &rest[start + 2 + len + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::state::task::Harness;

    const FLOW_BIN: &str = "/opt/flow/bin/flow";
    const BODY: &str = "Add retries with backoff to the example client.\n\nKeep the public API.";
    const CRITERIA: &str = "- Tests cover the retry path\n- A draft PR is open";

    fn task() -> TaskRecord {
        TaskRecord {
            id: "aaaaaaab".to_owned(),
            parent: None,
            title: "Add retry to the example client".to_owned(),
            host: "host-a".to_owned(),
            worktree: PathBuf::from("/workspaces/example-repo.wt/retry"),
            harness: Harness::Claude,
            created: "2026-10-26T12:00:00Z".parse().unwrap(),
            owner: "supervisor".to_owned(),
        }
    }

    fn rendered() -> String {
        render(&task(), BODY, CRITERIA, Path::new(FLOW_BIN))
    }

    /// Byte offset of `needle` in `text`, failing the test when it is absent.
    fn position(text: &str, needle: &str) -> usize {
        text.find(needle)
            .unwrap_or_else(|| panic!("brief lacks {needle:?}:\n{text}"))
    }

    #[test]
    fn brief_contains_id_worktree_and_criteria() {
        let text = rendered();
        let in_order = [
            "aaaaaaab",
            "Add retry to the example client",
            "/workspaces/example-repo.wt/retry",
            "Work only inside this directory; never touch the main checkout or other worktrees.",
            BODY,
            CRITERIA,
            r#"/opt/flow/bin/flow task status aaaaaaab working --message "<what you are doing>""#,
            r#"/opt/flow/bin/flow task status aaaaaaab needs-decision --message "<question>""#,
            r#"/opt/flow/bin/flow task status aaaaaaab blocked --message "<what blocks you>""#,
            "/opt/flow/bin/flow task request-dispatch aaaaaaab --title",
            "/opt/flow/bin/flow task done aaaaaaab --report <path>",
            "never address the user directly",
        ];
        let offsets: Vec<usize> = in_order.iter().map(|n| position(&text, n)).collect();
        assert!(
            offsets.is_sorted(),
            "brief sections out of order ({offsets:?}):\n{text}"
        );
        assert!(
            text.contains("wait for an answer to appear in your terminal"),
            "{text}"
        );
        assert!(text.contains("--brief <file> [--host <name>]"), "{text}");
    }

    #[test]
    fn brief_has_no_user_address() {
        let text = rendered().to_lowercase();
        assert!(!text.is_empty());
        for phrase in ["captain", "the user said"] {
            assert!(!text.contains(phrase), "brief contains {phrase:?}:\n{text}");
        }
    }

    #[test]
    fn brief_keeps_placeholder_text_in_the_ask_verbatim() {
        let body = "Rename {{criteria}} to {{id}} in the docs.";
        let text = render(&task(), body, CRITERIA, Path::new(FLOW_BIN));
        assert!(text.contains(body), "{text}");
    }

    #[test]
    fn template_stays_short() {
        let lines = include_str!("../../templates/brief.md").lines().count();
        assert!(lines < 60, "brief template has {lines} lines");
    }
}
