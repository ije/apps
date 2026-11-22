import '@/components/ui/checkbox'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-checkbox label="Sync" default-checked></spec-checkbox>
    <spec-checkbox label="Loop"></spec-checkbox>
    <spec-checkbox label="Metro"></spec-checkbox>`
}
