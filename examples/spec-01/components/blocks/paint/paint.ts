import '@/components/ui/alert-dialog'
import '@/components/ui/button'
import '@/components/ui/device'
import '@/components/ui/display'
import '@/components/ui/flat-knob'
import '@/components/ui/icon'
import '@/components/ui/mode-dial'
import { save } from '@/lib/save'

const ink = '#f4f4f0'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-device model="PX-1" name="Paint" plate="none">
      <spec-display aspect="4 / 3"><canvas data-paint-canvas data-tool="Pen"></canvas></spec-display>
      <spec-device-plate model="PX-1" name="Paint" under></spec-device-plate>
      <div data-paint-deck>
        <section data-section data-named aria-label="Tool">
          <spec-mode-dial label="Tool" sweep="150" default-value="Pen">${['Pen', 'Brush', 'Marker', 'Erase'].map((t) => `<option value="${t}">${t}</option>`).join('')}</spec-mode-dial>
        </section>
        <div data-paint-controls>
          <spec-flat-knob label="Size" color="white" default-value="30"></spec-flat-knob>
          <spec-flat-knob label="Flow" color="white" default-value="90"></spec-flat-knob>
          <div data-row>
            <spec-button disabled aria-label="Undo"><spec-icon name="undo"></spec-icon></spec-button>
            <spec-alert-dialog label="Clear canvas" heading="Clear the canvas?" description="Every stroke is removed. Undo brings it back." confirm="Clear"><spec-icon name="trash"></spec-icon></spec-alert-dialog>
            <spec-button tone="primary" aria-label="Save"><spec-icon name="save"></spec-icon></spec-button>
          </div>
        </div>
      </div>
    </spec-device>`

  const el = root.querySelector<HTMLCanvasElement>('[data-paint-canvas]')!
  const undoKey = root.querySelector<HTMLButtonElement>('[aria-label="Undo"]')!
  const [sizeKnob, flowKnob] = root.querySelectorAll('spec-flat-knob')
  let tool = 'Pen'
  let size = 30
  let flow = 90
  let before: ImageData | null = null
  let history: ImageData[] = []
  let stroke: { x: number; y: number } | null = null

  const ratio = devicePixelRatio || 1
  el.width = el.clientWidth * ratio
  el.height = el.clientHeight * ratio
  const context = el.getContext('2d')!
  context.scale(ratio, ratio)
  const layer = document.createElement('canvas')
  layer.width = el.width
  layer.height = el.height
  const pen = layer.getContext('2d')!
  pen.scale(ratio, ratio)
  pen.lineJoin = 'round'

  const remember = () => {
    history = [...history.slice(-29), context.getImageData(0, 0, el.width, el.height)]
    undoKey.disabled = false
  }

  const point = (event: PointerEvent) => {
    const box = el.getBoundingClientRect()
    const scale = box.width ? el.clientWidth / box.width : 1
    return { x: (event.clientX - box.left) * scale, y: (event.clientY - box.top) * scale, pressure: event.pressure || 0.5 }
  }

  const style = (pressure: number) => {
    const width = 1 + (size / 100) * 40
    pen.strokeStyle = ink
    pen.shadowColor = ink
    pen.shadowBlur = tool === 'Brush' ? width * 0.6 : 0
    pen.lineCap = 'round'
    pen.lineWidth = tool === 'Pen' ? Math.max(1, width * 0.35 * (0.5 + pressure)) : tool === 'Erase' ? width * 1.5 : width
  }

  const lay = () => {
    context.putImageData(before!, 0, 0)
    context.save()
    context.setTransform(1, 0, 0, 1, 0, 0)
    context.globalAlpha = tool === 'Erase' ? 1 : tool === 'Marker' ? 0.45 * (flow / 100) : flow / 100
    context.globalCompositeOperation = tool === 'Erase' ? 'destination-out' : 'source-over'
    context.drawImage(layer, 0, 0)
    context.restore()
  }

  el.addEventListener('pointerdown', (event) => {
    el.setPointerCapture(event.pointerId)
    const p = (stroke = point(event))
    before = context.getImageData(0, 0, el.width, el.height)
    pen.save()
    pen.setTransform(1, 0, 0, 1, 0, 0)
    pen.clearRect(0, 0, el.width, el.height)
    pen.restore()
    style(p.pressure)
    pen.beginPath()
    pen.moveTo(p.x, p.y)
    pen.lineTo(p.x + 0.01, p.y)
    pen.stroke()
    lay()
  })
  el.addEventListener('pointermove', (event) => {
    const last = stroke
    if (!last) return
    const p = point(event)
    style(p.pressure)
    pen.beginPath()
    pen.moveTo(last.x, last.y)
    pen.quadraticCurveTo(last.x, last.y, (last.x + p.x) / 2, (last.y + p.y) / 2)
    pen.lineTo(p.x, p.y)
    pen.stroke()
    lay()
    stroke = p
  })
  el.addEventListener('pointerup', () => {
    if (!stroke) return
    stroke = null
    remember()
  })

  root.querySelector('spec-mode-dial')!.addEventListener('valuechange', (event) => {
    el.dataset.tool = tool = (event as CustomEvent).detail.value
  })
  sizeKnob.addEventListener('valuechange', (event) => (size = (event as CustomEvent).detail.value))
  flowKnob.addEventListener('valuechange', (event) => (flow = (event as CustomEvent).detail.value))

  undoKey.addEventListener('click', () => {
    history.pop()
    const last = history.at(-1)
    if (last) context.putImageData(last, 0, 0)
    else context.clearRect(0, 0, el.width, el.height)
    undoKey.disabled = !history.length
  })

  root.querySelector('spec-alert-dialog')!.addEventListener('confirm', () => {
    context.clearRect(0, 0, el.width, el.height)
    remember()
  })

  root.querySelector('[aria-label="Save"]')!.addEventListener('click', () => {
    const flat = document.createElement('canvas')
    flat.width = el.width
    flat.height = el.height
    const ctx = flat.getContext('2d')!
    ctx.fillStyle = '#0b0b0a'
    ctx.fillRect(0, 0, flat.width, flat.height)
    ctx.drawImage(el, 0, 0)
    save('sketch.png', flat)
  })
}
