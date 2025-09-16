import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

interface QuickTooltipProps {
  text: string;
  delay?: number;
  children: (handlers: {
    onMouseEnter: (e: React.MouseEvent) => void;
    onMouseLeave: (e: React.MouseEvent) => void;
    onFocus: (e: React.FocusEvent) => void;
    onBlur: (e: React.FocusEvent) => void;
    ref: (el: HTMLElement | null) => void;
  }) => React.ReactNode;
}

export default function QuickTooltip({ text, delay = 120, children }: QuickTooltipProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const [offset, setOffset] = useState(0);
  const ref = useRef<HTMLElement | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const tooltipRef = useRef<HTMLDivElement | null>(null);

  useEffect(
    () => () => {
      if (timer.current) window.clearTimeout(timer.current);
    },
    []
  );

  const show = () => {
    if (!ref.current) return;
    const r = ref.current.getBoundingClientRect();
    const padding = 8;
    const preferredY = r.top - 28;
    const y = preferredY > 8 ? preferredY : Math.min(window.innerHeight - 28, r.bottom + 8);
    const centerX = r.left + r.width / 2;
    const x = Math.max(padding, Math.min(centerX, window.innerWidth - padding));
    setPos({ x, y });
    setOffset(0);
    setOpen(true);
  };

  const hide = () => {
    setOpen(false);
  };

  const handlers = {
    onMouseEnter: () => {
      if (timer.current) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(show, delay);
    },
    onMouseLeave: () => {
      if (timer.current) window.clearTimeout(timer.current);
      hide();
    },
    onFocus: () => {
      if (timer.current) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(show, Math.min(80, delay));
    },
    onBlur: () => {
      if (timer.current) window.clearTimeout(timer.current);
      hide();
    },
    ref: (el: HTMLElement | null) => {
      ref.current = el;
    },
  };

  const adjustWithinViewport = useCallback(() => {
    if (!open || !pos || !tooltipRef.current) return;
    const { width } = tooltipRef.current.getBoundingClientRect();
    const padding = 8;
    const half = width / 2;
    const available = Math.max(0, window.innerWidth - padding * 2);

    let nextOffset = 0;
    if (width > available) {
      // Align tooltip to the viewport padding when we cannot fit entirely.
      nextOffset = padding + half - pos.x;
    } else {
      const minCenter = padding + half;
      const maxCenter = window.innerWidth - padding - half;
      if (pos.x < minCenter) {
        nextOffset = minCenter - pos.x;
      } else if (pos.x > maxCenter) {
        nextOffset = maxCenter - pos.x;
      }
    }

    setOffset((prev) => {
      if (Math.abs(nextOffset - prev) <= 0.5) {
        return nextOffset === 0 ? 0 : prev;
      }
      return nextOffset;
    });
  }, [open, pos]);

  useLayoutEffect(() => {
    adjustWithinViewport();
  }, [adjustWithinViewport, text]);

  useEffect(() => {
    if (!open) return;
    const handle = () => adjustWithinViewport();
    window.addEventListener('resize', handle);
    return () => window.removeEventListener('resize', handle);
  }, [open, adjustWithinViewport]);

  return (
    <>
      {children(handlers)}
      {open &&
        pos &&
        createPortal(
          <div
            role="tooltip"
            ref={tooltipRef}
            className="fixed z-[1000] pointer-events-none select-none px-2 py-1 text-[12px] rounded bg-black/85 text-white shadow-lg text-center whitespace-nowrap"
            style={{
              left: pos.x,
              top: pos.y,
              transform: `translateX(-50%) translateX(${offset}px)`,
            }}
          >
            {text}
          </div>,
          document.body
        )}
    </>
  );
}
