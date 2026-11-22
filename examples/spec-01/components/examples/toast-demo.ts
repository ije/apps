import '@/components/ui/toast'

export default function mount(root: HTMLElement) {
  root.innerHTML = `<spec-toast-demo trigger="Save" heading="Saved untitled_04" description="12 samples, 4 patterns"></spec-toast-demo>`
}
