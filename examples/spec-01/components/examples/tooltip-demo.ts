import '@/components/ui/icon'
import '@/components/ui/tooltip'

export default function mount(root: HTMLElement) {
  root.innerHTML = `<spec-tooltip label="Tap tempo"><spec-icon name="info"></spec-icon></spec-tooltip>`
}
