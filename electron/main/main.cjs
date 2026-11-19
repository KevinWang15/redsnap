// RedSnap Settings: the Electron main process.
//
// RedSnap starts this app on demand with a private channel in the
// environment (REDSNAP_IPC_PATH, REDSNAP_IPC_TOKEN). RedSnap owns the
// settings: this app asks for them, sends edits, and shows what RedSnap
// answers. When the channel closes (RedSnap quit or crashed), this app quits.

const { app, BrowserWindow, dialog, ipcMain } = require('electron')
const net = require('node:net')
const path = require('node:path')

const ipcPath = process.env.REDSNAP_IPC_PATH
const token = process.env.REDSNAP_IPC_TOKEN
// Nothing started from here needs the token.
delete process.env.REDSNAP_IPC_TOKEN

/** The requests the window may make; anything else is refused here. */
const ALLOWED = new Set([
  'config.get',
  'config.save',
  'app.info',
  'hotkey.suspend',
  'hotkey.resume',
  'hotkey.test',
  'apps.stats',
  'apps.reload',
  'usage.reset',
  'clipboard.stats',
  'clipboard.clear',
  'logs.open',
  'data.open',
])

let socket = null
let buffer = ''
let nextId = 1
const pending = new Map()
let win = null

function call(method, params) {
  return new Promise((resolve, reject) => {
    if (!socket) return reject(new Error('not connected to RedSnap'))
    const id = nextId++
    pending.set(id, { resolve, reject })
    socket.write(JSON.stringify({ id, method, params: params ?? {} }) + '\n')
  })
}

function onLine(line) {
  let msg
  try {
    msg = JSON.parse(line)
  } catch {
    return
  }
  if (msg.id != null && pending.has(msg.id)) {
    const p = pending.get(msg.id)
    pending.delete(msg.id)
    if ('error' in msg) p.reject(new Error(msg.error))
    else p.resolve(msg.result)
    return
  }
  if (msg.event === 'focus') {
    focusWindow()
  } else if (msg.event && win) {
    win.webContents.send('redsnap:event', msg.event, msg.params)
  }
}

function connect() {
  return new Promise((resolve, reject) => {
    socket = net.connect(ipcPath)
    socket.setEncoding('utf8')
    socket.on('connect', () => resolve())
    socket.on('error', (e) => reject(e))
    socket.on('data', (chunk) => {
      buffer += chunk
      let i
      while ((i = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, i)
        buffer = buffer.slice(i + 1)
        onLine(line)
      }
    })
    // RedSnap went away: so do we.
    socket.on('close', () => app.quit())
  })
}

function focusWindow() {
  if (!win) return
  if (win.isMinimized()) win.restore()
  win.show()
  win.focus()
}

function createWindow() {
  win = new BrowserWindow({
    width: 980,
    height: 700,
    minWidth: 780,
    minHeight: 540,
    title: 'RedSnap Settings',
    // Shown once the first frame is ready, on the app's own background, so
    // it never flashes white.
    show: false,
    backgroundColor: '#0d1017',
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
    },
  })
  win.once('ready-to-show', () => {
    focusWindow()
    // For RedSnap's log: requested → shown.
    call('settings.shown', { backend: 'electron' }).catch(() => {})
  })
  // The window shows bundled files only.
  win.webContents.on('will-navigate', (e) => e.preventDefault())
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  const dev = process.env.REDSNAP_SETTINGS_DEV_URL
  if (dev) win.loadURL(dev)
  else win.loadFile(path.join(__dirname, '..', 'dist', 'index.html'))
  win.on('closed', () => (win = null))
}

if (!app.requestSingleInstanceLock()) {
  app.quit()
} else {
  app.on('second-instance', () => focusWindow())
  app.setAppUserModelId('com.kevinwang15.redsnap.settings')
  app.on('window-all-closed', () => app.quit())

  ipcMain.handle('redsnap:call', (_event, method, params) => {
    if (!ALLOWED.has(method)) throw new Error(`not allowed: ${method}`)
    return call(method, params)
  })

  app.whenReady().then(async () => {
    if (!ipcPath || !token) {
      dialog.showErrorBox('RedSnap Settings', 'Open Settings from RedSnap: its tray menu, or type "settings" in RedSnap.')
      app.quit()
      return
    }
    try {
      await connect()
      await call('hello', { token })
    } catch (e) {
      dialog.showErrorBox('RedSnap Settings', `Couldn't reach RedSnap: ${e.message}`)
      app.quit()
      return
    }
    createWindow()
  })
}
