import { dateParts, daysInMonth, isoDate, weekday as weekdayOf } from '@ui-sdk/core/date'
import { haptic } from '@/lib/haptics'
import { define, Kit } from '@/lib/kit'

type Field = 'year' | 'month' | 'day'
type Parts = { year: number; month: number; day: number }
const fields: Field[] = ['year', 'month', 'day']
const monthNames = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']
const weekdayNames = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']
const daysIn = (year: number, month: number) => daysInMonth(year, month + 1)
const parse = (iso: string | null): Parts => {
  const { year, month, day } = dateParts(iso ?? '') ?? { year: 2026, month: 1, day: 1 }
  return { year, month: month - 1, day }
}
const format = ({ year, month, day }: Parts) => isoDate(year, month + 1, day)!

class DateSetter extends Kit {
  static observedAttributes = ['value']
  #date: Parts = parse(null)
  #reported = ''
  #active: Field = 'day'
  #turn = 0
  #spins = {} as Record<Field, HTMLElement>
  #weekday!: HTMLElement
  #days: HTMLElement[] = []
  #jog!: HTMLElement
  #center!: HTMLButtonElement

  render() {
    this.#date = parse(this.getAttribute('value') ?? this.getAttribute('default-value') ?? '2026-01-01')
    this.#reported = format(this.#date)
    this.dataset.kit = 'date-setter'
    this.role = 'group'
    this.ariaLabel = this.getAttribute('label') ?? 'Date'
    this.innerHTML = `
      <div data-kit="date-lcd">
        <div data-kit="date-lcd-row"><span data-kit="date-lcd-tag">Set date</span><span data-kit="date-lcd-tag"></span></div>
        <div data-kit="date-lcd-fields">${fields.map((field, i) => `${i ? '<span data-kit="date-lcd-sep" aria-hidden="true">·</span>' : ''}<span role="spinbutton" tabindex="0" aria-label="${field}" data-kit="date-lcd-field" data-field="${field}"></span>`).join('')}</div>
        <div data-kit="date-lcd-month" aria-hidden="true">${'<i></i>'.repeat(42)}</div>
      </div>
      <div data-kit="jog" style="--turn: 0deg">
        <span data-kit="jog-ring" aria-hidden="true"></span>
        <button type="button" data-kit="jog-center">Set</button>
      </div>`
    this.#weekday = this.querySelector('[data-kit="date-lcd-tag"]:last-child')!
    this.#days = [...this.querySelectorAll<HTMLElement>('[data-kit="date-lcd-month"] i')]
    this.#jog = this.querySelector('[data-kit="jog"]')!
    this.#center = this.querySelector('[data-kit="jog-center"]')!
    for (const spin of this.querySelectorAll<HTMLElement>('[role="spinbutton"]')) {
      const field = spin.dataset.field as Field
      this.#spins[field] = spin
      spin.removeAttribute('data-field')
      spin.onfocus = spin.onclick = () => this.#activate(field)
      spin.onkeydown = (event) => {
        const by = ({ ArrowUp: 1, ArrowRight: 0, ArrowDown: -1, PageUp: 10, PageDown: -10 } as Record<string, number>)[event.key]
        if (event.key === 'Enter') {
          event.preventDefault()
          this.#advance()
        } else if (by) {
          event.preventDefault()
          this.#step(by, field)
        }
      }
    }
    this.#center.onclick = () => this.#advance()

    const jog = this.#jog
    let drag: { angle: number; rest: number } | null = null
    const angleOf = (event: PointerEvent) => {
      const box = jog.getBoundingClientRect()
      return (Math.atan2(event.clientY - box.top - box.height / 2, event.clientX - box.left - box.width / 2) * 180) / Math.PI
    }
    jog.addEventListener('wheel', (event) => this.#step(event.deltaY < 0 ? 1 : -1), { passive: true })
    jog.onpointerdown = (event) => {
      if ((event.target as HTMLElement).closest('button')) return
      jog.setPointerCapture(event.pointerId)
      drag = { angle: angleOf(event), rest: 0 }
    }
    jog.onpointermove = (event) => {
      if (!drag) return
      const angle = angleOf(event)
      let delta = angle - drag.angle
      if (delta > 180) delta -= 360
      if (delta < -180) delta += 360
      drag.angle = angle
      drag.rest += delta
      while (Math.abs(drag.rest) >= 15) {
        this.#step(drag.rest > 0 ? 1 : -1)
        drag.rest -= Math.sign(drag.rest) * 15
      }
    }
    jog.onpointerup = () => (drag = null)
    this.#activate('day')
    this.#paint()
  }

  update() {
    const value = this.getAttribute('value')
    if (value) this.value = value
  }

  get value() {
    return format(this.#date)
  }

  set value(value: string) {
    if (!this.rendered) this.setAttribute('value', value)
    else if (value !== format(this.#date)) {
      this.#date = parse(value)
      this.#reported = format(this.#date)
      this.#paint()
    }
  }

  #activate(field: Field) {
    this.#active = field
    for (const name of fields) {
      if (name === field) this.#spins[name].dataset.active = 'true'
      else this.#spins[name].removeAttribute('data-active')
    }
    this.#center.ariaLabel = `Next field, after ${field}`
  }

  #advance() {
    const next = fields[(fields.indexOf(this.#active) + 1) % fields.length]
    this.#activate(next)
    this.#spins[next].focus()
  }

  #step(by: number, field = this.#active) {
    haptic('selection')
    this.#turn += by * 15
    this.#jog.style.setProperty('--turn', `${this.#turn}deg`)
    let { year, month, day } = this.#date
    if (field === 'year') year = Math.min(2099, Math.max(1970, year + by))
    if (field === 'month') month = (month + by + 12) % 12
    if (field === 'day') day = ((day - 1 + by + daysIn(year, month)) % daysIn(year, month)) + 1
    this.#date = { year, month, day: Math.min(day, daysIn(year, month)) }
    this.#paint()
    const next = format(this.#date)
    if (next === this.#reported) return
    this.#reported = next
    this.emit('valuechange', next)
  }

  #paint() {
    const { year, month, day } = this.#date
    const days = daysIn(year, month)
    const first = weekdayOf(isoDate(year, month + 1, 1)!)
    this.#weekday.textContent = weekdayNames[weekdayOf(format(this.#date))]
    const values = {
      year: [String(year), year, 1970, 2099, String(year)],
      month: [String(month + 1).padStart(2, '0'), month + 1, 1, 12, monthNames[month]],
      day: [String(day).padStart(2, '0'), day, 1, days, String(day)],
    }
    for (const field of fields) {
      const [text, now, min, max, spoken] = values[field]
      const spin = this.#spins[field]
      spin.textContent = String(text)
      spin.ariaValueNow = String(now)
      spin.ariaValueMin = String(min)
      spin.ariaValueMax = String(max)
      spin.ariaValueText = String(spoken)
    }
    this.#days.forEach((cell, i) => {
      const blank = i < first || i >= first + days
      cell.toggleAttribute('data-blank', blank)
      cell.toggleAttribute('data-on', !blank && i - first + 1 === day)
    })
  }
}

define('spec-date-setter', DateSetter)
