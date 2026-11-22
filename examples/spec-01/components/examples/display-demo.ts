import '@/components/ui/display'
import highlands from '@/photos/highlands.jpg'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-display label="Highlands" aspect="21 / 9">
      <img src="${highlands}" alt="A road winding through green highlands under low cloud" draggable="false">
    </spec-display>`
}
