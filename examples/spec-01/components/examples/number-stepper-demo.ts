import '@/components/ui/stepper'

export default function mount(root: HTMLElement) {
  root.innerHTML = '<spec-stepper label="Tempo" default-value="120"></spec-stepper>'
}
