import {
  ArrowCounterClockwiseIcon,
  BracketsCurlyIcon,
  ChatCircleIcon,
  CheckCircleIcon,
  CheckIcon,
  CreditCardIcon,
  FileCssIcon,
  FileMdIcon,
  FileTsIcon,
  FileTsxIcon,
  ImagesIcon,
  LockSimpleIcon,
  MapTrifoldIcon,
  NotebookIcon,
  NotePencilIcon,
  SidebarSimpleIcon,
  WavesIcon,
  XIcon,
  type Icon,
} from "@phosphor-icons/react"
import {
  bindDrag,
  connectMessage,
  createChange,
  createConnect,
  createLasso,
  createResize,
  css,
  definePrimitive,
  dragThreshold,
  live,
  type ConnectDraft,
  type LassoDraft,
  type ResizeBox,
  type ResizeDraft,
  type ResizeHandle,
} from "@ui-sdk/core"
import {
  createVelocityTracker,
  rubberBand,
  solveSpring,
} from "@ui-sdk/core/gesture"
import {
  clamp,
  scrubTo,
  stepBy,
  stepFromKey,
  type NumberRange,
} from "@ui-sdk/core/number"
import { createParts } from "@ui-sdk/react/primitive"
import {
  Slider,
  SliderControl,
  SliderIndicator,
  SliderThumb,
  SliderTrack,
} from "@ui-sdk/react/slider"
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react"
import type { Tile } from "../bento/types"
import "./drag.css"

const reduced = () =>
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches

/** Runs a 1D spring from `from` to `to`, painting each frame. Jumps to the end with reduced motion. */
function spring(
  from: number,
  to: number,
  velocity: number,
  paint: (value: number) => void,
  { response = 0.42, damping = 0.82 } = {}
) {
  if (reduced() || Math.abs(from - to) < 0.5) {
    paint(to)
    return () => {}
  }
  const solver = solveSpring(response, damping, from - to, velocity)
  const start = performance.now()
  let frame = 0
  const tick = (now: number) => {
    const t = now - start
    const offset = solver.position(t)
    if (
      (Math.abs(offset) < 0.3 && Math.abs(solver.velocity(t)) < 0.01) ||
      t > 1500
    ) {
      return paint(to)
    }
    paint(to + offset)
    frame = requestAnimationFrame(tick)
  }
  frame = requestAnimationFrame(tick)
  return () => cancelAnimationFrame(frame)
}

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) {
      return
    }
    const observer = new ResizeObserver(() => setWidth(el.clientWidth))
    observer.observe(el)
    setWidth(el.clientWidth)
    return () => observer.disconnect()
  }, [])
  return [ref, width] as const
}

/* 1. The dragging sidebar: resize between a min and max, ease into collapsing below it */

type SidebarState = {
  width: number
  opened: number
  draft: number | null
  landing: number | null
  room: number
}

const sidebarLimits = (room: number) =>
  room < 440
    ? { min: 112, max: clamp(room - 132, 128, 190) }
    : { min: 168, max: clamp(room - 236, 200, 300) }

const widthAt = (
  pointer: number,
  { min, max }: { min: number; max: number }
) =>
  pointer >= min
    ? {
        value: Math.min(max, Math.round(pointer)),
        point: undefined as number | undefined,
      }
    : scrubTo(
        pointer,
        {
          min: 0,
          max: min,
          step: 1,
          snap: { points: [0, min], threshold: min / 2 },
        },
        { ease: true }
      )

const sidebar = definePrimitive({
  name: "dg-sidebar",
  state: (): SidebarState => ({
    width: 216,
    opened: 216,
    draft: null,
    landing: null,
    room: 560,
  }),
  api: (context) => {
    const { store } = context
    const limits = () => sidebarLimits(store.get().room)
    const resize = (width: number) =>
      store.set(width ? { width, opened: width } : { width: 0 })
    let stop: (() => void) | undefined
    context.onDestroy(() => stop?.())
    return {
      toggle() {
        const { width, opened } = store.get()
        resize(width ? 0 : opened)
      },
      pointerdown(event: PointerEvent, el: HTMLElement) {
        const root = context.element("root")
        if (event.button !== 0 || !root) {
          return
        }
        stop?.()
        const left = root.getBoundingClientRect().left + root.clientLeft
        // Where on the handle it was grabbed, so the edge doesn't jump to the pointer.
        const grab = event.clientX - left - store.get().width
        stop = bindDrag(el, event, {
          threshold: dragThreshold,
          cancelOnEscape: true,
          onMove: ({ x }) => {
            const { value, point } = widthAt(x - left - grab, limits())
            store.set({ draft: value, landing: point ?? null })
          },
          onEnd: (_end, canceled) => {
            const { draft, landing } = store.get()
            stop = undefined
            store.set({ draft: null, landing: null })
            if (!canceled && draft !== null) {
              resize(Math.round(landing ?? draft))
            }
          },
        })
      },
      keydown(event: KeyboardEvent) {
        const { width, opened } = store.get()
        const { min, max } = limits()
        const step = event.shiftKey ? 48 : 16
        const next =
          event.key === "Enter"
            ? width
              ? 0
              : opened
            : event.key === "ArrowLeft"
              ? width - step < min
                ? 0
                : width - step
              : event.key === "ArrowRight"
                ? width
                  ? Math.min(max, width + step)
                  : min
                : event.key === "Home"
                  ? 0
                  : event.key === "End"
                    ? max
                    : undefined
        if (next === undefined) {
          return
        }
        event.preventDefault()
        resize(next)
      },
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({
        shown: Math.round(state.draft ?? state.width),
        committed: state.width,
        landing: state.landing,
        dragging: state.draft !== null,
        min: sidebarLimits(state.room).min,
      }),
      attrs: (state) => {
        const shown = state.draft ?? state.width
        return {
          style: `--dg-side:${shown}px;--dg-side-min:${sidebarLimits(state.room).min}px`,
          "data-collapsed": shown === 0 ? "" : undefined,
          "data-dragging": state.draft !== null ? "" : undefined,
          "data-landing":
            state.landing === null
              ? undefined
              : state.landing
                ? "open"
                : "collapsed",
        }
      },
      bind: (el, part) => {
        let first = true
        const write = () => {
          const room = el.clientWidth
          if (!room) {
            return
          }
          const { min, max } = sidebarLimits(room)
          const { width, opened } = part.store.get()
          // A phone gets a narrower sidebar to start with, so the note still reads.
          const start = first && room < 440 ? min + 28 : width
          const reopen = first && start ? start : opened
          first = false
          part.store.set({
            room,
            width: start ? clamp(start, min, max) : 0,
            opened: clamp(reopen, min, max),
          })
        }
        const observer = new ResizeObserver(write)
        observer.observe(el)
        write()
        return () => observer.disconnect()
      },
    }),
    divider: slot({
      attrs: (state) => {
        const { min, max } = sidebarLimits(state.room)
        const width = Math.round(state.draft ?? state.width)
        return {
          role: "separator",
          tabindex: "0",
          "aria-label": "Resize sidebar",
          "aria-orientation": "vertical",
          "aria-valuenow": String(width),
          "aria-valuemin": "0",
          "aria-valuemax": String(max),
          "aria-valuetext": width ? `${width} pixels` : "Collapsed",
          "aria-keyshortcuts": "Enter ArrowLeft ArrowRight",
          "data-edge":
            width === 0
              ? "closed"
              : width >= max
                ? "max"
                : width <= min
                  ? "min"
                  : undefined,
          style: "touch-action:none",
        }
      },
      on: {
        pointerdown: (event, { el, api }) => api.pointerdown(event, el!),
        keydown: (event, { api }) => api.keydown(event),
      },
    }),
    toggle: slot({
      attrs: (state) => ({
        type: "button",
        "aria-label": state.width ? "Hide sidebar" : "Show sidebar",
        "aria-expanded": String(state.width > 0),
      }),
      press: ({ api }) => api.toggle(),
    }),
  }),
})

