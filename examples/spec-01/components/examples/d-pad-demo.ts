import '@/components/ui/dpad'

const moves: Record<string, [number, number]> = { up: [0, -1], down: [0, 1], left: [-1, 0], right: [1, 0] }

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-dpad-demo>
      <div data-dpad-grid aria-hidden="true">${'<i></i>'.repeat(15)}</div>
      <spec-dpad label="Move"></spec-dpad>
    </div>`
  const dots = root.querySelectorAll('i')
  const pad = root.querySelector('spec-dpad')!
  let [x, y] = [2, 1]
  dots[7].dataset.on = ''
  for (const [dir, [dx, dy]] of Object.entries(moves))
    pad.addEventListener(dir, () => {
      delete dots[y * 5 + x].dataset.on
      x = (x + dx + 5) % 5
      y = (y + dy + 3) % 3
      dots[y * 5 + x].dataset.on = ''
    })
}
