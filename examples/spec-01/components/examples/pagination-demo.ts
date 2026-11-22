import '@/components/ui/page-wheel'

export default function mount(root: HTMLElement) {
  root.innerHTML = '<spec-page-wheel page-count="24" default-page="3"></spec-page-wheel>'
}
