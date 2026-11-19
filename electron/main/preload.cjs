// The only bridge between the settings window and RedSnap: a request call
// (checked against an allow-list in main.cjs) and events pushed by RedSnap.

const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('redsnap', {
  call: (method, params) => ipcRenderer.invoke('redsnap:call', method, params),
  onEvent: (callback) => {
    const listener = (_event, name, params) => callback(name, params)
    ipcRenderer.on('redsnap:event', listener)
    return () => ipcRenderer.removeListener('redsnap:event', listener)
  },
  platform: process.platform,
})
