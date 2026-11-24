import { describe, it, expect, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useUndoStore } from '../../../store/useUndoStore';

vi.mock('@tauri-apps/api/core');

describe('useUndoStore', () => {
  // A move back can cross volumes, so it must be a move: a rename cannot copy.
  it('undoes a move with move_file', async () => {
    vi.mocked(invoke).mockResolvedValue(null);

    await useUndoStore.getState().executeUndoRecord({
      type: 'move',
      files: [{ originalPath: '/a/x.txt', currentPath: '/b/x.txt' }],
    });

    expect(invoke).toHaveBeenCalledWith('move_file', {
      fromPath: '/b/x.txt',
      toPath: '/a/x.txt',
    });
  });
});
