import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useAppStore } from '../../../store/useAppStore';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core');
vi.mock('@tauri-apps/plugin-dialog');

const mockInvoke = vi.mocked(invoke);

describe('useAppStore filter state', () => {
  beforeEach(() => {
    // Reset store to initial state with filter properties
    useAppStore.setState({
      currentPath: '/test',
      currentLocationRaw: 'file:///test',
      filterText: '',
      showFilterInput: false,
      pathHistory: ['/test'],
      historyIndex: 0,
    });

    vi.clearAllMocks();

    // Mock the directory commands
    mockInvoke.mockImplementation((cmd, payload) => {
      if (cmd === 'read_directory_streaming_command') {
        const args = payload as { path: string; sessionId: string };
        return Promise.resolve({
          sessionId: args.sessionId,
          location: {
            raw: 'file:///some/new/path',
            scheme: 'file',
            authority: null,
            path: '/some/new/path',
            displayPath: '/some/new/path',
          },
          capabilities: {
            scheme: 'file',
            displayName: 'Local Filesystem',
            canRead: true,
            canWrite: true,
            canCreateDirectories: true,
            canDelete: true,
            canRename: true,
            canCopy: true,
            canMove: true,
            supportsWatching: true,
            requiresExplicitRefresh: false,
          },
        });
      }
      if (cmd === 'get_git_status') {
        return Promise.resolve(null);
      }
      return Promise.resolve(undefined);
    });
  });

  it('should have empty filter text by default', () => {
    const state = useAppStore.getState();
    expect(state.filterText).toBe('');
  });

  it('should have showFilterInput false by default', () => {
    const state = useAppStore.getState();
    expect(state.showFilterInput).toBe(false);
  });

  it('should update filter text via setFilterText', () => {
    const { setFilterText } = useAppStore.getState();
    setFilterText('test');
    expect(useAppStore.getState().filterText).toBe('test');
  });

  it('should show filter input when filter text is set', () => {
    const { setFilterText } = useAppStore.getState();
    setFilterText('abc');
    expect(useAppStore.getState().showFilterInput).toBe(true);
  });

  it('should hide filter input when filter text is cleared', () => {
    const { setFilterText, clearFilter } = useAppStore.getState();
    setFilterText('test');
    clearFilter();
    expect(useAppStore.getState().filterText).toBe('');
    expect(useAppStore.getState().showFilterInput).toBe(false);
  });

  it('should clear filter when navigating to new directory', () => {
    useAppStore.setState({ filterText: 'test', showFilterInput: true });
    const { navigateTo } = useAppStore.getState();
    navigateTo('/some/new/path');
    expect(useAppStore.getState().filterText).toBe('');
    expect(useAppStore.getState().showFilterInput).toBe(false);
  });

  it('should append character to filter text via appendToFilter', () => {
    const { appendToFilter } = useAppStore.getState();
    appendToFilter('a');
    expect(useAppStore.getState().filterText).toBe('a');
    appendToFilter('b');
    expect(useAppStore.getState().filterText).toBe('ab');
    expect(useAppStore.getState().showFilterInput).toBe(true);
  });
});
