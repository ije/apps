import '@/components/ui/alert'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-alert tone="warning" heading="Usage at 82%">You'll reach your plan's limit in about 4 days.</spec-alert>`
}