const SidebarParts = createParts(sidebar, {
  root: "div",
  divider: "div",
  toggle: "button",
})

const NAV: { label: string; icon: Icon; count: number; active?: boolean }[] = [
  { label: "All notes", icon: NotePencilIcon, count: 24 },
  { label: "Field logs", icon: NotebookIcon, count: 9, active: true },
  { label: "Specimens", icon: ImagesIcon, count: 31 },
  { label: "Site maps", icon: MapTrifoldIcon, count: 4 },
]

const COUNTS = [3, 4, 4, 6, 8, 7, 11, 12, 14]

function sidebarHint(
  shown: number,
  committed: number,
  landing: number | null,
  dragging: boolean,
  min: number
) {
  if (landing === 0) {
    return { text: "Release to collapse", tone: "fold" }
  }
  if (landing) {
    return {
      text: committed
        ? `Release to rest at ${min} px`
        : `Release to open at ${min} px`,
      tone: "open",
    }
  }
  if (dragging) {
    return { text: `${shown} px`, tone: "size" }
  }
  if (!committed) {
    return { text: "Drag the edge right to open", tone: "idle" }
  }
  return { text: "Drag the edge, or press Enter on it", tone: "idle" }
}

function SidebarDemo() {
  return (
    <div className="dg-fill">
      <SidebarParts.Root className="dg-side">
        {({ shown, committed, landing, dragging, min }) => {
          const hint = sidebarHint(shown, committed, landing, dragging, min)
          return (
            <>
              <nav
                className="dg-side-nav"
                aria-label="Notebook"
                inert={committed === 0 && !dragging}
              >
                <div className="dg-side-inner">
                  <p className="dg-side-space">
                    <span className="dg-side-mark" aria-hidden>
                      <WavesIcon size={16} weight="fill" />
                    </span>
                    Cove survey
                  </p>
                  <ul className="dg-side-list">
                    {NAV.map(({ label, icon: NavIcon, count, active }) => (
                      <li
                        key={label}
                        className="dg-side-item"
                        data-active={active ? "" : undefined}
                      >
                        <NavIcon size={16} weight="fill" aria-hidden />
                        <span className="dg-side-label">{label}</span>
                        <span className="dg-side-count">{count}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              </nav>
              <SidebarParts.Divider className="dg-side-handle">
                <span className="dg-side-grip" aria-hidden />
              </SidebarParts.Divider>
              <section className="dg-side-main" aria-label="Note">
                <header className="dg-side-bar">
                  <SidebarParts.Toggle className="dg-side-toggle">
                    <SidebarSimpleIcon size={16} weight="fill" />
                  </SidebarParts.Toggle>
                  <span className="dg-side-crumb">Field logs</span>
                  <span
                    className="dg-side-width"
                    data-hidden={shown === 0 ? "" : undefined}
                  >
                    {shown ? `${shown} px` : "Hidden"}
                  </span>
                </header>
                <article className="dg-side-doc">
                  <p className="dg-side-meta">Nov 3 · Low tide 06:42</p>
                  <h3 className="dg-side-title">Ochre sea stars return</h3>
                  <p className="dg-side-text">
                    Fourteen adults on the north ledge of transect B, up from
                    three in March. Most sit just under the kelp line, out of
                    the afternoon sun.
                  </p>
                  <figure className="dg-side-chart">
                    <span className="dg-side-bars" aria-hidden>
                      {COUNTS.map((count, index) => (
                        <span
                          key={index}
                          style={{ height: `${(count / 14) * 100}%` }}
                        />
                      ))}
                    </span>
                    <figcaption>Adults counted, March to November</figcaption>
                  </figure>
                </article>
                <p
                  className="dg-side-hint"
                  data-tone={hint.tone}
                  aria-live="polite"
                >
                  {hint.text}
                </p>
              </section>
            </>
          )
        }}
      </SidebarParts.Root>
    </div>
  )
}

/* 2. Swipe to dismiss: a live state shows whether letting go dismisses */

type Toast = {
  id: string
  icon: Icon
  hue: number
  title: string
  body: string
  time: string
}

const TOASTS: Toast[] = [
  {
    id: "build",
    icon: CheckCircleIcon,
    hue: 150,
    title: "Build passed",
    body: "web-app · main · 2m 14s",
    time: "now",
  },
  {
    id: "comment",
    icon: ChatCircleIcon,
    hue: 250,
    title: "Mara Ilunga",
    body: "Can we move the legend under the map?",
    time: "4m",
  },
  {
    id: "payout",
    icon: CreditCardIcon,
    hue: 75,
    title: "Payout sent",
    body: "$2,316.40 to account ending 0932",
    time: "1h",
  },
]

const DISMISS_AT = 0.3

function SwipeToast({
  toast,
  onDismiss,
}: {
  toast: Toast
  onDismiss: (byButton: boolean) => void
}) {
  const [offset, setOffset] = useState(0)
  const [dragging, setDragging] = useState(false)
  const [dismissing, setDismissing] = useState(false)
  const [leaving, setLeaving] = useState(false)
  const card = useRef<HTMLDivElement>(null)
  const stop = useRef<(() => void) | undefined>(undefined)
  const timer = useRef(0)
  useEffect(
    () => () => {
      stop.current?.()
      clearTimeout(timer.current)
    },
    []
  )

  const dismiss = (side: number, byButton: boolean) => {
    const width = card.current?.offsetWidth ?? 260
    setOffset((side || 1) * (width + 16))
    setLeaving(true)
    timer.current = window.setTimeout(
      () => onDismiss(byButton),
      reduced() ? 0 : 440
    )
  }

  const pointerdown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (
      event.button !== 0 ||
      leaving ||
      (event.target as Element).closest("button")
    ) {
      return
    }
    const width = event.currentTarget.offsetWidth
    // One snap point at 0: within 30% of the width there's a point to go back to; past it, none.
    const keeps = {
      min: -width * 2,
      max: width * 2,
      step: 1,
      snap: { points: [0], threshold: width * DISMISS_AT },
    }
    const startX = event.clientX
    let moved = 0
    stop.current?.()
    stop.current = bindDrag(event.currentTarget, event.nativeEvent, {
      threshold: dragThreshold,
      cancelOnEscape: true,
      onStart: () => setDragging(true),
      onMove: ({ x }) => {
        moved = x - startX
        setOffset(moved)
        setDismissing(scrubTo(moved, keeps).point === undefined)
      },
      onEnd: (_end, canceled) => {
        stop.current = undefined
        setDragging(false)
        setDismissing(false)
        if (!canceled && scrubTo(moved, keeps).point === undefined) {
          dismiss(Math.sign(moved), false)
        } else {
          setOffset(0)
        }
      },
    })
  }

  const ToastIcon = toast.icon
  const side = offset < 0 ? "end" : "start"
  return (
    <li className="dg-toast-row" data-leaving={leaving ? "" : undefined}>
      <div className="dg-toast-slot">
        <div
          aria-hidden
          className="dg-toast-behind"
          data-side={side}
          data-dismissing={dismissing || leaving ? "" : undefined}
          style={{
            width: Math.abs(offset) + 18,
            visibility: offset === 0 && !dragging ? "hidden" : undefined,
          }}
        >
          <span>
            {dismissing || leaving ? "Release to dismiss" : "Dismiss"}
          </span>
        </div>
        <div
          ref={card}
          className="dg-toast"
          data-dragging={dragging ? "" : undefined}
          data-dismissing={dismissing ? "" : undefined}
          onPointerDown={pointerdown}
          style={{ translate: `${offset}px 0` }}
        >
          <span
            className="dg-toast-icon"
            style={{ "--dg-hue": toast.hue } as CSSProperties}
            aria-hidden
          >
            <ToastIcon size={16} weight="fill" />
          </span>
          <span className="dg-toast-text">
            <span className="dg-toast-head">
              <span className="dg-toast-title">{toast.title}</span>
              <span className="dg-toast-time">{toast.time}</span>
            </span>
            <span className="dg-toast-body">{toast.body}</span>
          </span>
          <button
            type="button"
            className="dg-toast-close"
            aria-label={`Dismiss ${toast.title}`}
            onClick={() => dismiss(1, true)}
          >
            <XIcon size={14} weight="bold" />
          </button>
        </div>
      </div>
    </li>
  )
}

function ToastsDemo() {
  const [toasts, setToasts] = useState(TOASTS)
  // Set once a toast comes back, so returning toasts fade in; the first paint doesn't animate.
  const [returned, setReturned] = useState(false)
  const list = useRef<HTMLUListElement>(null)
  const again = useRef<HTMLButtonElement>(null)
  const refocus = useRef<number | null>(null)
  useLayoutEffect(() => {
    const at = refocus.current
    if (at === null) {
      return
    }
    refocus.current = null
    const buttons =
      list.current?.querySelectorAll<HTMLButtonElement>(".dg-toast-close")
    const next = buttons?.[Math.min(at, buttons.length - 1)]
    ;(next ?? again.current)?.focus()
  })
  const missing = TOASTS.length - toasts.length
  return (
    <div className="dg-toasts" data-round={returned ? "" : undefined}>
      <header className="dg-toasts-head">
        <p className="dg-toasts-time">8:15</p>
        <p className="dg-toasts-date">Tuesday, November 3</p>
      </header>
      <ul aria-label="Notifications" className="dg-toasts-list" ref={list}>
        {toasts.map((toast, index) => (
          <SwipeToast
            key={toast.id}
            toast={toast}
            onDismiss={(byButton) => {
              if (byButton) {
                refocus.current = index
              }
              setToasts((current) =>
                current.filter((candidate) => candidate.id !== toast.id)
              )
            }}
          />
        ))}
        {toasts.length === 0 && (
          <li className="dg-toasts-empty">Nothing new since 8:00</li>
        )}
      </ul>
      <button
        ref={again}
        type="button"
        className="b-btn dg-toasts-again"
        data-shown={missing ? "" : undefined}
        tabIndex={missing ? 0 : -1}
        aria-hidden={missing ? undefined : true}
        onClick={() => {
          setReturned(true)
          setToasts(TOASTS)
        }}
      >
        <ArrowCounterClockwiseIcon size={16} weight="bold" />
        Show again
      </button>
    </div>
  )
}

/* 3. Reorder tabs: slot edges are snap points, the others slide aside */

const FILES = {
  button: {
    name: "Button.tsx",
    icon: FileTsxIcon,
    path: "src/components",
    line: 'export function Button({ tone = "neutral" }) {',
  },
  tokens: {
    name: "tokens.css",
    icon: FileCssIcon,
    path: "src/styles",
    line: "--radius-control: 10px;",
  },
  changelog: {
    name: "CHANGES.md",
    icon: FileMdIcon,
    path: "docs",
    line: "## 2.4.0 · Focus rings follow the accent",
  },
  vite: {
    name: "index.ts",
    icon: FileTsIcon,
    path: "src",
    line: 'export { Button } from "./components/Button"',
  },
  tsconfig: {
    name: "tsconfig.json",
    icon: BracketsCurlyIcon,
    path: "app",
    line: '"moduleResolution": "bundler",',
  },
}

type FileKey = keyof typeof FILES
type TabDrag = {
  key: FileKey
  left: number
  target: number
  others: FileKey[]
  widths: Record<string, number>
}

const TAB_GAP = 2
const TAB_PAD = 3

const insert = <T,>(others: T[], key: T, at: number) => [
  ...others.slice(0, at),
  key,
  ...others.slice(at),
]

const nearestIndex = (points: number[], value: number) =>
  points.reduce(
    (best, point, index) =>
      Math.abs(point - value) < Math.abs(points[best] - value) ? index : best,
    0
  )

const lefts = (order: FileKey[], widths: Record<string, number>) =>
  order.reduce<number[]>(
    (all, _key, index) => [
      ...all,
      index ? all[index - 1] + widths[order[index - 1]] + TAB_GAP : 0,
    ],
    []
  )

/** A line of code, with strings and a few keywords tinted. */
function Code({ text }: { text: string }) {
  return (
    <code className="dg-code">
      {text.split(/("[^"]*")/).map((piece, index) =>
        piece.startsWith('"') ? (
          <span key={index} className="dg-code-string">
            {piece}
          </span>
        ) : (
          piece.split(/\b(export|default|function)\b/).map((word, inner) =>
            /^(export|default|function)$/.test(word) ? (
              <span key={`${index}-${inner}`} className="dg-code-key">
                {word}
              </span>
            ) : (
              word
            )
          )
        )
      )}
    </code>
  )
}

function TabsDemo() {
  const [order, setOrder] = useState(Object.keys(FILES) as FileKey[])
  const [selected, setSelected] = useState<FileKey>("button")
  const [drag, setDrag] = useState<TabDrag | null>(null)
  const elements = useRef(new Map<FileKey, HTMLButtonElement>())
  const last = useRef(new Map<FileKey, number>())
  const refocus = useRef<FileKey | null>(null)
  const stop = useRef<(() => void) | undefined>(undefined)
  useEffect(() => () => stop.current?.(), [])

  // Tabs that changed place slide there, so a drop or an Alt+arrow move settles instead of jumping.
  useLayoutEffect(() => {
    const next = new Map<FileKey, number>()
    const calm = reduced()
    for (const [key, el] of elements.current) {
      const at = el.offsetLeft
      next.set(key, at)
      const was = last.current.get(key)
      if (drag || calm || was === undefined || Math.abs(was - at) < 0.5) {
        continue
      }
      el.animate([{ translate: `${was - at}px 0` }, { translate: "0 0" }], {
        duration: 220,
        easing: "cubic-bezier(0.22, 1, 0.36, 1)",
      })
    }
    last.current = next
    if (refocus.current) {
      elements.current.get(refocus.current)?.focus()
      refocus.current = null
    }
  })

  const pointerdown = (
    event: ReactPointerEvent<HTMLButtonElement>,
    key: FileKey
  ) => {
    if (event.button !== 0) {
      return
    }
    const widths = Object.fromEntries(
      order.map((candidate) => [
        candidate,
        elements.current.get(candidate)!.getBoundingClientRect().width,
      ])
    )
    const others = order.filter((candidate) => candidate !== key)
    const slots = lefts([...others, key], widths)
    const start = lefts(order, widths)[order.indexOf(key)] - event.clientX
    let latest: TabDrag | null = null
    stop.current?.()
    stop.current = bindDrag(event.currentTarget, event.nativeEvent, {
      threshold: dragThreshold,
      cancelOnEscape: true,
      onMove: ({ x, event: move }) => {
        const { value: left } = scrubTo(
          start + x,
          {
            min: 0,
            max: slots[slots.length - 1],
            step: 1,
            snap: { points: slots, threshold: 12 },
          },
          { snap: !(move.metaKey || move.ctrlKey) }
        )
        latest = {
          key,
          left,
          target: nearestIndex(slots, left),
          others,
          widths,
        }
        setDrag(latest)
      },
      onEnd: (_end, canceled) => {
        stop.current = undefined
        if (latest && !canceled) {
          setOrder(insert(latest.others, latest.key, latest.target))
        }
        setDrag(null)
      },
    })
  }

  const keydown = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    key: FileKey
  ) => {
    const dir =
      getComputedStyle(event.currentTarget).direction === "rtl" ? "rtl" : "ltr"
    const move = stepFromKey(event, { arrows: "horizontal", dir })
    if (!move) {
      return
    }
    event.preventDefault()
    const index = stepBy(order.indexOf(key), move, {
      min: 0,
      max: order.length - 1,
      step: 1,
    })
    if (event.altKey) {
      setOrder(
        insert(
          order.filter((candidate) => candidate !== key),
          key,
          index
        )
      )
      refocus.current = key
      return
    }
    setSelected(order[index])
    elements.current.get(order[index])?.focus()
  }

  const shown = drag ? insert(drag.others, drag.key, drag.target) : order
  const placed = drag ? lefts(shown, drag.widths) : []
  const file = FILES[selected]
  const SelectedIcon = file.icon
  return (
    <div className="dg-tabs">
      <div className="dg-tabs-scroll">
        <div
          role="tablist"
          aria-label="Open files"
          className="dg-tablist"
          data-dragging={drag ? "" : undefined}
          style={
            drag
              ? {
                  width:
                    placed[placed.length - 1] +
                    drag.widths[shown[shown.length - 1]] +
                    TAB_PAD * 2,
                }
              : undefined
          }
        >
          {shown.map((key, index) => {
            const TabIcon = FILES[key].icon
            return (
              <button
                key={key}
                ref={(el) => {
                  if (el) {
                    elements.current.set(key, el)
                  } else {
                    elements.current.delete(key)
                  }
                }}
                type="button"
                role="tab"
                id={`dg-tab-${key}`}
                aria-selected={key === selected}
                aria-controls="dg-tab-panel"
                aria-keyshortcuts="Alt+ArrowLeft Alt+ArrowRight"
                tabIndex={key === selected ? 0 : -1}
                className="dg-tab"
                data-moving={drag?.key === key ? "" : undefined}
                onPointerDown={(event) => pointerdown(event, key)}
                onKeyDown={(event) => keydown(event, key)}
                onClick={() => setSelected(key)}
                style={
                  drag
                    ? {
                        left:
                          (drag.key === key ? drag.left : placed[index]) +
                          TAB_PAD,
                      }
                    : undefined
                }
              >
                <TabIcon size={14} weight="fill" aria-hidden />
                {FILES[key].name}
              </button>
            )
          })}
        </div>
      </div>
      <div
        role="tabpanel"
        id="dg-tab-panel"
        aria-labelledby={`dg-tab-${selected}`}
        className="dg-tabs-panel"
      >
        <p className="dg-tabs-path">
          <SelectedIcon size={14} weight="fill" aria-hidden />
          <span>
            {file.path} / <strong>{file.name}</strong>
          </span>
          <span className="dg-tabs-pos">
            Tab {order.indexOf(selected) + 1} of {order.length}
          </span>
        </p>
        <p className="dg-tabs-line">
          <span className="dg-tabs-num" aria-hidden>
            1
          </span>
          <Code text={file.line} />
        </p>
      </div>
      <p className="dg-tabs-hint">
        <span>
          <kbd>⌘</kbd> while dragging places a tab freely
        </span>
        <span>
          <kbd>⌥</kbd>
          <kbd>←</kbd>
          <kbd>→</kbd> moves the focused tab
        </span>
      </p>
    </div>
  )
}

/* 4. Song timeline: the playhead snaps to section starts within 1.5 s */

const SECTIONS = [
  { at: 0, label: "Intro", hue: 250 },
  { at: 9.5, label: "Verse", hue: 195 },
  { at: 24, label: "Chorus", hue: 70 },
  { at: 38.5, label: "Bridge", hue: 150 },
  { at: 52, label: "Outro", hue: 25 },
]
const SONG = 60
const songRange: NumberRange = {
  min: 0,
  max: SONG,
  step: 0.1,
  largeStep: 5,
  snap: { points: SECTIONS.map((section) => section.at), threshold: 1.5 },
}

const clock = (time: number) =>
  `${Math.floor(time / 60)}:${(time % 60).toFixed(1).padStart(4, "0")}`
const sectionAt = (time: number) =>
  SECTIONS.reduce(
    (found, section) => (section.at <= time ? section : found),
    SECTIONS[0]
  )
const percent = (time: number) => `${(time / SONG) * 100}%`

// A waveform that reads like the arrangement: quiet intro, a loud chorus, a fading outro.
const BARS = Array.from({ length: 120 }, (_, index) => {
  const time = ((index + 0.5) / 120) * SONG
  const section = sectionAt(time)
  const energy = {
    Intro: 0.32,
    Verse: 0.56,
    Chorus: 0.9,
    Bridge: 0.48,
    Outro: 0.5 - (time - 52) / 20,
  }[section.label]!
  const noise = Math.abs(Math.sin(index * 12.9898) * 43758.5453) % 1
  const pulse = 0.72 + 0.28 * Math.sin(index * 0.9)
  return {
    height: Math.max(0.08, energy * (0.55 + 0.45 * noise) * pulse),
    hue: section.hue,
  }
})

function Waveform() {
  return (
    <svg
      className="dg-wave"
      viewBox="0 0 120 40"
      preserveAspectRatio="none"
      aria-hidden
    >
      {BARS.map((bar, index) => (
        <rect
          key={index}
          x={index + 0.18}
          width={0.64}
          y={(20 - bar.height * 19).toFixed(3)}
          height={(bar.height * 38).toFixed(3)}
          style={{ fill: `oklch(0.78 0.12 ${bar.hue})` }}
        />
      ))}
    </svg>
  )
}

function TimelineDemo() {
  const [time, setTime] = useState(17)
  const section = SECTIONS.find((candidate) => candidate.at === time)
  const current = sectionAt(time)
  return (
    <Slider
      className="dg-tl"
      value={time}
      min={0}
      max={SONG}
      step={0.1}
      largeStep={5}
      format={(value) => {
        const on = SECTIONS.find((candidate) => candidate.at === value)
        return on ? `${clock(value)}, ${on.label}` : clock(value)
      }}
      onValueChange={(next, details) => {
        const value = next as number
        const event = details.event as
          (KeyboardEvent & PointerEvent) | undefined
        const free = !!(event?.metaKey || event?.ctrlKey)
        const move =
          details.reason === "step-key" && event ? stepFromKey(event) : null
        if (move) {
          setTime(stepBy(time, move, songRange, { snap: !free }))
        } else {
          setTime(scrubTo(value, songRange, { snap: !free }).value)
        }
      }}
      style={{ "--dg-played": percent(time) } as CSSProperties}
    >
      <div className="dg-tl-head">
        <p className="dg-tl-lcd" aria-hidden>
          <span className="dg-tl-clock">{clock(time)}</span>
          <span className="dg-tl-total">/ 1:00.0</span>
        </p>
        <p className="dg-tl-section" data-on={section ? "" : undefined}>
          <span
            className="dg-tl-dot"
            style={{ "--dg-hue": current.hue } as CSSProperties}
            aria-hidden
          />
          {section ? section.label : `In ${current.label}`}
        </p>
        <p className="dg-tl-meta">
          <span className="dg-tl-track">Glasshouse · demo 3</span>
          <span className="dg-tl-free">
            <kbd>⌘</kbd> scrubs freely
          </span>
        </p>
      </div>
      <SliderControl className="dg-tl-control">
        <div className="dg-tl-art" aria-hidden>
          <div className="dg-tl-lanes">
            {SECTIONS.map((candidate, index) => (
              <span
                key={candidate.label}
                className="dg-tl-lane"
                data-active={candidate === section ? "" : undefined}
                style={
                  {
                    left: percent(candidate.at),
                    width: percent(
                      (SECTIONS[index + 1]?.at ?? SONG) - candidate.at
                    ),
                    "--dg-hue": candidate.hue,
                  } as CSSProperties
                }
              >
                {candidate.label}
              </span>
            ))}
          </div>
          <div className="dg-tl-waves">
            <Waveform />
            <div className="dg-tl-played">
              <Waveform />
            </div>
          </div>
        </div>
        <SliderTrack className="dg-tl-track-rail">
          <SliderIndicator className="dg-tl-fill" />
          <SliderThumb aria-label="Playhead" className="dg-tl-head-thumb">
            <span className="dg-tl-cap" aria-hidden />
          </SliderThumb>
        </SliderTrack>
      </SliderControl>
      <div className="dg-tl-ruler" aria-hidden>
        {["0:00", "0:15", "0:30", "0:45", "1:00"].map((mark) => (
          <span key={mark}>{mark}</span>
        ))}
      </div>
    </Slider>
  )
}

/* 5. Rubber band: a day-by-day pager that resists past its ends and springs home */

const DAYS = [
  {
    day: "Day 1 · Fri",
    title: "Ballarò market",
    detail: "Arancine by 10:00",
    art: "a",
  },
  {
    day: "Day 2 · Sat",
    title: "Scala dei Turchi",
    detail: "Swim before the crowds",
    art: "b",
  },
  {
    day: "Day 3 · Sun",
    title: "Etna at dusk",
    detail: "Cable car up at 16:30",
    art: "c",
  },
]
const PEEK = 18
const CARD_GAP = 10

function BandDemo() {
  const [viewport, width] = useWidth<HTMLDivElement>()
  const [page, setPage] = useState(0)
  const [dragging, setDragging] = useState(false)
  const [speed, setSpeed] = useState<number | null>(null)
  const track = useRef<HTMLDivElement>(null)
  const position = useRef(PEEK)
  const cancel = useRef<() => void>(() => {})
  const stop = useRef<(() => void) | undefined>(undefined)
  const [tracker] = useState(() => ({
    current: createVelocityTracker({ maxSpeed: 4 }),
  }))

  const card = Math.max(120, width - PEEK * 2)
  const pitch = card + CARD_GAP
  const restAt = (index: number) => PEEK - index * pitch

  const paint = (x: number) => {
    position.current = x
    track.current?.style.setProperty("--dg-x", `${x}px`)
  }
  const settle = (index: number, velocity: number) => {
    cancel.current()
    cancel.current = spring(position.current, restAt(index), velocity, paint)
  }
  useEffect(
    () => () => {
      cancel.current()
      stop.current?.()
    },
    []
  )
  // Keep the page in place when the tile changes size.
  useLayoutEffect(() => {
    if (!width) {
      return
    }
    cancel.current()
    paint(PEEK - page * (Math.max(120, width - PEEK * 2) + CARD_GAP))
    // Only a new width should snap it; page changes settle with a spring.
  }, [width])

  const go = (index: number, velocity: number) => {
    setPage(index)
    settle(index, velocity)
  }

  const down = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) {
      return
    }
    cancel.current()
    tracker.current.reset()
    const from = position.current
    const startX = event.clientX
    const startPage = page
    stop.current?.()
    stop.current = bindDrag(event.currentTarget, event.nativeEvent, {
      threshold: dragThreshold,
      cancelOnEscape: true,
      onStart: () => setDragging(true),
      onMove: ({ x, event: move }) => {
        const raw = from + x - startX
        const first = restAt(0)
        const final = restAt(DAYS.length - 1)
        // Past either end, the pull shows less and less of itself.
        const shown =
          raw > first
            ? first + rubberBand(raw - first, width)
            : raw < final
              ? final - rubberBand(final - raw, width)
              : raw
        tracker.current.add(move.timeStamp, shown)
        paint(shown)
      },
      onEnd: (end, canceled) => {
        stop.current = undefined
        setDragging(false)
        const velocity = canceled ? 0 : tracker.current.velocity(end.timeStamp)
        if (!canceled) {
          setSpeed(velocity)
        }
        const moved = position.current - restAt(startPage)
        const flick = Math.abs(velocity) > 0.35 ? -Math.sign(velocity) : 0
        const far = Math.abs(moved) > pitch / 2 ? -Math.sign(moved) : 0
        const next = canceled
          ? startPage
          : clamp(startPage + (flick || far), 0, DAYS.length - 1)
        go(next, velocity)
      },
    })
  }

  const keydown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step =
      event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0
    if (!step) {
      return
    }
    event.preventDefault()
    const next = page + step
    // At an end, a key nudges past it, so the edge still answers.
    if (next < 0 || next >= DAYS.length) {
      settle(page, -step * 1.1)
    } else {
      go(next, 0)
    }
  }

  return (
    <div className="dg-band">
      <div
        ref={viewport}
        className="dg-band-view"
        role="group"
        aria-roledescription="carousel"
        aria-label="Sicily itinerary"
        tabIndex={0}
        data-dragging={dragging ? "" : undefined}
        onPointerDown={down}
        onKeyDown={keydown}
      >
        <div
          ref={track}
          className="dg-band-track"
          style={{ "--dg-card": `${card}px` } as CSSProperties}
        >
          {DAYS.map((day, index) => (
            <article
              key={day.title}
              className="dg-band-card"
              aria-roledescription="slide"
              aria-label={`${index + 1} of ${DAYS.length}`}
              aria-hidden={index !== page}
            >
              <span className="dg-band-art" data-art={day.art} aria-hidden />
              <span className="dg-band-day">{day.day}</span>
              <span className="dg-band-title">{day.title}</span>
              <span className="dg-band-detail">{day.detail}</span>
            </article>
          ))}
        </div>
      </div>
      <div className="dg-band-foot">
        <span className="dg-band-dots" aria-hidden>
          {DAYS.map((day, index) => (
            <span key={day.title} data-on={index === page ? "" : undefined} />
          ))}
        </span>
        <span className="dg-band-speed" aria-live="polite">
          {speed === null
            ? "Drag past the last day"
            : `Let go at ${Math.abs(speed).toFixed(2)} px/ms`}
        </span>
      </div>
    </div>
  )
}

