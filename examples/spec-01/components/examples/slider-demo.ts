import '@/components/ui/slider'

export default function mount(root: HTMLElement) {
  root.innerHTML = '<spec-slider label="Volume" default-value="64"></spec-slider>'
}
