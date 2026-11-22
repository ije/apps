import '@/components/ui/date-picker'

export default function mount(root: HTMLElement) {
  root.innerHTML = `<spec-date-picker label="Release date" default-value="2026-11-14"></spec-date-picker>`
}