/* 6. Patch bay: drag cables from sources into channels */

type Cable = { from: string; to: string }
type BayState = { cables: Cable[]; drawing: ConnectDraft | null }

const SOURCES = ["mic", "gtr", "bass"]
const CHANNELS = ["ch1", "ch2", "ch3"]
const PORTS: Record<string, { label: string; name: string }> = {
  mic: { label: "MIC", name: "Mic" },
  gtr: { label: "GTR", name: "Guitar" },
  bass: { label: "BASS", name: "Bass" },
  ch1: { label: "CH 1", name: "Channel 1" },
  ch2: { label: "CH 2", name: "Channel 2" },
  ch3: { label: "CH 3", name: "Channel 3" },
}

function jackAt(port: string) {
  const source = SOURCES.includes(port)
  const row = (source ? SOURCES : CHANNELS).indexOf(port)
  return { x: source ? 92 : 150, y: 52 + row * 38 }
}

/** A cable hangs: both handles sit below the lower end, deeper the farther it reaches. */
function cablePath(
  from: { x: number; y: number },
  to: { x: number; y: number }
) {
  const dx = to.x - from.x
  const sag = 12 + Math.abs(dx) * 0.12 + Math.abs(to.y - from.y) * 0.2
  const low = Math.max(from.y, to.y) + sag
  return `M${from.x},${from.y} C${from.x + dx * 0.3},${low} ${to.x - dx * 0.3},${low} ${to.x},${to.y}`
}

