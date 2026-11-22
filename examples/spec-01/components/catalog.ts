import '@ui-sdk/web/popover'
import '@/components/ui/button'
import { notify } from '@/components/ui/toaster'
import { haptic } from '@/lib/haptics'
import accordionDemo from '@/components/examples/accordion-demo'
import alertDemo from '@/components/examples/alert-demo'
import alertDialogDemo from '@/components/examples/alert-dialog-demo'
import avatarDemo from '@/components/examples/avatar-demo'
import badgeDemo from '@/components/examples/badge-demo'
import barGraphDemo from '@/components/examples/bar-graph-demo'
import breadcrumbDemo from '@/components/examples/breadcrumb-demo'
import buttonDemo from '@/components/examples/button-demo'
import buttonGroupDemo from '@/components/examples/button-group-demo'
import calendarDemo from '@/components/examples/calendar-demo'
import carouselDemo from '@/components/examples/carousel-demo'
import checkboxDemo from '@/components/examples/checkbox-demo'
import codeBlockDemo from '@/components/examples/code-block-demo'
import collapsibleDemo from '@/components/examples/collapsible-demo'
import comboboxDemo from '@/components/examples/combobox-demo'
import commandDemo from '@/components/examples/command-demo'
import contextMenuDemo from '@/components/examples/context-menu-demo'
import dPadDemo from '@/components/examples/d-pad-demo'
import datePickerDemo from '@/components/examples/date-picker-demo'
import dialogDemo from '@/components/examples/dialog-demo'
import displayDemo from '@/components/examples/display-demo'
import drawerDemo from '@/components/examples/drawer-demo'
import flatKnobDemo from '@/components/examples/flat-knob-demo'
import hoverCardDemo from '@/components/examples/hover-card-demo'
import itemListDemo from '@/components/examples/item-list-demo'
import lcdDemo from '@/components/examples/lcd-demo'
import menuDemo from '@/components/examples/menu-demo'
import menubarDemo from '@/components/examples/menubar-demo'
import meterDemo from '@/components/examples/meter-demo'
import modeDialDemo from '@/components/examples/mode-dial-demo'
import navbarDemo from '@/components/examples/navbar-demo'
import numberStepperDemo from '@/components/examples/number-stepper-demo'
import otpInputDemo from '@/components/examples/otp-input-demo'
import paginationDemo from '@/components/examples/pagination-demo'
import popoverDemo from '@/components/examples/popover-demo'
import progressDemo from '@/components/examples/progress-demo'
import radioGroupDemo from '@/components/examples/radio-group-demo'
import resizableDemo from '@/components/examples/resizable-demo'
import scrollAreaDemo from '@/components/examples/scroll-area-demo'
import segmentedControlDemo from '@/components/examples/segmented-control-demo'
import selectDemo from '@/components/examples/select-demo'
import separatorDemo from '@/components/examples/separator-demo'
import slideSwitchDemo from '@/components/examples/slide-switch-demo'
import sliderDemo from '@/components/examples/slider-demo'
import switchDemo from '@/components/examples/switch-demo'
import tabsDemo from '@/components/examples/tabs-demo'
import textFieldDemo from '@/components/examples/text-field-demo'
import textareaDemo from '@/components/examples/textarea-demo'
import toastDemo from '@/components/examples/toast-demo'
import toggleGroupDemo from '@/components/examples/toggle-group-demo'
import toolbarDemo from '@/components/examples/toolbar-demo'
import tooltipDemo from '@/components/examples/tooltip-demo'

const panels: [n: string, title: string, mount: (root: HTMLElement) => void | (() => void), wide?: boolean][] = [
  ['01', 'Button', buttonDemo],
  ['02', 'Switch', switchDemo],
  ['03', 'Checkbox', checkboxDemo],
  ['04', 'Segmented control', segmentedControlDemo],
  ['05', 'Slider', sliderDemo, true],
  ['06', 'Flat knob', flatKnobDemo, true],
  ['07', 'Text field', textFieldDemo],
  ['08', 'Select', selectDemo],
  ['09', 'Meter', meterDemo],
  ['10', 'Menu', menuDemo],
  ['11', 'Dialog', dialogDemo],
  ['12', 'Tabs', tabsDemo, true],
  ['13', 'Calendar', calendarDemo, true],
  ['14', 'Radio group', radioGroupDemo],
  ['15', 'Number stepper', numberStepperDemo],
  ['16', 'Accordion', accordionDemo, true],
  ['17', 'Toolbar', toolbarDemo],
  ['18', 'Pagination', paginationDemo, true],
  ['19', 'Tooltip', tooltipDemo],
  ['20', 'Popover', popoverDemo],
  ['21', 'Toast', toastDemo],
  ['22', 'Combobox', comboboxDemo],
  ['23', 'Date picker', datePickerDemo],
  ['24', 'Textarea', textareaDemo],
  ['25', 'OTP input', otpInputDemo, true],
  ['26', 'Toggle group', toggleGroupDemo],
  ['27', 'Menubar', menubarDemo, true],
  ['28', 'Context menu', contextMenuDemo],
  ['29', 'Command', commandDemo],
  ['30', 'Alert dialog', alertDialogDemo],
  ['31', 'Drawer', drawerDemo],
  ['32', 'Hover card', hoverCardDemo],
  ['33', 'Alert', alertDemo, true],
  ['34', 'Badge', badgeDemo],
  ['35', 'Avatar', avatarDemo],
  ['36', 'Breadcrumb', breadcrumbDemo],
  ['37', 'Progress', progressDemo],
  ['38', 'Collapsible', collapsibleDemo],
  ['39', 'Carousel', carouselDemo, true],
  ['40', 'Resizable', resizableDemo, true],
  ['41', 'Scroll area', scrollAreaDemo],
  ['43', 'Separator', separatorDemo],
  ['44', 'Bar graph', barGraphDemo],
  ['45', 'Display', displayDemo, true],
  ['51', 'Item list', itemListDemo, true],
  ['52', 'D-pad', dPadDemo, true],
  ['53', 'Mode dial', modeDialDemo, true],
  ['54', 'Button group', buttonGroupDemo, true],
  ['55', 'Navbar', navbarDemo, true],
  ['56', 'LCD', lcdDemo, true],
  ['57', 'Slide switch', slideSwitchDemo, true],
  ['50', 'Code block', codeBlockDemo, true],
]

