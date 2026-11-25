import { describe, it, expect, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useUndoStore } from '../../../store/useUndoStore';
import { useToastStore } from '../../../store/useToastStore';

vi.mock('@tauri-apps/api/core');

describe('useUndoStore', () => {
  // A move back can cross volumes, so it must be a move: a rename cannot copy.
  it('undoes a move with move_file', async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    const id = useUndoStore.getState().pushUndo(
      {
        type: 'move',
        files: [{ originalPath: '/a/x.txt', currentPath: '/b/x.txt' }],
      },
      'Moved 1 item'
    );

    await useUndoStore.getState().executeUndoEntry(id);

    expect(invoke).toHaveBeenCalledWith('move_file', {
      fromPath: '/b/x.txt',
      toPath: '/a/x.txt',
    });
  });

  it('leaves copies in a folder with no Trash, says so, and drops the undo', async () => {
    vi.mocked(invoke).mockImplementation((cmd) =>
      Promise.resolve(
        cmd === 'trash_paths' ? { trashed: [], undoToken: null, noTrash: 'remote' } : undefined
      )
    );
    useToastStore.setState({ toasts: [] });
    useUndoStore.setState({ stack: [] });
    useUndoStore
      .getState()
      .pushUndo({ type: 'copy', copiedPaths: ['/share/a.txt'] }, 'Copied 1 item');

    await useUndoStore.getState().executeUndo();

    expect(invoke).not.toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
    expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
      'Undo failed: The copies are in a folder with no Trash, so they were left in place.',
    ]);
    // A retry would fail the same way, so the entry does not block the undos under it.
    expect(useUndoStore.getState().stack).toEqual([]);
  });

  it('drops the undo of a Move to Trash whose restore failed', async () => {
    vi.mocked(invoke).mockImplementation((cmd) =>
      cmd === 'undo_trash'
        ? Promise.reject(new Error('Failed to restore a.txt from Trash'))
        : Promise.resolve(undefined)
    );
    useToastStore.setState({ toasts: [] });
    useUndoStore.setState({ stack: [] });
    useUndoStore
      .getState()
      .pushUndo(
        { type: 'trash', undoToken: 'spent', trashedPaths: ['/test/a.txt'] },
        'Trash a.txt'
      );

    await useUndoStore.getState().executeUndo();

    expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
      'Undo failed: Failed to restore a.txt from Trash',
    ]);
    // `undo_trash` spent the token, so a retry could never work.
    expect(useUndoStore.getState().stack).toEqual([]);
  });

  it('keeps the undo of a copy, in its place, when the Trash refuses', async () => {
    vi.mocked(invoke).mockImplementation((cmd) =>
      Promise.resolve(
        cmd === 'trash_paths' ? { trashed: [], undoToken: null, noTrash: 'refused' } : undefined
      )
    );
    useToastStore.setState({ toasts: [] });
    useUndoStore.setState({ stack: [] });
    const older = useUndoStore
      .getState()
      .pushUndo({ type: 'copy', copiedPaths: ['/test/a.txt'] }, 'Copied a.txt');
    useUndoStore
      .getState()
      .pushUndo({ type: 'copy', copiedPaths: ['/test/b.txt'] }, 'Copied b.txt');

    // The older paste's toast button, which can be clicked after a newer paste.
    await useUndoStore.getState().executeUndoEntry(older);

    expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
      'Undo failed: The copies could not be moved to the Trash.',
    ]);
    expect(useUndoStore.getState().stack.map((entry) => entry.description)).toEqual([
      'Copied a.txt',
      'Copied b.txt',
    ]);
  });
});
