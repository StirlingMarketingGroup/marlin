declare namespace Intl {
  type Granularity = 'grapheme' | 'word' | 'sentence';

  interface SegmenterOptions {
    granularity?: Granularity;
  }

  interface SegmentData {
    segment: string;
    index: number;
    input: string;
    isWordLike?: boolean;
  }

  interface SegmenterResult extends Iterable<SegmentData> {
    containing(index: number): SegmentData;
  }

  class Segmenter {
    constructor(locales?: string | string[], options?: SegmenterOptions);
    segment(input: string): SegmenterResult;
  }
}
