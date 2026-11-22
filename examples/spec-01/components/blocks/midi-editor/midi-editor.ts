import { spatialMove } from '@ui-sdk/core'
import '@/components/ui/button'
import '@/components/ui/device'
import '@/components/ui/display'
import '@/components/ui/flat-knob'
import '@/components/ui/icon'
import '@/components/ui/slide-switch'
import '@/components/ui/tabs'

const waves: Record<string, OscillatorType> = { Sine: 'sine', Tri: 'triangle', Sqr: 'square', Saw: 'sawtooth' }

const notes = ['C5', 'B4', 'A#4', 'A4', 'G#4', 'G4', 'F#4', 'F4', 'E4', 'D#4', 'D4', 'C#4', 'C4']
const steps = 16
const names = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']
const frequencies = notes.map((note) => 440 * 2 ** ((names.indexOf(note.slice(0, -1)) + (Number(note.at(-1)) - 4) * 12 - 9) / 12))

const phrases: Record<string, [number, number][]> = {
  A: [[0, 12], [2, 8], [4, 5], [6, 8], [8, 3], [10, 5], [12, 8], [13, 5], [14, 3], [15, 0]],
  B: [[0, 5], [1, 8], [2, 12], [4, 3], [6, 5], [8, 0], [9, 3], [10, 5], [12, 8], [14, 12]],
  C: [[0, 12], [2, 12], [3, 0], [4, 12], [6, 10], [8, 12], [10, 8], [11, 5], [12, 12], [14, 3], [15, 5]],
  D: [[0, 12], [1, 8], [2, 5], [3, 0], [4, 5], [5, 8], [6, 12], [7, 8], [8, 10], [9, 7], [10, 3], [11, 7], [12, 12], [13, 8], [14, 5], [15, 8]],
  E: [[0, 3], [2, 0], [3, 3], [4, 5], [6, 8], [7, 10], [8, 3], [10, 5], [12, 8], [14, 10], [15, 12]],
  F: [[0, 8], [3, 8], [6, 5], [8, 8], [10, 3], [11, 5], [14, 0]],
  G: [[0, 12], [0, 8], [0, 5], [4, 7], [4, 3], [4, 0], [8, 10], [8, 5], [8, 1], [12, 12], [12, 8], [12, 5]],
  H: [[0, 0], [1, 1], [2, 3], [3, 5], [4, 7], [5, 8], [6, 10], [7, 12], [8, 12], [10, 8], [12, 5], [14, 3]],
}
const patterns = Object.keys(phrases)
const pentatonic = notes.map((n, row) => (/^(C|D|E|G|A)\d$/.test(n) ? row : -1)).filter((row) => row >= 0)
const synth = typeof AudioContext !== 'undefined' && 'createOscillator' in AudioContext.prototype