const bay = definePrimitive({
  name: "dg-bay",
  state: (): BayState => ({
    cables: [{ from: "mic", to: "ch2" }],
    drawing: null,
  }),
  live: ["drawing"],
  api: (context) => {
    const { store } = context
    const change = createChange(context)
    const connect = createConnect<BayState>(context, {
      slot: "jack",
      draft: "drawing",
      canConnect: (from, to) => SOURCES.includes(from) && CHANNELS.includes(to),
      point: (clientX, clientY) => {
        const box = context.element("root")!.getBoundingClientRect()
        return { x: clientX - box.left, y: clientY - box.top }
      },
      radius: () => 30,
      label: (port) => PORTS[port].name,
      message: (message, label) =>
        message.type === "connect"
          ? `${label(message.from)} into ${label(message.to)}.`
          : connectMessage(message, label),
      connect: (from, to, details) => {
        const { cables } = store.get()
        const patched = cables.some(
          (cable) => cable.from === from && cable.to === to
        )
        // Patching a pair again unplugs it; a channel takes one cable.
        const next = patched
          ? cables.filter((cable) => cable.from !== from || cable.to !== to)
          : [...cables.filter((cable) => cable.to !== to), { from, to }]
        return change("cables", next, details)
      },
    })
    return {
      connect,
      sourceKeydown(event: KeyboardEvent, source: string) {
        if (connect.keydown(event)) {
          return
        }
        if (event.key !== "Enter" && event.key !== " ") {
          return
        }
        event.preventDefault()
        connect.start(source)
      },
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({ cables: state.cables }),
      attrs: () => ({ role: "group", "aria-label": "Patch bay" }),
    }),
    source: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { key }) => {
        const into = state.cables
          .filter((cable) => cable.from === key)
          .map((cable) => PORTS[cable.to].name)
        return {
          type: "button",
          "aria-label": PORTS[key].name,
          "aria-description": `${into.length ? `Into ${into.join(", ")}. ` : ""}Press Enter to patch.`,
          "data-source": key,
        }
      },
      on: { keydown: (event, { api, key }) => api.sourceKeydown(event, key) },
    }),
    jack: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => {
        const { x, y } = jackAt(key)
        return {
          "aria-hidden": "true",
          style: css({ left: `${x}px`, top: `${y}px`, "touch-action": "none" }),
        }
      },
      live: live((state, { api, key }) => ({
        "data-connect": api.connect.role(state, key),
      })),
      on: {
        pointerdown: (event, { el, api, key }) =>
          api.connect.pointerdown(event, key, el!),
      },
    }),
    draft: slot({
      live: live((state) => {
        const draft = state.drawing
        if (!draft) {
          return { d: "", "data-source": undefined }
        }
        return {
          d: cablePath(jackAt(draft.from), draft.to ? jackAt(draft.to) : draft),
          "data-source": draft.from,
        }
      }),
    }),
    tip: slot({
      live: live((state) => {
        const draft = state.drawing
        if (!draft) {
          return { transform: "translate(-40 -40)", "data-source": undefined }
        }
        const at = draft.to ? jackAt(draft.to) : draft
        return {
          transform: `translate(${at.x} ${at.y})`,
          "data-source": draft.from,
        }
      }),
    }),
  }),
})

