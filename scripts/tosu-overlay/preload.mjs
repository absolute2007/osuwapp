import { contextBridge, ipcRenderer } from 'electron'

contextBridge.exposeInMainWorld('osuwappOverlay', {
  onState(callback) {
    ipcRenderer.on('overlay:state', (_event, payload) => callback(payload))
  },
  onEditor(callback) {
    ipcRenderer.on('overlay:editor', (_event, active) => callback(active))
  },
  onResize(callback) {
    ipcRenderer.on('overlay:resize', (_event, size) => callback(size))
  },
  updateSettings(settings) {
    ipcRenderer.send('settings:update', settings)
  },
})
