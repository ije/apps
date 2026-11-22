import '@/components/ui/device'
import '@/components/ui/display'
import '@/components/ui/button'
import '@/components/ui/checkbox'
import '@/components/ui/icon'
import '@/components/ui/flat-knob'
import '@/components/ui/slide-switch'
import photo from '@/photos/fjord.jpg'
import { save } from '@/lib/save'

const presets: Record<string, string> = {
  None: '',
  Mono: 'grayscale(1) contrast(1.1)',
  Warm: 'sepia(0.35) saturate(1.2) hue-rotate(-8deg)',
  Cool: 'saturate(0.9) hue-rotate(14deg) brightness(1.03)',
  Fade: 'contrast(0.82) brightness(1.08) saturate(0.75)',
}
const aspects: Record<string, number | null> = { Free: null, '1:1': 1, '4:3': 4 / 3, '3:2': 3 / 2, '16:9': 16 / 9 }
const knobs = [
  ['exposure', 'Exp'],
  ['contrast', 'Con'],
  ['saturation', 'Sat'],
  ['warmth', 'Warm'],
] as const

type Box = { x: number; y: number; w: number; h: number }

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-device model="IX-1" name="Photo" on>
      <spec-display aspect="4 / 3" label="Original">
        <div data-editor-screen>
          <img src="${photo}" alt="A fjord seen from a high granite cliff" draggable="false">
        </div>
      </spec-display>
      <div data-row data-editor-knobs>
        ${knobs.map(([key, label]) => `<spec-flat-knob data-key="${key}" label="${label}" color="white" default-value="50"></spec-flat-knob>`).join('')}
      </div>
      <div data-editor-deck>
        <div data-editor-switches>
          <section data-section data-named aria-label="Look">
            <spec-slide-switch data-look label="Look" side="start" default-value="None">${Object.keys(presets).map((name) => `<option>${name}</option>`).join('')}</spec-slide-switch>
          </section>
          <section data-section data-named aria-label="Aspect">
            <spec-slide-switch data-aspect label="Aspect" default-value="4:3">${Object.keys(aspects).map((name) => `<option>${name}</option>`).join('')}</spec-slide-switch>
          </section>
        </div>
        <div data-editor-keys>
          <spec-button aria-label="Rotate" data-rotate><spec-icon name="rotate"></spec-icon></spec-button>
          <spec-button aria-label="Flip" data-flip><spec-icon name="flip"></spec-icon></spec-button>
          <spec-checkbox label="Crop"></spec-checkbox>
          <spec-button tone="primary" aria-label="Save" data-save><spec-icon name="save"></spec-icon></spec-button>
        </div>
      </div>
    </spec-device>`

  const screen = root.querySelector<HTMLElement>('[data-editor-screen]')!
  const img = screen.querySelector('img')!
  const caption = root.querySelector('[data-kit="display-label"]')!
  const frame = document.createElement('div')
  frame.dataset.crop = ''
  frame.role = 'group'
  frame.ariaLabel = 'Crop frame, drag to move, drag a corner to resize'
  frame.innerHTML = ['nw', 'ne', 'sw', 'se'].map((corner) => `<i data-crop-cap="${corner}"></i>`).join('')

  const knob = { exposure: 50, contrast: 50, saturation: 50, warmth: 50 }
  let preset = 'None'
  let turns = 0
  let flipped = false
  let cropping = false
  let aspect = '4:3'
  let crop: Box = { x: 0.1, y: 0.1, w: 0.8, h: 0.8 }
  let filter = ''
  let drag: { x: number; y: number; from: Box; corner: string | null } | null = null

  const scale = (v: number, span: number) => 1 + ((v - 50) / 50) * span
  const look = () => {
    const { exposure, contrast, saturation, warmth } = knob
    filter = [
      presets[preset],
      `brightness(${scale(exposure, 0.5)})`,
      `contrast(${scale(contrast, 0.5)})`,
      `saturate(${scale(saturation, 1)})`,
      warmth >= 50 ? `sepia(${((warmth - 50) / 50) * 0.5})` : `hue-rotate(${((50 - warmth) / 50) * 25}deg)`,
    ]
      .filter(Boolean)
      .join(' ')
    img.style.filter = filter
  }
  const turn = () => {
    img.style.transform = `rotate(${turns * 90}deg) scale(${turns % 2 === 1 ? 4 / 3 : 1}) scaleX(${flipped ? -1 : 1})`
  }
  const label = () => {
    caption.textContent = cropping ? `Crop ${aspect}` : preset === 'None' ? 'Original' : preset
  }
  const place = (next: Box) => {
    crop = next
    Object.assign(frame.style, { left: `${next.x * 100}%`, top: `${next.y * 100}%`, width: `${next.w * 100}%`, height: `${next.h * 100}%` })
  }
  const fit = () => {
    const ratio = aspects[aspect]
    const box = screen.getBoundingClientRect()
    if (!ratio) return place({ x: 0.1, y: 0.1, w: 0.8, h: 0.8 })
    let w = 0.8
    let h = (w * box.width) / ratio / box.height
    if (h > 0.8) {
      h = 0.8
      w = (h * box.height * ratio) / box.width
    }
    place({ x: (1 - w) / 2, y: (1 - h) / 2, w, h })
  }
  look()
  turn()
  fit()

  for (const el of root.querySelectorAll<HTMLElement>('spec-flat-knob')) {
    el.addEventListener('valuechange', (event) => {
      knob[el.dataset.key as keyof typeof knob] = (event as CustomEvent<{ value: number }>).detail.value
      look()
    })
  }
  root.querySelector('[data-look]')!.addEventListener('valuechange', (event) => {
    preset = (event as CustomEvent<{ value: string }>).detail.value
    look()
    label()
  })
  root.querySelector('[data-aspect]')!.addEventListener('valuechange', (event) => {
    aspect = (event as CustomEvent<{ value: string }>).detail.value
    fit()
    label()
  })
  root.querySelector('[data-rotate]')!.addEventListener('click', () => {
    turns = (turns + 1) % 4
    turn()
  })
  root.querySelector('[data-flip]')!.addEventListener('click', () => {
    flipped = !flipped
    turn()
  })
  root.querySelector('spec-checkbox')!.addEventListener('checkedchange', (event) => {
    cropping = (event as CustomEvent<{ value: boolean }>).detail.value
    if (cropping) screen.append(frame)
    else frame.remove()
    label()
  })

  frame.addEventListener('pointerdown', (event) => {
    frame.setPointerCapture(event.pointerId)
    drag = { x: event.clientX, y: event.clientY, from: crop, corner: (event.target as HTMLElement).dataset.cropCap ?? null }
  })
  frame.addEventListener('pointermove', (event) => {
    if (!drag) return
    const box = screen.getBoundingClientRect()
    const { from, corner } = drag
    if (!corner) {
      return place({
        ...from,
        x: Math.min(1 - from.w, Math.max(0, from.x + (event.clientX - drag.x) / box.width)),
        y: Math.min(1 - from.h, Math.max(0, from.y + (event.clientY - drag.y) / box.height)),
      })
    }
    const west = corner.includes('w')
    const north = corner.includes('n')
    const ax = west ? from.x + from.w : from.x
    const ay = north ? from.y + from.h : from.y
    let w = Math.max(0.12, from.w + ((event.clientX - drag.x) / box.width) * (west ? -1 : 1))
    let h = Math.max(0.12, from.h + ((event.clientY - drag.y) / box.height) * (north ? -1 : 1))
    w = Math.min(w, west ? ax : 1 - ax)
    h = Math.min(h, north ? ay : 1 - ay)
    const ratio = aspects[aspect]
    if (ratio) {
      h = (w * box.width) / ratio / box.height
      const room = north ? ay : 1 - ay
      if (h > room) {
        h = room
        w = (h * box.height * ratio) / box.width
      }
    }
    place({ x: west ? ax - w : ax, y: north ? ay - h : ay, w, h })
  })
  frame.addEventListener('pointerup', () => {
    drag = null
  })

  root.querySelector('[data-save]')!.addEventListener('click', async () => {
    const box = screen.getBoundingClientRect()
    const image = new Image()
    image.src = photo
    await image.decode()
    const ratio = 2
    const area = cropping ? crop : { x: 0, y: 0, w: 1, h: 1 }
    const canvas = document.createElement('canvas')
    canvas.width = area.w * box.width * ratio
    canvas.height = area.h * box.height * ratio
    const ctx = canvas.getContext('2d')!
    ctx.scale(ratio, ratio)
    ctx.translate(-area.x * box.width, -area.y * box.height)
    ctx.translate(box.width / 2, box.height / 2)
    ctx.rotate((turns * Math.PI) / 2)
    const s = turns % 2 === 1 ? 4 / 3 : 1
    ctx.scale(s * (flipped ? -1 : 1), s)
    ctx.filter = filter || 'none'
    const cover = Math.max(box.width / image.width, box.height / image.height)
    ctx.drawImage(image, (-image.width * cover) / 2, (-image.height * cover) / 2, image.width * cover, image.height * cover)
    save('photo.png', canvas)
  })
}
