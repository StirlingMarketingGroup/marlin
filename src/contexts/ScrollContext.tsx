import { createContext, useContext, RefObject } from 'react';

type ScrollContextValue = RefObject<HTMLDivElement | null> | null;

export const ScrollContext = createContext<ScrollContextValue>(null);

// Returns the ref itself so virtualizer can call .current inside its callback
export function useScrollContainerRef(): RefObject<HTMLDivElement | null> | null {
  return useContext(ScrollContext);
}
