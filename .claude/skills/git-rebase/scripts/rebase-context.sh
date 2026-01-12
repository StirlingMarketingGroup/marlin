#!/bin/bash
# rebase-context.sh - Gather full context before rebasing
# Usage: ./rebase-context.sh [base-branch]
# Default base-branch: origin/main

set -e

BASE_BRANCH="${1:-origin/main}"
CURRENT_BRANCH=$(git branch --show-current)

echo "=============================================="
echo "REBASE CONTEXT ANALYSIS"
echo "=============================================="
echo "Current branch: $CURRENT_BRANCH"
echo "Target base:    $BASE_BRANCH"
echo ""

# Ensure we have latest
echo ">>> Fetching latest..."
git fetch origin --quiet

echo ""
echo "=============================================="
echo "COMMITS IN $BASE_BRANCH (not in $CURRENT_BRANCH)"
echo "These are the changes you'll be rebasing onto:"
echo "=============================================="
git log --oneline HEAD..$BASE_BRANCH

echo ""
echo "=============================================="
echo "YOUR COMMITS (will be replayed)"
echo "=============================================="
git log --oneline $BASE_BRANCH..HEAD

echo ""
echo "=============================================="
echo "FILES CHANGED IN BOTH (potential conflicts)"
echo "=============================================="
# Files changed in base branch
BASE_FILES=$(git diff --name-only HEAD...$BASE_BRANCH 2>/dev/null || echo "")
# Files changed in our branch
OUR_FILES=$(git diff --name-only $BASE_BRANCH...HEAD 2>/dev/null || echo "")

# Find intersection
if [ -n "$BASE_FILES" ] && [ -n "$OUR_FILES" ]; then
    OVERLAP=$(comm -12 <(echo "$BASE_FILES" | sort) <(echo "$OUR_FILES" | sort))
    if [ -n "$OVERLAP" ]; then
        echo "$OVERLAP"
        echo ""
        echo ">>> These files were modified in BOTH branches!"
        echo ">>> Review changes carefully before rebasing."
    else
        echo "(No overlapping files - conflicts unlikely)"
    fi
else
    echo "(Could not determine file overlap)"
fi

echo ""
echo "=============================================="
echo "COMMIT MESSAGE SUMMARY"
echo "=============================================="
echo ""
echo "--- Base branch commits (read these to understand new context) ---"
git log --format="  %h %s" HEAD..$BASE_BRANCH | head -20

echo ""
echo "--- Your commits (understand what you're trying to achieve) ---"
git log --format="  %h %s" $BASE_BRANCH..HEAD

echo ""
echo "=============================================="
echo "NEXT STEPS"
echo "=============================================="
echo "1. Review the commit messages above"
echo "2. For potential conflict files, examine both sides:"
echo "   git log -p $BASE_BRANCH..HEAD -- <file>"
echo "   git log -p HEAD..$BASE_BRANCH -- <file>"
echo "3. When ready: git rebase $BASE_BRANCH"
echo ""
