# Architecture

**Asks:** does the change extend the codebase's existing concepts rather than add new ones, deepen modules rather than make them shallower, and hide information rather than leak it?

**A finding must show:** the existing concept or module it should extend, by name and location, or the leak: the detail that crosses a boundary and the callers that now depend on it.

Use the repo's glossary and ADRs as the map of its concepts.
