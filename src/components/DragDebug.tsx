import { useState } from 'react'
import { Folder } from 'phosphor-react'

export default function DragDebug() {
  const [dragEvents, setDragEvents] = useState<string[]>([])
  const [isDragOver, setIsDragOver] = useState(false)

  const addEvent = (event: string) => {
    console.log(`🧪 DragDebug: ${event}`)
    setDragEvents(prev => [...prev.slice(-4), event])
  }

  return (
    <div className="fixed bottom-4 right-4 bg-app-dark border border-app-border rounded-lg p-4 shadow-lg" style={{ zIndex: 99999, pointerEvents: 'auto' }}>
      <h3 className="text-sm font-bold mb-2">Drag Debug</h3>
      
      {/* Test draggable directory */}
      <div
        draggable={true}
        onDragStart={(e) => {
          addEvent('dragStart - TEST DIR')
          const dragData = {
            type: 'file',
            path: '/test/directory',
            isDirectory: true,
            name: 'Test Directory'
          }
          // Try setting data in multiple formats
          try {
            e.dataTransfer.setData('text/plain', 'Test Directory')
            e.dataTransfer.setData('application/json', JSON.stringify(dragData))
            e.dataTransfer.effectAllowed = 'all'
            e.dataTransfer.dropEffect = 'copy'
          } catch (err) {
            addEvent(`dragStart error: ${err}`)
          }
        }}
        onDragEnd={() => addEvent('dragEnd')}
        className="bg-accent text-white px-3 py-2 rounded cursor-move mb-3 flex items-center gap-2"
      >
        <Folder className="w-4 h-4" weight="fill" />
        <span>Test Directory (Drag me)</span>
      </div>

      {/* Drop zone */}
      <div
        onDragEnter={(e) => {
          e.preventDefault()
          e.stopPropagation()
          addEvent('dragEnter - drop zone')
          setIsDragOver(true)
        }}
        onDragOver={(e) => {
          e.preventDefault()
          e.stopPropagation()
          e.dataTransfer.dropEffect = 'copy'
          // Add event only occasionally to avoid spam
          if (Math.random() < 0.1) addEvent('dragOver')
        }}
        onDragLeave={(e) => {
          e.preventDefault()
          e.stopPropagation()
          addEvent('dragLeave')
          setIsDragOver(false)
        }}
        onDrop={(e) => {
          e.preventDefault()
          e.stopPropagation()
          try {
            const jsonData = e.dataTransfer.getData('application/json')
            const textData = e.dataTransfer.getData('text/plain')
            if (jsonData) {
              const parsed = JSON.parse(jsonData)
              addEvent(`drop JSON: ${parsed.name}`)
            } else if (textData) {
              addEvent(`drop text: ${textData}`)
            } else {
              addEvent('drop: no data')
            }
          } catch (err) {
            addEvent(`drop error: ${err}`)
          }
          setIsDragOver(false)
        }}
        className={`border-2 border-dashed px-3 py-4 rounded text-center transition-colors ${
          isDragOver ? 'border-accent bg-accent/20' : 'border-app-border'
        }`}
      >
        Drop here to test
      </div>

      {/* Event log */}
      <div className="mt-3 text-xs">
        <div className="text-app-muted mb-1">Events:</div>
        {dragEvents.map((event, i) => (
          <div key={i} className="text-app-text font-mono text-[10px]">{event}</div>
        ))}
        {dragEvents.length === 0 && (
          <div className="text-app-muted">No events yet...</div>
        )}
      </div>
    </div>
  )
}