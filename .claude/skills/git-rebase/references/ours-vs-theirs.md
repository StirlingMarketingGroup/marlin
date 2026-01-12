# The Ours vs Theirs Confusion During Rebase

## The Problem

During `git rebase`, the meaning of `--ours` and `--theirs` is **reversed** from what you might expect. This trips up even experienced developers.

## Why It's Reversed

During a merge:
- You're on your branch
- You're merging another branch into yours
- "Ours" = your branch (current HEAD)
- "Theirs" = the branch being merged in

During a rebase:
- Git checks out the target branch (main)
- Git replays your commits one by one onto main
- From git's perspective, main is now "current HEAD"
- Your commits being replayed are "incoming"

So during rebase:
- **"Ours" = main** (the base branch you're rebasing onto)
- **"Theirs" = your commits** (being replayed)

## Visual Explanation

```
MERGE (on feature-branch, merging main):
                                    "ours"
                                      │
    main:     A ─── B ─── C          │
                     \               ▼
    feature:          D ─── E ─── [MERGE CONFLICT]
                                      ▲
                                      │
                                   "theirs"

REBASE (on feature-branch, rebasing onto main):
                                      "ours" (surprisingly!)
                                        │
    main:     A ─── B ─── C ◄───────────┘
                           \
    replaying:              D' ─── [CONFLICT]
                                      ▲
                                      │
                                   "theirs" (your commit!)
```

## Command Reference

### During Rebase

| Command | Effect |
|---------|--------|
| `git checkout --ours <file>` | Use main's version |
| `git checkout --theirs <file>` | Use your feature branch version |

### During Merge (for comparison)

| Command | Effect |
|---------|--------|
| `git checkout --ours <file>` | Use your current branch version |
| `git checkout --theirs <file>` | Use the incoming branch version |

## Memory Aid

Think of it this way during rebase:
- **"Ours"** = "the base we're standing on" = main
- **"Theirs"** = "the commits flying in to land" = your commits

Or remember this mnemonic:
> "Rebase Reverses"

## Safe Alternatives

If the ours/theirs confusion is too error-prone, use explicit references:

```bash
# During a rebase conflict, these refs are available:

# The commit being rebased onto (main's version)
git show REBASE_HEAD:<file>

# The original branch tip (your version before rebase)
git show ORIG_HEAD:<file>

# To accept main's version explicitly (stage 2 = ours during rebase):
git show :2:<file> > <file>
git add <file>

# To accept your version explicitly (stage 3 = theirs during rebase):
git show :3:<file> > <file>
git add <file>
```

## Practical Example

You're on `feature/login` and rebasing onto `main`.

Main has:
```javascript
function authenticate(user, password) {
  return validateCredentials(user, password);
}
```

Your branch has:
```javascript
function authenticate(user, password, token) {
  return validateWithMFA(user, password, token);
}
```

Conflict occurs. You want to keep YOUR version with MFA:

```bash
# WRONG (this keeps main's version!)
git checkout --ours src/auth.js

# CORRECT (this keeps your feature branch version)
git checkout --theirs src/auth.js
```

Or use index stages to avoid confusion:

```bash
# See what your version (stage 3 = theirs during rebase) has:
git show :3:src/auth.js

# Copy your version:
git show :3:src/auth.js > src/auth.js
git add src/auth.js
```

## Strategy Flags

The `-X` strategy flags follow the same reversed logic:

```bash
# Accept main's version for ALL conflicts (dangerous)
git rebase -X ours main

# Accept your version for ALL conflicts (dangerous)
git rebase -X theirs main
```

Only use these when you're certain one side should always win.

## Debugging: Which Version Is Which?

When in doubt during a conflict:

```bash
# Show what "ours" would give you
git show :2:<file>    # Stage 2 = ours

# Show what "theirs" would give you
git show :3:<file>    # Stage 3 = theirs

# Compare them
git diff :2:<file> :3:<file>
```

## Key Takeaway

During rebase:
- `--ours` = **main** (counterintuitive)
- `--theirs` = **your commits** (counterintuitive)

When in doubt, use `:2:<file>` (main's version) and `:3:<file>` (your version) for explicit, unambiguous references.
