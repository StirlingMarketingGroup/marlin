import { describe, it, expect } from 'vitest';
import { basename, parentPath, resolveTypedPath } from '../../../utils/pathUtils';
import { buildArchiveUri } from '../../../utils/archiveUri';

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

  describe('parentPath', () => {
    it('returns the folder that holds a path', () => {
      expect(parentPath('/a/b')).toBe('/a');
      expect(parentPath('/a')).toBe('/');
      expect(parentPath('/a/b/')).toBe('/a');
      expect(parentPath('C:\\Users\\me')).toBe('C:/Users');
      expect(parentPath('smb://server/share/folder')).toBe('smb://server/share');
      expect(parentPath('smb://server/share')).toBe('smb://server');
    });

    it('has no parent at a root', () => {
      for (const root of [
        '',
        '/',
        'C:',
        'C:\\',
        'smb://server',
        'smb://server/',
        'gdrive://me@example.com/',
      ]) {
        expect(parentPath(root)).toBeUndefined();
      }
    });

    it('returns the parent folder inside an archive', () => {
      expect(parentPath(buildArchiveUri('/a/pack.zip', '/inner/deep'))).toBe(
        buildArchiveUri('/a/pack.zip', '/inner')
      );
      expect(parentPath(buildArchiveUri('/a/pack.zip', '/inner'))).toBe(
        buildArchiveUri('/a/pack.zip', '/')
      );
    });

    it('returns the folder that holds the archive file at an archive root', () => {
      expect(parentPath(buildArchiveUri('/a/pack.zip', '/'))).toBe('/a');
      expect(parentPath(buildArchiveUri('smb://server/share/pack.zip', '/'))).toBe(
        'smb://server/share'
      );
    });

    it('returns the folder that holds a nested archive file', () => {
      const innerFile = buildArchiveUri('/a/outer.zip', '/dir/inner.zip');

      expect(parentPath(buildArchiveUri(innerFile, '/'))).toBe(
        buildArchiveUri('/a/outer.zip', '/dir')
      );
    });
  });

  describe('resolveTypedPath', () => {
    it('reads a pasted address with spaces around it as Enter does', () => {
      expect(resolveTypedPath(' smb://alice:secret@nas/share/ ')).toBe('smb://nas/share/');
      expect(resolveTypedPath(' sftp://alice:secret@host/home')).toBe('sftp://alice@host/home');
    });
  });
});
