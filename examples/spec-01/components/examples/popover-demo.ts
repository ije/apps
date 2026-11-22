import '@/components/ui/popover'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-popover
      trigger="Share"
      heading="Share untitled_04"
      description="Anyone with the link can listen. They can't edit."
      confirm="Copy link"
    ></spec-popover>`
}
