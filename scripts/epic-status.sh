#!/usr/bin/env bash
set -euo pipefail

# epic-status.sh - Show progress through an epic
# Usage: ./scripts/epic-status.sh <epic-id>

EPIC_ID="${1:-}"

if [ -z "$EPIC_ID" ]; then
    echo "Usage: $0 <epic-id>"
    echo "Example: $0 ize-jvi"
    exit 1
fi

# Get epic details
EPIC_SHOW=$(bd show "$EPIC_ID" 2>/dev/null) || {
    echo "Error: Could not find epic '$EPIC_ID'"
    exit 1
}

# Extract epic title and description
EPIC_TITLE=$(echo "$EPIC_SHOW" | head -n1 | sed -E 's/^[○✓] [^ ]+ (\[EPIC\])? · //' | sed 's/ *\[.*$//')
echo "═══════════════════════════════════════════════════════════════════"
echo "EPIC: $EPIC_ID"
echo "═══════════════════════════════════════════════════════════════════"
echo "$EPIC_TITLE"
echo ""

# Extract children section
CHILDREN_SECTION=$(echo "$EPIC_SHOW" | sed -n '/^CHILDREN$/,/^$/p' | tail -n +2)

if [ -z "$CHILDREN_SECTION" ]; then
    echo "No child issues found."
    exit 0
fi

# Count total, done, and remaining
TOTAL=$(echo "$CHILDREN_SECTION" | grep -c '↳' || echo "0")
DONE=$(echo "$CHILDREN_SECTION" | grep -c '↳ ✓' || echo "0")
REMAINING=$((TOTAL - DONE))

echo "Progress: $DONE/$TOTAL completed ($REMAINING remaining)"
echo ""

# Process completed issues
if [ "$DONE" -gt 0 ]; then
    echo "───────────────────────────────────────────────────────────────────"
    echo "✓ COMPLETED ($DONE)"
    echo "───────────────────────────────────────────────────────────────────"
    echo ""

    # Extract completed child IDs
    COMPLETED_IDS=$(echo "$CHILDREN_SECTION" | grep '↳ ✓' | sed -E 's/.*↳ ✓ ([^:]+):.*/\1/')

    while IFS= read -r CHILD_ID; do
        if [ -n "$CHILD_ID" ]; then
            CHILD_SHOW=$(bd show "$CHILD_ID" 2>/dev/null || echo "")
            if [ -n "$CHILD_SHOW" ]; then
                # Extract title
                CHILD_TITLE=$(echo "$CHILD_SHOW" | head -n1 | sed -E 's/^[○✓] [^ ]+ · //' | sed 's/ *\[.*$//')

                # Extract description (everything between DESCRIPTION and next section or blank lines)
                CHILD_DESC=$(echo "$CHILD_SHOW" | sed -n '/^DESCRIPTION$/,/^[A-Z][A-Z]/p' | sed '1d;$d' | sed '/^$/q' | sed 's/^  //')

                echo "✓ $CHILD_ID: $CHILD_TITLE"
                if [ -n "$CHILD_DESC" ]; then
                    echo "$CHILD_DESC" | sed 's/^/  /'
                fi
                echo ""
            fi
        fi
    done <<< "$COMPLETED_IDS"
fi

# Process remaining issues
if [ "$REMAINING" -gt 0 ]; then
    echo "───────────────────────────────────────────────────────────────────"
    echo "○ REMAINING ($REMAINING)"
    echo "───────────────────────────────────────────────────────────────────"
    echo ""

    # Extract open child entries
    OPEN_ISSUES=$(echo "$CHILDREN_SECTION" | grep '↳ ○')

    while IFS= read -r LINE; do
        if [ -n "$LINE" ]; then
            # Extract ID and title
            CHILD_ID=$(echo "$LINE" | sed -E 's/.*↳ ○ ([^:]+):.*/\1/')
            CHILD_TITLE=$(echo "$LINE" | sed -E 's/.*↳ ○ [^:]+: ([^●]+).*/\1/' | sed 's/ *$//')
            echo "○ $CHILD_ID: $CHILD_TITLE"
        fi
    done <<< "$OPEN_ISSUES"
    echo ""
fi

echo "═══════════════════════════════════════════════════════════════════"
