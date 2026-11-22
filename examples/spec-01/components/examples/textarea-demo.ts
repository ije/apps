import '@/components/ui/textarea'

export default function mount(root: HTMLElement) {
  root.innerHTML = '<spec-textarea label="Notes" placeholder="What changed in this release?"></spec-textarea>'
}