const BayParts = createParts(bay, {
  root: "div",
  source: "button",
  jack: "span",
  draft: "path",
  tip: "g",
})

function Plug({
  at,
  source,
}: {
  at: { x: number; y: number }
  source: string
}) {
  return (
    <g
      className="dg-plug"
      data-source={source}
      transform={`translate(${at.x} ${at.y})`}
    >
      <circle r={7} />
      <circle r={3.2} className="dg-plug-core" />
    </g>
  )
}

function PatchDemo() {
  return (
    <BayParts.Root className="dg-bay">
      {({ cables }) => (
        <>
          <span className="dg-bay-screw" data-at="tl" aria-hidden />
          <span className="dg-bay-screw" data-at="tr" aria-hidden />
          <span className="dg-bay-screw" data-at="bl" aria-hidden />
          <span className="dg-bay-screw" data-at="br" aria-hidden />
          <p className="dg-bay-rail" aria-hidden>
            <span>SOURCE</span>
            <span>CHANNEL</span>
          </p>
          {SOURCES.map((source, index) => (
            <BayParts.Source
              key={source}
              value={source}
              className="dg-bay-source"
              style={{ top: 40 + index * 38 }}
            >
              <span className="dg-bay-swatch" aria-hidden />
              {PORTS[source].label}
            </BayParts.Source>
          ))}
          {CHANNELS.map((channel, index) => {
            const on = cables.find((cable) => cable.to === channel)
            return (
              <span
                key={channel}
                className="dg-bay-channel"
                style={{ top: 40 + index * 38 }}
                data-on={on ? "" : undefined}
              >
                {PORTS[channel].label}
                <span className="dg-bay-led" aria-hidden />
              </span>
            )
          })}
          {[...SOURCES, ...CHANNELS].map((port) => (
            <BayParts.Jack key={port} value={port} className="dg-bay-jack" />
          ))}
          <svg className="dg-bay-cables" aria-hidden>
            {cables.map((cable) => (
              <g
                key={`${cable.from}-${cable.to}`}
                className="dg-cable"
                data-source={cable.from}
              >
                <path d={cablePath(jackAt(cable.from), jackAt(cable.to))} />
                <path
                  className="dg-cable-shine"
                  d={cablePath(jackAt(cable.from), jackAt(cable.to))}
                />
              </g>
            ))}
            {cables.map((cable) => (
              <g key={`plugs-${cable.from}-${cable.to}`}>
                <Plug at={jackAt(cable.from)} source={cable.from} />
                <Plug at={jackAt(cable.to)} source={cable.from} />
              </g>
            ))}
            <BayParts.Draft className="dg-cable dg-cable-draft" />
            <BayParts.Tip className="dg-plug dg-plug-tip">
              <circle r={7} />
              <circle r={3.2} className="dg-plug-core" />
            </BayParts.Tip>
          </svg>
        </>
      )}
    </BayParts.Root>
  )
}

