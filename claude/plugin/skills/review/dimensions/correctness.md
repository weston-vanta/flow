# Correctness

**Asks:** what bugs, edge cases and failure modes does the change have, weighted by their impact and likelihood?

**A finding must show:** a concrete failure scenario: the input or state, what happens, and what should happen instead. Say how likely it is and what it costs when it happens.

A risk you can't turn into a scenario is not a finding. Rank by impact times likelihood, so a rare failure with a small cost stays non-blocking.
