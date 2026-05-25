const app = document.querySelector('#app')

let settings = {
  enabled: true,
  showPp: true,
  showIfFc: true,
  showAccuracy: true,
  showCombo: true,
  showMods: true,
  showMap: false,
  showHits: true,
  offsetX: 24,
  offsetY: 24,
  opacity: 0.9,
  ppPanel: { enabled: true, showBackground: true, x: 0, y: 0, width: 106, height: 34 },
  statsPanel: { enabled: true, showBackground: true, x: 112, y: 0, width: 168, height: 34 },
  hitsPanel: { enabled: true, showBackground: true, x: 0, y: 38, width: 280, height: 24 },
  mapPanel: { enabled: false, showBackground: true, x: 0, y: 66, width: 360, height: 24 },
}
let snapshot = null
let editor = false
let selected = 'ppPanel'
let drag = null
let lastLocalEditAt = 0
let renderQueued = false

const panelMeta = {
  ppPanel: { label: 'PP' },
  statsPanel: { label: 'Stats' },
  hitsPanel: { label: 'Hits' },
  mapPanel: { label: 'Map' },
}

const visible = (key) => {
  if (!settings?.[key]?.enabled) return false
  if (key === 'ppPanel') return settings.showPp
  if (key === 'statsPanel') return settings.showIfFc || settings.showAccuracy || settings.showCombo || settings.showMods
  if (key === 'hitsPanel') return settings.showHits
  if (key === 'mapPanel') return settings.showMap
  return true
}

const clamp = (value, min, max) => Math.max(min, Math.min(max, value))
const now = () => performance.now()

const emit = () => {
  lastLocalEditAt = now()
  window.osuwappOverlay.updateSettings(settings)
}

const updatePanel = (key, patch) => {
  settings = { ...settings, [key]: { ...settings[key], ...patch } }
  emit()
  render()
}

const updateRoot = (patch) => {
  settings = { ...settings, ...patch }
  emit()
  render()
}

const visiblePanelBounds = () => {
  const panels = Object.keys(panelMeta)
    .filter((key) => visible(key))
    .map((key) => settings[key])

  if (panels.length === 0) {
    return { left: 0, top: 0, right: 1, bottom: 1 }
  }

  return panels.reduce(
    (bounds, panel) => ({
      left: Math.min(bounds.left, panel.x),
      top: Math.min(bounds.top, panel.y),
      right: Math.max(bounds.right, panel.x + panel.width),
      bottom: Math.max(bounds.bottom, panel.y + panel.height),
    }),
    { left: Infinity, top: Infinity, right: -Infinity, bottom: -Infinity },
  )
}

const viewportOffset = () => {
  const bounds = visiblePanelBounds()
  const margin = 4
  const minX = margin - bounds.left
  const minY = margin - bounds.top
  const maxX = window.innerWidth - margin - bounds.right
  const maxY = window.innerHeight - margin - bounds.bottom

  return {
    x: Math.max(minX, Math.min(maxX, settings.offsetX)),
    y: Math.max(minY, Math.min(maxY, settings.offsetY)),
  }
}

const panelStyle = (panel) => [
  `left:${viewportOffset().x + panel.x}px`,
  `top:${viewportOffset().y + panel.y}px`,
  `width:${panel.width}px`,
  `height:${panel.height}px`,
  `--alpha:${Math.max(.05, Math.min(1, settings.opacity))}`,
  `--radius:${Math.max(0, Math.min(32, settings.cornerRadius ?? 10))}px`,
  `--panel-scale:${Math.max(.2, Math.min(2.5, panel.scale ?? 1))}`,
  `--font-scale:${Math.max(.35, Math.min(2, panel.fontScale ?? 1))}`,
].join(';')

const session = () => snapshot?.session
const fmt = (value, digits = 2) => Number(value || 0).toFixed(digits)
const acc = () => session()?.live?.accuracy == null ? '--' : `${fmt(session().live.accuracy)}%`

const panelHtml = (key) => {
  const item = settings[key]
  if (!visible(key)) return ''
  const selectedClass = editor && selected === key ? ' selected' : ''
  const attrs = `class="hud-panel ${key.replace('Panel', '')}${selectedClass}" data-key="${key}" data-bg="${item.showBackground}" style="${panelStyle(item)}"`
  const data = session()
  if (!data) return `<div ${attrs}><div class="pp"><strong>Waiting for osu!</strong></div></div>`
  if (key === 'ppPanel') return `<div ${attrs}><strong>${fmt(data.pp.current)}</strong><span>PP</span></div>`
  if (key === 'statsPanel') {
    const cells = [
      settings.showIfFc && ['IF FC', fmt(data.pp.ifFc)],
      settings.showAccuracy && ['ACC', acc()],
      settings.showCombo && ['COMBO', `${data.live.combo}x`],
      settings.showMods && ['MODS', data.live.modsText || 'NM'],
    ].filter(Boolean)
    return `<div ${attrs}>${cells.map(([l, v]) => `<div class="cell"><span>${l}</span><strong>${v}</strong></div>`).join('')}</div>`
  }
  if (key === 'hitsPanel') {
    const h = data.live.hits
    return `<div ${attrs}>
      <div class="cell hit-blue"><span>100</span><strong>${h.n100}</strong></div>
      <div class="cell hit-orange"><span>50</span><strong>${h.n50}</strong></div>
      <div class="cell hit-red"><span>MISS</span><strong>${h.misses}</strong></div>
      <div class="cell hit-purple"><span>SB</span><strong>${h.sliderBreaks}</strong></div>
    </div>`
  }
  return `<div ${attrs}>${data.beatmap.artist} - ${data.beatmap.title} [${data.beatmap.difficultyName}]</div>`
}

