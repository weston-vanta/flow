---
name: explainer
description: Build an Explainer, an interactive one-diagram page for reviewers who need the design before the diffs. Use when the Director asks for an Explainer of a change, an existing system or a plan.
---

# Explainer

An Explainer lets a reader of unfamiliar code work out what drives its complexity, and whether that complexity is necessary, from a picture rather than paragraphs. Its subject is a **change**, an **existing system** or a **plan**. An IC builds one only when the Director asks, and may suggest one when a Stack has forcing facts its PR bodies can't carry. That Stack's PR bodies then link to it.

Apply `flow:explain` throughout: its reader, reader questions, forcing facts, checked claims and assumed knowledge are this page's too.

## Terms

- **Inclusion test:** draw something only if both hold: it belongs to the subject, and drawing it explains where data comes from or why the design has this shape. Anything else goes in the detail panel, or is left out as assumed knowledge.
- **At rest:** the page as it first loads, before the reader interacts with it.
- **Component:** any box in the diagram, at any level of nesting.

## Analyze

1. **Decide the audience and the subject's boundary.** Together they set the level of abstraction the diagram works at.
2. **Write the reader questions and your own answer to each,** as `flow:explain` describes. The alternative to compare against is the previous code for a change, the rejected options for a plan, or a simpler design for an existing system. Push each answer down to a fact about the code, and verify every forcing fact in the current code. If you find none, the subject may not need an Explainer.
3. **Trace each flow end to end,** listing every function it passes through and every call it makes. The trace is exhaustive so the diagram can be selective. For a plan, trace the proposed flows, verify the code they touch, and mark any claim you could not verify.

If the Director offers a sketch, follow its choices and correct it wherever the code disagrees.

## The page

At rest, the reader sees a title, a narrative under 200 words, and one diagram with any controls in a single row directly above it.

- **The narrative** says what the subject does or what problem it solves, gives each forcing fact a line, and says in one sentence what the facts force the code to do.
- **The diagram is readable without interaction.** At 1280×800 everything above fits without scrolling in either direction, with no diagram label smaller than 12px. If it doesn't fit, the diagram is at the wrong level of abstraction. Rethink the level rather than shrinking type or deleting components one at a time.

Below the diagram are a step bar when a flow is selected, a **detail panel** that every interaction fills, and **collapsed sections** for supporting evidence the diagram doesn't carry. Make every component clickable to show its detail, including links to the code behind each claim.

Match each interaction to the axis the subject varies along:
- **Sequence:** step through a flow. A flow is one user action on a main path, named for what it accomplishes. Drop a flow that mostly repeats another.
- **Time or scope:** highlight what a phase of a change or plan adds or changes.
- **Structure or alternatives:** highlight an area, or switch between options.

Readers figure out controls on their own, so leave them unexplained. Clicking outside any component clears the selection.

## The diagram

- **Group components by layer,** and lay them out along the dominant direction of the flow.
- **Draw each component at its interface.** Show the functions a call enters or leaves the component through, nested inside it, so the reader can see which one makes which call. What happens inside a component belongs in its detail panel.
- **Apply the inclusion test** to every component, arrow and label. Draw every route that carries the subject's data, including duplicate paths to the same data, so duplication and inconsistency are visible without being stated.
- **Let shape carry the forcing facts.** Where a component's shape is the fact, draw it, for example a store split across many collections as several boxes.
- **Use the system's own names:** verified identifiers for code, and product terms for anything users see. Most boxes carry only a name.
- **Make user-facing components evoke the real UI,** while keeping them simple. The reader starts from what users are trying to do.
- **Fix unclear parts in this order:** change the structure (level, shape, grouping), then add an interaction, then use the detail panel. Add a label, number or prose only as a last resort. Order belongs in a flow's steps, not in numbers on the diagram.
- **One step, one arrow.** A step usually highlights the single arrow it describes, so the reader always knows where they are.
- **Show how the system works at rest.** What a change or plan adds is revealed through highlights, not in the diagram at rest or in the narrative.

## Build and verify

1. **Build** with whatever design and diagramming guidance the harness offers. `template.html` beside this file is an optional starting point: it provides the page frame, clickable components, highlighting, a step bar and a detail panel. Check the rendered page at 1280×800.
2. **Test with fresh agents who see only full-page screenshots,** never the source, your notes or your answers. Make sure no screenshot catches a transition halfway; the template turns transitions off in automated browsers. A round is one first-glance test and one question test, each run by a new agent. After each round, fix what they found in the order above.
   - **First glance:** show the diagram at rest without the narrative. Ask what makes the design complex, and whether anything is hard to read. It passes when they name the forcing facts.
   - **Question test:** show the page at rest, every flow step, and every highlight or option, along with the reader questions. Grade the answers against yours. It passes when every answer is correct. Leave out the detail panels of individual components, because the diagram and its interactions should carry the answers, and the review below checks the panels.

   Stop after the first round in which both tests pass, or after three rounds, and report what is still unclear. If you can't run these tests, say so.
3. **Review for accuracy and completeness.** Give a fresh agent the page and the code, but not your notes.
   - It checks that every drawn component, call, route and detail-panel claim is true.
   - It checks that every value a flow shows the reader, and whatever decides which values appear, can be traced along drawn arrows to its source. A gap counts only if it passes the inclusion test, and the reviewer justifies each one against it.
   - It checks each forcing fact for depth: does it name what in the data model, architecture or an interface stops the obvious approach, or only a consequence of that? A consequence is a finding, and the reviewer names the deeper fact and verifies it in the code.

   Rerun the question test if the diagram changes.
4. **Iterate with the Director,** updating the same page in place.