export default function mount(root: HTMLElement) {
  const empty = () => notes.map(() => Array<boolean>(steps).fill(false))
  const bank: Record<string, boolean[][]> = {}
  for (const p of patterns) {
    bank[p] = empty()
    for (const [step, row] of phrases[p]) bank[p][row][step] = true
  }
  let pattern = 'A'
  let playing = false
  let step = -1
  let tempo = 50
  let cutoff = 70
  let swing = 0
  let wave: OscillatorType = 'triangle'
  let timer = 0
  let audio: AudioContext | null = null

  root.innerHTML = `
    <spec-device model="MX-1" name="MIDI" on="false" plate="none">
      <div data-midi-bank>
        <spec-tabs label="Patterns" value="A">
          ${patterns.map((p) => `<section value="${p}" label="${p}">${p === 'A' ? `
            <spec-display aspect="16 / 9">
              <div data-roll role="grid" aria-multiselectable="true" aria-label="Piano roll, pattern A">
                ${notes.map((note, r) => `<div role="row" data-roll-row${note.includes('#') ? ' data-sharp="true"' : ''}><span role="rowheader" data-roll-key>${/^C\d$/.test(note) ? note : ''}</span>${Array.from({ length: steps }, (_, c) => `<button type="button" role="gridcell" data-cell="${r}:${c}" tabindex="${r || c ? -1 : 0}" aria-label="${note}, step ${c + 1}" aria-selected="false"${c % 4 ? '' : ' data-beat="true"'}></button>`).join('')}</div>`).join('')}
                <div data-roll-steps aria-hidden="true"><span></span>${Array.from({ length: steps }, (_, c) => `<i${c % 4 ? '' : ' data-beat="true"'}></i>`).join('')}</div>
              </div>
            </spec-display>` : ''}</section>`).join('')}
        </spec-tabs>
      </div>
      <spec-device-plate model="MX-1" name="MIDI" on="false" under></spec-device-plate>
      <div data-midi-deck>
        <div data-midi-keys>
          <spec-button tone="primary" aria-label="Play"><spec-icon name="play"></spec-icon></spec-button>
          <spec-button aria-label="Random phrase"><spec-icon name="shuffle"></spec-icon></spec-button>
          <spec-button aria-label="Clear"><spec-icon name="trash"></spec-icon></spec-button>
        </div>
        <section data-section data-named aria-label="Wave" data-midi-wave>
          <spec-slide-switch label="Wave" default-value="Tri">${Object.keys(waves).map((w) => `<option>${w}</option>`).join('')}</spec-slide-switch>
        </section>
        <div data-midi-knobs>
          <spec-flat-knob label="Tempo" color="white" value="50"></spec-flat-knob>
          <spec-flat-knob label="Cutoff" color="orange" value="70"></spec-flat-knob>
          <spec-flat-knob label="Swing" color="green" value="0"></spec-flat-knob>
        </div>
      </div>
    </spec-device>`

  const device = root.querySelector('spec-device')!
  const tabs = root.querySelector('spec-tabs')!
  const display = root.querySelector('spec-display')!
  const roll = root.querySelector<HTMLElement>('[data-roll]')!
  const cells = [...roll.querySelectorAll<HTMLButtonElement>('[role="gridcell"]')]
  const leds = [...roll.querySelectorAll('[data-roll-steps] i')]
  const play = root.querySelector<HTMLButtonElement>('button[aria-label="Play"]')!
  const [tempoKnob, cutoffKnob, swingKnob] = root.querySelectorAll('spec-flat-knob')
  let active = cells[0]

  const show = () => {
    const grid = bank[pattern]
    for (let c = 0; c < steps; c++) {
      let filled = false
      for (let r = 0; r < notes.length; r++) {
        const on = grid[r][c]
        const cell = cells[r * steps + c]
        filled ||= on
        if ((cell.getAttribute('aria-selected') === 'true') !== on) cell.setAttribute('aria-selected', String(on))
      }
      leds[c].toggleAttribute('data-filled', filled)
    }
  }
  const mark = (next: number) => {
    for (const c of [step, next]) {
      if (c < 0) continue
      for (let r = 0; r < notes.length; r++) cells[r * steps + c].toggleAttribute('data-now', c === next)
      leds[c].toggleAttribute('data-now', c === next && playing)
    }
    step = next
  }
  show()

  const tick = () => {
    const column = (step + 1) % steps
    const bpm = Math.round(70 + (tempo / 100) * 110)
    const length = 60 / bpm / 4
    mark(column)
    if (audio) {
      const ctx = audio
      const hertz = Math.round(200 * 2 ** ((cutoff / 100) * Math.log2(9000 / 200)))
      bank[pattern].forEach((row, r) => {
        if (!row[column]) return
        const osc = ctx.createOscillator()
        const filter = ctx.createBiquadFilter()
        const gain = ctx.createGain()
        osc.type = wave
        osc.frequency.value = frequencies[r]
        filter.type = 'lowpass'
        filter.Q.value = 7
        filter.frequency.setValueAtTime(Math.min(12000, hertz * 2.2), ctx.currentTime)
        filter.frequency.exponentialRampToValueAtTime(hertz, ctx.currentTime + length * 0.9)
        gain.gain.setValueAtTime(0, ctx.currentTime)
        gain.gain.linearRampToValueAtTime(0.16, ctx.currentTime + 0.01)
        gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + length * 1.6)
        osc.connect(filter).connect(gain).connect(ctx.destination)
        osc.start()
        osc.stop(ctx.currentTime + length * 1.7)
      })
    }
    const shuffle = (swing / 100) * 0.5
    timer = window.setTimeout(tick, length * 1000 * (column % 2 === 0 ? 1 + shuffle : 1 - shuffle))
  }

  tabs.addEventListener('valuechange', (event) => {
    pattern = (event as CustomEvent).detail.value
    roll.setAttribute('aria-label', `Piano roll, pattern ${pattern}`)
    tabs.querySelector(`[data-kit="tab-panel"][value="${pattern}"]`)!.append(display)
    show()
  })

  roll.addEventListener('click', (event) => {
    const cell = (event.target as Element).closest<HTMLElement>('[role="gridcell"]')
    if (!cell) return
    const [r, c] = cell.dataset.cell!.split(':').map(Number)
    bank[pattern][r][c] = !bank[pattern][r][c]
    show()
  })
  roll.addEventListener('focusin', (event) => {
    const cell = event.target as HTMLButtonElement
    if (cell === active || !cell.matches('[role="gridcell"]')) return
    active.tabIndex = -1
    cell.tabIndex = 0
    active = cell
  })
  const move = spatialMove()
  roll.addEventListener('keydown', (event) => {
    const target = event.target as HTMLElement
    if (!target.matches('[role="gridcell"]')) return
    const items = cells.map((el) => ({ key: el.dataset.cell!, props: {}, el }))
    const key = move(event, { from: items.find((item) => item.el === target)!, items, rtl: false })
    const next = key && items.find((item) => item.key === key)
    if (!next) return
    event.preventDefault()
    next.el.focus()
  })

  play.addEventListener('click', () => {
    if (synth) {
      audio ??= new AudioContext()
      void audio.resume()
    }
    playing = !playing
    clearTimeout(timer)
    mark(-1)
    device.setAttribute('on', String(playing))
    play.setAttribute('aria-label', playing ? 'Stop' : 'Play')
    play.querySelector('spec-icon')!.setAttribute('name', playing ? 'stop' : 'play')
    if (playing) tick()
  })
  root.querySelector('button[aria-label="Random phrase"]')!.addEventListener('click', () => {
    const grid = (bank[pattern] = empty())
    let at = pentatonic.length - 1
    for (let s = 0; s < steps; s++) {
      const chance = s === 0 ? 1 : s % 4 === 0 ? 0.8 : s % 2 === 0 ? 0.5 : 0.25
      if (Math.random() > chance) continue
      at = Math.min(pentatonic.length - 1, Math.max(0, at + Math.round((Math.random() - 0.5) * 3.2)))
      grid[pentatonic[at]][s] = true
    }
    show()
  })
  root.querySelector('button[aria-label="Clear"]')!.addEventListener('click', () => {
    bank[pattern] = empty()
    show()
  })
  root.querySelector('spec-slide-switch')!.addEventListener('valuechange', (event) => {
    wave = waves[(event as CustomEvent).detail.value]
  })
  tempoKnob.addEventListener('valuechange', (event) => (tempo = (event as CustomEvent).detail.value))
  cutoffKnob.addEventListener('valuechange', (event) => (cutoff = (event as CustomEvent).detail.value))
  swingKnob.addEventListener('valuechange', (event) => (swing = (event as CustomEvent).detail.value))

  return () => {
    clearTimeout(timer)
    void audio?.close()
  }
}
