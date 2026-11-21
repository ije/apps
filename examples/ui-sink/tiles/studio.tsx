import {
  ArrowCounterClockwiseIcon,
  ArrowDownIcon,
  ArrowUUpLeftIcon,
  ArrowUUpRightIcon,
  CaretLeftIcon,
  CaretRightIcon,
  CoffeeIcon,
  CornersOutIcon,
  CrosshairSimpleIcon,
  FilePdfIcon,
  FilmStripIcon,
  ImageIcon,
  LockSimpleIcon,
  MagnifyingGlassIcon,
  MagnifyingGlassMinusIcon,
  MagnifyingGlassPlusIcon,
  MapTrifoldIcon,
  PaperPlaneRightIcon,
  PauseIcon,
  PlayIcon,
  TrashIcon,
} from "@phosphor-icons/react"
import {
  announce,
  createChange,
  createController,
  createGridFocus,
  createMove,
  createRovingFocus,
  createSpans,
  createStickToBottom,
  createUndoStack,
  createViewport,
  createVirtualizer,
  css,
  definePrimitive,
  live,
  matchesShortcut,
  spatialMove,
  stackSpans,
  type ChangeDetails,
  type GridCell,
  type MoveDraft,
  type MovePoint,
  type Span,
  type SpansDraft,
  type VirtualItem,
} from "@ui-sdk/core"
import { fuzzyScore } from "@ui-sdk/core/text-match"
import { createParts } from "@ui-sdk/react/primitive"
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react"
import { useShortcutHint } from "@ui-sdk/react/shortcuts"
import type { Tile } from "../bento/types"
import "./studio.css"

/* Shared helpers */

const reduced = () =>
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches

/** A shortcut's label comes wrapped in direction isolates; keycaps want the bare glyphs. */
const keycaps = (label: string) => {
  const bare = label.replace(/[\u2066-\u2069]/g, "")
  return bare.includes("+") ? bare.split("+") : [...bare]
}

function Keys({ shortcut }: { shortcut: string }) {
  const hint = useShortcutHint(shortcut)
  return (
    <span className="st-keys" aria-hidden="true">
      {keycaps(hint.text).map((key, index) => (
        <kbd key={index} className="st-kbd">
          {key}
        </kbd>
      ))}
    </span>
  )
}

function observeWidth(el: HTMLElement, write: (width: number) => void) {
  const observer = new ResizeObserver(() => write(el.clientWidth))
  observer.observe(el)
  write(el.clientWidth)
  return () => observer.disconnect()
}

const clock = (minutes: number) =>
  `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")}`

/* 1. History: a note with undo and redo */

const NOTE_SEED = [
  "",
  "Water the ferns",
  "Water the ferns, return the library books",
  "Water the ferns, return the library books, buy oat milk",
]
const NOTE_LIMIT = 12
const NOTE_DELAY = 800

const notes = definePrimitive({
  name: "st-notes",
  state: () => ({
    text: NOTE_SEED[NOTE_SEED.length - 1],
    back: NOTE_SEED.length - 1,
    ahead: 0,
  }),
  api: (context) => {
    const { store } = context
    const history = createUndoStack(NOTE_SEED[0], {
      limit: NOTE_LIMIT,
      groupDelay: NOTE_DELAY,
    })
    for (const text of NOTE_SEED.slice(1)) {
      history.record(text)
    }
    // The history keeps its entries private, so the step counts mirror its grouping rule.
    let open: number | null = null
    const api = {
      type(text: string) {
        const time = Date.now()
        const joins = open !== null && time - open < NOTE_DELAY
        history.record(text, { group: "typing", time })
        open = time
        const { back } = store.get()
        store.set({
          text,
          back: joins ? back : Math.min(back + 1, NOTE_LIMIT),
          ahead: 0,
        })
      },
      seal() {
        history.seal()
        open = null
      },
      undo() {
        open = null
        const text = history.undo()
        if (text === undefined) {
          return
        }
        const { back, ahead } = store.get()
        store.set({ text, back: back - 1, ahead: ahead + 1 })
      },
      redo() {
        open = null
        const text = history.redo()
        if (text === undefined) {
          return
        }
        const { back, ahead } = store.get()
        store.set({ text, back: back + 1, ahead: ahead - 1 })
      },
      keydown(event: KeyboardEvent) {
        if (matchesShortcut(event, "mod+shift+z")) {
          api.redo()
        } else if (matchesShortcut(event, "mod+z")) {
          api.undo()
        } else {
          return
        }
        event.preventDefault()
      },
    }
    return api
  },
  slots: (slot) => ({
    root: slot<object, { back: number; ahead: number }>({
      state: (state) => ({ back: state.back, ahead: state.ahead }),
    }),
    input: slot({
      attrs: (state) => ({
        value: state.text,
      }),
      on: {
        input: (event, { api }) =>
          api.type((event.target as HTMLTextAreaElement).value),
        keydown: (event, { api }) => api.keydown(event),
        pointerup: (_event, { api }) => api.seal(),
      },
    }),
    undo: slot({
      attrs: (state) => ({
        type: "button",
        disabled: state.back ? undefined : "",
      }),
      press: ({ api }) => api.undo(),
    }),
    redo: slot({
      attrs: (state) => ({
        type: "button",
        disabled: state.ahead ? undefined : "",
      }),
      press: ({ api }) => api.redo(),
    }),
  }),
})

const NotesParts = createParts(notes, {
  root: "div",
  input: "textarea",
  undo: "button",
  redo: "button",
})

function NoteHistory() {
  const undo = useShortcutHint("mod+z")
  const redo = useShortcutHint("mod+shift+z")
  return (
    <NotesParts.Root className="st-notes">
      {({ back, ahead }) => (
        <>
          <header className="st-notes-head">
            <span className="st-notes-title">Before Saturday</span>
            <Keys shortcut="mod+z" />
          </header>
          <NotesParts.Input
            aria-label="Note"
            aria-keyshortcuts={`${undo.ariaKeyshortcuts} ${redo.ariaKeyshortcuts}`}
            rows={3}
            spellCheck={false}
            className="st-notes-input"
          />
          <footer className="st-notes-foot">
            <NotesParts.Undo
              className="st-notes-btn"
              aria-label={`Undo, ${back} ${back === 1 ? "step" : "steps"}`}
            >
              <ArrowUUpLeftIcon size={16} weight="bold" aria-hidden />
              <span className="st-notes-count">{back}</span>
            </NotesParts.Undo>
            <NotesParts.Redo
              className="st-notes-btn"
              aria-label={`Redo, ${ahead} ${ahead === 1 ? "step" : "steps"}`}
            >
              <ArrowUUpRightIcon size={16} weight="bold" aria-hidden />
              <span className="st-notes-count">{ahead}</span>
            </NotesParts.Redo>
            <span className="st-notes-trail" aria-hidden="true">
              {Array.from({ length: back + 1 + ahead }, (_, index) => (
                <i
                  key={index}
                  data-at={
                    index < back ? "past" : index === back ? "now" : "ahead"
                  }
                />
              ))}
            </span>
          </footer>
        </>
      )}
    </NotesParts.Root>
  )
}

/* 2. A confirm button: a custom primitive that arms, then acts */

type ConfirmOptions = {
  defaultArmed?: boolean
  onConfirm?: () => void
  onArmedChange?: (
    armed: boolean,
    details: ChangeDetails<"press" | "timeout" | "escape-key">
  ) => void
  timeout?: number
}

const confirmButton = definePrimitive({
  name: "st-confirm",
  options: {
    defaultArmed: "boolean",
    onConfirm: "callback",
    onArmedChange: "callback",
    timeout: "number",
  },
  state: (options: ConfirmOptions) => ({
    armed: options.defaultArmed ?? false,
  }),
  api: (context) => {
    const { store, options, onDestroy } = context
    const change = createChange(context)
    let timer: ReturnType<typeof setTimeout> | undefined
    onDestroy(() => clearTimeout(timer))
    return {
      press(event?: Event) {
        clearTimeout(timer)
        if (store.get().armed) {
          change("armed", false, { reason: "press", event })
          options().onConfirm?.()
        } else if (change("armed", true, { reason: "press", event })) {
          timer = setTimeout(
            () => change("armed", false, { reason: "timeout" }),
            options().timeout ?? 3000
          )
        }
      },
      disarm(event: KeyboardEvent) {
        clearTimeout(timer)
        change("armed", false, { reason: "escape-key", event })
      },
    }
  },
  slots: (slot) => ({
    root: slot<object, { armed: boolean }>({
      state: (state) => ({ armed: state.armed }),
      attrs: (state) => ({
        type: "button",
        "data-armed": state.armed ? "" : undefined,
      }),
      press: ({ api }, event) => api.press(event),
      on: {
        keydown: (event, { api, store }) => {
          if (event.key !== "Escape" || !store.get().armed) {
            return
          }
          event.preventDefault()
          api.disarm(event)
        },
      },
    }),
  }),
})

