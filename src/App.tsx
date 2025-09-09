import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import Sidebar from './components/Sidebar'
import MainPanel from './components/MainPanel'
import PathBar from './components/PathBar'
import { useAppStore } from './store/useAppStore'

function App() {
  const { setCurrentPath, navigateTo, setLoading, setError, setFiles } = useAppStore()

  useEffect(() => {
    // Initialize the app by getting the home directory
    async function initializeApp() {
      try {
        setLoading(true)
        const homeDir = await invoke<string>('get_home_directory')
        setCurrentPath(homeDir)
        navigateTo(homeDir)
        
        // Load initial files
        const files = await invoke<any[]>('read_directory', { path: homeDir })
        setFiles(files)
        
        setError()
      } catch (error) {
        console.error('Failed to initialize app:', error)
        setError(String(error))
      } finally {
        setLoading(false)
      }
    }

    initializeApp()
  }, [setCurrentPath, navigateTo, setLoading, setError, setFiles])

  return (
    <div className="h-screen flex flex-col bg-discord-dark text-discord-text">
      {/* Path bar */}
      <PathBar />
      
      {/* Main content */}
      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <MainPanel />
      </div>
    </div>
  )
}

export default App