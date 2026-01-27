// src/utils/zipNaming.test.ts
import { describe, it, expect } from 'vitest';
import { getSuggestedZipName } from './zipNaming';
import type { FileItem } from '@/types';

const makeItem = (name: string, is_directory = false): FileItem => ({
  name,
  path: `/test/${name}`,
  is_directory,
  is_hidden: name.startsWith('.'),
  is_symlink: false,
  is_git_repo: false,
  size: 0,
  modified: new Date().toISOString(),
});

describe('getSuggestedZipName', () => {
  it('returns Archive.zip for empty selection', () => {
    const result = getSuggestedZipName({
      selectedItems: [],
      visibleItems: [makeItem('file.txt')],
      currentPath: '/test',
    });
    expect(result).toBe('Archive.zip');
  });

  it('returns folder name for single directory selection', () => {
    const folder = makeItem('MyFolder', true);
    const result = getSuggestedZipName({
      selectedItems: [folder],
      visibleItems: [folder, makeItem('other.txt')],
      currentPath: '/test',
    });
    expect(result).toBe('MyFolder.zip');
  });

  it('returns file name without extension for single file selection', () => {
    const file = makeItem('document.pdf');
    const result = getSuggestedZipName({
      selectedItems: [file],
      visibleItems: [file, makeItem('other.txt')],
      currentPath: '/test',
    });
    expect(result).toBe('document.zip');
  });

  it('uses parent dir name when selecting only file (100% selection)', () => {
    const file = makeItem('README');
    const result = getSuggestedZipName({
      selectedItems: [file],
      visibleItems: [file],
      currentPath: '/test',
    });
    // When selecting all visible items (100%), uses parent directory name
    expect(result).toBe('test.zip');
  });

  it('handles dotfiles - uses parent dir for 100% selection', () => {
    const file = makeItem('.gitignore');
    const result = getSuggestedZipName({
      selectedItems: [file],
      visibleItems: [file],
      currentPath: '/projects/myrepo',
    });
    // When selecting all visible items (100%), uses parent directory name
    expect(result).toBe('myrepo.zip');
  });

  it('uses file name without extension when not all items selected', () => {
    const file = makeItem('README');
    const other = makeItem('other.txt');
    const result = getSuggestedZipName({
      selectedItems: [file],
      visibleItems: [file, other],
      currentPath: '/test',
    });
    expect(result).toBe('README.zip');
  });

  it('strips extension from dotfiles when not all items selected', () => {
    const file = makeItem('.env.local');
    const other = makeItem('other.txt');
    const result = getSuggestedZipName({
      selectedItems: [file],
      visibleItems: [file, other],
      currentPath: '/test',
    });
    // stripExtension keeps leading dot but removes .local
    expect(result).toBe('.env.zip');
  });

  it('returns parent directory name when selecting all visible items (100%)', () => {
    const items = [makeItem('file1.txt'), makeItem('file2.txt')];
    const result = getSuggestedZipName({
      selectedItems: items,
      visibleItems: items,
      currentPath: '/home/user/Documents',
    });
    expect(result).toBe('Documents.zip');
  });

  it('returns parent directory name when selecting 90%+ of visible items', () => {
    const visible = Array.from({ length: 10 }, (_, i) => makeItem(`file${i}.txt`));
    const selected = visible.slice(0, 9); // 90%
    const result = getSuggestedZipName({
      selectedItems: selected,
      visibleItems: visible,
      currentPath: '/home/user/Projects',
    });
    expect(result).toBe('Projects.zip');
  });

  it('returns first item name when selecting less than 90% of items', () => {
    const visible = Array.from({ length: 10 }, (_, i) => makeItem(`file${i}.txt`));
    const selected = visible.slice(0, 8); // 80%
    const result = getSuggestedZipName({
      selectedItems: selected,
      visibleItems: visible,
      currentPath: '/home/user/Projects',
    });
    expect(result).toBe('file0.zip');
  });

  it('returns Archive.zip for root path selection', () => {
    const items = [makeItem('file.txt')];
    const result = getSuggestedZipName({
      selectedItems: items,
      visibleItems: items,
      currentPath: '/',
    });
    expect(result).toBe('Archive.zip');
  });

  it('returns first item name when visibleItems is empty', () => {
    const file = makeItem('lonely.txt');
    const result = getSuggestedZipName({
      selectedItems: [file],
      visibleItems: [],
      currentPath: '/test',
    });
    expect(result).toBe('lonely.zip');
  });

  it('handles directory with extension in name', () => {
    const folder = makeItem('project.bak', true);
    const result = getSuggestedZipName({
      selectedItems: [folder],
      visibleItems: [folder],
      currentPath: '/test',
    });
    expect(result).toBe('project.bak.zip');
  });

  it('uses first directory name for multiple directory selection (partial)', () => {
    const folders = [makeItem('FolderA', true), makeItem('FolderB', true)];
    const visible = [...folders, makeItem('file.txt')];
    const result = getSuggestedZipName({
      selectedItems: folders,
      visibleItems: visible,
      currentPath: '/test',
    });
    expect(result).toBe('FolderA.zip');
  });
});
