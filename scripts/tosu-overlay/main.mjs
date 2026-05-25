import { app, BrowserWindow, ipcMain } from 'electron'
import { Cursor, Overlay, defaultDllDir, length } from '@asdf-overlay/core'
import { ElectronOverlayInput } from '@asdf-overlay/electron/input'
import { ElectronOverlaySurface } from '@asdf-overlay/electron/surface'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import readline from 'node:readline'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const args = new Map()
for (let index = 2; index < process.argv.length; index += 2) {
  args.set(process.argv[index], process.argv[index + 1])
}

const pid = Number(args.get('--pid'))
const dllDir = args.get('--dll-dir') || defaultDllDir()
const initialWidth = Number(args.get('--width')) || 1280
const initialHeight = Number(args.get('--height')) || 720

process.on('uncaughtException', (error) => {
  if (error?.code === 'EPIPE') {
    return
  }
  console.error(error)
  process.exit(1)
})

const userDataRoot = process.env.LOCALAPPDATA || os.tmpdir()
const userDataPath = path.join(userDataRoot, 'Osuwapp', 'tosu-overlay-electron')
fs.mkdirSync(userDataPath, { recursive: true })
app.setPath('userData', userDataPath)
app.commandLine.appendSwitch('force-device-scale-factor', '1')
app.commandLine.appendSwitch('high-dpi-support', '1')
if (!Number.isFinite(pid) || pid <= 0) {
  console.error('Missing --pid')
  process.exit(2)
}

let overlay = null
let overlayWindowId = null
let surface = null
let input = null
let browserWindow = null
let editorActive = false
let lastState = null

const forwardState = () => {
  if (!browserWindow || !lastState) {
    return
  }
  browserWindow.webContents.send('overlay:state', lastState)
  browserWindow.webContents.invalidate()
}

const send = (message) => {
  try {
    if (!process.stdout.destroyed && process.stdout.writable) {
      process.stdout.write(`${JSON.stringify(message)}\n`)
    }
  } catch (error) {
    if (error?.code !== 'EPIPE') {
      throw error
    }
  }
}

const setInputMode = async (active) => {
  editorActive = active

  if (!overlay || overlayWindowId === null) {
    return
  }

  await overlay.listenInput(overlayWindowId, active, active)
  await overlay.blockInput(overlayWindowId, active)
  await overlay.setBlockingCursor(overlayWindowId, active ? Cursor.Default : undefined)

  if (active && !input) {
    input = ElectronOverlayInput.connect({ overlay, id: overlayWindowId }, browserWindow.webContents)
  } else if (!active && input) {
    await input.disconnect()
    input = null
  }
}

const connectWindow = async (id, width, height, luid) => {
  const nextWidth = width > 0 ? width : initialWidth
  const nextHeight = height > 0 ? height : initialHeight
  send({ type: 'added', id, width, height, appliedWidth: nextWidth, appliedHeight: nextHeight })
  overlayWindowId = id
  browserWindow.setSize(nextWidth, nextHeight)

  surface?.disconnect().catch(() => {})
  surface = ElectronOverlaySurface.connect({ overlay, id }, luid, browserWindow.webContents)
  surface.events.on('error', (error) => {
    send({ type: 'error', message: String(error?.message || error) })
  })

  await overlay.setAnchor(id, length(0), length(0))
  await overlay.setPosition(id, length(0), length(0))
  await setInputMode(editorActive)
  if (lastState) {
    forwardState()
  } else {
    browserWindow.webContents.invalidate()
  }
}

const start = async () => {
  await app.whenReady()

  browserWindow = new BrowserWindow({
    width: initialWidth,
    height: initialHeight,
    show: false,
    frame: false,
    transparent: true,
    backgroundColor: '#00000000',
    webPreferences: {
      offscreen: true,
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false,
      preload: path.join(__dirname, 'preload.mjs'),
    },
  })

  browserWindow.webContents.setFrameRate(60)

  browserWindow.webContents.on('paint', (_event, _rect, image) => {
    const size = image.getSize()
    if (size.width > 0 && size.height > 0) {
      send({ type: 'paint', width: size.width, height: size.height })
    }
  })

  browserWindow.webContents.on('did-finish-load', () => {
    send({ type: 'loaded' })
  })

  await browserWindow.loadFile(path.join(__dirname, 'renderer.html'))

  overlay = await Overlay.attach(dllDir, pid, 4000)

  overlay.event.on('added', (id, width, height, luid) => {
    connectWindow(id, width, height, luid).catch((error) => {
      send({ type: 'error', message: String(error?.message || error) })
    })
  })
  overlay.event.on('resized', (_id, width, height) => {
    send({ type: 'resized', width, height })
    if (width > 0 && height > 0) {
      browserWindow?.setSize(width, height)
    }
    browserWindow?.webContents.send('overlay:resize', { width, height })
    if (lastState) {
      forwardState()
    }
  })
  overlay.event.on('cursor_input', (_id, event) => input?.sendCursorInput(event))
  overlay.event.on('keyboard_input', (_id, event) => input?.sendKeyboardInput(event))
  overlay.event.on('input_blocking_ended', () => {
    setInputMode(false).catch(() => {})
    browserWindow?.webContents.send('overlay:editor', false)
  })
  overlay.event.on('destroyed', () => {
    send({ type: 'destroyed' })
    overlayWindowId = null
    surface?.disconnect().catch(() => {})
    surface = null
  })
  overlay.event.on('disconnected', () => {
    send({ type: 'disconnected' })
  })
  overlay.event.on('error', (error) => {
    send({ type: 'error', message: String(error?.message || error) })
  })

  process.stdin.setEncoding('utf8')
  process.stdin.resume()
  const rl = readline.createInterface({ input: process.stdin })
  rl.on('line', (line) => {
    try {
      send({ type: 'stdin', size: line.length })
      const message = JSON.parse(line)
      if (message.type === 'state') {
        lastState = message.payload
        send({
          type: 'state',
          hasSettings: Boolean(message.payload?.settings),
          hasSnapshot: Boolean(message.payload?.snapshot),
        })
        forwardState()
      }
      if (message.type === 'editor') {
        setInputMode(Boolean(message.active)).catch((error) => {
          send({ type: 'error', message: String(error?.message || error) })
        })
        browserWindow.webContents.send('overlay:editor', Boolean(message.active))
        browserWindow.webContents.invalidate()
      }
    } catch (error) {
      send({ type: 'error', message: String(error?.message || error) })
    }
  })

  send({ type: 'ready' })
  setInterval(forwardState, 250)
}

ipcMain.on('settings:update', (_event, settings) => {
  send({ type: 'settings', settings })
})

start().catch((error) => {
  console.error(error)
  process.exit(1)
})
