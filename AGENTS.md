# Working here

The rules of this place, read by whichever harness is driving. `CLAUDE.md`
imports this file; there is one set of rules, not one per tool.

Written by `nunki init` as a starting point — replace it with the rules that
actually hold in this project.

## What an agent may not do

- Never push, never merge, never reach the forge. The human pushes.
- Never touch a protected path (see `nunki.yaml`), and never a protected branch.
- Never ask a blocking question: in an autonomous run there is nobody to
  answer. Refusing is safe; asking is not.

## What a run must leave behind

- The lot committed and proved, or the failure stated plainly.
- A commitable tree.
- A `ÉTAT DE REPRISE` block at the top of `JOURNAL.md`, rewritten at every
  checkpoint and before stopping. It names the commit it describes: `nunki`
  refuses a block that does not carry the current `HEAD`.
- `PR.md`, the pull request the mission delivers, written as the work goes.
  It is a deliverable a gate looks for, and an empty one is a red gate.
- For the coder, that block ends with `Lot: <lot> — done`, or
  `Lot: <lot> — failed: <why>`: the line `nunki` reads to know the lot is done.
  `nunki` reads it **inside** the block — between its heading and the next one —
  so a line left further down the file is one `nunki` will not see.