const ConfirmParts = createParts(confirmButton, { root: "button" })

const TRASH_TIMEOUT = 3000

function EmptyTrash() {
  const [emptied, setEmptied] = useState(false)
  const again = useRef<HTMLButtonElement>(null)
  const confirm = useRef<HTMLButtonElement>(null)
  const moved = useRef(false)
  // The pressed button unmounts, so focus goes to the one that replaces it.
  useLayoutEffect(() => {
    if (!moved.current) {
      return
    }
    moved.current = false
    ;(emptied ? again : confirm).current?.focus()
  }, [emptied])
  const swap = (next: boolean) => {
    moved.current = true
    setEmptied(next)
  }
  return (
    <div className="st-trash" data-emptied={emptied || undefined}>
      <div className="st-trash-card">
        <div className="st-trash-head">
          <span className="st-trash-icon">
            <TrashIcon size={16} weight="fill" aria-hidden />
          </span>
          <span className="st-trash-text">
            <span className="st-trash-name">Trash</span>
            <span className="st-trash-meta">
              {emptied ? "Nothing in here" : "38 items · 1.2 GB"}
            </span>
          </span>
        </div>
        <div className="st-trash-files" aria-hidden={emptied || undefined}>
          <ul>
            <li>
              <FilePdfIcon size={16} weight="fill" aria-hidden />
              <span>invoice-0923.pdf</span>
              <span className="st-trash-size">2.1 MB</span>
            </li>
            <li>
              <ImageIcon size={16} weight="fill" aria-hidden />
              <span>IMG_4471.heic</span>
              <span className="st-trash-size">3.8 MB</span>
            </li>
          </ul>
        </div>
      </div>
      {emptied ? (
        <button
          ref={again}
          type="button"
          className="b-btn st-trash-undo"
          onClick={() => swap(false)}
        >
          <ArrowCounterClockwiseIcon size={16} weight="bold" aria-hidden />
          Put back 38 items
        </button>
      ) : (
        <ConfirmParts.Root
          ref={confirm}
          timeout={TRASH_TIMEOUT}
          onConfirm={() => swap(true)}
          className="st-confirm"
          style={{ "--st-timeout": `${TRASH_TIMEOUT}ms` } as CSSProperties}
        >
          {({ armed }) => (
            <>
              <span className="st-confirm-fill" aria-hidden="true" />
              <span className="st-confirm-labels">
                <span
                  className="st-confirm-label"
                  aria-hidden={armed || undefined}
                >
                  Empty trash
                </span>
                <span
                  className="st-confirm-label"
                  data-second=""
                  aria-hidden={!armed || undefined}
                >
                  Press again to empty
                </span>
              </span>
            </>
          )}
        </ConfirmParts.Root>
      )}
    </div>
  )
}

/* 3. A controller: a brew timer run by buttons outside it */

type TimerController = {
  start: () => void
  pause: () => void
  reset: () => void
}

const createTimerController = () => createController<TimerController>()

type TimerOptions = {
  controller?: TimerController
  duration?: number
  onRunningChange?: (
    running: boolean,
    details: ChangeDetails<"imperative" | "timeout">
  ) => void
}

const brewTimer = definePrimitive({
  name: "st-timer",
  options: {
    controller: "value",
    duration: "number",
    onRunningChange: "callback",
  },
  state: (options: TimerOptions) => ({
    left: options.duration ?? 240,
    running: false,
  }),
  api: (context) => {
    const { store } = context
    const change = createChange(context)
    const total = () => context.options().duration ?? 240
    let tick: ReturnType<typeof setInterval> | undefined
    let deadline = 0
    let remaining = total() * 1000
    context.onDestroy(() => clearInterval(tick))
    const show = () => {
      const left = Math.max(0, Math.ceil(remaining / 1000))
      if (left !== store.get().left) {
        store.set({ left })
      }
    }
    return {
      start() {
        if (store.get().running) {
          return
        }
        if (remaining <= 0) {
          remaining = total() * 1000
        }
        show()
        deadline = Date.now() + remaining
        change("running", true, { reason: "imperative" })
        tick = setInterval(() => {
          remaining = deadline - Date.now()
          show()
          if (remaining > 0) {
            return
          }
          clearInterval(tick)
          change("running", false, { reason: "timeout" })
        }, 200)
      },
      pause() {
        if (!store.get().running) {
          return
        }
        clearInterval(tick)
        remaining = deadline - Date.now()
        change("running", false, { reason: "imperative" })
      },
      reset() {
        clearInterval(tick)
        remaining = total() * 1000
        change("running", false, { reason: "imperative" })
        show()
      },
    }
  },
  slots: (slot) => ({
    root: slot<object, { left: number; running: boolean }>({
      state: (state) => ({ left: state.left, running: state.running }),
      attrs: (state) => ({
        role: "timer",
        "data-running": state.running ? "" : undefined,
      }),
    }),
  }),
})

const TimerParts = createParts(brewTimer, { root: "div" })

const BREW = 240
const TIMER_CELLS = 24

function BrewTimer() {
  const [controller] = useState(createTimerController)
  const [running, setRunning] = useState(false)
  return (
    <div className="st-timer">
      <TimerParts.Root
        controller={controller}
        duration={BREW}
        onRunningChange={(next) => setRunning(next)}
        aria-label="French press timer"
        className="st-timer-face"
      >
        {({ left }) => {
          const done = left === 0
          const readout = `${String(Math.floor(left / 60)).padStart(2, "0")}:${String(left % 60).padStart(2, "0")}`
          const lit = Math.round(((BREW - left) / BREW) * TIMER_CELLS)
          return (
            <>
              <div className="st-timer-top">
                <CoffeeIcon size={16} weight="fill" aria-hidden />
                <span>French press</span>
                <span className="st-timer-state" data-done={done || undefined}>
                  {done
                    ? "Ready to pour"
                    : running
                      ? "Steeping"
                      : left === BREW
                        ? "4 min steep"
                        : "Paused"}
                </span>
              </div>
              <p className="st-timer-readout" data-done={done || undefined}>
                <span
                  className="st-timer-digits st-timer-ghost"
                  aria-hidden="true"
                >
                  {[..."88:88"].map((char, index) => (
                    <span key={index} data-colon={char === ":" || undefined}>
                      {char}
                    </span>
                  ))}
                </span>
                <span className="st-timer-digits">
                  {[...readout].map((char, index) => (
                    <span key={index} data-colon={char === ":" || undefined}>
                      {char}
                    </span>
                  ))}
                </span>
              </p>
              <div className="st-timer-bar" aria-hidden="true">
                {Array.from({ length: TIMER_CELLS }, (_, index) => (
                  <i key={index} data-lit={index < lit || undefined} />
                ))}
              </div>
            </>
          )
        }}
      </TimerParts.Root>
      <div className="st-timer-remote" role="group" aria-label="Timer controls">
        <button
          type="button"
          className="b-btn st-timer-go"
          data-variant="primary"
          onClick={() => (running ? controller.pause() : controller.start())}
        >
          {running ? (
            <PauseIcon size={16} weight="fill" aria-hidden />
          ) : (
            <PlayIcon size={16} weight="fill" aria-hidden />
          )}
          {running ? "Pause" : "Start"}
        </button>
        <button
          type="button"
          className="b-btn st-timer-reset"
          onClick={() => controller.reset()}
        >
          <ArrowCounterClockwiseIcon size={16} weight="bold" aria-hidden />
          Reset
        </button>
      </div>
    </div>
  )
}

/* 4. Fuzzy search and grid focus: an emoji picker */

