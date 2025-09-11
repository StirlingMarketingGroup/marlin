export function toFileUrl(path: string): string {
  if (!path) return ''
  if (path.startsWith('file://')) return path
  // Windows path like C:\Users\... or C:/Users/...
  if (/^[A-Za-z]:[\\\/]/.test(path)) {
    const norm = path.replace(/\\/g, '/')
    return 'file:///' + encodeURI(norm)
  }
  // POSIX path
  return 'file://' + encodeURI(path)
}

export function downloadUrlDescriptor(name: string, fileUrl: string, mime?: string): string {
  const m = mime || 'application/octet-stream'
  return `${m}:${name}:${fileUrl}`
}

