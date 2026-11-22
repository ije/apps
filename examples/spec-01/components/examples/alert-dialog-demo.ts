import '@/components/ui/alert-dialog'
import '@/components/ui/icon'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-alert-dialog
      heading="Delete untitled_04?"
      description="This can't be undone. Every deployment and domain is removed."
      confirm="Delete"
    ><spec-icon name="trash"></spec-icon>Delete project</spec-alert-dialog>`
}
