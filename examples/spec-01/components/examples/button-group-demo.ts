import '@/components/ui/button-group'
import '@/components/ui/icon'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-rocker-demo>
      <spec-button-group label="Level">
        <button aria-label="Down" repeat><spec-icon name="minus"></spec-icon></button>
        <button aria-label="Up" repeat><spec-icon name="plus"></spec-icon></button>
      </spec-button-group>
      <spec-button-group label="Volume" orientation="vertical">
        <button aria-label="Volume up" repeat><spec-icon name="plus"></spec-icon></button>
        <button aria-label="Volume down" repeat><spec-icon name="minus"></spec-icon></button>
      </spec-button-group>
      <spec-button-group label="Transport" tone="primary">
        <button aria-label="Previous"><spec-icon name="prev"></spec-icon></button>
        <button aria-label="Play"><spec-icon name="play"></spec-icon></button>
        <button aria-label="Next"><spec-icon name="next"></spec-icon></button>
      </spec-button-group>
    </div>`
  const play = root.querySelector<HTMLButtonElement>('[aria-label="Play"]')!
  let playing = false
  root.querySelector('[label="Transport"]')!.addEventListener('press', (event) => {
    if ((event as CustomEvent).detail.value !== 1) return
    playing = !playing
    play.ariaLabel = playing ? 'Pause' : 'Play'
    play.firstElementChild!.setAttribute('name', playing ? 'pause' : 'play')
  })
}
