# File Name Truncation Analysis & Issues

## Current Problems

### 1. Grid/Thumb View Issues

#### Double Ellipsis Problem
- **Issue**: Some items show TWO ellipses (one from our middle truncation, one from CSS)
- **Example**: "Front SW - 9-...25,..." 
- **Cause**: Both JavaScript truncation AND CSS text-overflow are being applied

#### End-Only Ellipsis
- **Issue**: Some items only show ellipsis at the end, not in the middle
- **Example**: "Photos Library.photosl..."
- **Cause**: CSS text-overflow overriding our middle truncation logic

#### Text Movement on Selection
- **Issue**: When selecting an item, the text box narrows and text reflows/moves
- **Example**: "ChatGPT Image Sep 10, 2025, 06_11_25 PM.png" moves position when selected
- **Cause**: Selection state changes the layout/width calculations

### 2. List View Issues

#### Text Overflow
- **Issue**: Long file names overflow and render on top of other columns (Size, Type, Modified)
- **Example**: Long filenames like "2024 K1 Very Good Garage LLC - BRIAN LEISHMAN.pdf" overflow into adjacent columns
- **Cause**: Not properly constraining text within column boundaries

## Failed Approaches

### Attempt 1: useVisualTruncation Hook
- **Approach**: DOM measurement-based truncation using hidden measuring element
- **Problems**:
  - Flash on load (text appears full, then shrinks)
  - Performance issues from constant DOM measurements
  - Complex binary search algorithm
  - Incorrect measurements leading to over-truncation

### Attempt 2: Character Width Estimation
- **Approach**: Calculate max characters based on estimated character width (CHAR_WIDTH = 6.5px)
- **Problems**:
  - Character widths vary SIGNIFICANTLY (i, l, 1 vs W, M, @)
  - Numbers and uppercase letters are wider than lowercase
  - Special characters have unpredictable widths
  - Led to both over and under truncation

### Attempt 3: CSS + JS Hybrid
- **Approach**: Use CSS line-clamp for grid, JavaScript truncation for overflow
- **Problems**:
  - Double ellipsis from both systems
  - Inconsistent behavior between items
  - Text movement on selection
  - CSS and JS fighting each other

## Why Character Counting Fails

1. **Variable Width Fonts**: 
   - "iii" vs "WWW" - same character count, vastly different widths
   - "01977e0c-e97e-7371" much wider than "library.photosl"

2. **Font Rendering Differences**:
   - Different OS/browser combinations render fonts differently
   - Subpixel rendering affects actual width
   - Font weight changes (selected state) affect width

3. **Special Characters**:
   - Dots, dashes, underscores all have different widths
   - File extensions with numbers vs letters

## Requirements Recap

### List View
- ✅ Single line display
- ✅ Middle ellipsis (preserve start + extension)
- ✅ Tooltip on truncation
- ❌ No overflow into other columns
- ❌ Proper width constraints

### Grid/Thumb View  
- ✅ Maximum 2 lines of text
- ❌ Middle ellipsis when needed
- ❌ No double ellipsis
- ❌ Text stays in same position when selected
- ✅ Subtle background on selection (not black popout)
- ❌ No text movement/reflow on selection

## Potential Solutions to Explore

### 1. Canvas-Based Measurement
- Use Canvas API to accurately measure text width
- Cache measurements for performance
- Libraries: react-middle-truncate uses this approach

### 2. CSS-Only with Data Attributes
- Use CSS custom properties with actual measured widths
- Set truncated text as data attribute
- Let CSS handle display

### 3. Fixed-Width Container Approach
- Use fixed pixel widths for containers
- Let browser handle text-overflow
- Custom ellipsis using ::before/::after pseudo elements

### 4. Third-Party Libraries
- **react-middle-ellipsis**: Handles middle truncation with proper measurement
- **react-truncate-markup**: More sophisticated truncation options
- **use-resize-observer**: Better width detection

### 5. Native CSS Solutions (Future)
- CSS Working Group discussing `text-overflow: ellipsis-middle`
- Not yet implemented in browsers

## Key Insights

1. **Character counting will never work reliably** with variable-width fonts
2. **CSS and JavaScript truncation shouldn't be mixed** - pick one approach
3. **Real measurement is required** - either Canvas, getBoundingClientRect, or Range API
4. **Performance matters** - measurements should be cached and debounced
5. **Selection state needs special handling** - width changes must be accounted for

## Recommended Next Steps

1. **Remove all current truncation logic** - start fresh
2. **Choose ONE approach** - either pure CSS or pure JavaScript
3. **Implement proper measurement** - Canvas API or library
4. **Test with extreme cases**:
   - Very long names with numbers
   - Unicode characters
   - Mixed case text
   - Different file extensions
5. **Consider using a proven library** rather than reinventing the wheel

## Test Cases Needed

- Short names (should not truncate)
- Long names with short extensions (.c, .py)
- Long names with long extensions (.photoslibrary)
- Names with many dots (version.1.2.3.final.backup.old.txt)
- Unicode/emoji in names
- Very narrow grid tiles
- Very wide list columns
- Selected vs unselected states

## SOLUTION IMPLEMENTED (Dec 2024)

### Approach: Canvas-Based Text Measurement

Created a new utility (`src/utils/textMeasure.ts`) that uses the Canvas API to accurately measure text width. This provides:

1. **Accurate Text Measurement**:
   - Uses Canvas 2D context to measure exact pixel width
   - Caches measurements for performance
   - Binary search algorithm for optimal truncation point

2. **Smart Middle Truncation**:
   - Preserves file extension
   - Splits remaining space evenly between start and end
   - Falls back to end truncation if extension is too long

3. **View-Specific Handling**:
   - **List View**: Uses Canvas measurement for precise truncation
   - **Grid View**: Uses CSS `-webkit-line-clamp` for 2-line display
   - No mixing of CSS and JS truncation

### Key Changes

1. **New Files**:
   - `src/utils/textMeasure.ts` - Canvas-based measurement utility

2. **Modified Files**:
   - `src/components/FileNameDisplay.tsx` - Simplified to use new measurement
   - `src/components/FileList.tsx` - Proper column constraints
   - `src/components/FileGrid.tsx` - Stable layout on selection

### Results

✅ **List View**:
- No overflow into other columns
- Proper middle ellipsis with extension preservation
- Tooltip on truncation

✅ **Grid View**:
- CSS-only 2-line display (no JS truncation)
- No double ellipsis
- Stable text position on selection
- File size shown below name

### Performance Optimizations

- Measurement results cached in memory
- Cache automatically pruned at 1000 entries
- Binary search for finding truncation point
- Memoized components prevent unnecessary recalculation