import '@/components/ui/switch'

export default function mount(root: HTMLElement) {
  root.innerHTML = '<spec-switch label="Power" default-checked></spec-switch>'
}
