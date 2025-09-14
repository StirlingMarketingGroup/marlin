import { useToastStore } from '../store/useToastStore'
import { X, CheckCircle, XCircle, Info } from 'phosphor-react'

export default function Toast() {
  const { toasts, removeToast } = useToastStore()

  if (toasts.length === 0) return null

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onClose={() => removeToast(toast.id)} />
      ))}
    </div>
  )
}

function ToastItem({ toast, onClose }: { toast: any; onClose: () => void }) {
  const getIcon = () => {
    switch (toast.type) {
      case 'success':
        return <CheckCircle className="w-5 h-5 text-green-500" weight="fill" />
      case 'error':
        return <XCircle className="w-5 h-5 text-red-500" weight="fill" />
      case 'info':
        return <Info className="w-5 h-5 text-blue-500" weight="fill" />
    }
  }

  const getBgColor = () => {
    switch (toast.type) {
      case 'success':
        return 'bg-green-500/10 border-green-500/20'
      case 'error':
        return 'bg-red-500/10 border-red-500/20'
      case 'info':
        return 'bg-blue-500/10 border-blue-500/20'
    }
  }

  return (
    <div
      className={`flex items-center gap-3 px-4 py-3 rounded-lg border ${getBgColor()} bg-app-dark/95 backdrop-blur-sm shadow-lg min-w-[300px] max-w-[500px] animate-slide-in`}
    >
      {getIcon()}
      <div className="flex-1">
        <p className="text-sm text-app-text">{toast.message}</p>
      </div>
      {toast.action && (
        <button
          onClick={toast.action.onClick}
          className="text-accent hover:text-accent/80 text-sm font-medium transition-colors"
        >
          {toast.action.label}
        </button>
      )}
      <button
        onClick={onClose}
        className="ml-2 text-app-muted hover:text-app-text transition-colors"
      >
        <X className="w-4 h-4" weight="bold" />
      </button>
    </div>
  )
}