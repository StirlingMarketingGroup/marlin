import { useEffect } from 'react'
import { Folder } from 'phosphor-react'
import { useDragStore } from '@/store/useDragStore'

export default function DragPreview() {
  const { isDragging, draggedDirectory, dragPreviewPosition, updateDragPosition } = useDragStore()

  useEffect(() => {
    if (!isDragging) return

    const handleMouseMove = (e: MouseEvent) => {
      updateDragPosition(e.clientX, e.clientY)
    }

    document.addEventListener('mousemove', handleMouseMove)
    return () => document.removeEventListener('mousemove', handleMouseMove)
  }, [isDragging, updateDragPosition])

  if (!isDragging || !draggedDirectory || !dragPreviewPosition) return null

  return (
    <div
      className="fixed pointer-events-none z-[999999] bg-app-dark/95 backdrop-blur-sm border border-accent rounded-lg px-3 py-2 flex items-center gap-2 shadow-xl"
      style={{
        left: dragPreviewPosition.x + 10,
        top: dragPreviewPosition.y + 10,
      }}
    >
      <Folder className="w-4 h-4 text-accent" weight="fill" />
      <span className="text-sm font-medium text-white">{draggedDirectory.name}</span>
    </div>
  )
}