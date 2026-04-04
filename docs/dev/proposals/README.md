# Proposals

Design proposals for camdl language and engine changes.

## Format

```
docs/proposals/YYYY-MM-DD-proposal-slug.md
```

### Header block

Every proposal starts with a metadata block:

```markdown
# Proposal: short title

**Status:** Proposal | Accepted | Implemented | Superseded
**Date:** YYYY-MM-DD
**Implemented:** commit `abc1234`, YYYY-MM-DD (added when merged)
**Superseded by:** docs/proposals/YYYY-MM-DD-newer.md (if applicable)
**Motivation:** One-sentence summary of why this exists.
```

Update the status and implementation fields as the proposal progresses.
Once implemented, the proposal becomes a historical record of the
design rationale — don't delete it.

## Naming

Use the date the proposal was first written, not the implementation
date. The slug should be descriptive enough to find by scanning
filenames: `events-block`, `balance-compartment`, `cooling-schedule`.
