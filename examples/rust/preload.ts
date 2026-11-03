import { ipcRenderer } from '@pierre/apps'

export function greet(name: string) {
  return ipcRenderer.invoke('greet', name)
}