const EMOJI: [string, string, string][] = [
  ["😀", "grinning face", "smile happy"],
  ["😂", "face with tears of joy", "laugh lol"],
  ["🥹", "face holding back tears", "grateful touched"],
  ["😍", "smiling face with heart eyes", "love crush"],
  ["😎", "smiling face with sunglasses", "cool"],
  ["🤔", "thinking face", "hmm wonder"],
  ["🫡", "saluting face", "yes sir ok"],
  ["😴", "sleeping face", "tired zzz"],
  ["🥳", "partying face", "party celebrate birthday"],
  ["😭", "loudly crying face", "sad sob"],
  ["😤", "face with steam from nose", "proud determined"],
  ["🤯", "exploding head", "mind blown shocked"],
  ["👍", "thumbs up", "yes approve like"],
  ["👎", "thumbs down", "no dislike"],
  ["👏", "clapping hands", "applause bravo"],
  ["🙌", "raising hands", "hooray celebrate"],
  ["🙏", "folded hands", "please thanks"],
  ["💪", "flexed biceps", "strong"],
  ["👀", "eyes", "look see watching"],
  ["🫶", "heart hands", "love thanks"],
  ["❤️", "red heart", "love"],
  ["🔥", "fire", "hot lit"],
  ["⭐", "star", "favorite"],
  ["🎉", "party popper", "celebrate tada party"],
  ["🚀", "rocket", "ship launch fast"],
  ["✅", "check mark button", "done yes complete"],
  ["❌", "cross mark", "no wrong"],
  ["💯", "hundred points", "perfect score"],
  ["💡", "light bulb", "idea"],
  ["🐛", "bug", "insect issue"],
  ["🧪", "test tube", "experiment test"],
  ["📌", "pushpin", "pin important"],
  ["⏰", "alarm clock", "time late reminder"],
  ["☕", "hot beverage", "coffee tea"],
  ["🍕", "pizza", "food"],
  ["🌮", "taco", "food"],
  ["🍩", "doughnut", "donut food sweet"],
  ["🐶", "dog face", "puppy pet"],
  ["🐱", "cat face", "kitten pet"],
  ["🌈", "rainbow", "pride colors"],
  ["☀️", "sun", "sunny weather"],
  ["🌧️", "cloud with rain", "rain weather"],
  ["🌊", "water wave", "ocean sea"],
  ["🌱", "seedling", "plant grow new"],
  ["🎯", "bullseye", "target goal"],
  ["🏆", "trophy", "win award"],
]
const EMOJI_COLS = 7

const HIDDEN_EMOJI: [string, string, string][] = [
  ["▲", "vercel", "triangle deploy ship"],
]

const findsHidden = (query: string) => {
  const typed = query.trim().toLowerCase()
  return HIDDEN_EMOJI.filter(
    ([, name, keywords]) =>
      typed.length >= 3 &&
      `${name} ${keywords}`.split(" ").some((word) => word.startsWith(typed))
  )
}

const rankEmoji = (query: string) => {
  if (!query.trim()) {
    return EMOJI
  }
  return [...findsHidden(query), ...EMOJI]
    .map((entry) => ({
      entry,
      score: Math.max(
        fuzzyScore(entry[1], query),
        fuzzyScore(entry[2], query) * 0.9
      ),
    }))
    .filter(({ score }) => score > 0)
    .sort((a, b) => b.score - a.score)
    .map(({ entry }) => entry)
}

const emojiPicker = definePrimitive({
  name: "st-emoji",
  state: () => ({
    query: "",
    active: null as GridCell | null,
    hovered: null as number | null,
    focused: null as number | null,
    picked: "🎉" as string | null,
  }),
  api: (context) => {
    const { store } = context
    const results = () => rankEmoji(store.get().query)
    const grid = createGridFocus(context, {
      key: "active",
      rows: () => Math.ceil(results().length / EMOJI_COLS),
      cols: () => EMOJI_COLS,
      pageSize: () => 3,
      disabled: (cell) => cell.row * EMOJI_COLS + cell.col >= results().length,
    })
    const api = {
      grid,
      search(query: string) {
        store.set({ query, active: null, hovered: null })
        const count = rankEmoji(query).length
        announce(count === 0 ? "No emoji found" : `${count} emoji`)
      },
      pick(index: number) {
        const entry = results()[index]
        if (!entry) {
          return
        }
        store.set({ picked: entry[0] })
        announce(`Reacted with ${entry[1]}`)
      },
      setHovered: (index: number | null) => store.set({ hovered: index }),
      setFocused: (index: number | null) => store.set({ focused: index }),
      searchKeydown(event: KeyboardEvent) {
        if (!results().length) {
          return
        }
        if (event.key === "ArrowDown") {
          event.preventDefault()
          grid.focus({ row: 0, col: 0 }, { reason: "imperative", event })
        } else if (event.key === "Enter") {
          event.preventDefault()
          api.pick(0)
        }
      },
      cellKeydown(event: KeyboardEvent) {
        const typing =
          event.key.length === 1 &&
          event.key !== " " &&
          !event.ctrlKey &&
          !event.metaKey &&
          !event.altKey
        if (typing) {
          return void context.element("search")?.focus()
        }
        if (event.key === "ArrowUp" && grid.active()?.row === 0) {
          event.preventDefault()
          context.element("search")?.focus()
          return
        }
        grid.keydown(event)
      },
    }
    return api
  },
  slots: (slot) => ({
    root: slot<
      object,
      { query: string; preview: number | null; picked: string | null }
    >({
      state: (state) => ({
        query: state.query,
        preview: state.hovered ?? state.focused,
        picked: state.picked,
      }),
    }),
    search: slot({
      attrs: (state) => ({
        type: "search",
        value: state.query,
        autocomplete: "off",
      }),
      on: {
        input: (event, { api }) =>
          api.search((event.target as HTMLInputElement).value),
        keydown: (event, { api }) => api.searchKeydown(event),
      },
    }),
    grid: slot({
      attrs: () => ({ role: "grid" }),
      on: { pointerleave: (_event, { api }) => api.setHovered(null) },
    }),
    cell: slot<{ row: number; col: number }>({
      // Declaring `id` makes it the part's key, so each cell is its own part.
      props: { id: "string", row: "number", col: "number" },
      attrs: (state, { props, api }) => ({
        type: "button",
        role: "gridcell",
        tabindex: api.grid.tabIndex(state, props),
        "data-active": api.grid.isActive(state, props) ? "" : undefined,
      }),
      press: ({ api, props }) => api.pick(props.row * EMOJI_COLS + props.col),
      on: {
        keydown: (event, { api }) => api.cellKeydown(event),
        pointerenter: (_event, { api, props }) =>
          api.setHovered(props.row * EMOJI_COLS + props.col),
        focus: (_event, { api, props }) =>
          api.setFocused(props.row * EMOJI_COLS + props.col),
        blur: (_event, { api }) => api.setFocused(null),
      },
      bind: (el, { api, props }) => api.grid.bind(el, props),
    }),
  }),
})

const EmojiParts = createParts(emojiPicker, {
  root: "div",
  search: "input",
  grid: "div",
  cell: "button",
})

function EmojiPicker() {
  return (
    <EmojiParts.Root className="st-emoji">
      {({ query, preview, picked }) => {
        const results = rankEmoji(query)
        const rows = Array.from(
          { length: Math.ceil(results.length / EMOJI_COLS) },
          (_, row) =>
            results.slice(row * EMOJI_COLS, row * EMOJI_COLS + EMOJI_COLS)
        )
        const shown = preview !== null ? results[preview] : undefined
        const pickedName = [...EMOJI, ...HIDDEN_EMOJI].find(
          (entry) => entry[0] === picked
        )?.[1]
        return (
          <>
            <label className="st-emoji-search">
              <MagnifyingGlassIcon size={16} weight="bold" aria-hidden />
              <EmojiParts.Search
                aria-label="Search emoji"
                placeholder="Search emoji"
                className="st-emoji-input"
              />
            </label>
            <EmojiParts.Grid aria-label="Emoji" className="st-emoji-grid">
              {rows.map((row, r) => (
                <div key={r} role="row" className="st-emoji-row">
                  {row.map(([emoji, name], c) => (
                    <EmojiParts.Cell
                      key={c}
                      id={`st-emoji-${r}-${c}`}
                      row={r}
                      col={c}
                      aria-label={name}
                      className="st-emoji-cell"
                    >
                      {emoji}
                    </EmojiParts.Cell>
                  ))}
                </div>
              ))}
              {results.length === 0 && (
                <p className="st-emoji-none">No emoji for “{query}”</p>
              )}
            </EmojiParts.Grid>
            <div className="st-emoji-foot" aria-live="off">
              {shown ? (
                <>
                  <span className="st-emoji-big">{shown[0]}</span>
                  <span className="st-emoji-name">{shown[1]}</span>
                </>
              ) : picked ? (
                <>
                  <span className="st-emoji-big">{picked}</span>
                  <span className="st-emoji-name">
                    <span className="st-emoji-dim">Reacted with</span>{" "}
                    {pickedName}
                  </span>
                </>
              ) : null}
            </div>
          </>
        )
      }}
    </EmojiParts.Root>
  )
}

