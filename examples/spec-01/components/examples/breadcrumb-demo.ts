import '@/components/ui/breadcrumb'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-breadcrumb>
      <option>Acme</option>
      <option>Projects</option>
      <option>checkout-api</option>
    </spec-breadcrumb>`
}
