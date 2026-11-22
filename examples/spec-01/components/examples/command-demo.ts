import '@/components/ui/command-palette'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-command-palette>
      <option>New project</option>
      <option>Invite member</option>
      <option>Open settings</option>
      <option>Switch theme</option>
    </spec-command-palette>`
}