/* 5. A viewport: a map that fits everything, or just the selection */

type Sight = {
  key: string
  name: string
  kind: "temple" | "market" | "nature" | "street"
  x: number
  y: number
}

const SIGHTS: Sight[] = [
  { key: "bamboo", name: "Bamboo Grove", kind: "nature", x: 92, y: 318 },
  { key: "kinkaku", name: "Kinkaku-ji", kind: "temple", x: 352, y: 96 },
  { key: "nijo", name: "Nijō Castle", kind: "temple", x: 452, y: 282 },
  { key: "nishiki", name: "Nishiki Market", kind: "market", x: 554, y: 346 },
  { key: "gion", name: "Gion", kind: "street", x: 716, y: 384 },
  { key: "kiyomizu", name: "Kiyomizu-dera", kind: "temple", x: 742, y: 460 },
  { key: "path", name: "Philosopher’s Path", kind: "nature", x: 792, y: 196 },
  { key: "inari", name: "Fushimi Inari", kind: "temple", x: 676, y: 618 },
]

const boundsOf = (keys: string[]) => {
  const points = SIGHTS.filter((sight) => keys.includes(sight.key))
  return {
    left: Math.min(...points.map((p) => p.x)),
    top: Math.min(...points.map((p) => p.y)),
    right: Math.max(...points.map((p) => p.x)),
    bottom: Math.max(...points.map((p) => p.y)),
  }
}

const sightMap = definePrimitive({
  name: "st-map",
  state: () => ({
    viewport: { x: 0, y: 0, zoom: 1 },
    selected: ["gion", "kiyomizu"] as string[],
  }),
  live: ["viewport"],
  api: (context) => {
    const { store } = context
    const viewport = createViewport(context, {
      key: "viewport",
      surface: () => context.element("frame"),
      minZoom: () => 0.3,
      maxZoom: () => 2.5,
      message: (zoom) => `Map at ${Math.round(zoom * 100)} percent`,
    })
    let glideTimer: ReturnType<typeof setTimeout> | undefined
    context.onDestroy(() => clearTimeout(glideTimer))
    // Button moves ease into place; drags and the wheel follow the hand directly.
    const glide = () => {
      const frame = context.element("frame")
      if (!frame || reduced()) {
        return
      }
      frame.setAttribute("data-glide", "")
      clearTimeout(glideTimer)
      glideTimer = setTimeout(() => frame.removeAttribute("data-glide"), 480)
    }
    const fitAll = (animate = true) => {
      if (animate) {
        glide()
      }
      viewport.fit(boundsOf(SIGHTS.map((sight) => sight.key)), {
        padding: 64,
        maxZoom: 1,
      })
    }
    const api = {
      viewport,
      fitAll,
      fitSelection() {
        const { selected } = store.get()
        if (!selected.length) {
          return
        }
        glide()
        viewport.fit(boundsOf(selected), { padding: 88, maxZoom: 1.6 })
      },
      act(action: string) {
        if (action === "fit") {
          fitAll()
        } else if (action === "selection") {
          api.fitSelection()
        } else {
          glide()
          viewport.zoomBy(action === "in" ? 1.25 : 0.8)
        }
      },
      toggle(key: string) {
        const { selected } = store.get()
        store.set({
          selected: selected.includes(key)
            ? selected.filter((other) => other !== key)
            : [...selected, key],
        })
      },
      clear: () => store.set({ selected: [] }),
      keydown(event: KeyboardEvent, onFrame: boolean) {
        const pans: Record<string, [number, number]> = {
          ArrowLeft: [60, 0],
          ArrowRight: [-60, 0],
          ArrowUp: [0, 60],
          ArrowDown: [0, -60],
        }
        if (event.shiftKey && event.code === "Digit1") {
          fitAll()
        } else if (event.shiftKey && event.code === "Digit2") {
          api.fitSelection()
        } else if (event.key === "Escape" && store.get().selected.length) {
          api.clear()
        } else if (onFrame && pans[event.key]) {
          viewport.panBy(...pans[event.key])
        } else if (onFrame && viewport.keydown(event)) {
          return
        } else {
          return
        }
        event.preventDefault()
      },
    }
    return api
  },
  slots: (slot) => ({
    root: slot<object, { selected: string[] }>({
      state: (state) => ({ selected: state.selected }),
    }),
    frame: slot({
      attrs: () => ({
        tabindex: "0",
        role: "application",
        "aria-roledescription": "map",
        "aria-keyshortcuts": "Shift+1 Shift+2 + - 0",
        style: css({
          position: "relative",
          overflow: "clip",
          "touch-action": "none",
        }),
      }),
      live: live((state) => ({
        "data-far": state.viewport.zoom < 0.45 ? "" : undefined,
      })),
      on: {
        wheel: (event, { el, api }) => {
          // Plain wheel scrolls the page past the tile until the map has focus.
          if (
            event.ctrlKey ||
            event.metaKey ||
            el?.contains(document.activeElement)
          ) {
            api.viewport.wheel(event)
          }
        },
        pointerdown: (event, { api }) => {
          if (!(event.target as Element).closest("button")) {
            api.viewport.pointerdown(event)
          }
        },
        keydown: (event, { el, api }) =>
          api.keydown(event, event.target === el),
      },
      bind: (el, { api }) => {
        let width = 0
        const frame = requestAnimationFrame(() => api.fitAll(false))
        const observer = new ResizeObserver(() => {
          if (width && Math.abs(el.clientWidth - width) > 1) {
            api.fitAll(false)
          }
          width = el.clientWidth
        })
        observer.observe(el)
        return () => {
          cancelAnimationFrame(frame)
          observer.disconnect()
        }
      },
    }),
    world: slot({
      attrs: () => ({
        style: css({ position: "absolute", left: "0", top: "0" }),
      }),
      live: live((state, { api }) => ({ style: api.viewport.style(state) })),
    }),
    sight: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { key }) => ({
        type: "button",
        "aria-pressed": String(state.selected.includes(key)),
      }),
      press: ({ api, key }) => api.toggle(key),
      on: {
        focus: (_event, { el, api }) => {
          if (el) {
            api.viewport.reveal(api.viewport.rectOf(el), { padding: 56 })
          }
        },
      },
    }),
    action: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { key }) => ({
        type: "button",
        disabled:
          key === "selection" && !state.selected.length ? "" : undefined,
      }),
      press: ({ api, key }) => api.act(key),
    }),
    zoom: slot({
      live: live((state) => ({
        "data-zoom": `${Math.round(state.viewport.zoom * 100)}%`,
      })),
    }),
  }),
})

const MapParts = createParts(sightMap, {
  root: "div",
  frame: "div",
  world: "div",
  sight: "button",
  action: "button",
  zoom: "span",
})

function MapArt() {
  // The art runs far past the sights, so a fit never shows the canvas edge.
  return (
    <svg
      className="st-map-art"
      width={2600}
      height={2000}
      viewBox="-850 -650 2600 2000"
      style={{ position: "absolute", left: -850, top: -650 }}
      aria-hidden="true"
    >
      <defs>
        <pattern
          id="st-map-blocks"
          width="40"
          height="40"
          patternUnits="userSpaceOnUse"
          x="296"
          y="112"
        >
          <rect
            x="3"
            y="3"
            width="34"
            height="34"
            rx="2"
            className="st-map-block"
          />
        </pattern>
      </defs>
      <rect
        x={-850}
        y={-650}
        width={2600}
        height={2000}
        className="st-map-land"
      />
      <path
        className="st-map-hill"
        d="M-850 -650H1750V40C1500 70 1200 20 900 58S300 30 200 58 -300 30 -850 70Z"
      />
      <path
        className="st-map-hill"
        d="M-850 100C-200 140 60 160 170 260S120 420 150 520 90 700 -60 1350H-850Z"
      />
      <path
        className="st-map-hill"
        d="M1750 60C1100 100 840 160 846 300S800 460 842 560 860 760 1000 1350H1750Z"
      />
      <rect
        x={296}
        y={112}
        width={484}
        height={540}
        fill="url(#st-map-blocks)"
      />
      <g className="st-map-avenues">
        <line x1={296} y1={352} x2={780} y2={352} />
        <line x1={516} y1={112} x2={516} y2={652} />
        <line x1={436} y1={112} x2={436} y2={652} />
        <line x1={296} y1={192} x2={780} y2={192} />
      </g>
      <rect
        className="st-map-park"
        x={482}
        y={150}
        width={84}
        height={104}
        rx={6}
      />
      <path
        className="st-map-river"
        d="M612 -650C600 120 628 210 616 320S640 520 628 1350"
      />
      <path
        className="st-map-river"
        data-wide=""
        d="M-400 -100C0 100 90 230 150 380S220 560 300 1350"
      />
      <text
        className="st-map-label"
        x={600}
        y={560}
        transform="rotate(-86 600 560)"
      >
        Kamo River
      </text>
      <text className="st-map-label" x={10} y={430}>
        Arashiyama
      </text>
      <text
        className="st-map-label"
        x={872}
        y={250}
        transform="rotate(90 872 250)"
      >
        Higashiyama
      </text>
    </svg>
  )
}

