import '@/components/ui/dialog'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-dialog
      trigger="Delete"
      heading="Delete untitled_04?"
      description="The project and its 12 samples are removed from this device."
      confirm="Delete"
    ></spec-dialog>`
}