const prompt = `Install components from the SPEC-01 shadcn registry,
https://spec-01.vercel.app/r/registry.json
Wrap the app in <Hardware> from components/ui/hardware.`

// A hole that dense packing can't fill is taken up by the panel before it.
function fill(grid: HTMLElement) {
  const panels = [...grid.children] as HTMLElement[]
  for (const panel of panels) panel.style.removeProperty('grid-column')
  const style = getComputedStyle(grid)
  const columns = style.gridTemplateColumns.split(' ').length
  if (columns < 2) return
  const box = grid.getBoundingClientRect()
  const gap = parseFloat(style.columnGap) || 0
  const column = (box.width + gap) / columns
  const rows = new Map<number, { panel: HTMLElement; start: number; span: number }[]>()
  for (const panel of panels) {
    const rect = panel.getBoundingClientRect()
    const top = Math.round(rect.top)
    rows.set(top, [
      ...(rows.get(top) ?? []),
      { panel, start: Math.round((rect.left - box.left) / column), span: Math.round((rect.width + gap) / column) },
    ])
  }
  for (const row of rows.values()) {
    row.sort((a, b) => a.start - b.start)
    row.forEach((item, i) => {
      const empty = (row[i + 1]?.start ?? columns) - (item.start + item.span)
      if (empty > 0) item.panel.style.gridColumn = `${item.start + 1} / span ${item.span + empty}`
    })
    const first = row[0]
    if (first.start > 0) first.panel.style.gridColumn = `1 / span ${first.start + first.span}`
  }
}

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <header data-kit="masthead">
      <h1>SPEC-01/Hardware</h1>
      <p>
        A hardware theme for <a href="https://ui-sdk.dev" target="_blank">Vercel UI SDK</a>, a library of behaviors to
        build your own components. Every control here is a Vercel UI SDK component underneath.
      </p>
      <div data-install-prompt>
        <ui-popover>
          <ui-popover-trigger><button data-kit="link">Installation prompt</button></ui-popover-trigger>
          <ui-popover-content data-kit="popover" side="bottom" align="start" side-offset="12" aria-label="Installation prompt">
            <div data-kit="popover-surface" data-install-prompt-surface>
              <p data-install-prompt-text>${prompt.replace('<', '&lt;')}<span data-install-caret aria-hidden="true"></span></p>
              <div data-kit="actions">
                <ui-popover-close><button data-kit="button" data-tone="secondary">Close</button></ui-popover-close>
                <spec-button tone="primary">Copy</spec-button>
              </div>
            </div>
          </ui-popover-content>
        </ui-popover>
      </div>
    </header>
    <div data-kit="grid">
      ${panels
        .map(
          ([n, title, , wide]) => `
            <section data-kit="module" ${wide ? 'data-wide' : ''} aria-label="${title}">
              <h2 data-kit="caption"><span>${n}</span>${title}</h2>
              <div data-kit="stage"><div data-kit="surface"></div></div>
            </section>`,
        )
        .join('')}
    </div>`
  const leaves = [...root.querySelectorAll<HTMLElement>('[data-kit="surface"]')].map((surface, i) => panels[i][2](surface))

  const copy = root.querySelector<HTMLElement>('spec-button button')!
  copy.onclick = () => {
    notify('Copied', { description: 'Paste it into your coding agent.' })
    haptic('success')
    copy.textContent = 'Copied'
    setTimeout(() => (copy.textContent = 'Copy'), 1400)
    navigator.clipboard.writeText(prompt).catch(() => {
      copy.textContent = 'Copy'
      haptic('error')
      notify("Couldn't copy", { description: "The clipboard wasn't available." })
    })
  }

  const grid = root.querySelector<HTMLElement>('[data-kit="grid"]')!
  const resize = new ResizeObserver(() => fill(grid))
  resize.observe(grid)
  return () => {
    resize.disconnect()
    for (const leave of leaves) leave?.()
  }
}