function SightMap() {
  return (
    <MapParts.Root className="st-map">
      {({ selected }) => (
        <MapParts.Frame aria-label="Sights map" className="st-map-frame">
          <MapParts.World className="st-map-world">
            <MapArt />
            {SIGHTS.map((sight) => (
              <div
                key={sight.key}
                className="st-pin-anchor"
                style={{ left: sight.x, top: sight.y }}
              >
                <MapParts.Sight
                  value={sight.key}
                  className="st-pin"
                  data-kind={sight.kind}
                >
                  <span className="st-pin-dot" aria-hidden="true" />
                  <span className="st-pin-name">{sight.name}</span>
                </MapParts.Sight>
              </div>
            ))}
          </MapParts.World>
          <div className="st-map-title">
            <MapTrifoldIcon size={16} weight="fill" aria-hidden />
            <span>Day 3</span>
            <span className="st-map-sub">{SIGHTS.length} sights</span>
          </div>
          <div className="st-map-tools" role="toolbar" aria-label="View">
            <MapParts.Action
              value="fit"
              className="st-map-tool"
              title="Fit all (Shift+1)"
            >
              <CornersOutIcon size={16} weight="bold" aria-hidden />
              Fit all
            </MapParts.Action>
            <MapParts.Action
              value="selection"
              className="st-map-tool"
              title="Zoom to selection (Shift+2)"
            >
              <CrosshairSimpleIcon size={16} weight="bold" aria-hidden />
              Selection
              {selected.length > 0 && (
                <span className="st-map-badge">{selected.length}</span>
              )}
            </MapParts.Action>
          </div>
          <p className="st-map-status">
            {selected.length ? (
              <>
                {selected.length} selected
                <span className="st-map-esc">{"\u00a0· Esc clears"}</span>
              </>
            ) : (
              "Click sights to select"
            )}
          </p>
          <div className="st-map-zoom" role="group" aria-label="Zoom">
            <MapParts.Action
              value="out"
              className="st-map-tool"
              aria-label="Zoom out"
            >
              <MagnifyingGlassMinusIcon size={16} weight="bold" aria-hidden />
            </MapParts.Action>
            <MapParts.Zoom className="st-map-percent" />
            <MapParts.Action
              value="in"
              className="st-map-tool"
              aria-label="Zoom in"
            >
              <MagnifyingGlassPlusIcon size={16} weight="bold" aria-hidden />
            </MapParts.Action>
          </div>
        </MapParts.Frame>
      )}
    </MapParts.Root>
  )
}

/* 6. Spans: a week you draw on */

type Plan = Span & { title: string; hue: number }

const WEEK = [
  { key: "mon", name: "Mon", long: "Monday", date: 2 },
  { key: "tue", name: "Tue", long: "Tuesday", date: 3 },
  { key: "wed", name: "Wed", long: "Wednesday", date: 4 },
  { key: "thu", name: "Thu", long: "Thursday", date: 5 },
  { key: "fri", name: "Fri", long: "Friday", date: 6 },
]
const W_START = 8 * 60
const W_END = 18 * 60
const HOUR_PX = 36
const dayLong = (key: string) =>
  WEEK.find((day) => day.key === key)?.long ?? key

const weekPlan = definePrimitive({
  name: "st-week",
  state: () => ({
    plans: [
      {
        id: "yoga",
        track: "mon",
        start: 9 * 60,
        end: 10 * 60,
        title: "Yoga",
        hue: 150,
      },
      {
        id: "sync",
        track: "mon",
        start: 9 * 60 + 30,
        end: 11 * 60,
        title: "Sync",
        hue: 255,
      },
      {
        id: "ramen",
        track: "tue",
        start: 12 * 60,
        end: 13 * 60,
        title: "Ramen",
        hue: 40,
      },
      {
        id: "portfolio",
        track: "wed",
        start: 10 * 60,
        end: 11 * 60 + 30,
        title: "Folio review",
        hue: 300,
      },
      {
        id: "dentist",
        track: "thu",
        start: 9 * 60 + 15,
        end: 10 * 60,
        title: "Dentist",
        hue: 200,
      },
      {
        id: "demo",
        track: "fri",
        start: 10 * 60 + 30,
        end: 12 * 60,
        title: "Demo day",
        hue: 20,
      },
    ] as Plan[],
    draft: null as SpansDraft<Plan> | null,
    hover: null as { track: string; at: number } | null,
    days: 5,
  }),
  api: (context) => {
    const { store } = context
    const change = createChange(context)
    let made = 0
    type State = ReturnType<typeof store.get>
    const spans = createSpans<State, Plan>(context, {
      key: "plans",
      draft: "draft",
      set: (value, details) => change("plans", value, details),
      min: () => W_START,
      max: () => W_END,
      step: () => 15,
      minSize: () => 30,
      maxSize: () => 240,
      size: () => 60,
      tracks: () => WEEK.slice(0, store.get().days).map((day) => day.key),
      create: (span) => ({
        ...span,
        id: `st-block-${++made}`,
        title: "Focus time",
        hue: 85,
      }),
      label: (plan) =>
        `${plan.title}, ${dayLong(plan.track)} ${clock(plan.start)} to ${clock(plan.end)}`,
      createAutoScroll: { edge: 28 },
    })
    return {
      spans,
      hover(track: string | null, event?: PointerEvent) {
        const { hover, draft } = store.get()
        const onSpan =
          event && (event.target as Element).closest("[data-span-id]")
        if (!track || !event || onSpan || draft) {
          if (hover) {
            store.set({ hover: null })
          }
          return
        }
        const at = Math.min(
          spans.valueAt(track, event.clientX, event.clientY),
          W_END - 60
        )
        if (hover?.track !== track || hover.at !== at) {
          store.set({ hover: { track, at } })
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot<
      object,
      {
        plans: Plan[]
        hover: { track: string; at: number } | null
        days: number
      }
    >({
      // `spans(state)` hands back the stored array or the draft's, so it keeps its identity between renders.
      state: (state, { api }) => ({
        plans: api.spans.spans(state),
        hover: state.hover,
        days: state.days,
      }),
      bind: (el, { store }) =>
        observeWidth(el, (width) => {
          const days = width < 440 ? 3 : 5
          if (store.get().days !== days) {
            store.set({ days })
          }
        }),
    }),
    day: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => ({
        role: "group",
        "aria-label": `${dayLong(key)}, November ${WEEK.find((day) => day.key === key)?.date}`,
        style: css({
          position: "relative",
          "touch-action": "none",
          "user-select": "none",
        }),
      }),
      on: {
        pointerdown: (event, { api, key }) =>
          api.spans.trackPointerdown(event, key),
        pointermove: (event, { api, key }) => api.hover(key, event),
        pointerleave: (_event, { api }) => api.hover(null),
      },
      bind: (el, { api, key }) => api.spans.bindTrack(el, key),
    }),
    plan: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { api, key }) => {
        const all = api.spans.spans(state)
        const plan = all.find((other) => other.id === key)
        if (!plan) {
          return {}
        }
        const lanes = stackSpans(
          all.filter((other) => other.track === plan.track)
        )
        return {
          ...api.spans.spanAttrs(state, key),
          tabindex: "0",
          role: "button",
          "aria-roledescription": "event",
          "aria-label": `${plan.title}, ${dayLong(plan.track)} ${clock(plan.start)} to ${clock(plan.end)}`,
          "data-short": plan.end - plan.start < 60 ? "" : undefined,
          style: `${api.spans.spanStyle(plan, lanes.get(key))};--hue:${plan.hue}`,
        }
      },
      on: {
        pointerdown: (event, { api, key }) =>
          api.spans.spanPointerdown(event, key),
        keydown: (event, { api, key }) => void api.spans.keydown(event, key),
      },
      bind: (el, { api, key }) => api.spans.bindSpan(el, key),
    }),
  }),
})

