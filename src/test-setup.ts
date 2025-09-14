import '@testing-library/jest-dom'
import { vi } from 'vitest'

// Mock Tauri API
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({
  message: vi.fn(),
}))

// Mock window object extensions that Tauri adds
Object.defineProperty(window, '__TAURI_INTERNALS__', {
  value: {},
})