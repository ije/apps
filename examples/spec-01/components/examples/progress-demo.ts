import '@/components/ui/progress'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-progress label="Uploading" value="42"></spec-progress>`
}