/* 7. Lasso: draw a rectangle across photos; locked ones stay out */

type PhotosState = { selected: string[]; lasso: LassoDraft | null }

const PHOTOS = ["Dunes", "Fjord", "Lagoon", "Glacier", "Mesa", "Aurora"]
const LOCKED = ["Lagoon", "Mesa"]

const photos = definePrimitive({
  name: "dg-photos",
  state: (): PhotosState => ({ selected: ["Fjord"], lasso: null }),
  live: ["lasso"],
  api: (context) => {
    const change = createChange(context)
    return {
      lasso: createLasso<PhotosState>(context, {
        slot: "photo",
        draft: "lasso",
        selected: () => context.store.get().selected,
        set: (keys, details) => change("selected", keys, details),
        selectable: (key) => !LOCKED.includes(key),
        message: (count) =>
          `${count} ${count === 1 ? "photo" : "photos"} selected`,
      }),
      clear: () => context.store.set({ selected: [] }),
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({ count: state.selected.length }),
      attrs: () => ({
        role: "group",
        "aria-label": "Photos",
        style: "touch-action:none",
      }),
      on: {
        pointerdown: (event, { el, api }) =>
          event.target === el && api.lasso.pointerdown(event, el!),
      },
    }),
    photo: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { key }) => {
        const selected = state.selected.includes(key)
        return {
          role: "img",
          "aria-label": `${key}${LOCKED.includes(key) ? ", locked" : ""}${selected ? ", selected" : ""}`,
          "data-selected": selected ? "" : undefined,
          "data-locked": LOCKED.includes(key) ? "" : undefined,
          "data-photo": key.toLowerCase(),
        }
      },
      live: live((state, { key, api }) => ({
        "data-lassoed": api.lasso.touches(state, key) ? "" : undefined,
      })),
    }),
    rect: slot({
      attrs: () => ({ "aria-hidden": "true" }),
      live: live((state, { api }) => ({ style: api.lasso.style(state) })),
    }),
    clear: slot({
      attrs: (state) => ({
        type: "button",
        "data-shown": state.selected.length ? "" : undefined,
      }),
      press: ({ api }) => api.clear(),
    }),
  }),
})

