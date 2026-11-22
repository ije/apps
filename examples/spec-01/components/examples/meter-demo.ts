import '@/components/ui/meter'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-meter label="CPU" value="68"></spec-meter>`
}
