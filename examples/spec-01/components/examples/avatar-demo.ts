import '@/components/ui/avatar'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-avatar name="Ada Lovelace"></spec-avatar>
    <spec-avatar name="Grace Hopper"></spec-avatar>
    <spec-avatar name="Alan Turing" size="sm"></spec-avatar>`
}
