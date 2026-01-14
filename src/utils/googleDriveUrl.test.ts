// src/utils/googleDriveUrl.test.ts
import { describe, it, expect } from 'vitest';
import {
  parseGoogleDriveUrl,
  isGoogleDrivePath,
  parseGoogleDrivePathEmail,
} from './googleDriveUrl';

describe('parseGoogleDriveUrl', () => {
  it('extracts ID from /drive/folders/ URL', () => {
    const url = 'https://drive.google.com/drive/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from /open?id= URL', () => {
    const url = 'https://drive.google.com/open?id=1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS&usp=drive_fs';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from /file/d/ URL', () => {
    const url = 'https://drive.google.com/file/d/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS/view';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from URL with account index', () => {
    const url = 'https://drive.google.com/drive/u/0/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from docs.google.com document URL', () => {
    const url = 'https://docs.google.com/document/d/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS/edit';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from docs.google.com spreadsheet URL', () => {
    const url = 'https://docs.google.com/spreadsheets/d/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS/edit';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('returns null for non-Google Drive URLs', () => {
    expect(parseGoogleDriveUrl('https://example.com/folder')).toBeNull();
    expect(parseGoogleDriveUrl('/Users/home/folder')).toBeNull();
    expect(parseGoogleDriveUrl('gdrive://email/My Drive')).toBeNull();
  });

  it('returns null for malformed Google Drive URLs', () => {
    expect(parseGoogleDriveUrl('https://drive.google.com/drive/folders/')).toBeNull();
    expect(parseGoogleDriveUrl('https://drive.google.com/open')).toBeNull();
  });
});

describe('isGoogleDrivePath', () => {
  it('returns true for gdrive:// paths', () => {
    expect(isGoogleDrivePath('gdrive://test@example.com/My Drive')).toBe(true);
    expect(isGoogleDrivePath('gdrive://user@gmail.com/Shared drives/Team/folder')).toBe(true);
    expect(isGoogleDrivePath('gdrive://email')).toBe(true);
  });

  it('returns false for non-gdrive paths', () => {
    expect(isGoogleDrivePath('/Users/home/folder')).toBe(false);
    expect(isGoogleDrivePath('https://drive.google.com/folders/123')).toBe(false);
    expect(isGoogleDrivePath('file://local/path')).toBe(false);
    expect(isGoogleDrivePath('')).toBe(false);
  });
});

describe('parseGoogleDrivePathEmail', () => {
  it('extracts email from gdrive:// path with path component', () => {
    expect(parseGoogleDrivePathEmail('gdrive://test@example.com/My Drive/folder')).toBe(
      'test@example.com'
    );
    expect(parseGoogleDrivePathEmail('gdrive://user@gmail.com/Shared drives')).toBe(
      'user@gmail.com'
    );
  });

  it('extracts email from gdrive:// path without path component', () => {
    expect(parseGoogleDrivePathEmail('gdrive://test@example.com')).toBe('test@example.com');
  });

  it('returns null for non-gdrive paths', () => {
    expect(parseGoogleDrivePathEmail('/Users/home/folder')).toBeNull();
    expect(parseGoogleDrivePathEmail('https://drive.google.com')).toBeNull();
  });

  it('returns null for gdrive:// with empty email', () => {
    expect(parseGoogleDrivePathEmail('gdrive://')).toBeNull();
    expect(parseGoogleDrivePathEmail('gdrive:///path')).toBeNull();
  });
});
