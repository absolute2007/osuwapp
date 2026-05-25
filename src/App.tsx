import {
  type CSSProperties,
  startTransition,
  useDeferredValue,
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import '@fontsource-variable/inter/index.css'
import appIconUrl from './assets/app-icon.png'
import './App.css'
import { initialSnapshot } from './mockSnapshot'
import type {
  AppSnapshot,
  OverlayElementSettings,
  OverlaySettings,
  RecentPlaySnapshot,
  SessionSnapshot,
} from './types'

const SNAPSHOT_EVENT = 'session-updated'
const OVERLAY_SETTINGS_EVENT = 'overlay-settings-updated'
const OPEN_OVERLAY_SETTINGS_EVENT = 'open-overlay-settings'
const MAX_GRAPH_POINTS = 96
const integerFormatter = new Intl.NumberFormat('en-US')
const RECENT_PLAY_LIMIT = 30
type OverlayPanelKey = keyof Pick<OverlaySettings, 'ppPanel' | 'statsPanel' | 'hitsPanel' | 'mapPanel'>

const compactOverlayPanels = {
  ppPanel: { enabled: true, showBackground: true, x: 0, y: 0, width: 106, height: 34, scale: 1, fontScale: 1 },
  statsPanel: { enabled: true, showBackground: true, x: 112, y: 0, width: 168, height: 34, scale: 1, fontScale: 1 },
  hitsPanel: { enabled: true, showBackground: true, x: 0, y: 38, width: 280, height: 24, scale: 1, fontScale: 1 },
  mapPanel: { enabled: false, showBackground: true, x: 0, y: 66, width: 360, height: 24, scale: 1, fontScale: 1 },
} satisfies Pick<OverlaySettings, 'ppPanel' | 'statsPanel' | 'hitsPanel' | 'mapPanel'>

const tournamentOverlayPanels = {
  ppPanel: { enabled: true, showBackground: true, x: 0, y: 0, width: 150, height: 42, scale: 1.06, fontScale: 1.05 },
  statsPanel: { enabled: true, showBackground: true, x: 158, y: 0, width: 238, height: 42, scale: 1, fontScale: 1 },
  hitsPanel: { enabled: true, showBackground: true, x: 0, y: 48, width: 396, height: 28, scale: 1, fontScale: 1 },
  mapPanel: { enabled: true, showBackground: true, x: 0, y: 82, width: 396, height: 24, scale: 1, fontScale: 1 },
} satisfies Pick<OverlaySettings, 'ppPanel' | 'statsPanel' | 'hitsPanel' | 'mapPanel'>

const minimalOverlayPanels = {
  ppPanel: { enabled: true, showBackground: true, x: 0, y: 0, width: 104, height: 30, scale: 1, fontScale: 0.94 },
  statsPanel: { enabled: true, showBackground: true, x: 110, y: 0, width: 156, height: 30, scale: 1, fontScale: 0.92 },
  hitsPanel: { enabled: true, showBackground: true, x: 0, y: 34, width: 266, height: 22, scale: 1, fontScale: 0.9 },
  mapPanel: { enabled: false, showBackground: true, x: 0, y: 60, width: 266, height: 22, scale: 1, fontScale: 0.9 },
} satisfies Pick<OverlaySettings, 'ppPanel' | 'statsPanel' | 'hitsPanel' | 'mapPanel'>

const DEFAULT_OVERLAY_SETTINGS: OverlaySettings = {
  enabled: true,
  showPp: true,
  showIfFc: true,
  showAccuracy: true,
  showCombo: true,
  showMods: true,
  showMap: false,
  showHits: true,
  width: 280,
  height: 62,
  offsetX: 24,
  offsetY: 24,
  scale: 1,
  fontScale: 1,
  padding: 0,
  cornerRadius: 10,
  opacity: 0.9,
  showBackground: true,
  toggleKey: 'Insert',
  editorPanelWidth: 760,
  editorPanelHeight: 520,
  dataUpdateIntervalMs: 90,
  ...compactOverlayPanels,
}

type AppView = 'session' | 'recent' | 'overlay' | 'settings' | 'about'

type PerformanceSample = {
  progress: number
  passedObjects: number
  ppCurrent: number
  ppIfFc: number
  accuracy: number | null
  combo: number
  score: number
  misses: number
  sliderBreaks: number
  hp: number | null
}

const PRIMARY_NAV = [
  { id: 'session', label: 'Session', icon: SessionIcon },
  { id: 'recent', label: 'Recent Plays', icon: HistoryIcon },
  { id: 'overlay', label: 'Overlay', icon: OverlayIcon },
] as const

const SECONDARY_NAV = [
  { id: 'settings', label: 'Settings', icon: SettingsIcon },
  { id: 'about', label: 'About', icon: AboutIcon },
] as const

const isTauriRuntime = () =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const isOverlayRoute = () =>
  typeof window !== 'undefined' &&
  new URL(window.location.href).searchParams.get('overlay') === '1'

const isOverlayEditorRoute = () =>
  typeof window !== 'undefined' &&
  new URL(window.location.href).searchParams.get('overlayEditor') === '1'

const formatPp = (value: number) => `${value.toFixed(2)} PP`

const formatPlainPp = (value: number) => value.toFixed(2)
const formatCount = (value: number) => integerFormatter.format(Math.max(0, value))

const formatAccuracy = (value: number | null) =>
  value === null ? '—' : `${value.toFixed(2)}%`

const formatDelta = (value: number | null) =>
  value === null ? '—' : `±${Math.abs(value).toFixed(2)}%`

const formatNumber = (value: number) => integerFormatter.format(Math.max(0, Math.round(value)))

const formatLength = (milliseconds: number) => {
  const safeValue = Math.max(0, milliseconds)
  const totalSeconds = Math.floor(safeValue / 1000)
  const minutes = Math.floor(totalSeconds / 60)
  const remainder = totalSeconds % 60

  return `${minutes}:${String(remainder).padStart(2, '0')}`
}

const clampPercent = (value: number) => `${Math.max(0, Math.min(100, value)).toFixed(2)}%`

const hotkeyFromKeyboardEvent = (event: KeyboardEvent | ReactKeyboardEvent<HTMLElement>) => {
  if (event.key === 'Escape') {
    return null
  }

  if (['Control', 'Shift', 'Alt', 'Meta'].includes(event.key)) {
    return null
  }

  if (event.key === ' ') {
    return 'Space'
  }

  if (event.key === 'PageUp') {
    return 'PageUp'
  }

  if (event.key === 'PageDown') {
    return 'PageDown'
  }

  if (event.key === 'ArrowLeft') {
    return 'Left'
  }

  if (event.key === 'ArrowRight') {
    return 'Right'
  }

  if (event.key === 'ArrowUp') {
    return 'Up'
  }

  if (event.key === 'ArrowDown') {
    return 'Down'
  }

  if (event.key.length === 1) {
    return event.key.toUpperCase()
  }

  return event.key
}

const sampleSession: SessionSnapshot = {
  phase: 'playing',
  beatmap: {
    artist: 'Camellia',
    title: "Exit This Earth's Atomosphere",
    difficultyName: 'Expert+',
    creator: 'Realazy',
    status: 'Ranked',
    mode: 'osu!',
    path: 'preview.osu',
    coverPath: null,
    lengthMs: 258000,
    objectCount: 1428,
    starRating: 6.82,
    ar: 9.6,
    od: 9.1,
    cs: 4,
    hp: 6.5,
    bpm: 182,
    mods: ['HD', 'HR'],
  },
  live: {
    username: 'player',
    gameState: 'Playing',
    accuracy: 98.43,
    combo: 891,
    maxCombo: 1048,
    score: 8445321,
    misses: 1,
    retries: 2,
    hp: 0.72,
    progress: 0.64,
    passedObjects: 914,
    modsText: 'HDHR',
    hits: {
      nGeki: 0,
      nKatu: 0,
      n300: 642,
      n100: 18,
      n50: 2,
      misses: 1,
      sliderBreaks: 1,
    },
  },
  pp: {
    current: 436.72,
    ifFc: 512.44,
    fullMap: 548.18,
    calculator: 'rosu-pp 4.0.1',
    difficultyAdjust: 1.08,
    modsMultiplier: 1.12,
    components: [
      { label: 'Aim', value: 214.2 },
      { label: 'Speed', value: 151.8 },
      { label: 'Accuracy', value: 70.72 },
    ],
  },
}

const formatRelativeTime = (timestampMs: number) => {
  const diffMinutes = Math.max(0, Math.round((Date.now() - timestampMs) / 60000))

  if (diffMinutes < 1) {
    return 'now'
  }

  if (diffMinutes < 60) {
    return `${diffMinutes}m ago`
  }

  const hours = Math.floor(diffMinutes / 60)

  if (hours < 24) {
    return `${hours}h ago`
  }

  const days = Math.floor(hours / 24)
  return `${days}d ago`
}

const accuracyTone = (accuracy: number | null) => {
  if (accuracy === null) {
    return 'neutral'
  }

  if (accuracy >= 98) {
    return 'good'
  }

  if (accuracy >= 95) {
    return 'warn'
  }

  return 'danger'
}

const visibleOverlayElements = (settings: OverlaySettings) => {
  const elements: Array<{ id: string; visible: boolean; settings: OverlayElementSettings }> = [
    { id: 'pp', visible: settings.showPp && settings.ppPanel.enabled, settings: settings.ppPanel },
    {
      id: 'stats',
      visible:
        settings.statsPanel.enabled &&
        (settings.showIfFc || settings.showAccuracy || settings.showCombo || settings.showMods),
      settings: settings.statsPanel,
    },
    { id: 'hits', visible: settings.showHits && settings.hitsPanel.enabled, settings: settings.hitsPanel },
    { id: 'map', visible: settings.showMap && settings.mapPanel.enabled, settings: settings.mapPanel },
  ]

  return elements.filter((element) => element.visible)
}

const overlayPreviewBounds = (settings: OverlaySettings) => {
  const elements = visibleOverlayElements(settings)

  if (elements.length === 0) {
    return { left: 0, top: 0, width: Math.max(settings.width, 1), height: Math.max(settings.height, 1) }
  }

  const left = Math.min(0, ...elements.map((element) => element.settings.x))
  const top = Math.min(0, ...elements.map((element) => element.settings.y))
  const right = Math.max(settings.width, ...elements.map((element) => element.settings.x + element.settings.width))
  const bottom = Math.max(settings.height, ...elements.map((element) => element.settings.y + element.settings.height))

  return {
    left,
    top,
    width: Math.max(1, right - left),
    height: Math.max(1, bottom - top),
  }
}

const graphPath = (values: number[], width: number, height: number) => {
  if (values.length === 0) {
    return ''
  }

  const max = Math.max(...values)
  const min = Math.min(...values)
  const range = max - min || 1

  return values
    .map((value, index) => {
      const x = (index / Math.max(values.length - 1, 1)) * width
      const y = height - ((value - min) / range) * height
      return `${index === 0 ? 'M' : 'L'} ${x.toFixed(2)} ${y.toFixed(2)}`
    })
    .join(' ')
}

const pageTitleForView = (view: AppView) => {
  switch (view) {
    case 'session':
      return 'Session'
    case 'recent':
      return 'Recent Plays'
    case 'overlay':
      return 'Overlay'
    case 'settings':
      return 'Settings'
    case 'about':
      return 'About'
  }
}

const startTauriWindowDrag = async (event: ReactPointerEvent<HTMLElement>) => {
  if (!isTauriRuntime() || event.button !== 0) {
    return
  }

  const target = event.target

  if (
    target instanceof HTMLElement &&
    target.closest('button, a, input, textarea, select, [data-no-drag="true"]')
  ) {
    return
  }

  event.preventDefault()

  try {
    await getCurrentWindow().startDragging()
  } catch (error) {
    console.error('Failed to start window drag', error)
  }
}

function App() {
  const overlayMode = isOverlayRoute()
  const overlayEditorMode = isOverlayEditorRoute()
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot)
  const [overlaySettings, setOverlaySettings] = useState<OverlaySettings>(DEFAULT_OVERLAY_SETTINGS)
  const [activeView, setActiveView] = useState<AppView>('session')
  const [mapGraph, setMapGraph] = useState<number[]>([])
  const [mapTimeline, setMapTimeline] = useState<PerformanceSample[]>([])
  const [sessionGraph, setSessionGraph] = useState<number[]>([])
  const [alwaysOnTop, setAlwaysOnTop] = useState(false)
  const [isMaximized, setIsMaximized] = useState(false)
  const [coverImage, setCoverImage] = useState<{ path: string; src: string } | null>(null)
  const currentMapKeyRef = useRef<string | null>(null)
  const mapGraphRef = useRef<number[]>([])
  const mapTimelineRef = useRef<PerformanceSample[]>([])
  const sessionGraphRef = useRef<number[]>([])
  const viewModel = useDeferredValue(snapshot)

  const applySnapshot = useEffectEvent((nextSnapshot: AppSnapshot) => {
    const session = nextSnapshot.session

    if (!session || session.phase === 'preview') {
      currentMapKeyRef.current = null
      mapGraphRef.current = []
      mapTimelineRef.current = []
      sessionGraphRef.current = []
      setMapGraph([])
      setMapTimeline([])
      setSessionGraph([])
      setSnapshot(nextSnapshot)
      return
    }

    const mapKey = `${session.beatmap.path}:${session.live.modsText}`
    const previousMapKey = currentMapKeyRef.current
    const currentPp = Number(session.pp.current.toFixed(2))

    const nextMapGraph = (() => {
      const base = previousMapKey === mapKey ? mapGraphRef.current : []
      const last = base.at(-1)

      currentMapKeyRef.current = mapKey

      if (last === currentPp) {
        return base
      }

      return [...base, currentPp].slice(-MAX_GRAPH_POINTS)
    })()

    const nextSessionGraph = (() => {
      const last = sessionGraphRef.current.at(-1)

      if (last === currentPp) {
        return sessionGraphRef.current
      }

      return [...sessionGraphRef.current, currentPp].slice(-MAX_GRAPH_POINTS)
    })()

    const nextMapTimeline = (() => {
      const base = previousMapKey === mapKey ? mapTimelineRef.current : []
      const progress = session.phase === 'result' ? 1 : session.live.progress
      const nextSample: PerformanceSample = {
        progress,
        passedObjects: session.live.passedObjects,
        ppCurrent: session.pp.current,
        ppIfFc: session.pp.ifFc,
        accuracy: session.live.accuracy,
        combo: session.live.combo,
        score: session.live.score,
        misses: session.live.hits.misses,
        sliderBreaks: session.live.hits.sliderBreaks,
        hp: session.live.hp,
      }
      const last = base.at(-1)

      if (
        last &&
        last.passedObjects === nextSample.passedObjects &&
        last.misses === nextSample.misses &&
        last.sliderBreaks === nextSample.sliderBreaks &&
        Math.abs(last.progress - nextSample.progress) < 0.001
      ) {
        return base
      }

      return [...base, nextSample].slice(-MAX_GRAPH_POINTS)
    })()

    mapGraphRef.current = nextMapGraph
    mapTimelineRef.current = nextMapTimeline
    sessionGraphRef.current = nextSessionGraph
    setMapGraph(nextMapGraph)
    setMapTimeline(nextMapTimeline)
    setSessionGraph(nextSessionGraph)
    setSnapshot(nextSnapshot)
  })

  useEffect(() => {
    document.body.dataset.overlayMode = overlayMode ? 'true' : 'false'
    document.documentElement.dataset.overlayMode = overlayMode ? 'true' : 'false'
    document.body.dataset.overlayEditorMode = overlayEditorMode ? 'true' : 'false'
    document.documentElement.dataset.overlayEditorMode = overlayEditorMode ? 'true' : 'false'

    return () => {
      delete document.body.dataset.overlayMode
      delete document.documentElement.dataset.overlayMode
      delete document.body.dataset.overlayEditorMode
      delete document.documentElement.dataset.overlayEditorMode
    }
  }, [overlayEditorMode, overlayMode])

  useEffect(() => {
    const preventContextMenu = (event: globalThis.MouseEvent) => {
      event.preventDefault()
    }

    window.addEventListener('contextmenu', preventContextMenu)

    return () => {
      window.removeEventListener('contextmenu', preventContextMenu)
    }
  }, [])

  useEffect(() => {
    if (!isTauriRuntime()) {
      return
    }

    let mounted = true
    let cleanupSnapshot: (() => void) | undefined
    let cleanupOverlaySettings: (() => void) | undefined
    let cleanupOpenOverlaySettings: (() => void) | undefined

    listen<AppSnapshot>(SNAPSHOT_EVENT, (event) => {
      if (!mounted) {
        return
      }

      startTransition(() => {
        applySnapshot(event.payload)
      })
    })
      .then((unlisten) => {
        if (!mounted) {
          unlisten()
          return
        }

        cleanupSnapshot = unlisten
      })
      .catch((error) => {
        console.error('Failed to subscribe to live updates', error)
      })

    listen<OverlaySettings>(OVERLAY_SETTINGS_EVENT, (event) => {
      if (!mounted) {
        return
      }

      setOverlaySettings(event.payload)
    })
      .then((unlisten) => {
        if (!mounted) {
          unlisten()
          return
        }

        cleanupOverlaySettings = unlisten
      })
      .catch((error) => {
        console.error('Failed to subscribe to overlay settings', error)
      })

    listen(OPEN_OVERLAY_SETTINGS_EVENT, () => {
      if (!mounted) {
        return
      }

      setActiveView('overlay')
    })
      .then((unlisten) => {
        if (!mounted) {
          unlisten()
          return
        }

        cleanupOpenOverlaySettings = unlisten
      })
      .catch((error) => {
        console.error('Failed to subscribe to overlay settings opener', error)
      })

    invoke<AppSnapshot>('get_initial_snapshot')
      .then((initial) => {
        if (!mounted) {
          return
        }

        startTransition(() => {
          applySnapshot(initial)
        })
      })
      .catch((error) => {
        console.error('Failed to load initial snapshot', error)
      })

    invoke('start_live_updates').catch((error) => {
      console.error('Failed to start live reader', error)
    })

    invoke<OverlaySettings>('get_overlay_settings')
      .then((settings) => {
        if (mounted) {
          setOverlaySettings(settings)
        }
      })
      .catch((error) => {
        console.error('Failed to load overlay settings', error)
      })

    return () => {
      mounted = false
      cleanupSnapshot?.()
      cleanupOverlaySettings?.()
      cleanupOpenOverlaySettings?.()
    }
  }, [])

  useEffect(() => {
    if (!isTauriRuntime()) {
      return
    }

    const currentWindow = getCurrentWindow()
    let mounted = true
    let unlistenResize: (() => void) | undefined

    const syncWindowState = async () => {
      try {
        const [maximized, pinned] = await Promise.all([
          currentWindow.isMaximized(),
          currentWindow.isAlwaysOnTop(),
        ])

        if (!mounted) {
          return
        }

        setIsMaximized(maximized)
        setAlwaysOnTop(pinned)
      } catch (error) {
        console.error('Failed to sync window state', error)
      }
    }

    void syncWindowState()

    currentWindow
      .onResized(() => {
        void syncWindowState()
      })
      .then((unlisten) => {
        if (!mounted) {
          unlisten()
          return
        }

        unlistenResize = unlisten
      })
      .catch((error) => {
        console.error('Failed to subscribe to window resize', error)
      })

    return () => {
      mounted = false
      unlistenResize?.()
    }
  }, [overlayMode])

  const session = viewModel.session
  const coverPath = session?.beatmap.coverPath ?? null
  const coverSrc = coverImage?.path === coverPath ? coverImage.src : null

  useEffect(() => {
    if (!coverPath || !isTauriRuntime()) {
      return
    }

    let cancelled = false

    invoke<string>('load_image_data_uri', { path: coverPath })
      .then((dataUri) => {
        if (!cancelled) {
          setCoverImage({ path: coverPath, src: dataUri })
        }
      })
      .catch(() => {
        if (!cancelled) {
          setCoverImage(null)
        }
      })

    return () => {
      cancelled = true
    }
  }, [coverPath])

  const sidebarStatusTitle =
    viewModel.connection.status === 'connected'
      ? 'osu! connected'
      : viewModel.connection.status === 'error'
        ? 'connection error'
        : 'waiting for osu!'

  const currentViewTitle = pageTitleForView(activeView)

  const persistOverlaySettings = async (nextSettings: OverlaySettings) => {
    if (!isTauriRuntime()) {
      setOverlaySettings(nextSettings)
      return
    }

    try {
      const saved = await invoke<OverlaySettings>('save_overlay_settings', {
        settings: nextSettings,
      })
      setOverlaySettings(saved)
    } catch (error) {
      console.error('Failed to save overlay settings', error)
    }
  }

  const handleWindowAction = async (
    action: 'minimize' | 'toggleMaximize' | 'close' | 'toggleAlwaysOnTop',
  ) => {
    if (!isTauriRuntime()) {
      return
    }

    const currentWindow = getCurrentWindow()

      try {
        if (action === 'minimize') {
          await invoke('hide_main_window')
          return
        }

      if (action === 'close') {
        await invoke('quit_application')
        return
      }

      if (action === 'toggleAlwaysOnTop') {
        const nextValue = !alwaysOnTop
        await currentWindow.setAlwaysOnTop(nextValue)
        setAlwaysOnTop(nextValue)
        return
      }

      if (isMaximized) {
        await currentWindow.unmaximize()
        setIsMaximized(false)
      } else {
        await currentWindow.maximize()
        setIsMaximized(true)
      }
    } catch (error) {
      console.error(`Failed to perform window action: ${action}`, error)
    }
  }

  if (overlayMode) {
    return <OverlayHudWindowPage session={session} settings={overlaySettings} />
  }

  if (overlayEditorMode) {
    return (
      <OverlayEditorWindowPage
        settings={overlaySettings}
        onUpdateSettings={(nextSettings) => {
          void persistOverlaySettings(nextSettings)
        }}
      />
    )
  }

  return (
    <div className="window-shell window-shell--intro">
      <header className="titlebar">
        <div
          className="titlebar__drag"
          onPointerDown={(event) => {
            void startTauriWindowDrag(event)
          }}
        >
          <div className="titlebar__brand">
            <div className="titlebar__badge" aria-hidden="true">
              <AppIcon />
            </div>
            <div className="titlebar__brand-copy">
              <strong>osu! Companion</strong>
              <span>{session?.live.gameState ?? viewModel.connection.detail}</span>
            </div>
          </div>

          <div className="titlebar__center">
            <span className="titlebar__view">{currentViewTitle}</span>
          </div>
        </div>

        <div className="titlebar__controls">
          <button
            aria-label="Minimize"
            className="window-button"
            type="button"
            onClick={() => {
              void handleWindowAction('minimize')
            }}
          >
            <MinimizeIcon />
          </button>
          <button
            aria-label={isMaximized ? 'Restore' : 'Maximize'}
            className="window-button"
            type="button"
            onClick={() => {
              void handleWindowAction('toggleMaximize')
            }}
          >
            {isMaximized ? <RestoreIcon /> : <MaximizeIcon />}
          </button>
          <button
            aria-label="Close"
            className="window-button window-button--close"
            type="button"
            onClick={() => {
              void handleWindowAction('close')
            }}
          >
            <CloseIcon />
          </button>
        </div>
      </header>

      <div className="workspace-shell">
        <aside className="sidebar">
          <nav className="sidebar__nav" aria-label="Primary">
            {PRIMARY_NAV.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                className={`sidebar__item ${activeView === id ? 'sidebar__item--active' : ''}`}
                type="button"
                onClick={() => {
                  setActiveView(id)
                }}
              >
                <span className="sidebar__icon">
                  <Icon />
                </span>
                <span>{label}</span>
              </button>
            ))}
          </nav>

          <div className="sidebar__footer-nav">
            {SECONDARY_NAV.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                className={`sidebar__item ${activeView === id ? 'sidebar__item--active' : ''}`}
                type="button"
                onClick={() => {
                  setActiveView(id)
                }}
              >
                <span className="sidebar__icon">
                  <Icon />
                </span>
                <span>{label}</span>
              </button>
            ))}
          </div>

          <div className="sidebar__status-card">
            <div className={`status-dot status-dot--${viewModel.connection.status}`} />
            <div>
              <div className="sidebar__status-title">{sidebarStatusTitle}</div>
              <div className="sidebar__status-subtitle">
                {session?.live.username ?? viewModel.connection.detail}
              </div>
            </div>
          </div>
        </aside>

        <main className="workspace-main">
          {activeView === 'session' ? (
            <SessionView
              connection={viewModel.connection}
              coverSrc={coverSrc}
              mapGraph={mapGraph}
              mapTimeline={mapTimeline}
              recentPlays={viewModel.recentPlays}
              session={session}
              sessionGraph={sessionGraph}
              onOpenHistory={() => {
                setActiveView('recent')
              }}
            />
          ) : null}

          {activeView === 'recent' ? (
            <RecentHistoryView recentPlays={viewModel.recentPlays} />
          ) : null}

          {activeView === 'overlay' ? (
            <OverlayView
              settings={overlaySettings}
              onUpdateSettings={(nextSettings) => {
                void persistOverlaySettings(nextSettings)
              }}
            />
          ) : null}

          {activeView === 'settings' ? (
            <SettingsView
              calculator={session?.pp.calculator ?? 'rosu-pp 4.0.1 · osu!stable scoring'}
              recentPlayCount={viewModel.recentPlays.length}
            />
          ) : null}

          {activeView === 'about' ? <AboutView /> : null}
        </main>
      </div>

      <footer className="statusbar">
          <span className={`statusbar__badge statusbar__badge--${viewModel.connection.status}`}>
          {viewModel.connection.status}
        </span>
        <span>{session?.live.gameState ?? 'Idle'}</span>
        <span>{viewModel.recentPlays.length} / {RECENT_PLAY_LIMIT} saved plays</span>
        <span>{session?.pp.calculator ?? 'rosu-pp 4.0.1'}</span>
      </footer>

      <div className="app-intro" aria-hidden="true">
        <div className="app-intro__mark">
          <AppIcon />
        </div>
      </div>
    </div>
  )
}