const WeekParts = createParts(weekPlan, {
  root: "div",
  day: "div",
  plan: "div",
})

function WeekSpans() {
  const scroller = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (scroller.current) {
      scroller.current.scrollTop = HOUR_PX - 12
    }
  }, [])
  const hours = Array.from(
    { length: (W_END - W_START) / 60 - 1 },
    (_, i) => 9 + i
  )
  return (
    <WeekParts.Root className="st-week">
      {({ plans, hover, days }) => {
        const shown = WEEK.slice(0, days)
        const columns = {
          gridTemplateColumns: `44px repeat(${days}, minmax(0, 1fr))`,
        }
        return (
          <>
            <header className="st-week-head">
              <span className="st-week-range">
                November <span>2 – {shown[shown.length - 1].date}</span>
              </span>
              <span className="st-week-hint">
                {hover
                  ? `${dayLong(hover.track)} ${clock(hover.at)}`
                  : "Drag on a day to add"}
              </span>
            </header>
            <div ref={scroller} className="st-week-scroll">
              <div className="st-week-days" style={columns} aria-hidden="true">
                <span />
                {shown.map((day) => (
                  <span key={day.key} className="st-week-day">
                    {day.name} <b>{day.date}</b>
                  </span>
                ))}
              </div>
              <div className="st-week-body" style={columns}>
                <div className="st-week-hours" aria-hidden="true">
                  {hours.map((hour) => (
                    <span
                      key={hour}
                      style={{ top: (hour * 60 - W_START) * (HOUR_PX / 60) }}
                    >
                      {hour}:00
                    </span>
                  ))}
                </div>
                {shown.map((day) => (
                  <WeekParts.Day
                    key={day.key}
                    value={day.key}
                    className="st-week-track"
                  >
                    {hover?.track === day.key && (
                      <span
                        className="st-week-ghost"
                        aria-hidden="true"
                        style={{
                          top: `${((hover.at - W_START) / (W_END - W_START)) * 100}%`,
                          height: `${(60 / (W_END - W_START)) * 100}%`,
                        }}
                      >
                        {clock(hover.at)}
                      </span>
                    )}
                    {plans
                      .filter((plan) => plan.track === day.key)
                      .map((plan) => (
                        <WeekParts.Plan
                          key={plan.id}
                          value={plan.id}
                          className="st-plan"
                        >
                          <span
                            data-span-edge="start"
                            className="st-plan-edge"
                            data-edge="start"
                          />
                          <span className="st-plan-title">{plan.title}</span>
                          <span className="st-plan-time">
                            {clock(plan.start)}–{clock(plan.end)}
                          </span>
                          <span
                            data-span-edge="end"
                            className="st-plan-edge"
                            data-edge="end"
                          />
                        </WeekParts.Plan>
                      ))}
                  </WeekParts.Day>
                ))}
              </div>
            </div>
          </>
        )
      }}
    </WeekParts.Root>
  )
}

/* 7. Stick to bottom: a chat that follows until you scroll up */

type Message = {
  id: number
  from: "me" | "them"
  text: string
  time: string
  fresh?: boolean
}

const CHAT: Message[] = [
  {
    id: 1,
    from: "them",
    text: "Kiln’s loaded. Firing starts at 6 tomorrow.",
    time: "17:42",
  },
  {
    id: 2,
    from: "me",
    text: "Did the celadon test tiles go in?",
    time: "17:44",
  },
  {
    id: 3,
    from: "them",
    text: "Top shelf, left side. Fingers crossed.",
    time: "17:45",
  },
  {
    id: 4,
    from: "me",
    text: "I’ll bring boards for the open day.",
    time: "17:51",
  },
  {
    id: 5,
    from: "them",
    text: "Can you grab 20 kg of buff stoneware?",
    time: "17:52",
  },
  {
    id: 6,
    from: "them",
    text: "The speckled clay bloated last time.",
    time: "17:52",
  },
  { id: 7, from: "me", text: "On it. See you at 9?", time: "17:58" },
  {
    id: 8,
    from: "them",
    text: "9 works. Doors open to visitors at 11.",
    time: "18:01",
  },
]
const REPLIES = [
  "Cone 6 reached. Holding for 15 minutes.",
  "Cooling now. Don’t open it before noon!",
  "Peeked anyway. The celadon came out glassy.",
  "One tile cracked, the rest are perfect.",
  "Bring the wide shelf for the display.",
]
const time = () =>
  new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date())

const thread = definePrimitive({
  name: "st-thread",
  state: () => ({
    messages: CHAT,
    draft: "",
    replies: 0,
    newBelow: 0,
    following: true,
    atBottom: true,
    unseen: false,
  }),
  api: (context) => {
    const { store } = context
    const stick = createStickToBottom({ threshold: 48 })
    context.onDestroy(
      stick.subscribe(() => {
        const next = stick.get()
        store.set({
          ...next,
          newBelow: next.following ? 0 : store.get().newBelow,
        })
      })
    )
    let id = CHAT.length
    const add = (from: Message["from"], text: string) => {
      const { messages, newBelow } = store.get()
      store.set({
        messages: [
          ...messages,
          { id: ++id, from, text, time: time(), fresh: true },
        ],
        newBelow: stick.get().following ? 0 : newBelow + 1,
      })
    }
    return {
      stick,
      type: (draft: string) => store.set({ draft }),
      send() {
        const text = store.get().draft.trim()
        if (!text) {
          return
        }
        store.set({ draft: "" })
        stick.scrollToBottom()
        add("me", text)
      },
      reply() {
        const { replies } = store.get()
        store.set({ replies: replies + 1 })
        add("them", REPLIES[replies % REPLIES.length])
      },
      jump: () =>
        stick.scrollToBottom({ behavior: reduced() ? "auto" : "smooth" }),
    }
  },
  slots: (slot) => ({
    root: slot<
      object,
      {
        messages: Message[]
        following: boolean
        newBelow: number
        draft: string
      }
    >({
      state: (state) => ({
        messages: state.messages,
        following: state.following,
        newBelow: state.newBelow,
        draft: state.draft,
      }),
    }),
    log: slot({
      attrs: () => ({
        role: "log",
        tabindex: "0",
        style: css({ overflow: "auto" }),
      }),
      bind: (el, { api }) => api.stick.connect(el),
    }),
    input: slot({
      attrs: (state) => ({ value: state.draft, autocomplete: "off" }),
      on: {
        input: (event, { api }) =>
          api.type((event.target as HTMLInputElement).value),
        keydown: (event, { api }) => {
          if (event.key !== "Enter" || event.isComposing) {
            return
          }
          event.preventDefault()
          api.send()
        },
      },
    }),
    send: slot({
      attrs: (state) => ({
        type: "button",
        disabled: state.draft.trim() ? undefined : "",
      }),
      press: ({ api }) => api.send(),
    }),
    reply: slot({
      attrs: () => ({ type: "button" }),
      press: ({ api }) => api.reply(),
    }),
    jump: slot({
      attrs: (state) => ({
        type: "button",
        tabindex: state.newBelow ? undefined : "-1",
        "aria-hidden": state.newBelow ? undefined : "true",
        "data-visible": state.newBelow ? "" : undefined,
      }),
      press: ({ api }) => api.jump(),
    }),
  }),
})

const ThreadParts = createParts(thread, {
  root: "div",
  log: "div",
  input: "input",
  send: "button",
  reply: "button",
  jump: "button",
})

