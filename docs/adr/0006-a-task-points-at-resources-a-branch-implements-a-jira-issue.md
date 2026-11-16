# A Task points at Resources; a Branch implements a Jira issue

A Jira issue can show up in flow in two places: as the one Jira issue a Branch (and so its PR) implements, recorded with `flow branch jira set`, or as one of a Task's Resources, recorded with `flow task resource add`. The two places mean different things. A Branch's Jira issue is a one-to-one fact about one PR, so flow models it as Jira: a key that the poller fetches status and summary for. A Task's relationship to the things outside it (its Jira Source, several epics, docs, Explainers) has no fixed shape. A Task can have several epics, an epic can span several Feature branches, and a Feature branch can cover several epics. So flow records those links generically, as Resources, and records no relationship between epics and Branches, rather than locking in one way of working.

## Considered Options

- **Branches only**: Jira issues exist from plan approval, before any Branch. Answer and spec Tasks have no Branch. A small Task's epic would need a holder branch.
- **Jira-specific links on the Task as well as on Branches**: this special-cases Jira among the things a Task points at, and invites modelling epic↔Branch structure.
- **Task only, as `{key, branch?}`**: a link would stay with the old Task when its Branch changes owner, which breaks ownership (ADR-0002).

## Consequences

- The same key may be both a Branch's Jira issue and a Task's Resource. That is fine, because they say different things.
- For now a Resource is an inert link, `{url, title?}`, and the poller reads only Branches' Jira issues. This decision does **not** rule out Resources gaining fetched information such as a status or a title. That is expected, and when it comes it should work for every kind of Resource, not as a Jira special case.
