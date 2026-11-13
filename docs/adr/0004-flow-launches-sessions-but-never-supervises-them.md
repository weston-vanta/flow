# flow launches Sessions but never supervises them

flow v2 began as observation only, because v1 failed by making a supervisor its central component. The software factory now reverses part of that: `flow session start` launches an IC when a Manager asks, and the `flow@flow` plugin ships the Manager and IC skills. flow still supervises nothing. It starts a Session only on request, and never wakes, steers or polls a Session on another's behalf. A Manager reads an IC's state only when the Director asks, through the same `flow` CLI as any Reader. The Director is the only one watching all the work, on the dashboard. Manager and IC are roles that skills give a Session, not kinds of Session in the Store. We chose this because launching was the one part of dispatch that agents kept getting wrong in skill text alone, while supervision, which v1 put at the centre, was what made v1 too complex to use.

## Considered Options

- **Stay observation only**: launching lives in skill text, and v1's live starts failed 11 of 11 that way.
- **A v1-style supervisor that tracks and dispatches every Stack's owner**: the complexity that sank v1.
- **Cross-Session messaging, so ICs report to their Manager**: it wakes the Manager on IC activity, v1's failure mode, and fills the Manager's context with work the Director already sees on the dashboard.

## Consequences

- There is no lineage from Manager to IC in the Store. A Manager finds its ICs from the Task directory its skill writes, and finds nothing if that is gone.
- A new feature that would have flow act on a Session without being asked (nudging an idle IC, routing CI failures to an Owner) has to revisit this decision.
