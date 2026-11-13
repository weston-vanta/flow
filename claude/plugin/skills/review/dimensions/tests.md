# Tests

**Asks:** do the tests check behaviour at the right level, fail when the code is wrong, and justify their cost (CI runtime, and the risk if they were removed)?

**A finding must show:** one of:

- the risk a test covers, or the risk no test covers, as a behaviour that could break unnoticed;
- the cost a test adds, against the risk it covers;
- why the level is wrong: the seam it tests through, and the seam it should use.

A test that would still pass with the code under it broken is a finding; show the break.
