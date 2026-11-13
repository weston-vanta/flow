# Reviewability

**Asks:** is each PR one concern, small, and well described? Is the Stack in a sensible order?

**A finding must show:** one of:

- the measure: the PR's size, the concerns it mixes (name each), or the order that would read better;
- the line to cut from a title, body or commit message;
- the reference the reader can't follow: a term, decision or document the text leans on but never gives.

Read every PR title, body and commit message cold, as a staff-level reviewer who was not in the planning and knows only the repo. Each body passes two tests: that reader can judge the change from the body and the diff alone, and would skip no line of the body.