function StickyChat() {
  return (
    <ThreadParts.Root className="st-chat">
      {({ messages, following, newBelow }) => (
        <>
          <header className="st-chat-head">
            <span className="st-chat-avatar" aria-hidden="true">
              ID
            </span>
            <span className="st-chat-who">
              <span className="st-chat-name">Inês Duarte</span>
              <span className="st-chat-sub">Clay studio</span>
            </span>
          </header>
          <div className="st-chat-wrap">
            <ThreadParts.Log
              aria-label="Messages with Inês"
              className="st-chat-log"
            >
              {messages.map((message, index) => {
                const next = messages[index + 1]
                const last = !next || next.from !== message.from
                return (
                  <div
                    key={message.id}
                    className="st-msg"
                    data-from={message.from}
                    data-last={last || undefined}
                    data-fresh={message.fresh || undefined}
                  >
                    <p className="st-bubble">{message.text}</p>
                    {last && (
                      <span className="st-msg-time">{message.time}</span>
                    )}
                  </div>
                )
              })}
            </ThreadParts.Log>
            <ThreadParts.Jump className="st-chat-jump">
              <ArrowDownIcon size={14} weight="bold" aria-hidden />
              {newBelow} new {newBelow === 1 ? "message" : "messages"}
            </ThreadParts.Jump>
          </div>
          <div className="st-chat-compose">
            <ThreadParts.Input
              aria-label="Message"
              placeholder="Message Inês"
              className="st-chat-input"
            />
            <ThreadParts.Send className="st-chat-send" aria-label="Send">
              <PaperPlaneRightIcon size={16} weight="fill" aria-hidden />
            </ThreadParts.Send>
          </div>
          <div className="st-chat-foot">
            <span
              className="st-chat-follow"
              data-following={following || undefined}
            >
              {following ? "Following" : "Paused"}
            </span>
            <ThreadParts.Reply
              className="b-btn st-chat-reply"
              data-variant="ghost"
            >
              Simulate reply
            </ThreadParts.Reply>
          </div>
        </>
      )}
    </ThreadParts.Root>
  )
}

/* 8. Move: a group of layers that travel together */

const SLIDE = { w: 246, h: 138 }
const LAYERS: Record<
  string,
  { label: string; w: number; h: number; locked?: boolean }
> = {
  title: { label: "Title, Q4 revenue", w: 116, h: 24 },
  chart: { label: "Bar chart", w: 96, h: 62 },
  callout: { label: "Callout, up 18 percent", w: 78, h: 24 },
  stripe: { label: "Brand stripe, locked", w: 44, h: 16, locked: true },
}

type SlideState = {
  positions: Record<string, MovePoint>
  selected: string[]
  focused: string | null
  moving: MoveDraft | null
}

const groupBox = (state: SlideState) => {
  if (!state.selected.length) {
    return null
  }
  const boxes = state.selected.map((key) => {
    const at = state.positions[key]
    const drag = state.moving?.keys.includes(key)
    const x = at.x + (drag ? state.moving!.dx : 0)
    const y = at.y + (drag ? state.moving!.dy : 0)
    return { x, y, r: x + LAYERS[key].w, b: y + LAYERS[key].h }
  })
  const x = Math.min(...boxes.map((box) => box.x))
  const y = Math.min(...boxes.map((box) => box.y))
  return {
    x,
    y,
    w: Math.max(...boxes.map((box) => box.r)) - x,
    h: Math.max(...boxes.map((box) => box.b)) - y,
  }
}

const slide = definePrimitive({
  name: "st-slide",
  state: (): SlideState => ({
    positions: {
      title: { x: 16, y: 14 },
      chart: { x: 16, y: 54 },
      callout: { x: 124, y: 70 },
      stripe: { x: 186, y: 16 },
    },
    selected: ["chart", "callout"],
    focused: null,
    moving: null,
  }),
  live: ["moving"],
  api: (context) => {
    const { store } = context
    const change = createChange(context)
    const move = createMove(context, {
      key: "positions",
      draft: "moving",
      set: (value, details) => change("positions", value, details),
      group: (key) =>
        store.get().selected.includes(key) ? store.get().selected : [key],
      step: () => 2,
      bounds: (key) => ({
        minX: 0,
        minY: 0,
        maxX: SLIDE.w - LAYERS[key].w,
        maxY: SLIDE.h - LAYERS[key].h,
      }),
      disabled: (key) => !!LAYERS[key].locked,
      label: (key) => LAYERS[key].label,
      message: ({ keys, label, at }) =>
        keys.length > 1
          ? `Moved ${keys.length} layers.`
          : `${label} is at ${at.x}, ${at.y}.`,
    })
    return {
      move,
      roving: createRovingFocus<SlideState, { value: string }>(context, {
        slot: "layer",
        key: "focused",
        move: spatialMove(),
      }),
      select(key: string, add: boolean) {
        if (LAYERS[key].locked) {
          return
        }
        const { selected } = store.get()
        if (add) {
          store.set({
            selected: selected.includes(key)
              ? selected.filter((other) => other !== key)
              : [...selected, key],
          })
        } else if (!selected.includes(key)) {
          store.set({ selected: [key] })
        }
      },
      clear: () => store.set({ selected: [] }),
    }
  },
  slots: (slot) => ({
    root: slot<object, { selected: string[] }>({
      state: (state) => ({ selected: state.selected }),
    }),
    board: slot({
      attrs: () => ({
        role: "listbox",
        "aria-multiselectable": "true",
        "aria-roledescription": "slide",
        style: css({ position: "relative" }),
      }),
      on: {
        pointerdown: (event, { el, api }) => {
          if (event.target === el) {
            api.clear()
          }
        },
        keydown: (event, { api }) => {
          if (event.key === "Escape") {
            api.clear()
          }
        },
      },
    }),
    layer: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => {
        const at = state.positions[part.key]
        const layer = LAYERS[part.key]
        return {
          role: "option",
          "aria-label": layer.label,
          "aria-selected": String(state.selected.includes(part.key)),
          "aria-disabled": layer.locked ? "true" : undefined,
          "aria-keyshortcuts":
            "Space Shift+Space Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight",
          tabindex: part.api.roving.tabIndex(state, part),
          style: css({
            position: "absolute",
            left: `${at.x}px`,
            top: `${at.y}px`,
            width: `${layer.w}px`,
            height: `${layer.h}px`,
            "touch-action": "none",
          }),
        }
      },
      live: live((state, { api, key }) => ({
        "data-dragging": api.move.dragging(state, key) ? "" : undefined,
        style: api.move.style(state, key),
      })),
      on: {
        keydown: (event, { el, api, key }) => {
          if (event.key === " ") {
            event.preventDefault()
            return api.select(key, true)
          }
          if (!api.move.keydown(event, key)) {
            api.roving.keydown(event, el!)
          }
        },
        pointerdown: (event, { el, api, key }) => {
          api.select(key, event.shiftKey)
          api.move.pointerdown(event, key, el!)
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
    bounds: slot({
      attrs: (state) => {
        const box = groupBox(state)
        return {
          "aria-hidden": "true",
          style: box
            ? css({
                left: `${box.x}px`,
                top: `${box.y}px`,
                width: `${box.w}px`,
                height: `${box.h}px`,
              })
            : css({ display: "none" }),
        }
      },
      live: live((state: SlideState) => {
        const box = groupBox(state)
        if (!box) {
          return { style: css({ display: "none" }) }
        }
        return {
          style: css({
            left: `${box.x}px`,
            top: `${box.y}px`,
            width: `${box.w}px`,
            height: `${box.h}px`,
          }),
        }
      }),
    }),
    readout: slot({
      attrs: (state) => {
        const box = groupBox(state)
        return {
          "data-x": box ? String(Math.round(box.x)) : "–",
          "data-y": box ? String(Math.round(box.y)) : "–",
        }
      },
      live: live((state: SlideState) => {
        const box = groupBox(state)
        return {
          "data-x": box ? String(Math.round(box.x)) : "–",
          "data-y": box ? String(Math.round(box.y)) : "–",
        }
      }),
    }),
  }),
})

const SlideParts = createParts(slide, {
  root: "div",
  board: "div",
  layer: "div",
  bounds: "div",
  readout: "span",
})

function SlideGroup() {
  return (
    <SlideParts.Root className="st-slide">
      {({ selected }) => (
        <>
          <SlideParts.Board
            aria-label="Slide layers"
            className="st-slide-board"
          >
            <SlideParts.Layer value="title" className="st-layer st-layer-title">
              Q4 revenue
            </SlideParts.Layer>
            <SlideParts.Layer value="chart" className="st-layer st-layer-chart">
              {[38, 52, 46, 70, 88].map((height, index) => (
                <i key={index} style={{ height: `${height}%` }} />
              ))}
            </SlideParts.Layer>
            <SlideParts.Layer
              value="callout"
              className="st-layer st-layer-callout"
            >
              +18% vs Q3
            </SlideParts.Layer>
            <SlideParts.Layer
              value="stripe"
              className="st-layer st-layer-stripe"
            >
              <LockSimpleIcon size={14} weight="fill" aria-hidden />
            </SlideParts.Layer>
            <SlideParts.Bounds className="st-slide-bounds">
              <i data-corner="nw" />
              <i data-corner="ne" />
              <i data-corner="sw" />
              <i data-corner="se" />
            </SlideParts.Bounds>
          </SlideParts.Board>
          <p className="st-slide-foot">
            <span>
              {selected.length
                ? `${selected.length} selected`
                : "Shift-click to group"}
            </span>
            <SlideParts.Readout className="st-slide-xy" />
          </p>
        </>
      )}
    </SlideParts.Root>
  )
}

/* 9. A horizontal virtualizer: a year of film frames */

const FRAME_W = 66
const DAYS = 365
const LATEST = 306
const frameDate = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  timeZone: "UTC",
})

