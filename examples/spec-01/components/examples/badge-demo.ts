import '@/components/ui/badge'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-badge tone="success">Ready</spec-badge>
    <spec-badge tone="info">Building</spec-badge>
    <spec-badge tone="warning">Degraded</spec-badge>
    <spec-badge tone="danger">Error</spec-badge>
    <spec-badge tone="neutral">Queued</spec-badge>`
}