function OverlayHudWindowPage({
  session,
  settings,
}: {
  session: SessionSnapshot | null
  settings: OverlaySettings
}) {
  const overlayCardClassName = 'overlay-card overlay-card--hud'

  return (
    <div
      className="overlay-window"
      style={
        {
          '--overlay-width': `${settings.width}px`,
          '--overlay-height': `${settings.height}px`,
          '--overlay-scale': settings.scale.toString(),
          '--overlay-font-scale': settings.fontScale.toString(),
          '--overlay-padding': `${settings.padding}px`,
          '--overlay-radius': `${settings.cornerRadius}px`,
          '--overlay-opacity': settings.opacity.toString(),
        } as CSSProperties
      }
    >
      <div className="overlay-frame">
        <OverlayHudCard className={overlayCardClassName} session={session} settings={settings} />
      </div>
    </div>
  )
}

function OverlayHudCard({
  className,
  session,
  settings,
}: {
  className: string
  session: SessionSnapshot | null
  settings: OverlaySettings
}) {
  const shellRef = useRef<HTMLDivElement | null>(null)
  const contentRef = useRef<HTMLDivElement | null>(null)
  const [autoScale, setAutoScale] = useState(1)

  useLayoutEffect(() => {
    if (!session) {
      return
    }

    const shell = shellRef.current
    const content = contentRef.current

    if (!shell || !content) {
      return
    }

    let frame = 0

    const syncScale = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        const availableWidth = shell.clientWidth
        const availableHeight = shell.clientHeight
        const currentScale = settings.scale * autoScale || 1
        const naturalWidth = content.scrollWidth * currentScale
        const naturalHeight = content.scrollHeight * currentScale

        if (availableWidth <= 0 || availableHeight <= 0) {
          return
        }

        const nextAutoScale = Math.min(
          1,
          availableWidth / Math.max(naturalWidth, 1),
          availableHeight / Math.max(naturalHeight, 1),
        )
        const clampedScale = Math.max(0.42, Number(nextAutoScale.toFixed(3)))

        setAutoScale((current) => (Math.abs(current - clampedScale) > 0.01 ? clampedScale : current))
      })
    }

    syncScale()

    const resizeObserver = new ResizeObserver(syncScale)
    resizeObserver.observe(shell)
    resizeObserver.observe(content)

    return () => {
      cancelAnimationFrame(frame)
      resizeObserver.disconnect()
    }
  }, [autoScale, session, settings.scale])

  const effectiveScale = settings.scale * autoScale

  return (
    <div className={`${className} ${settings.showBackground ? '' : 'overlay-card--bare'}`}>
      {session ? (
        <div
          className="overlay-card__scale-shell"
          ref={shellRef}
          style={
            {
              '--overlay-effective-scale': effectiveScale.toString(),
            } as CSSProperties
          }
        >
          <div className="overlay-card__scale-content" ref={contentRef}>
            <div className="overlay-card__content overlay-card__content--hud">
              {settings.showPp ? (
                <div className="overlay-card__hero overlay-card__hero--hud">
                  <strong>{session.pp.current.toFixed(2)}</strong>
                  <span>PP</span>
                </div>
              ) : null}

              <div className="overlay-card__stats overlay-card__stats--hud">
                {settings.showIfFc ? (
                  <div className="overlay-metric">
                    <span>IF FC</span>
                    <strong>{session.pp.ifFc.toFixed(2)}</strong>
                  </div>
                ) : null}
                {settings.showAccuracy ? (
                  <div className="overlay-metric">
                    <span>ACC</span>
                    <strong>{formatAccuracy(session.live.accuracy)}</strong>
                  </div>
                ) : null}
                {settings.showCombo ? (
                  <div className="overlay-metric">
                    <span>COMBO</span>
                    <strong>{session.live.combo}x</strong>
                  </div>
                ) : null}
                {settings.showMods ? (
                  <div className="overlay-metric">
                    <span>MODS</span>
                    <strong>{session.live.modsText}</strong>
                  </div>
                ) : null}
              </div>

              {settings.showHits ? (
                <div className="overlay-hit-grid">
                  <div className="overlay-hit overlay-hit--blue">
                    <span>100</span>
                    <strong>{formatCount(session.live.hits.n100)}</strong>
                  </div>
                  <div className="overlay-hit overlay-hit--orange">
                    <span>50</span>
                    <strong>{formatCount(session.live.hits.n50)}</strong>
                  </div>
                  <div className="overlay-hit overlay-hit--red">
                    <span>MISS</span>
                    <strong>{formatCount(session.live.hits.misses)}</strong>
                  </div>
                  <div className="overlay-hit overlay-hit--amber">
                    <span>SB</span>
                    <strong>{formatCount(session.live.hits.sliderBreaks)}</strong>
                  </div>
                </div>
              ) : null}

              {settings.showMap ? (
                <div className="overlay-card__map overlay-card__map--hud">
                  {session.beatmap.artist} - {session.beatmap.title} [{session.beatmap.difficultyName}]
                </div>
              ) : null}
            </div>
          </div>
        </div>
      ) : (
        <div className="overlay-card__empty overlay-card__empty--hud">
          Waiting for osu! beatmap data.
        </div>
      )}
    </div>
  )
}

