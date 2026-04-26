export type ConnectionStatus = 'searching' | 'connected' | 'error'
export type SessionPhase = 'preview' | 'playing' | 'result'

export interface AppSnapshot {
  connection: {
    status: ConnectionStatus
    detail: string
    updatedAtMs: number
  }
  session: SessionSnapshot | null
  recentPlays: RecentPlaySnapshot[]
}

export interface OverlaySettings {
  enabled: boolean
  showPp: boolean
  showIfFc: boolean
  showAccuracy: boolean
  showCombo: boolean
  showMods: boolean
  showMap: boolean
  showHits: boolean
  width: number
  height: number
  offsetX: number
  offsetY: number
  scale: number
  fontScale: number
  padding: number
  cornerRadius: number
  opacity: number
  showBackground: boolean
  toggleKey: string
}

export interface SessionSnapshot {
  phase: SessionPhase
  beatmap: {
    artist: string
    title: string
    difficultyName: string
    creator: string
    status: string
    mode: string
    path: string
    coverPath: string | null
    lengthMs: number
    objectCount: number
    starRating: number
    ar: number
    od: number
    cs: number
    hp: number
    bpm: number | null
    mods: string[]
  }
  live: {
    username: string | null
    gameState: string
    accuracy: number | null
    combo: number
    maxCombo: number
    score: number
    misses: number
    retries: number
    hp: number | null
    progress: number
    passedObjects: number
    modsText: string
    hits: {
      nGeki: number
      nKatu: number
      n300: number
      n100: number
      n50: number
      misses: number
      sliderBreaks: number
    }
  }
  pp: {
    current: number
    ifFc: number
    fullMap: number
    calculator: string
    difficultyAdjust: number
    modsMultiplier: number
    components: Array<{
      label: string
      value: number
    }>
  }
}

export interface RecentPlaySnapshot {
  timestampMs: number
  title: string
  modsText: string
  accuracy: number
  combo: number
  pp: number
}
