import {
  AlarmIcon,
  ArrowCounterClockwiseIcon,
  CoffeeIcon,
  MinusIcon,
  PlusIcon,
  StarIcon,
  TrashIcon,
  UsersIcon,
} from "@phosphor-icons/react"
import {
  bindDrag,
  bindHold,
  bindRepeat,
  bindScrollPicker,
  createPointerKind,
  isFocusFromKeyboard,
  sourceOf,
} from "@ui-sdk/core"
import { sheetPhysics } from "@ui-sdk/core/drawer"
import {
  createVelocityTracker,
  projectDistance,
  rubberBand,
  solveSpring,
} from "@ui-sdk/core/gesture"
import { clamp } from "@ui-sdk/core/number"
import { Listbox, ListboxItem } from "@ui-sdk/react/listbox"
import { usePreference } from "@ui-sdk/react/preferences"
import {
  useEffect,
  useEffectEvent,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type FocusEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react"
import type { Tile } from "../bento/types"
import "./gestures.css"

const reduced = () =>
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches

/* 1. Hold to delete: bindHold */

const HOLD_DELAY = 1200

function HoldToDelete() {
  const button = useRef<HTMLButtonElement>(null)
  const undo = useRef<HTMLButtonElement>(null)
  const refocus = useRef(false)
  const hint = useId()
  const [deleted, setDeleted] = useState(false)
  const [early, setEarly] = useState(false)

  useEffect(() => {
    if (refocus.current) {
      refocus.current = false
      ;(deleted ? undo : button).current?.focus()
    }
    if (deleted) {
      return
    }
    return bindHold(button.current!, {
      delay: HOLD_DELAY,
      onPress: () => setEarly(false),
      onHold: () => {
        refocus.current = document.activeElement === button.current
        setDeleted(true)
      },
      onEnd: ({ held, canceled }) => setEarly(!held && !canceled),
    })
  }, [deleted])

  const restore = () => {
    refocus.current = document.activeElement === undo.current
    setEarly(false)
    setDeleted(false)
  }

  return (
    <div className="gs-hold" data-deleted={deleted ? "" : undefined}>
      <div className="gs-hold-project">
        <span className="gs-hold-icon" aria-hidden>
          <TrashIcon size={16} weight="bold" />
        </span>
        <span className="gs-hold-name">acme-storefront</span>
        <span className="gs-hold-meta">Production · 128 deploys</span>
      </div>
      {deleted ? (
        <div className="gs-hold-done" role="status">
          <span>Project deleted.</span>
          <button
            ref={undo}
            type="button"
            className="b-btn"
            data-variant="ghost"
            onClick={restore}
          >
            <ArrowCounterClockwiseIcon size={14} weight="bold" aria-hidden />
            Undo
          </button>
        </div>
      ) : (
        <>
          <button
            ref={button}
            type="button"
            className="gs-hold-button"
            aria-describedby={hint}
          >
            <span className="gs-hold-fill" aria-hidden />
            <span className="gs-hold-label">Delete project</span>
          </button>
          <span
            id={hint}
            className="gs-hold-hint"
            data-early={early || undefined}
          >
            {early ? "Keep holding to delete." : "Press and hold. Esc cancels."}
          </span>
        </>
      )}
    </div>
  )
}

/* 2. Guest stepper: bindRepeat */

const CAPACITY = 120

function isPaced(count: number) {
  const tick = count - 1
  const every = tick <= 12 ? 4 : tick <= 36 ? 2 : 1
  return tick % every === 0
}

function StepButton({
  label,
  atBound,
  onStep,
  children,
}: {
  label: string
  atBound: boolean
  onStep: () => boolean
  children: ReactNode
}) {
  const button = useRef<HTMLButtonElement>(null)
  const bound = useRef(atBound)
  bound.current = atBound
  const step = useEffectEvent(onStep)

  useEffect(
    () =>
      bindRepeat(button.current!, {
        repeat: 40,
        disabled: () => bound.current,
        onStep: ({ count }) => !isPaced(count) || step(),
      }),
    []
  )

  return (
    <button
      ref={button}
      type="button"
      className="gs-step"
      aria-label={label}
      aria-disabled={atBound || undefined}
    >
      {children}
    </button>
  )
}

function GuestStepper() {
  const [guests, setGuests] = useState(24)
  const current = useRef(guests)

  const add = (amount: number) => {
    const next = clamp(current.current + amount, 0, CAPACITY)
    current.current = next
    setGuests(next)
    return next > 0 && next < CAPACITY
  }

  return (
    <div className="gs-guests">
      <div className="gs-guests-event">
        <span className="gs-guests-name">Rooftop launch</span>
        <span className="gs-guests-when">Fri · 7 PM</span>
      </div>
      <div className="gs-guests-row">
        <StepButton
          label="Remove guest"
          atBound={guests === 0}
          onStep={() => add(-1)}
        >
          <MinusIcon size={18} weight="bold" aria-hidden />
        </StepButton>
        <output className="gs-guests-count" aria-live="polite">
          <span className="gs-guests-number">{guests}</span>
          <span className="gs-guests-unit">
            <UsersIcon size={12} weight="bold" aria-hidden />
            guests
          </span>
        </output>
        <StepButton
          label="Add guest"
          atBound={guests === CAPACITY}
          onStep={() => add(1)}
        >
          <PlusIcon size={18} weight="bold" aria-hidden />
        </StepButton>
      </div>
      <div className="gs-guests-capacity" aria-hidden>
        <span
          className="gs-guests-bar"
          style={{ "--fill": guests / CAPACITY } as CSSProperties}
        />
      </div>
      <span className="gs-guests-left">
        {guests === CAPACITY ? "Venue full" : `${CAPACITY - guests} spots left`}
      </span>
    </div>
  )
}

/* 3. Alarm wheels: bindScrollPicker */

const HOURS = Array.from({ length: 12 }, (_, index) => String(index + 1))
const MINUTES = Array.from({ length: 12 }, (_, index) =>
  String(index * 5).padStart(2, "0")
)
const PERIODS = ["AM", "PM"]

function Wheel({
  label,
  options,
  value,
  onValueChange,
}: {
  label: string
  options: string[]
  value: string
  onValueChange: (value: string) => void
}) {
  const list = useRef<HTMLDivElement>(null)
  const select = useEffectEvent((index: number) =>
    onValueChange(options[index])
  )

  useLayoutEffect(
    () =>
      bindScrollPicker(list.current!, { onSelect: (index) => select(index) }),
    []
  )

  return (
    <Listbox
      ref={list}
      className="gs-wheel"
      aria-label={label}
      keepInView={false}
      value={value}
      onValueChange={(next) => next && onValueChange(next)}
    >
      {options.map((option) => (
        <ListboxItem key={option} value={option} className="gs-wheel-item">
          {option}
        </ListboxItem>
      ))}
    </Listbox>
  )
}

function AlarmWheels() {
  const [hour, setHour] = useState("7")
  const [minute, setMinute] = useState("30")
  const [period, setPeriod] = useState("AM")
  return (
    <div className="gs-alarm">
      <div className="gs-alarm-head">
        <AlarmIcon size={14} weight="bold" aria-hidden />
        <span>
          Wake up at{" "}
          <strong>
            {hour}:{minute} {period}
          </strong>
        </span>
      </div>
      <div className="gs-wheels" dir="ltr">
        <Wheel
          label="Hour"
          options={HOURS}
          value={hour}
          onValueChange={setHour}
        />
        <span className="gs-wheels-colon" aria-hidden>
          :
        </span>
        <Wheel
          label="Minute"
          options={MINUTES}
          value={minute}
          onValueChange={setMinute}
        />
        <Wheel
          label="AM or PM"
          options={PERIODS}
          value={period}
          onValueChange={setPeriod}
        />
      </div>
    </div>
  )
}

/* 4. Bottom sheet: sheetPhysics */

const SNAPS = ["Peek", "Half", "Full"]
const CAFES = [
  { name: "Café Lumen", rating: "4.8", far: "2 min" },
  { name: "Fábrica Coffee", rating: "4.7", far: "5 min" },
  { name: "Copenhagen Coffee Lab", rating: "4.6", far: "7 min" },
  { name: "Hello, Kristof", rating: "4.6", far: "9 min" },
  { name: "Dear Breakfast", rating: "4.5", far: "11 min" },
  { name: "Comoba", rating: "4.5", far: "12 min" },
  { name: "Café Janis", rating: "4.4", far: "14 min" },
  { name: "Heim Café", rating: "4.4", far: "15 min" },
]
const FILTERS = ["Open now", "Wi-Fi", "Outdoor seats", "Oat milk"]
const physics = sheetPhysics.ios

function stopsFor(height: number) {
  return [64, Math.round(height * 0.5), height - 10]
}

function nearestStop(stops: number[], at: number) {
  let best = 0
  for (let index = 1; index < stops.length; index++) {
    if (Math.abs(stops[index] - at) < Math.abs(stops[best] - at)) {
      best = index
    }
  }
  return best
}

function releaseStop(stops: number[], shown: number, velocity: number) {
  if (Math.abs(velocity) < physics.minFlingVelocity) {
    return nearestStop(stops, shown)
  }
  const landing = nearestStop(
    stops,
    shown + projectDistance(velocity, physics.projectionRate)
  )
  const ahead =
    velocity > 0
      ? stops.findIndex((stop) => stop > shown + 1)
      : stops.findLastIndex((stop) => stop < shown - 1)
  if (ahead === -1) {
    return landing
  }
  return velocity > 0 ? Math.max(landing, ahead) : Math.min(landing, ahead)
}

function MapSheet() {
  const frame = useRef<HTMLDivElement>(null)
  const sheet = useRef<HTMLDivElement>(null)
  const shown = useRef(0)
  const stops = useRef([0, 0, 0])
  const snapRef = useRef(0)
  const cancel = useRef<() => void>(() => {})
  const stop = useRef<(() => void) | undefined>(undefined)
  const heightId = useId()
  const [snap, setSnap] = useState(0)

  const paint = (height: number) => {
    shown.current = height
    const el = sheet.current
    if (!el) {
      return
    }
    const [low, , high] = stops.current
    el.style.translate = `0 ${high - height}px`
    frame.current?.style.setProperty(
      "--sheet-progress",
      String(clamp((height - low) / (high - low), 0, 1))
    )
  }

  const settle = (index: number, velocity = 0) => {
    cancel.current()
    snapRef.current = index
    setSnap(index)
    const to = stops.current[index]
    if (reduced()) {
      paint(to)
      return
    }
    const damping =
      Math.abs(velocity) >= physics.minFlingVelocity
        ? physics.momentumDampingRatio
        : physics.dampingRatio
    const spring = solveSpring(
      physics.response,
      damping,
      shown.current - to,
      velocity
    )
    const start = performance.now()
    let frameId = 0
    const tick = (now: number) => {
      const offset = spring.position(now - start)
      if (
        Math.abs(offset) < 0.5 &&
        Math.abs(spring.velocity(now - start)) < 0.01
      ) {
        paint(to)
        return
      }
      paint(to + offset)
      frameId = requestAnimationFrame(tick)
    }
    frameId = requestAnimationFrame(tick)
    cancel.current = () => cancelAnimationFrame(frameId)
  }

  useLayoutEffect(() => {
    const el = frame.current!
    const observer = new ResizeObserver(() => {
      stops.current = stopsFor(el.clientHeight)
      sheet.current!.style.height = `calc(${stops.current[2]}px + var(--gs-sheet-bleed))`
      cancel.current()
      paint(stops.current[snapRef.current])
    })
    observer.observe(el)
    return () => {
      observer.disconnect()
      cancel.current()
      stop.current?.()
    }
  }, [])

  const down = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) {
      return
    }
    cancel.current()
    const el = event.currentTarget
    const startY = event.clientY
    const from = shown.current
    const tracker = createVelocityTracker()
    tracker.add(event.timeStamp, from)
    const [low, , high] = stops.current
    const stretch = physics.rubberBandDimension ?? high
    const band = (raw: number) =>
      raw > high
        ? high + rubberBand(raw - high, stretch, physics.rubberBand)
        : raw < low
          ? low - rubberBand(low - raw, stretch, physics.rubberBand)
          : raw
    stop.current?.()
    stop.current = bindDrag(el, event.nativeEvent, {
      threshold:
        event.pointerType === "mouse" ? physics.mouseSlop : physics.touchSlop,
      cancelOnEscape: true,
      onStart: () => el.setAttribute("data-dragging", ""),
      onMove: ({ y, event: move }) => {
        const raw = from + startY - y
        tracker.add(move.timeStamp, raw)
        paint(band(raw))
      },
      onEnd: (up, canceled) => {
        el.removeAttribute("data-dragging")
        if (canceled) {
          settle(snapRef.current)
          return
        }
        const velocity = tracker.velocity(up.timeStamp)
        settle(releaseStop(stops.current, shown.current, velocity), velocity)
      },
    })
  }

  const key = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    const last = SNAPS.length - 1
    const next = {
      ArrowUp: Math.min(snap + 1, last),
      ArrowDown: Math.max(snap - 1, 0),
      Home: 0,
      End: last,
    }[event.key]
    if (next === undefined) {
      return
    }
    event.preventDefault()
    settle(next)
  }

  return (
    <div ref={frame} className="gs-map">
      <div className="gs-map-ground" aria-hidden>
        <span className="gs-map-road" data-road="a" />
        <span className="gs-map-road" data-road="b" />
        <span className="gs-map-road" data-road="c" />
        <span className="gs-map-park" />
        {[0, 1, 2, 3, 4].map((index) => (
          <span key={index} className="gs-map-pin" data-pin={index}>
            <CoffeeIcon size={11} weight="fill" />
          </span>
        ))}
      </div>
      <span className="gs-map-scrim" aria-hidden />
      <div ref={sheet} className="gs-sheet" onPointerDown={down}>
        <button
          type="button"
          className="gs-sheet-handle"
          aria-label="Resize results"
          aria-describedby={heightId}
          onClick={() => settle((snap + 1) % SNAPS.length)}
          onKeyDown={key}
        />
        <span id={heightId} className="sr-only" aria-live="polite">
          {SNAPS[snap]} height
        </span>
        <div className="gs-sheet-head">
          <span className="gs-sheet-title">Coffee nearby</span>
          <span className="gs-sheet-sub">8 open now · walking</span>
        </div>
        <div className="gs-sheet-filters">
          {FILTERS.map((filter) => (
            <span key={filter}>{filter}</span>
          ))}
        </div>
        <ul className="gs-sheet-list">
          {CAFES.map((cafe) => (
            <li key={cafe.name}>
              <span className="gs-sheet-cup" aria-hidden>
                <CoffeeIcon size={14} weight="fill" />
              </span>
              <span className="gs-sheet-name">{cafe.name}</span>
              <span className="gs-sheet-rating">
                <StarIcon size={10} weight="fill" aria-hidden />
                {cafe.rating}
              </span>
              <span className="gs-sheet-far">{cafe.far}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}

/* 5. Mention previews: createPointerKind, isFocusFromKeyboard */

const PEOPLE: Record<string, { name: string; role: string; time: string }> = {
  maya: { name: "Maya Chen", role: "Design lead", time: "4:12 PM in Lisbon" },
  jonas: {
    name: "Jonas Weber",
    role: "Infrastructure",
    time: "5:12 PM in Berlin",
  },
  priya: { name: "Priya Raman", role: "Product", time: "8:42 PM in Pune" },
}

const SOURCES: Record<string, string> = {
  mouse: "Opened by hover",
  touch: "Opened by tap",
  pen: "Opened by pen",
  keyboard: "Opened from the keyboard",
  virtual: "Opened by a virtual click",
}

const pointerKind = createPointerKind()

function MentionPreviews() {
  const kind = usePreference(pointerKind)
  const thread = useRef<HTMLDivElement>(null)
  const timer = useRef(0)
  const cardId = useId()
  const [open, setOpen] = useState<string | null>(null)
  const [pinned, setPinned] = useState(false)
  const [person, setPerson] = useState("maya")
  const [place, setPlace] = useState({ x: 0, y: 0, fresh: true })
  const [source, setSource] = useState<string | null>(null)
  const hovers = kind.hover === "hover"

  useEffect(() => () => clearTimeout(timer.current), [])

  const show = (target: HTMLElement, id: string, how: string, pin = false) => {
    clearTimeout(timer.current)
    const box = thread.current!
    const width = Math.min(232, box.clientWidth)
    setPlace({
      x: clamp(target.offsetLeft, 0, box.clientWidth - width),
      y: target.offsetTop + target.offsetHeight + 6,
      fresh: open === null,
    })
    setPerson(id)
    setOpen(id)
    setPinned(pin)
    setSource(how)
  }
  const close = () => {
    clearTimeout(timer.current)
    setOpen(null)
    setPinned(false)
  }
  const closeSoon = () => {
    clearTimeout(timer.current)
    if (!pinned) {
      timer.current = window.setTimeout(close, 220)
    }
  }

  const mention = (id: string) => ({
    type: "button" as const,
    className: "gs-mention",
    "aria-expanded": open === id,
    "aria-controls": cardId,
    onPointerEnter: (event: ReactPointerEvent<HTMLButtonElement>) => {
      if (!hovers || event.pointerType !== "mouse" || pinned) {
        return
      }
      const target = event.currentTarget
      clearTimeout(timer.current)
      timer.current = window.setTimeout(
        () => show(target, id, "mouse"),
        open ? 0 : 320
      )
    },
    onPointerLeave: () => closeSoon(),
    onFocus: (event: FocusEvent<HTMLButtonElement>) => {
      if (isFocusFromKeyboard()) {
        show(event.currentTarget, id, "keyboard")
      }
    },
    onBlur: (event: FocusEvent<HTMLButtonElement>) => {
      const next = event.relatedTarget
      if (!(next instanceof HTMLElement && next.matches(".gs-mention"))) {
        close()
      }
    },
    onClick: (event: ReactMouseEvent<HTMLButtonElement>) => {
      if (open === id && pinned) {
        close()
        return
      }
      show(event.currentTarget, id, sourceOf(event.nativeEvent), true)
    },
  })

  const hint =
    kind.hover === "hover"
      ? "Hover a name to preview it. Click to pin."
      : kind.hover === "none"
        ? "Tap a name to open it."
        : "Point at a name, tap it, or Tab to it."
  const shownPerson = PEOPLE[person]

  return (
    <div
      ref={thread}
      className="gs-thread"
      onKeyDown={(event) => event.key === "Escape" && open && close()}
    >
      <p className="gs-thread-hint">
        <span>{hint}</span>
        {source && <span className="gs-thread-source">{SOURCES[source]}</span>}
      </p>
      <div className="gs-message">
        <span className="gs-message-avatar" aria-hidden>
          AL
        </span>
        <div className="gs-message-body">
          <span className="gs-message-meta">
            <strong>Alex Lindqvist</strong> 10:24
          </span>
          <p className="gs-message-text">
            <button {...mention("maya")}>@Maya</button> can you check the new
            onboarding with <button {...mention("jonas")}>@Jonas</button> before
            Friday? <button {...mention("priya")}>@Priya</button> has the launch
            checklist.
          </p>
        </div>
      </div>
      <div
        id={cardId}
        className="gs-card"
        data-open={open ? "" : undefined}
        data-fresh={place.fresh ? "" : undefined}
        aria-hidden={!open}
        style={{ translate: `${place.x}px ${place.y}px` }}
        onPointerEnter={() => clearTimeout(timer.current)}
        onPointerLeave={closeSoon}
      >
        <span className="gs-card-avatar" aria-hidden>
          {shownPerson.name
            .split(" ")
            .map((part) => part[0])
            .join("")}
        </span>
        <span className="gs-card-name">{shownPerson.name}</span>
        <span className="gs-card-role">{shownPerson.role}</span>
        <span className="gs-card-time">{shownPerson.time}</span>
      </div>
    </div>
  )
}

const tiles: Tile[] = [
  {
    id: "motion-hold",
    title: "Hold to delete.",
    blurb: "Let go early and nothing happens.",
    tone: "paper",
    Demo: HoldToDelete,
  },
  {
    id: "motion-sheet",
    title: "Pull the sheet up.",
    blurb: "Three resting heights; a flick can skip one.",
    size: "tall",
    tone: "sky",
    Demo: MapSheet,
  },
  {
    id: "motion-guests",
    title: "Add guests quickly.",
    blurb: "Hold a button and the count speeds up.",
    tone: "butter",
    Demo: GuestStepper,
  },
  {
    id: "motion-wheel",
    title: "Spin to the hour.",
    blurb: "Flick a wheel, or use the arrow keys.",
    tone: "ink",
    Demo: AlarmWheels,
  },
  {
    id: "motion-mentions",
    title: "Tap opens, hover previews.",
    blurb: "Mentions adapt to a mouse, a finger or the keys.",
    size: "wide",
    Demo: MentionPreviews,
  },
]

export { tiles }
