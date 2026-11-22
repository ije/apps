import '@ui-sdk/web/radio'
import '@/components/ui/hardware'
import '@/components/ui/icon'
import '@/components/ui/navbar'
import '@/components/ui/toaster'
import { haptic } from '@/lib/haptics'
import { onSoundChange, setSoundEnabled, soundEnabled } from '@/lib/sound'

type Mount = (root: HTMLElement) => void | (() => void)

const pages: Record<string, () => Promise<{ default: Mount }>> = {
  '': () => import('@/components/catalog'),
  music: () => import('@/components/blocks/music-player/music-player'),
  paint: () => import('@/components/blocks/paint/paint'),
  midi: () => import('@/components/blocks/midi-editor/midi-editor'),
  timer: () => import('@/components/blocks/timer/timer'),
  color: () => import('@/components/blocks/color/color'),
  image: () => import('@/components/blocks/image-editor/image-editor'),
  dashboard: () => import('@/components/blocks/dashboard/dashboard'),
}

const site = document.querySelector<HTMLElement>('spec-hardware')!
const navbar = site.querySelector<HTMLElement>('spec-navbar')!
const main = site.querySelector('main')!

if (navigator.platform.startsWith('Mac')) site.dataset.titlebar = ''
new ResizeObserver(() => site.style.setProperty('--nav-h', `${navbar.offsetHeight}px`)).observe(navbar)

let leave: ReturnType<Mount>
async function show() {
  const hash = location.hash
  const route = hash.slice(2) in pages ? hash.slice(2) : ''
  const { default: mount } = await pages[route]()
  if (location.hash !== hash) return
  leave?.()
  for (const link of navbar.querySelectorAll('a')) {
    if (link.getAttribute('href') === `#/${route}`) link.setAttribute('aria-current', 'page')
    else link.removeAttribute('aria-current')
  }
  main.dataset.sitePage = route || 'components'
  main.replaceChildren()
  leave = mount(main)
  scrollTo(0, 0)
}
addEventListener('hashchange', show)
show()

const picker = navbar.querySelector('ui-radio-group')!
picker.addEventListener('valuechange', (event) => {
  const key = picker.querySelector<HTMLElement>(`[value="${event.detail.value}"]`)!
  site.dataset.key = key.getAttribute('value')!
  site.setAttribute('key-color', key.style.getPropertyValue('--swatch').slice(6, -1))
})

const sound = navbar.querySelector<HTMLElement>('[data-kit="sound-toggle"]')!
const showSound = (on: boolean) => {
  sound.ariaPressed = String(on)
  sound.title = on ? 'Sounds on' : 'Sounds off'
  sound.firstElementChild!.setAttribute('name', on ? 'volume' : 'volume-mute')
}
showSound(soundEnabled())
onSoundChange(showSound)
sound.onclick = () => {
  const on = !soundEnabled()
  setSoundEnabled(on)
  if (on) haptic('selection')
}

document.addEventListener('click', (event) => {
  const link = (event.target as Element).closest<HTMLAnchorElement>('a[target="_blank"]')
  if (!link) return
  event.preventDefault()
  open(link.href)
})
