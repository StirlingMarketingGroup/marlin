import { describe, it, expect } from 'vitest';
import { basename } from '../../../utils/pathUtils';

describe('pathUtils', () => {
  describe('basename', () => {
    it('should extract filename from Unix path', () => {
      expect(basename('/Users/test/file.txt')).toBe('file.txt');
    });

    it('should extract filename from Windows path', () => {
      expect(basename('C:\\Users\\test\\file.txt')).toBe('file.txt');
    });

    it('should handle mixed slashes', () => {
      expect(basename('C:\\Users/test\\file.txt')).toBe('file.txt');
    });

    it('should handle trailing slashes', () => {
      expect(basename('/Users/test/folder/')).toBe('folder');
    });

    it('should handle root path', () => {
      expect(basename('/')).toBe('/');
    });

    it('should handle empty string', () => {
      expect(basename('')).toBe('');
    });

    it('should handle filename only', () => {
      expect(basename('file.txt')).toBe('file.txt');
    });

    it('should handle Windows drive root', () => {
      expect(basename('C:\\')).toBe('C:');
    });
  });
});
