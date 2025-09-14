# Testing Guide for Marlin File Browser

## Overview

This project uses a comprehensive testing strategy to prevent regressions and ensure reliable functionality across UI and backend components.

## Test Infrastructure

### Unit Tests (Vitest + @testing-library/react)
- **Location**: `src/__tests__/unit/`
- **Purpose**: Test individual components, store logic, and utilities
- **Key Focus**: State management, preference persistence, navigation logic

### End-to-End Tests (Playwright)
- **Location**: `e2e/`  
- **Purpose**: Test complete user workflows and integration
- **Key Focus**: User interactions, cross-session persistence, regression prevention

## Running Tests

### Unit Tests
```bash
# Run tests in watch mode (development)
npm test

# Run tests once
npm run test:run

# Open Vitest UI
npm run test:ui
```

### E2E Tests
```bash
# Run E2E tests (requires app to be running)
npm run test:e2e

# Open Playwright test UI
npm run test:e2e-ui
```

## Critical Test Coverage

### Regression Prevention Tests
These tests specifically target the issues mentioned in the initial requirements:

1. **Hidden Files Toggle** (`src/__tests__/unit/store/useAppStore.test.ts`)
   - Tests global and directory-specific preference persistence
   - Verifies state synchronization with native menus
   - Ensures preferences survive navigation

2. **Directory Persistence** (E2E tests in `e2e/preferences.spec.ts`)
   - Tests that the app reopens to the last visited directory
   - Verifies directory-specific preferences are maintained
   - Tests rapid state changes don't corrupt data

3. **Navigation State** (E2E tests in `e2e/navigation.spec.ts`)
   - Tests history management across sessions
   - Verifies keyboard shortcuts work correctly
   - Tests path editing and validation

### State Management Tests
- `toggleHiddenFiles()` - Critical for hidden file regression prevention
- `updateDirectoryPreferences()` - Ensures per-directory settings persist
- `navigateTo()` - Tests navigation history and state management
- Preference persistence across app restarts

## Test Structure

### Unit Tests
```typescript
describe('useAppStore', () => {
  describe('toggleHiddenFiles', () => {
    it('should toggle global hidden files preference', async () => {
      // Test implementation
    })
  })
})
```

### E2E Tests
```typescript
test('should toggle hidden files and persist across navigation', async ({ page }) => {
  // Full user workflow test
})
```

## Mocking Strategy

### Unit Tests
- Mock `@tauri-apps/api/core` invoke function
- Mock `@tauri-apps/plugin-dialog` for UI interactions
- Mock filesystem operations with predictable responses

### E2E Tests
- Run against actual Tauri dev server
- Test real user interactions with browser automation
- Verify persistent state across page reloads

## Adding New Tests

### For New Features
1. Add unit tests for state management logic
2. Add component tests for UI behavior
3. Add E2E tests for complete user workflows

### For Bug Fixes
1. Write a failing test that reproduces the bug
2. Fix the bug
3. Verify the test passes
4. Add regression test to prevent future occurrences

## CI/CD Integration

Tests are configured to run automatically:
- Unit tests run on file changes during development
- All tests should pass before creating pull requests
- E2E tests verify the complete application works end-to-end

## Performance Considerations

- Unit tests are fast and run frequently
- E2E tests are slower but provide comprehensive coverage
- Tests focus on critical user paths and known regression areas
- Mock heavy operations to keep tests fast and reliable