function OverlayEditorWindowPage({
  settings,
  onUpdateSettings,
}: {
  settings: OverlaySettings
  onUpdateSettings: (settings: OverlaySettings) => void
}) {
  const [draft, setDraft] = useState(settings)
  const latestDraftRef = useRef(settings)
  const [selectedElement, setSelectedElement] = useState<OverlayPanelKey>('ppPanel')
  const dragRef = useRef<{
    key: OverlayPanelKey
    mode: 'move' | 'resize'
    pointerX: number
    pointerY: number
    startX: number
    startY: number
    startWidth: number
    startHeight: number
  } | null>(null)

  useEffect(() => {
    if (dragRef.current) {
      return
    }

    latestDraftRef.current = settings
    setDraft(settings)
  }, [settings])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        void getCurrentWindow().close()
      }

      if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
        onUpdateSettings(draft)
      }
    }

    window.addEventListener('keydown', handleKeyDown)

    return () => {
      window.removeEventListener('keydown', handleKeyDown)
    }
  }, [draft, onUpdateSettings])

  const updateDraft = (nextSettings: OverlaySettings) => {
    latestDraftRef.current = nextSettings
    setDraft(nextSettings)
  }

  const commitDraft = (nextSettings: OverlaySettings) => {
    latestDraftRef.current = nextSettings
    setDraft(nextSettings)
    onUpdateSettings(nextSettings)
  }

  const applyPreset = (preset: Pick<OverlaySettings, 'width' | 'height'> & Partial<OverlaySettings>) => {
    const nextSettings = {
      ...draft,
      ...preset,
    }
    commitDraft(nextSettings)
  }

  const updateElementPosition = (
    key: OverlayPanelKey,
    x: number,
    y: number,
    commit = false,
  ) => {
    const currentDraft = latestDraftRef.current
    const element = currentDraft[key]
    const nextSettings = {
      ...currentDraft,
      [key]: {
        ...element,
        x: Math.round(x),
        y: Math.round(y),
      },
    }

    if (commit) {
      commitDraft(nextSettings)
      return
    }

    updateDraft(nextSettings)
  }

  const updateElementSize = (
    key: OverlayPanelKey,
    width: number,
    height: number,
  ) => {
    const currentDraft = latestDraftRef.current
    const element = currentDraft[key]
    updateDraft({
      ...currentDraft,
      [key]: {
        ...element,
        width: Math.max(40, Math.round(width)),
        height: Math.max(24, Math.round(height)),
      },
    })
  }

  const beginDrag = (
    key: OverlayPanelKey,
    event: ReactPointerEvent<HTMLElement>,
    mode: 'move' | 'resize' = 'move',
  ) => {
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    setSelectedElement(key)
    dragRef.current = {
      key,
      mode,
      pointerX: event.clientX,
      pointerY: event.clientY,
      startX: draft[key].x,
      startY: draft[key].y,
      startWidth: draft[key].width,
      startHeight: draft[key].height,
    }
  }

  const handleDragMove = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current

    if (!drag) {
      return
    }

    const deltaX = event.clientX - drag.pointerX
    const deltaY = event.clientY - drag.pointerY

    if (drag.mode === 'resize') {
      updateElementSize(drag.key, drag.startWidth + deltaX, drag.startHeight + deltaY)
      return
    }

    updateElementPosition(drag.key, drag.startX + deltaX, drag.startY + deltaY)
  }

  const endDrag = () => {
    dragRef.current = null
    onUpdateSettings(latestDraftRef.current)
  }

  const isPanelVisible = (settingsValue: OverlaySettings, key: OverlayPanelKey) => {
    if (key === 'ppPanel') {
      return settingsValue.showPp && settingsValue.ppPanel.enabled
    }

    if (key === 'statsPanel') {
      return (
        settingsValue.statsPanel.enabled &&
        (settingsValue.showIfFc || settingsValue.showAccuracy || settingsValue.showCombo || settingsValue.showMods)
      )
    }

    if (key === 'hitsPanel') {
      return settingsValue.showHits && settingsValue.hitsPanel.enabled
    }

    return settingsValue.showMap && settingsValue.mapPanel.enabled
  }

  const setPanelVisible = (key: OverlayPanelKey, visible: boolean) => {
    const currentDraft = latestDraftRef.current
    const nextSettings: OverlaySettings = { ...currentDraft, [key]: { ...currentDraft[key], enabled: visible } }

    if (key === 'ppPanel') {
      nextSettings.showPp = visible
    } else if (key === 'statsPanel') {
      if (visible && !nextSettings.showIfFc && !nextSettings.showAccuracy && !nextSettings.showCombo && !nextSettings.showMods) {
        nextSettings.showIfFc = true
        nextSettings.showAccuracy = true
      }
    } else if (key === 'hitsPanel') {
      nextSettings.showHits = visible
    } else {
      nextSettings.showMap = visible
    }

    commitDraft(nextSettings)
  }

  const updatePanelSettings = (key: OverlayPanelKey, element: OverlayElementSettings) => {
    const currentDraft = latestDraftRef.current
    commitDraft({
      ...currentDraft,
      [key]: element,
    })
  }

  const updateStatsMetric = (
    key: 'showIfFc' | 'showAccuracy' | 'showCombo' | 'showMods',
    value: boolean,
  ) => {
    const currentDraft = latestDraftRef.current
    commitDraft({
      ...currentDraft,
      statsPanel: {
        ...currentDraft.statsPanel,
        enabled: true,
      },
      [key]: value,
    })
  }

  const elementButtons: Array<{
    key: OverlayPanelKey
    label: string
  }> = [
    { key: 'ppPanel', label: 'PP' },
    { key: 'statsPanel', label: 'Stats' },
    { key: 'hitsPanel', label: 'Hits' },
    { key: 'mapPanel', label: 'Map' },
  ]
  const selectedPanel = draft[selectedElement]
  const selectedVisible = isPanelVisible(draft, selectedElement)

  return (
    <main className="overlay-editor-window">
      <div className="overlay-editor-topbar">
        <div>
          <strong>Overlay Editor</strong>
          <span>Drag panels over the game. Ctrl+Enter saves, Esc closes.</span>
        </div>
        <div className="overlay-editor-actions">
          <button type="button" onClick={() => applyPreset({ width: 280, height: 62, showMap: false, ...compactOverlayPanels })}>
            Compact
          </button>
          <button type="button" onClick={() => applyPreset({ width: 396, height: 106, showMap: true, ...tournamentOverlayPanels })}>
            Tournament
          </button>
          <button type="button" onClick={() => applyPreset({ width: 266, height: 56, showMap: false, showBackground: false, ...minimalOverlayPanels })}>
            Minimal
          </button>
          <button
            className="overlay-editor-actions__primary"
            type="button"
            onClick={() => {
              onUpdateSettings(draft)
              void getCurrentWindow().close()
            }}
          >
            Done
          </button>
        </div>
      </div>

      <section
        className="overlay-editor-canvas"
        onPointerMove={handleDragMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
      >
        <EditableOverlayElement
          active={selectedElement === 'ppPanel'}
          elementKey="ppPanel"
          label={`${sampleSession.pp.current.toFixed(2)} PP`}
          settings={draft}
          visible={isPanelVisible(draft, 'ppPanel')}
          onPointerDown={beginDrag}
        />
        <EditableOverlayElement
          active={selectedElement === 'statsPanel'}
          elementKey="statsPanel"
          label={`IF FC ${sampleSession.pp.ifFc.toFixed(0)} · ACC ${formatAccuracy(sampleSession.live.accuracy)} · ${sampleSession.live.combo}x · ${sampleSession.live.modsText}`}
          settings={draft}
          visible={isPanelVisible(draft, 'statsPanel')}
          onPointerDown={beginDrag}
        />
        <EditableOverlayElement
          active={selectedElement === 'hitsPanel'}
          elementKey="hitsPanel"
          label={`100 ${sampleSession.live.hits.n100}    50 ${sampleSession.live.hits.n50}    MISS ${sampleSession.live.hits.misses}    SB ${sampleSession.live.hits.sliderBreaks}`}
          settings={draft}
          visible={isPanelVisible(draft, 'hitsPanel')}
          onPointerDown={beginDrag}
        />
        <EditableOverlayElement
          active={selectedElement === 'mapPanel'}
          elementKey="mapPanel"
          label={`${sampleSession.beatmap.artist} - ${sampleSession.beatmap.title}`}
          settings={draft}
          visible={isPanelVisible(draft, 'mapPanel')}
          onPointerDown={beginDrag}
        />
      </section>

      <aside className="overlay-editor-side">
        <div className="overlay-editor-tabs">
          {elementButtons.map((item) => (
            <button
              className={[
                'overlay-editor-tab',
                selectedElement === item.key ? 'overlay-editor-tab--active' : '',
                isPanelVisible(draft, item.key) ? '' : 'overlay-editor-tab--muted',
              ]
                .filter(Boolean)
                .join(' ')}
              key={item.key}
              type="button"
              onClick={() => setSelectedElement(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>
        <div className="overlay-editor-fields">
          <label className="overlay-editor-check overlay-editor-check--wide">
            <input
              checked={selectedVisible}
              type="checkbox"
              onChange={(event) => setPanelVisible(selectedElement, event.target.checked)}
            />
            <span>Visible</span>
          </label>
          <label className="overlay-editor-check overlay-editor-check--wide">
            <input
              checked={selectedPanel.showBackground}
              type="checkbox"
              onChange={(event) =>
                updatePanelSettings(selectedElement, {
                  ...selectedPanel,
                  showBackground: event.target.checked,
                })
              }
            />
            <span>Background</span>
          </label>
          <label>
            <span>X</span>
            <input
              type="number"
              value={selectedPanel.x}
              onChange={(event) =>
                updateElementPosition(selectedElement, Number(event.target.value), selectedPanel.y, true)
              }
            />
          </label>
          <label>
            <span>Y</span>
            <input
              type="number"
              value={selectedPanel.y}
              onChange={(event) =>
                updateElementPosition(selectedElement, selectedPanel.x, Number(event.target.value), true)
              }
            />
          </label>
          <label>
            <span>Width</span>
            <input
              min={24}
              type="number"
              value={selectedPanel.width}
              onChange={(event) =>
                updatePanelSettings(selectedElement, {
                  ...selectedPanel,
                  width: Math.max(24, Number(event.target.value)),
                })
              }
            />
          </label>
          <label>
            <span>Height</span>
            <input
              min={16}
              type="number"
              value={selectedPanel.height}
              onChange={(event) =>
                updatePanelSettings(selectedElement, {
                  ...selectedPanel,
                  height: Math.max(16, Number(event.target.value)),
                })
              }
            />
          </label>
          {selectedElement === 'statsPanel' ? (
            <div className="overlay-editor-metric-grid">
              {[
                ['showIfFc', 'IF FC'],
                ['showAccuracy', 'Acc'],
                ['showCombo', 'Combo'],
                ['showMods', 'Mods'],
              ].map(([key, label]) => (
                <label className="overlay-editor-check" key={key}>
                  <input
                    checked={Boolean(draft[key as 'showIfFc' | 'showAccuracy' | 'showCombo' | 'showMods'])}
                    type="checkbox"
                    onChange={(event) =>
                      updateStatsMetric(key as 'showIfFc' | 'showAccuracy' | 'showCombo' | 'showMods', event.target.checked)
                    }
                  />
                  <span>{label}</span>
                </label>
              ))}
            </div>
          ) : null}
        </div>
      </aside>
    </main>
  )
}

function EditableOverlayElement({
  active,
  elementKey,
  label,
  settings,
  visible,
  onPointerDown,
}: {
  active: boolean
  elementKey: OverlayPanelKey
  label: string
  settings: OverlaySettings
  visible: boolean
  onPointerDown: (
    key: OverlayPanelKey,
    event: ReactPointerEvent<HTMLElement>,
    mode?: 'move' | 'resize',
  ) => void
}) {
  const element = settings[elementKey]

  return (
    <button
      className={[
        'overlay-editor-element',
        active ? 'overlay-editor-element--active' : '',
        visible ? '' : 'overlay-editor-element--hidden',
      ]
        .filter(Boolean)
        .join(' ')}
      style={{
        left: `${element.x}px`,
        top: `${element.y}px`,
        width: `${element.width}px`,
        height: `${element.height}px`,
      }}
      type="button"
      onPointerDown={(event) => onPointerDown(elementKey, event)}
    >
      <span>{label}</span>
      <i
        aria-hidden="true"
        className="overlay-editor-element__resize"
        onPointerDown={(event) => {
          event.stopPropagation()
          onPointerDown(elementKey, event, 'resize')
        }}
      />
    </button>
  )
}

function AppIcon() {
  return <img alt="" src={appIconUrl} />
}

function SessionView({
  connection,
  coverSrc,
  mapGraph,
  mapTimeline,
  recentPlays,
  session,
  sessionGraph,
  onOpenHistory,
}: {
  connection: AppSnapshot['connection']
  coverSrc: string | null
  mapGraph: number[]
  mapTimeline: PerformanceSample[]
  recentPlays: RecentPlaySnapshot[]
  session: SessionSnapshot | null
  sessionGraph: number[]
  onOpenHistory: () => void
}) {
  const headline =
    session?.phase === 'preview'
      ? 'Selected Beatmap'
      : session?.phase === 'result'
        ? 'Result'
        : 'Session'
  return (
    <section className="page-shell">
      <header className="page-header">
        <h1>{headline}</h1>
      </header>

      {session ? (
        <div className="page-grid">
          <section className="page-grid__main">
            <NowPlayingCard coverSrc={coverSrc} session={session} />
            {session.phase === 'preview' ? (
              <PreviewMetricsCard session={session} />
            ) : (
              <LivePlayCard graph={sessionGraph} session={session} timeline={mapTimeline} />
            )}
            <RecentPlaysCard recentPlays={recentPlays} onOpenHistory={onOpenHistory} />
          </section>

          <aside className="page-grid__side">
            <LivePpPanel graph={mapGraph} session={session} />
          </aside>
        </div>
      ) : (
        <StatusPlaque connection={connection} />
      )}
    </section>
  )
}

function StatusPlaque({ connection }: { connection: AppSnapshot['connection'] }) {
  const title =
    connection.status === 'connected'
      ? 'No selected beatmap'
      : connection.status === 'error'
        ? 'Connection problem'
        : 'osu! is not running'

  return (
    <section className="panel panel--status">
      <div className="status-banner">
        <span className={`status-pill status-pill--${connection.status}`}>{connection.status}</span>
        <h2>{title}</h2>
        <p>{connection.detail}</p>
      </div>
    </section>
  )
}

function NowPlayingCard({
  coverSrc,
  session,
}: {
  coverSrc: string | null
  session: SessionSnapshot
}) {
  const { beatmap, live, phase } = session
  const currentTime = phase === 'preview' ? 0 : Math.round(beatmap.lengthMs * live.progress)
  const cardStyle = coverSrc
    ? ({
        '--beatmap-cover': `url("${coverSrc}")`,
      } as CSSProperties)
    : undefined

  return (
    <section
      className={`panel now-playing-panel ${coverSrc ? 'now-playing-panel--with-cover' : ''}`}
      style={cardStyle}
    >
      <div className="panel__title">
        {phase === 'preview' ? 'Selected Beatmap' : 'Now Playing'}
      </div>

      <div className="now-playing">
        <div className="cover-art">
          {coverSrc ? (
            <img src={coverSrc} alt={`${beatmap.artist} cover`} />
          ) : (
            <div className="cover-art__fallback">No cover</div>
          )}
        </div>

        <div className="now-playing__meta">
          <div className="now-playing__state">{live.gameState}</div>
          <h2>
            {beatmap.artist} - {beatmap.title}
          </h2>
          <div className="now-playing__difficulty">[{beatmap.difficultyName}]</div>
          <div className="now-playing__creator">
            mapset by <span>{beatmap.creator}</span>
          </div>

          <div className="map-summary">
            <div className="map-summary__item">
              <span>☆</span>
              <strong>{beatmap.starRating.toFixed(2)}</strong>
            </div>
            {beatmap.bpm ? (
              <div className="map-summary__item">
                <strong>{Math.round(beatmap.bpm)} BPM</strong>
              </div>
            ) : null}
            <div className="map-summary__item">
              <strong>{beatmap.status}</strong>
            </div>
          </div>

          <div className="mods-row">
            <span>Mods</span>
            <div className="mods-row__chips">
              {beatmap.mods.length > 0 ? (
                beatmap.mods.map((mod) => (
                  <span className="mod-chip" key={mod}>
                    {mod}
                  </span>
                ))
              ) : (
                <span className="mod-chip">NM</span>
              )}
            </div>
          </div>
        </div>
      </div>

      <div className="progress-row">
        <span>{formatLength(currentTime)}</span>
        <div className="progress-track">
          <div className="progress-track__fill" style={{ width: `${live.progress * 100}%` }} />
        </div>
        <span>{formatLength(beatmap.lengthMs)}</span>
      </div>
    </section>
  )
}

function PreviewMetricsCard({ session }: { session: SessionSnapshot }) {
  const { beatmap, live, pp } = session

  return (
    <section className="panel">
      <div className="panel__title">Beatmap Preview</div>

      <div className="live-metrics">
        <div className="metric-block metric-block--primary">
          <div className="metric-block__label">Selected PP</div>
          <div className="metric-block__value">{formatPlainPp(pp.fullMap)}</div>
        </div>

        <div className="metric-block">
          <div className="metric-block__label">Difficulty</div>
          <div className="metric-block__value metric-block__value--small">
            {beatmap.starRating.toFixed(2)}★
          </div>
          <div className="metric-block__subvalue">{beatmap.objectCount} objects</div>
        </div>

        <div className="metric-block">
          <div className="metric-block__label">Max Combo</div>
          <div className="metric-block__value metric-block__value--small">{live.maxCombo}x</div>
          <div className="metric-block__subvalue">{live.modsText}</div>
        </div>
      </div>

      <div className="preview-note">
        Preview mode uses the selected beatmap in song select together with the currently enabled mods.
      </div>

      <div className="stats-footer">
        <span>Length: {formatLength(beatmap.lengthMs)}</span>
        <span>CS: {beatmap.cs.toFixed(1)}</span>
        <span>AR: {beatmap.ar.toFixed(1)}</span>
        <span>OD: {beatmap.od.toFixed(1)}</span>
        <span>HP: {beatmap.hp.toFixed(1)}</span>
        <span>Mode: {beatmap.mode}</span>
      </div>
    </section>
  )
}

function LivePlayCard({
  graph,
  session,
  timeline,
}: {
  graph: number[]
  session: SessionSnapshot
  timeline: PerformanceSample[]
}) {
  const { beatmap, live, phase, pp } = session
  const comboTarget = beatmap.objectCount > 0 ? Math.max(live.maxCombo, live.combo) : live.maxCombo
  const ppPath = graphPath(graph, 860, 120)
  const panelTitle = phase === 'result' ? 'Play Result' : 'Live Play'
  const ppLabel = phase === 'result' ? 'Result PP' : 'Live PP'

  return (
    <section className="panel">
      <div className="panel__title">{panelTitle}</div>

      <div className="live-metrics">
        <div className="metric-block metric-block--primary">
          <div className="metric-block__label">{ppLabel}</div>
          <div className="metric-block__value">{formatPlainPp(pp.current)}</div>
        </div>

        <div className="metric-block">
          <div className="metric-block__label">Accuracy</div>
          <div className="metric-block__value metric-block__value--small">
            {formatAccuracy(live.accuracy)}
          </div>
          <div className="metric-block__subvalue">
            {formatDelta(live.accuracy === null ? null : 100 - live.accuracy)}
          </div>
        </div>

        <div className="metric-block">
          <div className="metric-block__label">Combo</div>
          <div className="metric-block__value metric-block__value--small">{live.combo}x</div>
          <div className="metric-block__subvalue">/ {comboTarget}x</div>
        </div>
      </div>

      <div className="live-secondary">
        <div>
          <span>If FC</span>
          <strong>{formatPp(pp.ifFc)}</strong>
        </div>
        <div>
          <span>Full Map</span>
          <strong>{formatPp(pp.fullMap)}</strong>
        </div>
        <div>
          <span>Score</span>
          <strong>{formatNumber(live.score)}</strong>
        </div>
      </div>

      <div className="hit-strip">
        <div className="hit-strip__item hit-strip__item--green">
          <span>300</span>
          <strong>{live.hits.n300}</strong>
        </div>
        <div className="hit-strip__item hit-strip__item--blue">
          <span>100</span>
          <strong>{live.hits.n100}</strong>
        </div>
        <div className="hit-strip__item hit-strip__item--orange">
          <span>50</span>
          <strong>{live.hits.n50}</strong>
        </div>
        <div className="hit-strip__item hit-strip__item--red">
          <span>Miss</span>
          <strong>{live.hits.misses}</strong>
        </div>
      </div>

      <div className="chart-panel">
        {graph.length > 1 ? (
          <svg className="chart-svg" viewBox="0 0 860 120" preserveAspectRatio="none">
            <path className="chart-svg__fill" d={`${ppPath} L 860 120 L 0 120 Z`} />
            <path className="chart-svg__line" d={ppPath} />
          </svg>
        ) : (
          <div className="chart-empty">
            {phase === 'result'
              ? 'The final result is locked in. The graph fills once live updates have been captured.'
              : 'Live graph appears after PP updates start.'}
          </div>
        )}
      </div>

      <PerformanceTimeline session={session} timeline={timeline} />

      <div className="stats-footer">
        <span>Play Length: {formatLength(beatmap.lengthMs)}</span>
        <span>CS: {beatmap.cs.toFixed(1)}</span>
        <span>AR: {beatmap.ar.toFixed(1)}</span>
        <span>OD: {beatmap.od.toFixed(1)}</span>
        <span>HP: {beatmap.hp.toFixed(1)}</span>
        <span>SR: {beatmap.starRating.toFixed(2)}★</span>
      </div>
    </section>
  )
}

function PerformanceTimeline({
  session,
  timeline,
}: {
  session: SessionSnapshot
  timeline: PerformanceSample[]
}) {
  const { beatmap, live, pp } = session
  const samples =
    timeline.length > 0
      ? timeline
      : [
          {
            progress: live.progress,
            passedObjects: live.passedObjects,
            ppCurrent: pp.current,
            ppIfFc: pp.ifFc,
            accuracy: live.accuracy,
            combo: live.combo,
            score: live.score,
            misses: live.hits.misses,
            sliderBreaks: live.hits.sliderBreaks,
            hp: live.hp,
          },
        ]
  const latest = samples.at(-1)
  const peak = samples.reduce((best, sample) => (sample.ppCurrent > best.ppCurrent ? sample : best), samples[0])
  const firstMiss = samples.find((sample, index) => index > 0 && sample.misses > samples[index - 1].misses)
  const firstSliderBreak = samples.find(
    (sample, index) => index > 0 && sample.sliderBreaks > samples[index - 1].sliderBreaks,
  )
  const accuracyDrop = samples.find((sample) => sample.accuracy !== null && sample.accuracy < 98)
  const potentialLoss = Math.max(0, pp.ifFc - pp.current)
  const timelineEvents = [
    { label: 'Peak PP', sample: peak, tone: 'good' },
    firstSliderBreak ? { label: 'Slider break', sample: firstSliderBreak, tone: 'warn' } : null,
    firstMiss ? { label: 'Miss', sample: firstMiss, tone: 'danger' } : null,
    accuracyDrop ? { label: 'Acc drop', sample: accuracyDrop, tone: 'blue' } : null,
  ].filter(Boolean) as Array<{ label: string; sample: PerformanceSample; tone: string }>

  return (
    <section className="performance-timeline">
      <div className="performance-timeline__head">
        <div>
          <strong>Performance Timeline</strong>
          <span>{formatCount(latest?.passedObjects ?? 0)} / {formatCount(beatmap.objectCount)} objects</span>
        </div>
        <div className="performance-timeline__loss">
          <span>FC gap</span>
          <strong>{potentialLoss.toFixed(2)} PP</strong>
        </div>
      </div>

      <div className="performance-track" aria-hidden="true">
        <div className="performance-track__rail" />
        <div
          className="performance-track__fill"
          style={{ width: clampPercent((latest?.progress ?? live.progress) * 100) }}
        />
        {timelineEvents.map((event) => (
          <div
            className={`performance-event performance-event--${event.tone}`}
            key={`${event.label}-${event.sample.passedObjects}-${event.sample.ppCurrent}`}
            style={{ left: clampPercent(event.sample.progress * 100) }}
          >
            <span>{event.label}</span>
          </div>
        ))}
      </div>

      <div className="performance-summary">
        <div>
          <span>Peak</span>
          <strong>{peak.ppCurrent.toFixed(2)} PP</strong>
        </div>
        <div>
          <span>Accuracy</span>
          <strong>{formatAccuracy(latest?.accuracy ?? live.accuracy)}</strong>
        </div>
        <div>
          <span>Combo</span>
          <strong>{formatCount(latest?.combo ?? live.combo)}x</strong>
        </div>
        <div>
          <span>Miss / SB</span>
          <strong>{formatCount(latest?.misses ?? live.hits.misses)} / {formatCount(latest?.sliderBreaks ?? live.hits.sliderBreaks)}</strong>
        </div>
      </div>
    </section>
  )
}

function RecentPlaysCard({
  recentPlays,
  onOpenHistory,
}: {
  recentPlays: RecentPlaySnapshot[]
  onOpenHistory: () => void
}) {
  return (
    <section className="panel">
      <div className="panel__title panel__title--split">
        <span>Recent Plays</span>
        <button className="link-button" type="button" onClick={onOpenHistory}>
          View full history
        </button>
      </div>

      <PlaysTable
        recentPlays={recentPlays}
        emptyLabel="Completed plays will appear here and stay saved between launches."
      />
    </section>
  )
}

function LivePpPanel({
  graph,
  session,
}: {
  graph: number[]
  session: SessionSnapshot
}) {
  const { phase, pp } = session
  const graphWidth = 248
  const graphHeight = 112
  const linePath = graphPath(graph, graphWidth, graphHeight)
  const maxValue = graph.length > 0 ? Math.max(...graph).toFixed(0) : '0'
  const panelTitle =
    phase === 'preview' ? 'Selected PP' : phase === 'result' ? 'Result PP' : 'Live PP'

  return (
    <section className="pp-panel">
      <div className="pp-panel__section">
        <div className="pp-panel__title">{panelTitle}</div>
        <div className="pp-panel__hero">
          <strong>{pp.current.toFixed(2)}</strong>
          <span>PP</span>
        </div>
      </div>

      <div className="pp-panel__section">
        <div className="pp-panel__subtitle">PP Breakdown</div>
        <div className="breakdown-list">
          {pp.components.map((component) => {
            const ratio = pp.current > 0 ? (component.value / pp.current) * 100 : 0

            return (
              <div className="breakdown-item" key={component.label}>
                <div className="breakdown-item__head">
                  <span>{component.label}</span>
                  <div>
                    <strong>{component.value.toFixed(2)} PP</strong>
                    <span>{ratio.toFixed(1)}%</span>
                  </div>
                </div>
                <div className="breakdown-item__track">
                  <div
                    className="breakdown-item__fill"
                    style={{ width: `${Math.max(8, Math.min(100, ratio))}%` }}
                  />
                </div>
              </div>
            )
          })}
        </div>
      </div>

      <div className="pp-panel__section">
        <div className="pp-stats">
          <div>
            <span>Difficulty Adjust</span>
            <strong>{pp.difficultyAdjust.toFixed(2)}x</strong>
          </div>
          <div>
            <span>Mods Multiplier</span>
            <strong>{pp.modsMultiplier.toFixed(2)}x</strong>
          </div>
          <div className="pp-stats__total">
            <span>Total</span>
            <strong>{formatPp(pp.current)}</strong>
          </div>
        </div>
      </div>

      <div className="pp-panel__section">
        <div className="pp-panel__subtitle">
          {phase === 'preview' ? 'PP Graph' : 'PP Graph (This Map)'}
        </div>
        <div className="map-graph">
          {graph.length > 1 ? (
            <>
              <svg
                className="map-graph__svg"
                viewBox={`0 0 ${graphWidth} ${graphHeight}`}
                preserveAspectRatio="none"
              >
                <path
                  className="map-graph__fill"
                  d={`${linePath} L ${graphWidth} ${graphHeight} L 0 ${graphHeight} Z`}
                />
                <path className="map-graph__line" d={linePath} />
              </svg>
              <div className="map-graph__axis">
                <span>0%</span>
                <span>50%</span>
                <span>100%</span>
              </div>
              <div className="map-graph__scale">{maxValue}</div>
            </>
          ) : (
            <div className="chart-empty chart-empty--compact">
              {phase === 'preview'
                ? 'A live graph appears only while the map is running.'
                : 'Waiting for enough live points.'}
            </div>
          )}
        </div>
      </div>
    </section>
  )
}

function RecentHistoryView({ recentPlays }: { recentPlays: RecentPlaySnapshot[] }) {
  const [coverImages, setCoverImages] = useState<Record<string, string>>({})

  useEffect(() => {
    if (!isTauriRuntime()) {
      return
    }

    const paths = Array.from(
      new Set(
        recentPlays
          .map((play) => play.coverPath)
          .filter((path): path is string => Boolean(path && !Object.prototype.hasOwnProperty.call(coverImages, path))),
      ),
    )

    if (paths.length === 0) {
      return
    }

    let cancelled = false

    void Promise.all(
      paths.map(async (path) => {
        try {
          const src = await invoke<string>('load_image_data_uri', { path })
          return [path, src] as const
        } catch {
          return [path, ''] as const
        }
      }),
    ).then((loaded) => {
      if (cancelled) {
        return
      }

      setCoverImages((current) => {
        const next = { ...current }
        let changed = false

        for (const [path, src] of loaded) {
          next[path] = src
          changed = true
        }

        return changed ? next : current
      })
    })

    return () => {
      cancelled = true
    }
  }, [coverImages, recentPlays])

  return (
    <section className="page-shell page-shell--single">
      <header className="page-header">
        <h1>Recent Plays</h1>
      </header>

      <RecentPlayCards coverImages={coverImages} recentPlays={recentPlays} />

      <section className="panel">
        <div className="panel__title">Saved History</div>
        <PlaysTable
          recentPlays={recentPlays}
          emptyLabel="No completed plays have been captured yet."
        />
      </section>
    </section>
  )
}

function RecentPlayCards({
  coverImages,
  recentPlays,
}: {
  coverImages: Record<string, string>
  recentPlays: RecentPlaySnapshot[]
}) {
  if (recentPlays.length === 0) {
    return null
  }

  const bestPp = Math.max(...recentPlays.map((play) => play.pp), 1)

  return (
    <section className="recent-card-grid" aria-label="Recent play cards">
      {recentPlays.map((play) => {
        const tone = accuracyTone(play.accuracy)
        const ppWidth = (play.pp / bestPp) * 100
        const coverSrc = play.coverPath ? coverImages[play.coverPath] : null

        return (
          <article className="recent-play-card" key={`card-${play.timestampMs}-${play.title}-${play.pp}`}>
            <div className={`recent-play-card__cover recent-play-card__cover--${tone}`}>
              {coverSrc ? <img src={coverSrc} alt="" /> : null}
            </div>
            <div className="recent-play-card__body">
              <div className="recent-play-card__meta">
                <span>{formatRelativeTime(play.timestampMs)}</span>
                <span>{play.modsText}</span>
              </div>
              <h2>{play.title}</h2>
              <div className="recent-play-card__stats">
                <div>
                  <strong>{play.pp.toFixed(2)}</strong>
                  <span>PP</span>
                </div>
                <div>
                  <strong>{formatAccuracy(play.accuracy)}</strong>
                  <span>Acc</span>
                </div>
                <div>
                  <strong>{formatCount(play.combo)}x</strong>
                  <span>Combo</span>
                </div>
              </div>
              <div className="recent-play-card__bar">
                <div style={{ width: clampPercent(ppWidth) }} />
              </div>
            </div>
          </article>
        )
      })}
    </section>
  )
}

function OverlayView({
  settings,
  onUpdateSettings,
}: {
  settings: OverlaySettings
  onUpdateSettings: (settings: OverlaySettings) => void
}) {
  const [capturingHotkey, setCapturingHotkey] = useState(false)
  const overlayPresets: Array<{
    label: string
    description: string
    settings: Partial<OverlaySettings>
  }> = [
    {
      label: 'Compact',
      description: 'Small readable HUD for regular play.',
      settings: {
        width: 280,
        height: 62,
        scale: 1,
        fontScale: 1,
        padding: 0,
        cornerRadius: 10,
        opacity: 0.9,
        showBackground: true,
        showMap: false,
        ...compactOverlayPanels,
      },
    },
    {
      label: 'Tournament',
      description: 'Larger numbers for capture and streams.',
      settings: {
        width: 396,
        height: 106,
        scale: 1,
        fontScale: 1,
        padding: 0,
        cornerRadius: 12,
        opacity: 0.94,
        showBackground: true,
        showMap: true,
        ...tournamentOverlayPanels,
      },
    },
    {
      label: 'Minimal',
      description: 'Transparent stats with low screen weight.',
      settings: {
        width: 266,
        height: 56,
        scale: 1,
        fontScale: 0.92,
        padding: 0,
        cornerRadius: 8,
        opacity: 0.72,
        showBackground: false,
        showMap: false,
        ...minimalOverlayPanels,
      },
    },
  ]

  const updateSetting = <K extends keyof OverlaySettings>(key: K, value: OverlaySettings[K]) => {
    onUpdateSettings({
      ...settings,
      [key]: value,
    })
  }

  const updateNumericSetting = (
    key: 'editorPanelWidth' | 'editorPanelHeight' | 'dataUpdateIntervalMs',
    value: string,
  ) => {
    const nextValue = Number(value)

    if (Number.isNaN(nextValue)) {
      return
    }

    updateSetting(key, nextValue)
  }

  const updateHotkey = (value: string | null) => {
    if (!value) {
      setCapturingHotkey(false)
      return
    }

    updateSetting('toggleKey', value)
    setCapturingHotkey(false)
  }

  const resetOverlaySettings = () => {
    onUpdateSettings(DEFAULT_OVERLAY_SETTINGS)
  }

  return (
    <section className="page-shell page-shell--single overlay-settings-page">
      <header className="page-header">
        <h1>Overlay</h1>
      </header>

      <section className="overlay-workspace">
        <div className="overlay-settings-grid">
          <article className="overlay-settings-card overlay-settings-card--primary">
            <div className="overlay-settings-card__copy">
              <span className="overlay-settings-card__eyebrow">Overlay status</span>
              <strong>{settings.enabled ? 'Enabled' : 'Disabled'}</strong>
              <p>Turns the in-game HUD on or off.</p>
            </div>
            <button
              className={`toggle-button ${settings.enabled ? 'toggle-button--active' : ''}`}
              type="button"
              onClick={() => updateSetting('enabled', !settings.enabled)}
            >
              {settings.enabled ? 'Enabled' : 'Disabled'}
            </button>
          </article>

          <article className="overlay-settings-card overlay-settings-card--presets">
            <div className="overlay-settings-card__copy">
              <span className="overlay-settings-card__eyebrow">HUD presets</span>
              <strong>Layout starting points</strong>
            </div>
            <div className="overlay-preset-list">
              {overlayPresets.map((preset) => (
                <button
                  className="overlay-preset"
                  key={preset.label}
                  type="button"
                  onClick={() =>
                    onUpdateSettings({
                      ...settings,
                      ...preset.settings,
                    })
                  }
                >
                  <span>{preset.label}</span>
                  <small>{preset.description}</small>
                </button>
              ))}
            </div>
          </article>

          <article className="overlay-settings-card">
            <div className="overlay-settings-card__copy">
              <span className="overlay-settings-card__eyebrow">In-game settings panel</span>
              <strong>Editor window size</strong>
              <p>Controls the settings panel shown inside osu!.</p>
            </div>
            <div className="number-grid overlay-settings-card__fields">
              <label className="number-field">
                <span>Width</span>
                <input
                  max={1100}
                  min={760}
                  type="number"
                  value={settings.editorPanelWidth}
                  onChange={(event) => updateNumericSetting('editorPanelWidth', event.target.value)}
                />
              </label>
              <label className="number-field">
                <span>Height</span>
                <input
                  max={760}
                  min={520}
                  type="number"
                  value={settings.editorPanelHeight}
                  onChange={(event) => updateNumericSetting('editorPanelHeight', event.target.value)}
                />
              </label>
            </div>
          </article>

          <article className="overlay-settings-card overlay-settings-card--hotkey">
            <div className="overlay-settings-card__copy">
              <span className="overlay-settings-card__eyebrow">Editor hotkey</span>
              <strong>{settings.toggleKey}</strong>
              <p>Opens the in-game overlay editor window.</p>
            </div>
            <button
              className={`hotkey-capture-button ${capturingHotkey ? 'hotkey-capture-button--active' : ''}`}
              type="button"
              onBlur={() => setCapturingHotkey(false)}
              onClick={() => setCapturingHotkey(true)}
              onKeyDown={(event) => {
                event.preventDefault()
                event.stopPropagation()
                updateHotkey(hotkeyFromKeyboardEvent(event))
              }}
            >
              {capturingHotkey ? 'Press key' : settings.toggleKey}
            </button>
          </article>

          <article className="overlay-settings-card">
            <div className="overlay-settings-card__copy">
              <span className="overlay-settings-card__eyebrow">Overlay data</span>
              <strong>Data update interval</strong>
              <p>Controls how often the overlay reads and redraws live osu! data.</p>
            </div>
            <label className="number-field overlay-settings-card__single-field">
              <span>Milliseconds</span>
              <input
                max={1000}
                min={16}
                type="number"
                value={settings.dataUpdateIntervalMs}
                onChange={(event) => updateNumericSetting('dataUpdateIntervalMs', event.target.value)}
              />
            </label>
          </article>

          <article className="overlay-settings-card overlay-settings-card--reset">
            <div className="overlay-settings-card__copy">
              <span className="overlay-settings-card__eyebrow">Defaults</span>
              <strong>Reset overlay settings</strong>
              <p>Restores the HUD, in-game editor and hotkey settings.</p>
            </div>
            <button className="reset-button" type="button" onClick={resetOverlaySettings}>
              Reset
            </button>
          </article>
        </div>

        <aside className="overlay-preview-panel">
          <div className="overlay-preview-panel__head">
            <div>
              <span>Preview</span>
              <strong>{settings.width} x {settings.height}</strong>
            </div>
            <span>{Math.round(settings.opacity * 100)}%</span>
          </div>
          <div
            className="overlay-preview-stage"
            style={
              {
                '--overlay-width': `${settings.width}px`,
                '--overlay-height': `${settings.height}px`,
                '--overlay-scale': settings.scale.toString(),
                '--overlay-font-scale': settings.fontScale.toString(),
                '--overlay-padding': `${settings.padding}px`,
                '--overlay-radius': `${settings.cornerRadius}px`,
                '--overlay-opacity': settings.opacity.toString(),
              } as CSSProperties
            }
          >
            <NativeOverlayPreview session={sampleSession} settings={settings} />
          </div>
        </aside>
      </section>
    </section>
  )
}

function NativeOverlayPreview({
  session,
  settings,
}: {
  session: SessionSnapshot
  settings: OverlaySettings
}) {
  const bounds = overlayPreviewBounds(settings)
  const metricCells = [
    settings.showIfFc ? ['IF FC', session.pp.ifFc.toFixed(2)] : null,
    settings.showAccuracy ? ['ACC', formatAccuracy(session.live.accuracy)] : null,
    settings.showCombo ? ['COMBO', `${session.live.combo}x`] : null,
    settings.showMods ? ['MODS', session.live.modsText] : null,
  ].filter(Boolean) as Array<[string, string]>
  const hitCells = [
    ['100', formatCount(session.live.hits.n100), 'blue'],
    ['50', formatCount(session.live.hits.n50), 'orange'],
    ['MISS', formatCount(session.live.hits.misses), 'red'],
    ['SB', formatCount(session.live.hits.sliderBreaks), 'amber'],
  ] as const

  const elementStyle = (element: OverlayElementSettings) =>
    ({
      left: `${element.x - bounds.left}px`,
      top: `${element.y - bounds.top}px`,
      width: `${element.width}px`,
      height: `${element.height}px`,
    }) as CSSProperties

  return (
    <div
      className="native-overlay-preview"
      style={
        {
          width: `${bounds.width}px`,
          height: `${bounds.height}px`,
          '--overlay-opacity': settings.opacity.toString(),
          '--overlay-radius': `${settings.cornerRadius}px`,
        } as CSSProperties
      }
    >
      {settings.showPp && settings.ppPanel.enabled ? (
        <div className="native-overlay-panel native-overlay-panel--pp" style={elementStyle(settings.ppPanel)}>
          <strong>{session.pp.current.toFixed(2)}</strong>
          <span>PP</span>
        </div>
      ) : null}

      {settings.statsPanel.enabled && metricCells.length > 0 ? (
        <div className="native-overlay-panel native-overlay-panel--stats" style={elementStyle(settings.statsPanel)}>
          {metricCells.map(([label, value]) => (
            <div className="native-overlay-cell" key={label}>
              <span>{label}</span>
              <strong>{value}</strong>
            </div>
          ))}
        </div>
      ) : null}

      {settings.showHits && settings.hitsPanel.enabled ? (
        <div className="native-overlay-panel native-overlay-panel--hits" style={elementStyle(settings.hitsPanel)}>
          {hitCells.map(([label, value, tone]) => (
            <div className={`native-overlay-hit native-overlay-hit--${tone}`} key={label}>
              <span>{label}</span>
              <strong>{value}</strong>
            </div>
          ))}
        </div>
      ) : null}

      {settings.showMap && settings.mapPanel.enabled ? (
        <div className="native-overlay-panel native-overlay-panel--map" style={elementStyle(settings.mapPanel)}>
          {session.beatmap.artist} - {session.beatmap.title} [{session.beatmap.difficultyName}]
        </div>
      ) : null}
    </div>
  )
}

function SettingsView({
  calculator,
  recentPlayCount,
}: {
  calculator: string
  recentPlayCount: number
}) {
  return (
    <section className="page-shell page-shell--single">
      <header className="page-header">
        <h1>Settings</h1>
      </header>

      <section className="panel panel--stacked">
        <div className="settings-row">
          <div>
            <strong>Reader target</strong>
          </div>
          <span className="settings-value">stable</span>
        </div>

        <div className="settings-row">
          <div>
            <strong>PP calculator</strong>
          </div>
          <span className="settings-value">{calculator}</span>
        </div>

        <div className="settings-row">
          <div>
            <strong>Saved history</strong>
          </div>
          <span className="settings-value">{recentPlayCount} / {RECENT_PLAY_LIMIT}</span>
        </div>
      </section>
    </section>
  )
}

function AboutView() {
  return (
    <section className="page-shell page-shell--single">
      <header className="page-header">
        <h1>About</h1>
      </header>

      <section className="panel panel--stacked">
        <div className="settings-row">
          <div>
            <strong>Stack</strong>
          </div>
          <span className="settings-value">desktop</span>
        </div>

        <div className="settings-row">
          <div>
            <strong>Design direction</strong>
          </div>
          <span className="settings-value">dark</span>
        </div>
      </section>
    </section>
  )
}

function PlaysTable({
  recentPlays,
  emptyLabel,
}: {
  recentPlays: RecentPlaySnapshot[]
  emptyLabel: string
}) {
  return (
    <div className="plays-table">
      <div className="plays-table__head">
        <span>Time</span>
        <span>Map</span>
        <span>Mods</span>
        <span>Acc</span>
        <span>Combo</span>
        <span>PP</span>
      </div>

      {recentPlays.length > 0 ? (
        recentPlays.map((play) => (
          <div className="plays-table__row" key={`${play.timestampMs}-${play.title}-${play.pp}`}>
            <span>{formatRelativeTime(play.timestampMs)}</span>
            <span>{play.title}</span>
            <span>{play.modsText}</span>
            <span>{formatAccuracy(play.accuracy)}</span>
            <span>{play.combo}x</span>
            <span>{play.pp.toFixed(2)}</span>
          </div>
        ))
      ) : (
        <div className="plays-table__empty">{emptyLabel}</div>
      )}
    </div>
  )
}

function SessionIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path d="M4 15.5V4.5" />
      <path d="M7 15.5V9.5" />
      <path d="M10 15.5V6.5" />
      <path d="M13 15.5V11.5" />
      <path d="M16 15.5V3.5" />
      <path d="M3.5 15.5H16.5" />
    </svg>
  )
}

function HistoryIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <circle cx="10" cy="10" r="6.5" />
      <path d="M10 6.5V10.5L12.75 12.25" />
    </svg>
  )
}

function OverlayIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <rect x="3.5" y="4" width="13" height="10.5" rx="2" />
      <path d="M7 16.5H13" />
    </svg>
  )
}

function SettingsIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path d="M10 3.75L11.1 5.5L13.2 5.95L12.5 8L13.8 9.7L12.1 11.1L12.2 13.25L10 13.1L8 13.25L7.9 11.1L6.2 9.7L7.5 8L6.8 5.95L8.9 5.5L10 3.75Z" />
      <circle cx="10" cy="9.25" r="2.2" />
    </svg>
  )
}

function AboutIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <circle cx="10" cy="10" r="6.5" />
      <path d="M10 8V12" />
      <circle cx="10" cy="6" r="0.6" fill="currentColor" stroke="none" />
    </svg>
  )
}

function MinimizeIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M4 8.5H12" />
    </svg>
  )
}

function MaximizeIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <rect x="4" y="4" width="8" height="8" rx="1.2" />
    </svg>
  )
}

function RestoreIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M6 4.5H11.5V10" />
      <path d="M4.5 6H10V11.5H4.5Z" />
    </svg>
  )
}

function CloseIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M4.5 4.5L11.5 11.5" />
      <path d="M11.5 4.5L4.5 11.5" />
    </svg>
  )
}

export default App
