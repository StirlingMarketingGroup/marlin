declare module 'tauri-plugin-macos-permissions-api' {
  export function checkFullDiskAccessPermission(): Promise<boolean>;
  export function requestFullDiskAccessPermission(): Promise<void>;
}