const PhotoParts = createParts(photos, {
  root: "div",
  photo: "div",
  rect: "div",
  clear: "button",
})

function LassoDemo() {
  return (
    <PhotoParts.Root className="dg-photos">
      {({ count }) => (
        <>
          <p className="dg-photos-head">
            <span aria-live="polite">
              {count
                ? `${count} ${count === 1 ? "photo" : "photos"} selected`
                : "Drag across the photos"}
            </span>
            <PhotoParts.Clear className="dg-photos-clear">
              Clear
            </PhotoParts.Clear>
          </p>
          {PHOTOS.map((name) => (
            <PhotoParts.Photo key={name} value={name} className="dg-photo">
              <span className="dg-photo-name">{name}</span>
              <span className="dg-photo-badge" aria-hidden>
                {LOCKED.includes(name) ? (
                  <LockSimpleIcon size={14} weight="fill" />
                ) : (
                  <CheckIcon size={14} weight="bold" />
                )}
              </span>
            </PhotoParts.Photo>
          ))}
          <PhotoParts.Rect className="dg-lasso" />
        </>
      )}
    </PhotoParts.Root>
  )
}

/* 8. Desktop windows: resize from any edge or corner, move by the title bar */

type DesktopState = {
  windows: Record<string, ResizeBox>
  draft: ResizeDraft | null
  stack: string[]
  active: string
  area: { w: number; h: number }
}

const WINDOWS: Record<string, { title: string; locked?: boolean }> = {
  reminders: { title: "Reminders" },
  console: { title: "Console" },
  timer: { title: "Timer", locked: true },
}

const EDGES: Exclude<ResizeHandle, "move" | "se">[] = [
  "n",
  "e",
  "s",
  "w",
  "ne",
  "nw",
  "sw",
]

