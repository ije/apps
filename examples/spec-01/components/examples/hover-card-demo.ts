import '@/components/ui/hover-card'

export default function mount(root: HTMLElement) {
  root.innerHTML = `<spec-profile-hover-card name="Ada Lovelace" handle="@ada" bio="Engineer. Writes the first programs."></spec-profile-hover-card>`
}
