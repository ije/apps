import '@/components/ui/button-group'
import '@/components/ui/checkbox'
import '@/components/ui/display'
import '@/components/ui/flat-knob'
import '@/components/ui/icon'
import '@/components/ui/item-list'
import '@/components/ui/lcd'
import '@/components/ui/slider'
import { clock } from '@/components/ui/device'
import { chapters, filmLength, frame, ready } from '@/components/blocks/music-player/film'
import highlands from '@/photos/highlands.jpg'
import lioness from '@/photos/lioness.jpg'
import peaks from '@/photos/peaks.jpg'
import strawberries from '@/photos/strawberries.jpg'
import walrus from '@/photos/walrus.jpg'

type Item = { kind: 'audio' | 'video'; title: string; artist: string; length: number; art?: string }

const items: Item[] = [
  { kind: 'audio', title: 'Never Gonna Give You Up', artist: 'Rick Astley', length: 213, art: highlands },
  { kind: 'audio', title: 'Low Orbit', artist: 'Halcyon Unit', length: 214, art: strawberries },
  { kind: 'video', title: 'Field Notes', artist: 'Short film', length: filmLength },
  { kind: 'audio', title: 'Patch Bay', artist: 'Moiré', length: 187, art: lioness },
  { kind: 'audio', title: 'Tape Stop', artist: 'Kit & Cable', length: 242, art: walrus },
  { kind: 'audio', title: 'Detent', artist: 'Oscillo', length: 231, art: peaks },
]

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-device model="MP-1" name="Player" on="false">
      <div data-player>
        <div data-player-screen>
          <spec-display aspect="1 / 1">
            <div data-art><div data-spectrum aria-hidden="true">${Array.from({ length: 16 }, (_, i) => `<i style="--i: ${i}"></i>`).join('')}</div></div>
          </spec-display>
        </div>
        <spec-lcd data-player-now><strong></strong><spec-lcd-digits value="0:00" ghost="8:88"></spec-lcd-digits></spec-lcd>
        <div data-player-cell data-wide>
          <spec-button-group label="Transport" tone="primary">
            <button aria-label="Previous"><spec-icon name="skip-back"></spec-icon></button>
            <button aria-label="Play"><spec-icon name="play"></spec-icon></button>
            <button aria-label="Next"><spec-icon name="skip-forward"></spec-icon></button>
          </spec-button-group>
        </div>
        <div data-player-cell><spec-checkbox label="Shuffle" icon="shuffle"></spec-checkbox><small aria-hidden="true">Shuf</small></div>
        <div data-player-cell><spec-checkbox label="Repeat" icon="repeat" default-checked></spec-checkbox><small aria-hidden="true">Rpt</small></div>
        <div data-player-cell><spec-flat-knob label="Volume" color="white" default-value="70"></spec-flat-knob><small aria-hidden="true">Vol</small></div>
        <div data-player-scrub><spec-slider label="Position" value="0" min="0" max="${items[0].length}" step="0.5"></spec-slider></div>
      </div>
      <spec-item-list label="Queue"></spec-item-list>
    </spec-device>`
  const device = root.firstElementChild!
  const screen = root.querySelector('[data-kit="display-screen"]')!
  const art = screen.firstElementChild as HTMLElement
  const spectrum = art.firstElementChild!
  const film = document.createElement('canvas')
  film.setAttribute('role', 'img')
  const lcd = root.querySelector('spec-lcd')!
  const title = lcd.querySelector('strong')!
  const digits = lcd.querySelector('spec-lcd-digits')!
  const play = root.querySelector<HTMLButtonElement>('[aria-label="Play"]')!
  const slider = root.querySelector<HTMLElement & { value: number }>('spec-slider')!
  const list = root.querySelector('spec-item-list')!

  let index = 0
  let time = 0
  let playing = false
  let live = true
  let id = 0
  let shown = '0:00'
  let chapter = ''

  const draw = () => {
    const ratio = devicePixelRatio || 1
    if (film.width !== film.clientWidth * ratio) {
      film.width = film.clientWidth * ratio
      film.height = film.clientHeight * ratio
    }
    const ctx = film.getContext('2d')!
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0)
    frame(ctx, film.clientWidth, film.clientHeight, time)
    const now = chapters.findLast((c) => time >= c.start)!.title
    if (now !== chapter) film.ariaLabel = `Film, ${(chapter = now)}`
  }

  const show = () => {
    const item = items[index]
    lcd.setAttribute('tag', `${String(index + 1).padStart(2, '0')} · ${item.kind === 'video' ? 'Video' : 'Audio'}`)
    title.textContent = item.title
    slider.setAttribute('max', String(item.length))
    list.value = item.title
    if (item.kind === 'video') screen.replaceChildren(film)
    else {
      art.style.background = `center / cover no-repeat url(${item.art})`
      if (film.isConnected) screen.replaceChildren(art)
    }
  }

  const seek = (next: number) => {
    time = next
    if (time >= items[index].length) {
      index = (index + 1) % items.length
      time = 0
      show()
    }
    const text = clock(time)
    if (text !== shown) digits.setAttribute('value', (shown = text))
    slider.value = time
    list.progress = time / items[index].length
    if (film.isConnected) draw()
  }

  const go = (step: number) => {
    index = (index + step + items.length) % items.length
    show()
    seek(0)
  }

  const run = (next: boolean) => {
    if (next === playing) return
    playing = next
    device.setAttribute('on', String(playing))
    play.ariaLabel = playing ? 'Pause' : 'Play'
    play.firstElementChild!.setAttribute('name', playing ? 'pause' : 'play')
    spectrum.toggleAttribute('data-playing', playing)
    cancelAnimationFrame(id)
    if (!playing) return
    let last = performance.now()
    id = requestAnimationFrame(function tick(now) {
      // WARNING: the first frame time can be before the start, and a negative time has no film chapter.
      seek(time + Math.max(0, now - last) / 1000)
      last = now
      id = requestAnimationFrame(tick)
    })
  }

  root.querySelector('spec-button-group')!.addEventListener('press', (event) => {
    const part = (event as CustomEvent).detail.value
    if (part === 1) run(!playing)
    else go(part === 0 ? -1 : 1)
  })
  slider.addEventListener('valuechange', (event) => {
    const from = index
    seek((event as CustomEvent).detail.value)
    // WARNING: the slider takes the dragged value after this event, so the start of the next item is set after it.
    if (index !== from) queueMicrotask(() => (slider.value = time))
  })
  list.addEventListener('valuechange', (event) => {
    index = items.findIndex((entry) => entry.title === (event as CustomEvent).detail.value)
    show()
    seek(0)
    run(true)
  })
  list.rowKey = (entry: Item) => entry.title
  list.columns = [
    { key: 'n', label: 'No', width: '22px', render: (entry: Item) => String(items.indexOf(entry) + 1).padStart(2, '0') },
    { key: 'title', label: 'Title', width: 'minmax(0, 1fr)', render: (entry: Item) => entry.title },
    { key: 'len', label: 'Time', width: '38px', align: 'end', render: (entry: Item) => clock(entry.length) },
  ]
  list.rows = items
  show()
  seek(0)
  void ready.then(() => live && film.isConnected && draw())

  return () => {
    live = false
    cancelAnimationFrame(id)
  }
}