const desktop = definePrimitive({
  name: "dg-desktop",
  state: (): DesktopState => ({
    windows: {
      reminders: { x: 8, y: 8, width: 122, height: 82 },
      console: { x: 108, y: 60, width: 134, height: 80 },
      timer: { x: 142, y: 8, width: 100, height: 46 },
    },
    draft: null,
    stack: ["timer", "reminders", "console"],
    active: "console",
    area: { w: 250, h: 150 },
  }),
  api: (context) => {
    const { store } = context
    const change = createChange(context)
    const resize = createResize<DesktopState>(context, {
      boxes: (state) => state.windows,
      draft: "draft",
      set: (value, details) => change("windows", value, details),
      min: () => ({ width: 96, height: 56 }),
      bounds: () => {
        const { area } = store.get()
        return { left: 0, top: 0, right: area.w, bottom: area.h }
      },
      step: () => 8,
      disabled: (key) => !!WINDOWS[key].locked,
      label: (key) => WINDOWS[key].title,
      message: ({ label, handle, box }) =>
        handle === "move"
          ? `${label} moved.`
          : `${label} is ${Math.round(box.width)} by ${Math.round(box.height)}.`,
    })
    return {
      resize,
      raise(key: string) {
        const { stack } = store.get()
        store.set({
          active: key,
          stack:
            stack[stack.length - 1] === key
              ? stack
              : [...stack.filter((k) => k !== key), key],
        })
      },
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => {
        const box = api.resize.box(state, state.active)
        return {
          active: state.active,
          w: Math.round(box.width),
          h: Math.round(box.height),
        }
      },
      attrs: () => ({ role: "group", "aria-label": "Desktop" }),
    }),
    area: slot({
      bind: (el, part) => {
        const write = () =>
          el.clientWidth &&
          part.store.set({ area: { w: el.clientWidth, h: el.clientHeight } })
        const observer = new ResizeObserver(write)
        observer.observe(el)
        write()
        return () => observer.disconnect()
      },
    }),
    window: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { api, key }) => ({
        role: "group",
        "aria-label": WINDOWS[key].title,
        "aria-disabled": WINDOWS[key].locked ? "true" : undefined,
        "data-resizing": api.resize.dragging(state, key),
        "data-front":
          state.stack[state.stack.length - 1] === key ? "" : undefined,
        style: [
          api.resize.boxStyle(state, key),
          api.resize.style(state, key),
          `z-index:${state.stack.indexOf(key) + 1}`,
        ]
          .filter(Boolean)
          .join(";"),
      }),
      on: { pointerdown: (_event, { api, key }) => api.raise(key) },
    }),
    titlebar: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) =>
        WINDOWS[key].locked
          ? {}
          : {
              tabindex: "0",
              role: "button",
              "aria-label": `Move ${WINDOWS[key].title}`,
              "aria-keyshortcuts": "ArrowLeft ArrowRight ArrowUp ArrowDown",
              style: "touch-action:none",
            },
      on: {
        pointerdown: (event, { el, api, key }) =>
          api.resize.pointerdown(event, key, "move", el!),
        keydown: (event, { api, key }) =>
          void api.resize.keydown(event, key, "move"),
        focus: (_event, { api, key }) => api.raise(key),
      },
    }),
    edge: slot<{ id: string; box: string; handle: string }>({
      props: { id: "string", box: "string", handle: "string" },
      attrs: (_state, { api, props }) => ({
        ...api.resize.handleAttrs(props.handle as ResizeHandle),
        "aria-hidden": "true",
      }),
      on: {
        pointerdown: (event, { el, api, props }) =>
          api.resize.pointerdown(
            event,
            props.box,
            props.handle as ResizeHandle,
            el!
          ),
      },
    }),
    grip: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { api, key }) => ({
        ...api.resize.handleAttrs("se"),
        type: "button",
        "aria-label": `Resize ${WINDOWS[key].title}`,
      }),
      on: {
        pointerdown: (event, { el, api, key }) =>
          api.resize.pointerdown(event, key, "se", el!),
        keydown: (event, { api, key }) =>
          void api.resize.keydown(event, key, "se"),
        focus: (_event, { api, key }) => api.raise(key),
      },
    }),
  }),
})

const DesktopParts = createParts(desktop, {
  root: "div",
  area: "div",
  window: "div",
  titlebar: "div",
  edge: "span",
  grip: "button",
})

function WindowBody({ id }: { id: string }) {
  if (id === "reminders") {
    return (
      <ul className="dg-win-todo">
        <li data-done="">Renew passport</li>
        <li>Call the vet</li>
      </ul>
    )
  }
  if (id === "console") {
    return (
      <p className="dg-win-shell">
        <span>$ npm run build</span>
        <span className="dg-win-ok">built in 1.84s</span>
      </p>
    )
  }
  return <p className="dg-win-timer">24:59</p>
}

function WindowsDemo() {
  return (
    <DesktopParts.Root className="dg-desk">
      {({ active, w, h }) => (
        <>
          <p className="dg-desk-menu">
            <strong>{WINDOWS[active].title}</strong>
            <span>
              {w} × {h}
            </span>
          </p>
          <DesktopParts.Area className="dg-desk-area">
            {Object.entries(WINDOWS).map(([id, win]) => (
              <DesktopParts.Window key={id} value={id} className="dg-win">
                <DesktopParts.Titlebar value={id} className="dg-win-title">
                  <span className="dg-win-lights" aria-hidden>
                    <span />
                    <span />
                    <span />
                  </span>
                  <span className="dg-win-name">{win.title}</span>
                  {win.locked && (
                    <LockSimpleIcon
                      size={14}
                      weight="fill"
                      aria-label="Locked"
                    />
                  )}
                </DesktopParts.Titlebar>
                <WindowBody id={id} />
                {!win.locked && (
                  <>
                    {EDGES.map((handle) => (
                      <DesktopParts.Edge
                        key={handle}
                        id={`${id}-${handle}`}
                        box={id}
                        handle={handle}
                        className="dg-win-edge"
                        data-at={handle}
                      />
                    ))}
                    <DesktopParts.Grip value={id} className="dg-win-grip" />
                  </>
                )}
              </DesktopParts.Window>
            ))}
          </DesktopParts.Area>
        </>
      )}
    </DesktopParts.Root>
  )
}

export const tiles: Tile[] = [
  {
    id: "drag-sidebar",
    title: "Fold the sidebar.",
    blurb: "Drag below its minimum and it eases shut.",
    size: "large",
    Demo: SidebarDemo,
  },
  {
    id: "drag-toasts",
    title: "Swipe a toast away.",
    blurb: "Past a third, letting go dismisses it.",
    size: "tall",
    tone: "sky",
    Demo: ToastsDemo,
  },
  {
    id: "drag-tabs",
    title: "Reorder the tabs.",
    blurb: "Each slot snaps the tab in as it nears.",
    size: "wide",
    tone: "paper",
    Demo: TabsDemo,
  },
  {
    id: "drag-timeline",
    title: "Scrub to the chorus.",
    blurb: "The playhead snaps to each section.",
    size: "wide",
    tone: "ink",
    Demo: TimelineDemo,
  },
  {
    id: "drag-band",
    title: "Pull past the end.",
    blurb: "It resists, then springs home.",
    tone: "signal",
    Demo: BandDemo,
  },
  {
    id: "drag-patch",
    title: "Patch a cable.",
    blurb: "Drag from a source into a channel.",
    tone: "paper",
    Demo: PatchDemo,
  },
  {
    id: "drag-lasso",
    title: "Lasso a few photos.",
    blurb: "Locked ones stay out; Shift adds more.",
    Demo: LassoDemo,
  },
  {
    id: "drag-windows",
    title: "Resize the windows.",
    blurb: "Grab any edge or corner.",
    tone: "sage",
    Demo: WindowsDemo,
  },
]
