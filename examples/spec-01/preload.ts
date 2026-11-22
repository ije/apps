import { contextBridge, ipcRenderer } from '@pierre/apps'

contextBridge.exposeInMainWorld('app', {
  save: (name: string, png: string) => ipcRenderer.send('save', name, png),
})
