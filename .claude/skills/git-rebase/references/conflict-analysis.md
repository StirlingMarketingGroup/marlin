# Conflict Analysis: Understanding Intent Before Resolution

## The Problem with Mechanical Resolution

Most conflict resolution guides focus on the mechanics:
- "Accept theirs"
- "Accept ours"
- "Edit the file and remove markers"

This misses the point. **Conflicts are communication.** They tell you that two people made different decisions about the same code. Understanding those decisions is essential to resolving them correctly.

## The Intent-First Approach

Before touching any conflict, answer these questions:

### 1. What was main trying to accomplish?

```bash
# Find the commit(s) that touched this file in main
git log -p ORIG_HEAD..origin/main -- <conflicted-file>
```

Look for:
- Bug fixes (does this fix something our code depends on?)
- Refactoring (did the structure change?)
- New features (do we need to integrate with this?)
- API changes (did signatures/interfaces change?)

### 2. What was our branch trying to accomplish?

```bash
# Find our commits that touched this file
git log -p origin/main..ORIG_HEAD -- <conflicted-file>
```

Look for:
- What feature were we adding?
- What bug were we fixing?
- What's the minimal change needed?

### 3. Are both changes trying to solve the same problem?

Sometimes two developers fix the same bug independently. In this case:
- Evaluate which fix is more complete/correct
- Consider if they can be combined
- Don't blindly keep both

## Conflict Type Analysis

### Type 1: Infrastructure Change in Main

**Symptoms:**
- Main refactored a function/class you were using
- Main renamed variables/files
- Main changed an API signature

**Resolution principle:** Adapt your changes to the new infrastructure.

Main has already been merged and likely deployed. Your code needs to work with the new reality, not fight it.

```bash
# See what changed in the function/API
git diff ORIG_HEAD..origin/main -- <file>
```

### Type 2: Both Added to Same Location

**Symptoms:**
- Both branches added new code to the same spot (e.g., new function, new import)
- The code doesn't actually conflict logically

**Resolution principle:** Include both additions, maintaining logical order.

```
<<<<<<< HEAD (main's version)
import { newFeatureA } from './features';
=======
import { newFeatureB } from './features';
>>>>>>> your-commit

# Resolution: Include both
import { newFeatureA } from './features';
import { newFeatureB } from './features';
```

### Type 3: Semantic Conflict (Code Merges Clean But Breaks)

**Symptoms:**
- Git doesn't report a conflict
- But tests fail after rebase
- Runtime errors occur

**This is the most dangerous type.**

**Resolution principle:** Always run tests after each commit in the rebase.

```bash
git rebase -i --exec "npm test" origin/main
```

### Type 4: Delete vs Modify Conflict

**Symptoms:**
- One side deleted a file/function
- Other side modified it

**Resolution principle:** Understand why it was deleted.

```bash
# Find the deletion commit
git log --diff-filter=D --summary ORIG_HEAD..origin/main -- <file>
```

If main deleted it intentionally:
- Your changes might need to go elsewhere
- The feature might have been replaced
- Ask the team if unclear

### Type 5: Dependency/Version Conflict

**Symptoms:**
- Different package versions in package.json, go.mod, etc.
- Both sides updated dependencies

**Resolution principle:** Usually take the newer version, but test.

```bash
# After resolving, verify dependencies work
npm install && npm test
# or
go mod tidy && go test ./...
```

## Resolution Workflow

For each conflicted file:

### Step 1: Gather Context

```bash
# List conflicted files
git diff --name-only --diff-filter=U

# For each file, understand both sides
git log -p ORIG_HEAD..origin/main -- <file>    # Main's changes
git log -p origin/main..ORIG_HEAD -- <file>    # Our changes (before rebase started)
```

### Step 2: Categorize the Conflict

Ask: Which type from above does this match?

### Step 3: Apply the Appropriate Principle

- Infrastructure change? Adapt to new structure.
- Both added? Include both.
- Delete vs modify? Understand why deleted.
- Dependency? Take newer, test.

### Step 4: Verify

```bash
# Stage the resolution
git add <file>

# Check for remaining markers
git diff --check

# Run tests if possible
npm test
```

### Step 5: Continue

```bash
git rebase --continue
```

## When to Ask for Help

Escalate to the team when:
- You don't understand why main made a change
- The conflict involves critical business logic
- You're unsure if your feature is still needed
- The original author is available and can clarify

## Anti-Patterns to Avoid

### 1. "Accept Theirs" for Everything

This discards your work. Only appropriate when your changes are truly obsolete.

### 2. "Accept Ours" for Everything

This ignores main's updates. Your code may break when deployed.

### 3. Resolving Without Reading Commit Messages

You're guessing. Read the messages.

### 4. Not Running Tests After Each Resolution

Semantic conflicts won't be caught.

### 5. Resolving Quickly to "Just Get It Done"

Technical debt accumulates. Incorrect resolutions cause bugs.