const editorHtml = () => {
  if (!editor || !settings) return ''
  const item = settings[selected]
  return `<div class="editor-bar">
    <div class="editor-title"><strong>Osuwapp overlay</strong><span>Insert closes editor</span></div>
    <div class="seg">
      ${Object.entries(panelMeta).map(([key, meta]) => `<button data-select="${key}" class="${selected === key ? 'active' : ''}">${meta.label}</button>`).join('')}
    </div>
    <label>X<input data-field="x" value="${item.x}" /></label>
    <label>Y<input data-field="y" value="${item.y}" /></label>
    <label>W<input data-field="width" value="${item.width}" /></label>
    <label>H<input data-field="height" value="${item.height}" /></label>
    <label>Scale<input data-field="scale" value="${Math.round((item.scale ?? 1) * 100)}" /></label>
    <label>Font<input data-field="fontScale" value="${Math.round((item.fontScale ?? 1) * 100)}" /></label>
    <label>Opacity<input data-root="opacity" value="${Math.round(settings.opacity * 100)}" /></label>
    <label>Radius<input data-root="cornerRadius" value="${settings.cornerRadius ?? 10}" /></label>
    <label class="switch"><input type="checkbox" data-field-check="showBackground" ${item.showBackground ? 'checked' : ''} /><span>BG</span></label>
    <label class="switch"><input type="checkbox" data-field-check="enabled" ${item.enabled ? 'checked' : ''} /><span>On</span></label>
  </div>`
}

const render = () => {
  if (!settings) return
  app.innerHTML = `${Object.keys(panelMeta).map(panelHtml).join('')}${editorHtml()}`
  document.body.dataset.rendered = 'true'
}

const scheduleRender = () => {
  if (renderQueued) return
  renderQueued = true
  requestAnimationFrame(() => {
    renderQueued = false
    render()
  })
}

app.addEventListener('pointerdown', (event) => {
  const panel = event.target.closest('.hud-panel')
  if (!editor || !panel) return
  const key = panel.dataset.key
  selected = key
  const item = settings[key]
  const rect = panel.getBoundingClientRect()
  drag = {
    key,
    startX: event.clientX,
    startY: event.clientY,
    x: item.x,
    y: item.y,
    width: item.width,
    height: item.height,
    resize: event.clientX > rect.right - 14 || event.clientY > rect.bottom - 14,
  }
  panel.setPointerCapture(event.pointerId)
  render()
})

app.addEventListener('pointermove', (event) => {
  if (!drag) return
  const dx = Math.round(event.clientX - drag.startX)
  const dy = Math.round(event.clientY - drag.startY)
  if (drag.resize) {
    updatePanel(drag.key, { width: Math.max(24, drag.width + dx), height: Math.max(14, drag.height + dy) })
  } else {
    updatePanel(drag.key, { x: drag.x + dx, y: drag.y + dy })
  }
})

app.addEventListener('pointerup', () => { drag = null })

app.addEventListener('click', (event) => {
  const select = event.target.closest('[data-select]')?.dataset.select
  if (select) {
    selected = select
    render()
  }
})

app.addEventListener('change', (event) => {
  const input = event.target
  if (!(input instanceof HTMLInputElement)) return
  if (input.dataset.fieldCheck) {
    updatePanel(selected, { [input.dataset.fieldCheck]: input.checked })
    return
  }
  const value = Number(input.value)
  if (!Number.isFinite(value)) return
  if (input.dataset.field === 'scale' || input.dataset.field === 'fontScale') {
    updatePanel(selected, { [input.dataset.field]: clamp(value, 20, 250) / 100 })
    return
  }
  if (input.dataset.field) updatePanel(selected, { [input.dataset.field]: Math.round(value) })
  if (input.dataset.root === 'opacity') {
    updateRoot({ opacity: clamp(value, 5, 100) / 100 })
  }
  if (input.dataset.root === 'cornerRadius') {
    updateRoot({ cornerRadius: Math.round(clamp(value, 0, 32)) })
  }
})

window.osuwappOverlay.onState((payload) => {
  if (editor && now() - lastLocalEditAt < 180) {
    snapshot = payload.snapshot
    scheduleRender()
    return
  }
  settings = payload.settings
  snapshot = payload.snapshot
  scheduleRender()
})

window.osuwappOverlay.onEditor((active) => {
  editor = active
  scheduleRender()
})

window.osuwappOverlay.onResize(scheduleRender)

render()
