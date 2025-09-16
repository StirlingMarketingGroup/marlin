import { useEffect, useState } from 'react';
import { Minus, Plus, ArrowCounterClockwise } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';

interface ZoomSliderProps {
  visible: boolean;
}

export default function ZoomSlider({ visible }: ZoomSliderProps) {
  const {
    currentPath,
    globalPreferences,
    directoryPreferences,
    updateDirectoryPreferences,
    showZoomSliderNow,
    scheduleHideZoomSlider,
  } = useAppStore();
  const merged = { ...globalPreferences, ...directoryPreferences[currentPath] };
  const defaultGrid = globalPreferences.gridSize ?? 120;
  const initial = Math.max(80, Math.min(320, merged.gridSize ?? 120));
  const [value, setValue] = useState<number>(initial);

  useEffect(() => {
    const next = Math.max(80, Math.min(320, merged.gridSize ?? 120));
    setValue(next);
  }, [currentPath, merged.gridSize]);

  const setSize = (n: number) => {
    const clamped = Math.max(80, Math.min(320, Math.round(n)));
    setValue(clamped);
    updateDirectoryPreferences(currentPath, { gridSize: clamped });
  };

  if (!visible) return null;

  return (
    <>
      {/* Fixed to viewport so it stays visible when scrolling. Offset below the toolbar. */}
      <div
        className="fixed top-16 right-3 z-50 select-none"
        data-tauri-drag-region={false}
        onMouseEnter={() => showZoomSliderNow()}
        onMouseLeave={() => scheduleHideZoomSlider(250)}
      >
        <div className="flex items-center gap-2 bg-app-gray/90 border border-app-border rounded-md px-2 py-1 shadow">
          <button
            className="p-1 rounded hover:bg-app-light"
            onClick={() => setSize((value || 120) - 8)}
            aria-label="Zoom out"
          >
            <Minus className="w-4 h-4" />
          </button>
          <input
            type="range"
            min={80}
            max={320}
            step={8}
            value={value}
            onChange={(e) => setSize(parseInt(e.target.value))}
            className="w-32 accent-[var(--accent)]"
          />
          <button
            className="p-1 rounded hover:bg-app-light"
            onClick={() => setSize((value || 120) + 8)}
            aria-label="Zoom in"
          >
            <Plus className="w-4 h-4" />
          </button>
          <div className="w-px h-4 bg-app-border mx-1" />
          <button
            className="p-1 rounded hover:bg-app-light"
            onClick={() => setSize(defaultGrid)}
            aria-label="Reset zoom"
            title="Reset zoom"
          >
            <ArrowCounterClockwise className="w-4 h-4" />
          </button>
        </div>
      </div>
    </>
  );
}
