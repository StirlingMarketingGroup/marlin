import { useState, useEffect } from 'react'

export default function DragDropTest() {
  const [status, setStatus] = useState('Ready to test drag and drop')
  const [dragCounter, setDragCounter] = useState(0)

  // Test native HTML5 drag support
  useEffect(() => {
    const testElement = document.createElement('div')
    testElement.draggable = true
    
    let dragSupported = false
    testElement.ondragstart = () => { dragSupported = true }
    
    // Simulate drag start
    try {
      const event = new DragEvent('dragstart')
      testElement.dispatchEvent(event)
    } catch (e) {
      console.log('DragEvent constructor not supported')
    }
    
    setStatus(prev => prev + ` | Drag support: ${dragSupported ? 'YES' : 'NO'}`)
  }, [])

  return (
    <div className="fixed top-20 right-4 bg-app-dark border-2 border-accent p-4 rounded-lg shadow-xl" style={{ zIndex: 100000 }}>
      <h3 className="text-accent font-bold mb-2">Drag & Drop Test</h3>
      
      {/* Simple draggable */}
      <div
        draggable
        onDragStart={(e) => {
          setStatus('Drag started!')
          e.dataTransfer.setData('text', 'test')
        }}
        onDragEnd={() => setStatus('Drag ended')}
        className="bg-blue-500 text-white p-2 rounded mb-2 cursor-move"
      >
        Drag Me (Simple)
      </div>

      {/* Drop zone */}
      <div
        onDragEnter={(e) => {
          e.preventDefault()
          setDragCounter(c => c + 1)
          setStatus('Drag entered drop zone')
        }}
        onDragOver={(e) => {
          e.preventDefault()
          e.dataTransfer.dropEffect = 'copy'
        }}
        onDragLeave={(e) => {
          e.preventDefault()
          setDragCounter(c => c - 1)
          if (dragCounter <= 1) {
            setStatus('Drag left drop zone')
          }
        }}
        onDrop={(e) => {
          e.preventDefault()
          setDragCounter(0)
          const text = e.dataTransfer.getData('text')
          setStatus(`Dropped: ${text || 'no data'}`)
        }}
        className={`border-2 border-dashed p-4 rounded mt-2 ${
          dragCounter > 0 ? 'border-green-500 bg-green-500/20' : 'border-gray-500'
        }`}
      >
        Drop Zone
      </div>

      <div className="mt-2 text-xs text-gray-400">
        Status: {status}
      </div>
    </div>
  )
}