import { describe, expect, it } from 'vitest';
import { buildArchiveUri, parseArchiveUri } from '@/utils/archiveUri';

describe('buildArchiveUri', () => {
  it("encodes !'()* as the backend does", () => {
    const uri = buildArchiveUri("/a/Bob's pack (1)!*.zip", '/in (2)');

    expect(uri).toBe(
      'archive:///?src=%2Fa%2FBob%27s%20pack%20%281%29%21%2A.zip&path=%2Fin%20%282%29'
    );
    expect(parseArchiveUri(uri)).toEqual({ src: "/a/Bob's pack (1)!*.zip", path: '/in (2)' });
  });
});
