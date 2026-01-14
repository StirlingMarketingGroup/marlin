// src/utils/googleDriveUrl.test.ts
import { describe, it, expect } from 'vitest';
import { parseGoogleDriveUrl } from './googleDriveUrl';

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
