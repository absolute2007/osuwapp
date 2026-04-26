import {
  type CSSProperties,
  startTransition,
  useDeferredValue,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import '@fontsource-variable/inter/index.css'
import './App.css'
import { initialSnapshot } from './mockSnapshot'
import type {
  AppSnapshot,
  OverlaySettings,
  RecentPlaySnapshot,
  SessionSnapshot,
} from './types'

const SNAPSHOT_EVENT = 'session-updated'
const OVERLAY_SETTINGS_EVENT = 'overlay-settings-updated'
const OPEN_OVERLAY_SETTINGS_EVENT = 'open-overlay-settings'
const MAX_GRAPH_POINTS = 96
const integerFormatter = new Intl.NumberFormat('en-US')
const DEFAULT_OVERLAY_SETTINGS: OverlaySettings = {
  enabled: true,
  showPp: true,
  showIfFc: true,
  showAccuracy: true,
  showCombo: true,
  showMods: true,
  showMap: true,
  showHits: true,
  width: 420,
  height: 248,
  offsetX: 24,
  offsetY: 24,
  scale: 0.82,
  fontScale: 0.9,
  padding: 8,
  cornerRadius: 12,
  opacity: 0.92,
  showBackground: true,
  toggleKey: 'Insert',
}

type AppView = 'session' | 'recent' | 'overlay' | 'settings' | 'about'

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

const normalizeToggleKey = (key: string) => {
  if (/^F([1-9]|1[0-2])$/i.test(key)) {
    return key.toUpperCase()
  }

  switch (key) {
    case ' ':
      return 'Space'
    case 'ArrowUp':
      return 'Up'
    case 'ArrowDown':
      return 'Down'
    case 'ArrowLeft':
      return 'Left'
    case 'ArrowRight':
      return 'Right'
    case 'PageUp':
      return 'PageUp'
    case 'PageDown':
      return 'PageDown'
    case 'Delete':
      return 'Delete'
    case 'Insert':
      return 'Insert'
    case 'Home':
      return 'Home'
    case 'End':
      return 'End'
    case 'Tab':
      return 'Tab'
    case 'Enter':
      return 'Enter'
    default:
      if (/^[a-z0-9]$/i.test(key)) {
        return key.toUpperCase()
      }

      return ''
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
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot)
  const [overlaySettings, setOverlaySettings] = useState<OverlaySettings>(DEFAULT_OVERLAY_SETTINGS)
  const [activeView, setActiveView] = useState<AppView>('session')
  const [mapGraph, setMapGraph] = useState<number[]>([])
  const [sessionGraph, setSessionGraph] = useState<number[]>([])
  const [alwaysOnTop, setAlwaysOnTop] = useState(false)
  const [isMaximized, setIsMaximized] = useState(false)
  const currentMapKeyRef = useRef<string | null>(null)
  const mapGraphRef = useRef<number[]>([])
  const sessionGraphRef = useRef<number[]>([])
  const viewModel = useDeferredValue(snapshot)

  const applySnapshot = useEffectEvent((nextSnapshot: AppSnapshot) => {
    const session = nextSnapshot.session

    if (!session || session.phase === 'preview') {
      currentMapKeyRef.current = null
      mapGraphRef.current = []
      sessionGraphRef.current = []
      setMapGraph([])
      setSessionGraph([])
      setSnapshot(nextSnapshot)
      return
    }

    const mapKey = `${session.beatmap.path}:${session.live.modsText}`
    const currentPp = Number(session.pp.current.toFixed(2))

    const nextMapGraph = (() => {
      const base = currentMapKeyRef.current === mapKey ? mapGraphRef.current : []
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

    mapGraphRef.current = nextMapGraph
    sessionGraphRef.current = nextSessionGraph
    setMapGraph(nextMapGraph)
    setSessionGraph(nextSessionGraph)
    setSnapshot(nextSnapshot)
  })

  useEffect(() => {
    document.body.dataset.overlayMode = overlayMode ? 'true' : 'false'
    document.documentElement.dataset.overlayMode = overlayMode ? 'true' : 'false'

    return () => {
      delete document.body.dataset.overlayMode
      delete document.documentElement.dataset.overlayMode
    }
  }, [overlayMode])

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
  let coverSrc: string | null = null

  if (session?.beatmap.coverPath && isTauriRuntime()) {
    try {
      coverSrc = convertFileSrc(session.beatmap.coverPath)
    } catch {
      coverSrc = null
    }
  }

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

  return (
    <div className="window-shell">
      <header className="titlebar">
        <div
          className="titlebar__drag"
          onPointerDown={(event) => {
            void startTauriWindowDrag(event)
          }}
        >
          <div className="titlebar__brand">
            <div className="titlebar__badge" aria-hidden="true">
              <OsuLogo />
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
              alwaysOnTop={alwaysOnTop}
              settings={overlaySettings}
              onToggleAlwaysOnTop={() => {
                void handleWindowAction('toggleAlwaysOnTop')
              }}
              onUpdateSettings={(nextSettings) => {
                void persistOverlaySettings(nextSettings)
              }}
              session={session}
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
        <span>{viewModel.recentPlays.length} saved plays</span>
        <span>{session?.pp.calculator ?? 'rosu-pp 4.0.1'}</span>
      </footer>
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
  return (
    <div className={`${className} ${settings.showBackground ? '' : 'overlay-card--bare'}`}>
      {session ? (
        <div className="overlay-card__scale-shell">
          <div className="overlay-card__scale-content">
            <div className="overlay-card__content overlay-card__content--hud">
              {settings.showPp ? (
                <div className="overlay-card__hero overlay-card__hero--hud">
                  <strong>{session.pp.current.toFixed(2)}</strong>
                  <span>PP</span>
                </div>
              ) : null}

              <div className="overlay-card__stats overlay-card__stats--hud">
                {settings.showIfFc ? (
                  <div>
                    <span>IF FC</span>
                    <strong>{session.pp.ifFc.toFixed(2)}</strong>
                  </div>
                ) : null}
                {settings.showAccuracy ? (
                  <div>
                    <span>ACC</span>
                    <strong>{formatAccuracy(session.live.accuracy)}</strong>
                  </div>
                ) : null}
                {settings.showCombo ? (
                  <div>
                    <span>COMBO</span>
                    <strong>{session.live.combo}x</strong>
                  </div>
                ) : null}
                {settings.showMods ? (
                  <div>
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

function OsuLogo() {
  return (
    <svg viewBox="0 0 64 64" fill="none" aria-hidden="true">
      <circle cx="32" cy="32" r="32" fill="url(#osu-pink)" />
      <circle cx="32" cy="32" r="22.5" stroke="rgba(255,255,255,0.95)" strokeWidth="5" />
      <circle cx="32" cy="32" r="26.75" stroke="rgba(255,255,255,0.18)" strokeWidth="1.5" />
      <text
        x="32"
        y="39"
        fill="#ffffff"
        fontFamily="-apple-system,BlinkMacSystemFont,'SF Pro Text','SF Pro Display','Helvetica Neue',sans-serif"
        fontSize="17"
        fontWeight="700"
        textAnchor="middle"
      >
        osu!
      </text>
      <defs>
        <linearGradient id="osu-pink" x1="32" x2="32" y1="0" y2="64" gradientUnits="userSpaceOnUse">
          <stop stopColor="#FF7DB2" />
          <stop offset="1" stopColor="#F05793" />
        </linearGradient>
      </defs>
    </svg>
  )
}

function SessionView({
  connection,
  coverSrc,
  mapGraph,
  recentPlays,
  session,
  sessionGraph,
  onOpenHistory,
}: {
  connection: AppSnapshot['connection']
  coverSrc: string | null
  mapGraph: number[]
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
              <LivePlayCard graph={sessionGraph} session={session} />
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

  return (
    <section className="panel">
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
}: {
  graph: number[]
  session: SessionSnapshot
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
  return (
    <section className="page-shell page-shell--single">
      <header className="page-header">
        <h1>Recent Plays</h1>
      </header>

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

function OverlayView({
  alwaysOnTop,
  settings,
  onToggleAlwaysOnTop,
  onUpdateSettings,
  session,
}: {
  alwaysOnTop: boolean
  settings: OverlaySettings
  onToggleAlwaysOnTop: () => void
  onUpdateSettings: (settings: OverlaySettings) => void
  session: SessionSnapshot | null
}) {
  const updateSetting = <K extends keyof OverlaySettings>(key: K, value: OverlaySettings[K]) => {
    onUpdateSettings({
      ...settings,
      [key]: value,
    })
  }

  const updateNumericSetting = (
    key: 'width' | 'height' | 'offsetX' | 'offsetY' | 'padding' | 'cornerRadius',
    value: string,
  ) => {
    const nextValue = Number(value)

    if (Number.isNaN(nextValue)) {
      return
    }

    updateSetting(key, nextValue)
  }

  return (
    <section className="page-shell page-shell--single">
      <header className="page-header">
        <h1>Overlay</h1>
      </header>

      <section className="panel overlay-settings-preview">
        <div className="panel__title">
          <span>Preview</span>
          <span className="settings-value">{settings.width} x {settings.height}</span>
        </div>
        <div
          className="overlay-settings-preview__stage"
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
          <OverlayHudCard
            className="overlay-card overlay-card--hud"
            session={session}
            settings={settings}
          />
        </div>
      </section>

      <section className="panel panel--stacked">
        <div className="settings-row">
          <div>
            <strong>Overlay enabled</strong>
          </div>
          <button
            className="toggle-button"
            type="button"
            onClick={() => updateSetting('enabled', !settings.enabled)}
          >
            {settings.enabled ? 'Enabled' : 'Enable'}
          </button>
        </div>

        <div className="settings-row">
          <div>
            <strong>In-game editor</strong>
          </div>
          <span className="settings-value">{settings.toggleKey}</span>
        </div>

        <div className="settings-row">
          <div>
            <strong>Overlay toggle key</strong>
          </div>
          <label className="number-field settings-field">
            <span>Key</span>
            <input
              type="text"
              value={settings.toggleKey}
              readOnly
              onKeyDown={(event) => {
                event.preventDefault()
                event.stopPropagation()

                const nextKey = normalizeToggleKey(event.key)

                if (nextKey) {
                  updateSetting('toggleKey', nextKey)
                }
              }}
            />
          </label>
        </div>

        <div className="settings-row">
          <div>
            <strong>Main window always on top</strong>
          </div>
          <button className="toggle-button" type="button" onClick={onToggleAlwaysOnTop}>
            {alwaysOnTop ? 'Enabled' : 'Enable'}
          </button>
        </div>

        <div className="settings-row">
          <div>
            <strong>Overlay opacity</strong>
          </div>
          <div className="slider-control">
            <input
              max={100}
              min={5}
              step={1}
              type="range"
              value={Math.round(settings.opacity * 100)}
              onChange={(event) => updateSetting('opacity', Number(event.target.value) / 100)}
            />
            <span className="settings-value">{Math.round(settings.opacity * 100)}%</span>
          </div>
        </div>

        <div className="settings-row">
          <div>
            <strong>Overlay scaling</strong>
          </div>
          <div className="slider-control">
            <input
              max={250}
              min={15}
              step={1}
              type="range"
              value={Math.round(settings.scale * 100)}
              onChange={(event) => updateSetting('scale', Number(event.target.value) / 100)}
            />
            <span className="settings-value">{Math.round(settings.scale * 100)}%</span>
          </div>
        </div>

        <div className="settings-row">
          <div>
            <strong>Text scale</strong>
          </div>
          <div className="slider-control">
            <input
              max={180}
              min={45}
              step={1}
              type="range"
              value={Math.round(settings.fontScale * 100)}
              onChange={(event) => updateSetting('fontScale', Number(event.target.value) / 100)}
            />
            <span className="settings-value">{Math.round(settings.fontScale * 100)}%</span>
          </div>
        </div>

        <div className="settings-row">
          <div>
            <strong>Overlay position</strong>
          </div>
          <div className="number-grid">
            <label className="number-field">
              <span>X</span>
              <input
                type="number"
                value={settings.offsetX}
                onChange={(event) => updateNumericSetting('offsetX', event.target.value)}
              />
            </label>
            <label className="number-field">
              <span>Y</span>
              <input
                type="number"
                value={settings.offsetY}
                onChange={(event) => updateNumericSetting('offsetY', event.target.value)}
              />
            </label>
          </div>
        </div>

        <div className="settings-row">
          <div>
            <strong>Overlay size</strong>
          </div>
          <div className="number-grid">
            <label className="number-field">
              <span>Width</span>
              <input
                type="number"
                value={settings.width}
                onChange={(event) => updateNumericSetting('width', event.target.value)}
              />
            </label>
            <label className="number-field">
              <span>Height</span>
              <input
                type="number"
                value={settings.height}
                onChange={(event) => updateNumericSetting('height', event.target.value)}
              />
            </label>
          </div>
        </div>

        <div className="settings-row settings-row--stacked">
          <div>
            <strong>Shape</strong>
          </div>
          <div className="number-grid">
            <label className="number-field">
              <span>Padding</span>
              <input
                min={0}
                type="number"
                value={settings.padding}
                onChange={(event) => updateNumericSetting('padding', event.target.value)}
              />
            </label>
            <label className="number-field">
              <span>Radius</span>
              <input
                min={0}
                type="number"
                value={settings.cornerRadius}
                onChange={(event) => updateNumericSetting('cornerRadius', event.target.value)}
              />
            </label>
          </div>
          <button
            className="toggle-button"
            type="button"
            onClick={() => updateSetting('showBackground', !settings.showBackground)}
          >
            {settings.showBackground ? 'Background on' : 'Background off'}
          </button>
        </div>

        <div className="settings-row settings-row--stacked">
          <div>
            <strong>Visible metrics</strong>
          </div>
          <div className="metric-toggle-grid">
            <button
              className={`metric-toggle ${settings.showPp ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showPp', !settings.showPp)}
            >
              PP
            </button>
            <button
              className={`metric-toggle ${settings.showIfFc ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showIfFc', !settings.showIfFc)}
            >
              If FC
            </button>
            <button
              className={`metric-toggle ${settings.showAccuracy ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showAccuracy', !settings.showAccuracy)}
            >
              Accuracy
            </button>
            <button
              className={`metric-toggle ${settings.showCombo ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showCombo', !settings.showCombo)}
            >
              Combo
            </button>
            <button
              className={`metric-toggle ${settings.showMods ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showMods', !settings.showMods)}
            >
              Mods
            </button>
            <button
              className={`metric-toggle ${settings.showMap ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showMap', !settings.showMap)}
            >
              Map
            </button>
            <button
              className={`metric-toggle ${settings.showHits ? 'metric-toggle--active' : ''}`}
              type="button"
              onClick={() => updateSetting('showHits', !settings.showHits)}
            >
              Hit counts
            </button>
          </div>
        </div>

        <div className="settings-row">
          <div>
            <strong>Current source</strong>
            <span className="settings-value">{session ? session.live.gameState : 'Idle'}</span>
          </div>
          <span className="settings-value">{session ? session.live.modsText : 'Idle'}</span>
        </div>
      </section>
    </section>
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
            <strong>Polling interval</strong>
          </div>
          <span className="settings-value">120 ms</span>
        </div>

        <div className="settings-row">
          <div>
            <strong>Saved history</strong>
          </div>
          <span className="settings-value">{recentPlayCount} / 5</span>
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