/** A small deterministic landscape per day: winter blues into spring greens, summer gold, autumn rust. */
function frameLook(index: number) {
  const random = (seed: number) => {
    const noise = Math.sin(index * 12.9898 + seed * 78.233) * 43758.5453
    return noise - Math.floor(noise)
  }
  const ground =
    [215, 135, 95, 45][Math.floor(((index + 20) % 365) / 91.25)] +
    random(1) * 24
  const dusk = random(2) > 0.72
  const sky = dusk ? 40 + random(3) * 30 : 225 + random(3) * 25
  const light = 0.62 + random(4) * 0.22
  const horizon = 42 + random(5) * 30
  const sun = random(6) > 0.4
  return {
    background: [
      sun
        ? `radial-gradient(circle at ${18 + random(7) * 64}% ${horizon - 14}%, oklch(0.97 0.07 85 / 0.9) 0 6%, transparent 8%)`
        : "linear-gradient(transparent, transparent)",
      `linear-gradient(to bottom, oklch(${light} ${dusk ? 0.1 : 0.07} ${sky}) 0, oklch(${light + 0.08} 0.05 ${sky + 10}) ${horizon}%, oklch(${light - 0.26} 0.09 ${ground}) ${horizon}% 100%)`,
    ].join(","),
  }
}

const filmStrip = definePrimitive({
  name: "st-strip",
  state: () => ({ items: [] as VirtualItem[], size: 0 }),
  api: (context) => {
    const virtual = createVirtualizer({
      count: () => DAYS,
      estimate: () => FRAME_W,
      horizontal: true,
      overscan: 3,
    })
    context.onDestroy(
      virtual.subscribe(() =>
        context.store.set({ items: virtual.items(), size: virtual.size() })
      )
    )
    return {
      virtual,
      act(action: string) {
        const el = context.element("reel")
        const behavior = reduced() ? "auto" : "smooth"
        if (action === "latest") {
          virtual.scrollToIndex(LATEST, { align: "center", behavior })
        } else {
          el?.scrollBy({
            left: (action === "next" ? 1 : -1) * el.clientWidth * 0.8,
            behavior,
          })
        }
      },
      keydown(event: KeyboardEvent, el: HTMLElement) {
        const by: Record<string, number> = {
          ArrowRight: FRAME_W,
          ArrowLeft: -FRAME_W,
          PageDown: el.clientWidth,
          PageUp: -el.clientWidth,
        }
        if (event.key in by) {
          el.scrollBy({ left: by[event.key] })
        } else if (event.key === "Home") {
          el.scrollTo({ left: 0 })
        } else if (event.key === "End") {
          el.scrollTo({ left: el.scrollWidth })
        } else {
          return
        }
        event.preventDefault()
      },
    }
  },
  slots: (slot) => ({
    root: slot<object, { items: VirtualItem[] }>({
      state: (state) => ({ items: state.items }),
    }),
    nav: slot<{ value: string }>({
      props: { value: "string" },
      attrs: () => ({ type: "button" }),
      press: ({ api, key }) => api.act(key),
    }),
    reel: slot({
      attrs: () => ({
        tabindex: "0",
        role: "region",
        style: css({ overflow: "auto hidden" }),
      }),
      on: { keydown: (event, { el, api }) => api.keydown(event, el!) },
      bind: (el, { api }) => {
        const disconnect = api.virtual.connect(el)
        const frame = requestAnimationFrame(() =>
          api.virtual.scrollToIndex(LATEST, { align: "center" })
        )
        return () => {
          cancelAnimationFrame(frame)
          disconnect()
        }
      },
    }),
    body: slot({
      attrs: (state) => ({
        role: "list",
        style: css({
          position: "relative",
          width: `${state.size}px`,
          height: "100%",
        }),
      }),
    }),
    frame: slot<{ value: string; start: number }>({
      props: { value: "string", start: "number" },
      attrs: (_state, { props }) => ({
        role: "listitem",
        style: css({
          position: "absolute",
          top: "0",
          bottom: "0",
          left: "0",
          width: `${FRAME_W}px`,
          transform: `translateX(${props.start}px)`,
        }),
      }),
    }),
  }),
})

const StripParts = createParts(filmStrip, {
  root: "div",
  nav: "button",
  reel: "div",
  body: "div",
  frame: "div",
})

function FilmStrip() {
  return (
    <StripParts.Root className="st-film">
      {({ items }) => (
        <>
          <header className="st-film-head">
            <span className="st-film-title">
              <FilmStripIcon size={16} weight="fill" aria-hidden />
              <span className="st-film-name">One frame a day</span>
            </span>
            <span className="st-film-count">
              <b>{items.length}</b> of {DAYS} rendered
            </span>
            <span className="st-film-nav">
              <StripParts.Nav
                value="prev"
                className="st-film-btn"
                aria-label="Earlier frames"
              >
                <CaretLeftIcon size={16} weight="bold" aria-hidden />
              </StripParts.Nav>
              <StripParts.Nav
                value="latest"
                className="st-film-btn"
                data-wide=""
              >
                Latest
              </StripParts.Nav>
              <StripParts.Nav
                value="next"
                className="st-film-btn"
                aria-label="Later frames"
              >
                <CaretRightIcon size={16} weight="bold" aria-hidden />
              </StripParts.Nav>
            </span>
          </header>
          <StripParts.Reel
            aria-label="Frames, January to December"
            className="st-film-reel"
          >
            <StripParts.Body aria-label="Frames" className="st-film-body">
              {items.map((item) => {
                const blank = item.index > LATEST
                const date = frameDate.format(Date.UTC(2026, 0, item.index + 1))
                return (
                  <StripParts.Frame
                    key={item.key}
                    value={item.key}
                    start={item.start}
                    className="st-frame"
                    data-blank={blank || undefined}
                    data-latest={item.index === LATEST || undefined}
                    aria-label={blank ? `${date}, not shot yet` : date}
                  >
                    <span className="st-frame-no" aria-hidden="true">
                      {item.index + 1}
                    </span>
                    <span
                      className="st-frame-photo"
                      style={blank ? undefined : frameLook(item.index)}
                    />
                    <span className="st-frame-date" aria-hidden="true">
                      {date}
                    </span>
                  </StripParts.Frame>
                )
              })}
            </StripParts.Body>
          </StripParts.Reel>
        </>
      )}
    </StripParts.Root>
  )
}

export const tiles: Tile[] = [
  {
    id: "studio-map",
    title: "Fit the whole city.",
    blurb: "Zoom to fit all, or just your picks.",
    size: "large",
    Demo: SightMap,
  },
  {
    id: "studio-notes",
    title: "Take it back.",
    blurb: "Typing undoes in runs, not letters.",
    tone: "paper",
    Demo: NoteHistory,
  },
  {
    id: "studio-trash",
    title: "Press twice.",
    blurb: "It arms first, then disarms itself.",
    Demo: EmptyTrash,
  },
  {
    id: "studio-timer",
    title: "Run it from outside.",
    blurb: "Buttons drive it through a controller.",
    tone: "ink",
    Demo: BrewTimer,
  },
  {
    id: "studio-week",
    title: "Block out the week.",
    blurb: "Draw, move and stretch across days.",
    size: "wide",
    tone: "paper",
    Demo: WeekSpans,
  },
  {
    id: "studio-chat",
    title: "Follow the thread.",
    blurb: "Scroll up and new replies wait.",
    size: "tall",
    tone: "sky",
    Demo: StickyChat,
  },
  {
    id: "studio-emoji",
    title: "React in a word.",
    blurb: "Fuzzy search, then arrows through the grid.",
    tone: "butter",
    Demo: EmojiPicker,
  },
  {
    id: "studio-film",
    title: "A year on one strip.",
    blurb: "Only the frames in view exist.",
    size: "wide",
    Demo: FilmStrip,
  },
  {
    id: "studio-slide",
    title: "Move them together.",
    blurb: "Shift-click to group, then drag one.",
    tone: "blueprint",
    Demo: SlideGroup,
  },
]
