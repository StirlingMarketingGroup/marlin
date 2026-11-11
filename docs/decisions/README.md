# Decisions

A decision record captures one choice that later work depends on: what we chose, why, and what we rejected. Write one when a choice is expensive to reverse, or when the next person would otherwise argue it again.

A wiki page says what the code does today. A decision record says why it is that way. Link from the page to the record.

## Rules

- One file per decision, named `NNNN-short-title.md`, numbered in order from `0001`.
- A record is not edited after it is accepted, except to change its status. A new decision that replaces it gets a new number and says which record it supersedes.
- Add the record to the list below and to [`../index.md`](../index.md) in the same branch.
- Mark anything you could not test as untested. A record never claims more than the evidence shows.

## Shape

```markdown
# NNNN: Title

**Status:** proposed | accepted | superseded by NNNN
**Date:** YYYY-MM-DD
**Issue:** #N

## Decision

What we chose, in a few sentences. Lead with this.

## Why

The problem, the constraints, and the evidence.

## Rejected

Each alternative, and the reason it lost.

## What follows

What this makes easy, what it makes hard, and what work it unblocks.
```

## Records

None yet.
