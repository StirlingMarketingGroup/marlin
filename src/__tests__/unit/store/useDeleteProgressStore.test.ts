import { describe, it, expect, beforeEach } from 'vitest';
import { useDeleteProgressStore } from '../../../store/useDeleteProgressStore';

describe('useDeleteProgressStore', () => {
  beforeEach(() => {
    useDeleteProgressStore.getState().reset();
  });

  describe('setContext', () => {
    it('should initialize state from payload', () => {
      const payload = {
        requestId: 'test-123',
        totalItems: 5,
        items: [
          { path: '/test/file1.txt', name: 'file1.txt', isDirectory: false },
          { path: '/test/file2.txt', name: 'file2.txt', isDirectory: false },
        ],
      };

      useDeleteProgressStore.getState().setContext(payload);

      const state = useDeleteProgressStore.getState();
      expect(state.requestId).toBe('test-123');
      expect(state.totalItems).toBe(5);
      expect(state.items).toEqual(payload.items);
      expect(state.completed).toBe(0);
      expect(state.finished).toBe(false);
      expect(state.error).toBeUndefined();
      expect(state.history).toEqual([]);
    });
  });

  describe('applyUpdate', () => {
    beforeEach(() => {
      useDeleteProgressStore.getState().setContext({
        requestId: 'test-123',
        totalItems: 3,
        items: [
          { path: '/test/file1.txt', name: 'file1.txt' },
          { path: '/test/file2.txt', name: 'file2.txt' },
          { path: '/test/file3.txt', name: 'file3.txt' },
        ],
      });
    });

    it('should update progress', () => {
      useDeleteProgressStore.getState().applyUpdate({
        requestId: 'test-123',
        currentPath: '/test/file1.txt',
        completed: 1,
        total: 3,
        finished: false,
      });

      const state = useDeleteProgressStore.getState();
      expect(state.completed).toBe(1);
      expect(state.currentPath).toBe('/test/file1.txt');
      expect(state.finished).toBe(false);
    });

    it('should add current path to history when completed increases', () => {
      useDeleteProgressStore.getState().applyUpdate({
        requestId: 'test-123',
        currentPath: '/test/file1.txt',
        completed: 1,
        total: 3,
        finished: false,
      });

      const state = useDeleteProgressStore.getState();
      expect(state.history).toContain('/test/file1.txt');
    });

    it('should ignore updates for different request id', () => {
      useDeleteProgressStore.getState().applyUpdate({
        requestId: 'different-id',
        currentPath: '/test/file1.txt',
        completed: 1,
        total: 3,
        finished: false,
      });

      const state = useDeleteProgressStore.getState();
      expect(state.completed).toBe(0);
    });

    it('should mark as finished', () => {
      useDeleteProgressStore.getState().applyUpdate({
        requestId: 'test-123',
        completed: 3,
        total: 3,
        finished: true,
      });

      const state = useDeleteProgressStore.getState();
      expect(state.finished).toBe(true);
      expect(state.completed).toBe(3);
    });

    it('should set error on failure', () => {
      useDeleteProgressStore.getState().applyUpdate({
        requestId: 'test-123',
        completed: 1,
        total: 3,
        finished: true,
        error: 'Permission denied',
      });

      const state = useDeleteProgressStore.getState();
      expect(state.error).toBe('Permission denied');
      expect(state.finished).toBe(true);
    });
  });

  describe('reset', () => {
    it('should reset to initial state', () => {
      useDeleteProgressStore.getState().setContext({
        requestId: 'test-123',
        totalItems: 5,
        items: [{ path: '/test/file.txt', name: 'file.txt' }],
      });
      useDeleteProgressStore.getState().applyUpdate({
        requestId: 'test-123',
        completed: 3,
        total: 5,
        finished: false,
      });

      useDeleteProgressStore.getState().reset();

      const state = useDeleteProgressStore.getState();
      expect(state.requestId).toBeUndefined();
      expect(state.items).toEqual([]);
      expect(state.totalItems).toBe(0);
      expect(state.completed).toBe(0);
      expect(state.finished).toBe(false);
    });
  });
});
