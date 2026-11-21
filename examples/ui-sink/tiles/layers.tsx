import {
  ArrowClockwiseIcon,
  ArrowCounterClockwiseIcon,
  CaretDownIcon,
  ChartBarIcon,
  CheckIcon,
  DesktopIcon,
  EnvelopeSimpleIcon,
  FilePdfIcon,
  HouseIcon,
  LinkIcon,
  MoonIcon,
  PaperPlaneTiltIcon,
  PauseIcon,
  PencilSimpleIcon,
  PlayIcon,
  ShareNetworkIcon,
  StarIcon,
  SunIcon,
  TrashIcon,
  TrayIcon,
  UsersIcon,
} from "@phosphor-icons/react"
import {
  bindPanel,
  bindPersistentLayer,
  createColorScheme,
  createLocalStorage,
  createPopup,
  createPreference,
  definePrimitive,
  hiddenAttrs,
  isDismissal,
  openAttrs,
  popupArrow,
  popupClose,
  popupContent,
  popupDescription,
  popupState,
  popupTitle,
  popupTrigger,
  supportsTopLayer,
  type ChangeDetails,
} from "@ui-sdk/core"
import { popoverPreset } from "@ui-sdk/core/popover"
import { createConfirm } from "@ui-sdk/react/alert-dialog"
import {
  Dialog,
  DialogBackdrop,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@ui-sdk/react/dialog"
import {
  Menu,
  MenuContent,
  MenuItem,
  MenuSeparator,
  MenuTrigger,
} from "@ui-sdk/react/menu"
import { createOverlays, OverlayHost } from "@ui-sdk/react/overlays"
import {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverTitle,
  PopoverTrigger,
} from "@ui-sdk/react/popover"
import { Preferences, usePreference } from "@ui-sdk/react/preferences"
import { createParts } from "@ui-sdk/react/primitive"
import {
  createResizableController,
  Resizable,
  ResizableHandle,
  ResizablePanel,
  resizableStorageKey,
} from "@ui-sdk/react/resizable"
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from "@ui-sdk/react/segmented-control"
import { registerShortcut, useShortcutHint } from "@ui-sdk/react/shortcuts"
import { useEffect, useLayoutEffect, useRef, useState } from "react"
import type { Tile } from "../bento/types"
import "./layers.css"

const colorScheme = createColorScheme({
  storage: createLocalStorage("sink-layers-scheme"),
})

const accents = ["iris", "moss", "ember"] as const

const accentLabels = { iris: "Iris", moss: "Moss", ember: "Ember" }

const accent = createPreference({
  values: accents,
  defaultValue: "iris",
  attribute: "data-accent",
  storage: createLocalStorage("sink-layers-accent"),
})

const schemeOptions = [
  { value: "light", label: "Light", icon: SunIcon },
  { value: "dark", label: "Dark", icon: MoonIcon },
  { value: "system", label: "System", icon: DesktopIcon },
] as const

const revenueBars = [38, 52, 44, 61, 57, 72, 66, 84]

const appStats = [
  { label: "Orders", value: "1,284" },
  { label: "Refunds", value: "12" },
  { label: "Avg. order", value: "$37.50" },
]

function Theme() {
  const scheme = usePreference(colorScheme)
  const { value: accentValue } = usePreference(accent)
  const status =
    scheme.value !== "system"
      ? "Saved in this browser"
      : scheme.system === "unknown"
        ? "Follows your system"
        : `Your system is ${scheme.system} right now`
  return (
    <div className="ly-theme">
      <div className="ly-theme-bar">
        <SegmentedControl
          className="ly-seg"
          aria-label="Theme"
          value={scheme.value}
          onValueChange={(value) => value && colorScheme.set(value)}
        >
          <SegmentedControlIndicator className="ly-seg-indicator" />
          {schemeOptions.map(({ value, label, icon: Icon }) => (
            <SegmentedControlItem
              key={value}
              value={value}
              className="ly-seg-item"
            >
              <Icon size={14} weight="bold" aria-hidden />
              {label}
            </SegmentedControlItem>
          ))}
        </SegmentedControl>
        <div className="ly-swatches" role="group" aria-label="Accent">
          {accents.map((name) => (
            <button
              key={name}
              type="button"
              className="ly-swatch"
              data-accent={name}
              aria-label={accentLabels[name]}
              aria-pressed={accentValue === name}
              onClick={() => accent.set(name)}
            >
              <CheckIcon size={11} weight="bold" aria-hidden />
            </button>
          ))}
        </div>
      </div>
      <p className="ly-theme-status" aria-live="polite">
        {status}
      </p>
      <Preferences preferences={[colorScheme, accent]}>
        <div className="ly-app" aria-label="Preview" role="img">
          <div className="ly-app-side" aria-hidden>
            <span className="ly-app-logo" />
            <span className="ly-app-nav" data-current>
              <HouseIcon size={13} weight="fill" />
              Home
            </span>
            <span className="ly-app-nav">
              <ChartBarIcon size={13} weight="bold" />
              Reports
            </span>
            <span className="ly-app-nav">
              <UsersIcon size={13} weight="bold" />
              Customers
            </span>
            <span className="ly-app-me">
              <span className="ly-app-avatar">JB</span>
              Jo Berg
            </span>
          </div>
          <div className="ly-app-main" aria-hidden>
            <span className="ly-app-head">
              <span>
                <span className="ly-app-kicker">This quarter</span>
                <span className="ly-app-figure">$48,210</span>
              </span>
              <span className="ly-app-delta">+12.4%</span>
            </span>
            <span className="ly-app-stats">
              {appStats.map((stat) => (
                <span key={stat.label} className="ly-app-stat">
                  <span className="ly-app-kicker">{stat.label}</span>
                  <span className="ly-app-stat-value">{stat.value}</span>
                </span>
              ))}
            </span>
            <span className="ly-app-chart">
              {revenueBars.map((height, index) => (
                <span
                  key={index}
                  className="ly-app-bar"
                  style={{ height: `${height}%` }}
                />
              ))}
            </span>
            <span className="ly-app-rows">
              <span className="ly-app-row">
                <span className="ly-app-avatar">MO</span>
                Mira Okafor
                <span className="ly-app-amount">$1,240</span>
              </span>
              <span className="ly-app-row">
                <span className="ly-app-avatar">TL</span>
                Tomás Lindqvist
                <span className="ly-app-amount">$860</span>
              </span>
              <span className="ly-app-row">
                <span className="ly-app-avatar">AS</span>
                Aiko Sato
                <span className="ly-app-amount">$615</span>
              </span>
            </span>
          </div>
        </div>
      </Preferences>
    </div>
  )
}

const splitId = "sink-layers-mail"
const splitDefault = [38, 62]

const readSavedWidth = () => {
  try {
    const saved = JSON.parse(
      localStorage.getItem(resizableStorageKey(splitId)) ?? "{}"
    ) as Record<string, number[]>
    return Object.values(saved)[0]?.[0]
  } catch {
    return undefined
  }
}

const folders = [
  { label: "Inbox", icon: TrayIcon, count: 12 },
  { label: "Starred", icon: StarIcon, count: 3 },
  { label: "Sent", icon: PaperPlaneTiltIcon, count: 0 },
]

function Split() {
  const [controller] = useState(createResizableController)
  const [savedWidth, setSavedWidth] = useState<number>()
  useEffect(() => setSavedWidth(readSavedWidth()), [])

  function reset() {
    controller.setLayout(splitDefault)
    localStorage.removeItem(resizableStorageKey(splitId))
    setSavedWidth(undefined)
  }

  return (
    <div className="ly-split">
      <Resizable
        className="ly-split-group"
        autoSaveId={splitId}
        controller={controller}
        onLayoutCommit={() => setSavedWidth(readSavedWidth())}
      >
        <ResizablePanel
          id="ly-folders"
          defaultSize={splitDefault[0]}
          minSize={24}
          maxSize={64}
          className="ly-split-pane ly-split-folders"
        >
          {folders.map(({ label, icon: Icon, count }, index) => (
            <span
              key={label}
              className="ly-folder"
              data-current={index === 0 || undefined}
            >
              <Icon size={14} weight="bold" aria-hidden />
              <span className="ly-folder-label">{label}</span>
              {count > 0 && <span className="ly-folder-count">{count}</span>}
            </span>
          ))}
        </ResizablePanel>
        <ResizableHandle
          aria-label="Resize folders"
          className="ly-split-handle"
        >
          <span className="ly-split-grip" />
        </ResizableHandle>
        <ResizablePanel id="ly-message" className="ly-split-pane">
          <span className="ly-mail-from">
            <EnvelopeSimpleIcon size={14} weight="fill" aria-hidden />
            Ines Moreau
          </span>
          <span className="ly-mail-subject">Friday review moved</span>
          <span className="ly-mail-line" />
          <span className="ly-mail-line" />
          <span className="ly-mail-line" data-short />
        </ResizablePanel>
      </Resizable>
      <div className="ly-split-foot">
        <span className="ly-split-saved" aria-live="polite">
          {savedWidth === undefined
            ? "Default layout"
            : `Saved at ${Math.round(savedWidth)}%`}
        </span>
        <button
          type="button"
          className="b-btn ly-split-reset"
          disabled={savedWidth === undefined}
          onClick={reset}
        >
          <ArrowCounterClockwiseIcon size={13} weight="bold" aria-hidden />
          Reset
        </button>
      </div>
    </div>
  )
}

const episodeLength = 47 * 60 + 12

const clock = (seconds: number) =>
  `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`

function Player() {
  const [playing, setPlaying] = useState(false)
  const [elapsed, setElapsed] = useState(12 * 60 + 34)
  const [lifted, setLifted] = useState<DOMRect | null>(null)
  const player = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (player.current) {
      return bindPersistentLayer(player.current)
    }
  }, [])

  useEffect(() => {
    if (!playing) {
      return
    }
    const timer = setInterval(
      () => setElapsed((seconds) => Math.min(seconds + 1, episodeLength)),
      1000
    )
    return () => clearInterval(timer)
  }, [playing])

  useLayoutEffect(() => {
    if (lifted) {
      player.current?.showPopover()
    }
  }, [lifted])

  const skip = (seconds: number) =>
    setElapsed((current) =>
      Math.min(Math.max(current + seconds, 0), episodeLength)
    )

  return (
    <div className="ly-listen">
      <div className="ly-player-slot">
        <div
          ref={player}
          className="ly-player"
          data-playing={playing || undefined}
          popover={lifted ? "manual" : undefined}
          style={
            lifted
              ? { top: lifted.top, left: lifted.left, width: lifted.width }
              : undefined
          }
        >
          <span className="ly-cover" aria-hidden>
            <span className="ly-eq">
              <span />
              <span />
              <span />
            </span>
          </span>
          <span className="ly-track">
            <span className="ly-track-title">Shipping on Fridays</span>
            <span className="ly-track-show">Changelog Radio · Ep. 112</span>
          </span>
          <span className="ly-transport">
            <button
              type="button"
              className="ly-icon-btn"
              aria-label="Back 15 seconds"
              onClick={() => skip(-15)}
            >
              <ArrowCounterClockwiseIcon size={16} weight="bold" />
            </button>
            <button
              type="button"
              className="ly-icon-btn ly-play"
              aria-label={playing ? "Pause" : "Play"}
              onClick={() => setPlaying(!playing)}
            >
              {playing ? (
                <PauseIcon size={16} weight="fill" />
              ) : (
                <PlayIcon size={16} weight="fill" />
              )}
            </button>
            <button
              type="button"
              className="ly-icon-btn"
              aria-label="Forward 30 seconds"
              onClick={() => skip(30)}
            >
              <ArrowClockwiseIcon size={16} weight="bold" />
            </button>
          </span>
          <span className="ly-progress">
            <span className="ly-time">{clock(elapsed)}</span>
            <span
              className="ly-progress-track"
              role="progressbar"
              aria-label="Played"
              aria-valuemin={0}
              aria-valuemax={episodeLength}
              aria-valuenow={elapsed}
              aria-valuetext={clock(elapsed)}
            >
              <span
                className="ly-progress-fill"
                style={{ width: `${(elapsed / episodeLength) * 100}%` }}
              />
            </span>
            <span className="ly-time">-{clock(episodeLength - elapsed)}</span>
          </span>
        </div>
      </div>
      <Dialog
        onOpenChange={(open) => {
          if (open && supportsTopLayer() && player.current) {
            setLifted(player.current.getBoundingClientRect())
          }
        }}
        onOpenChangeComplete={(open) => {
          if (!open && lifted) {
            player.current?.hidePopover()
            setLifted(null)
          }
        }}
      >
        <DialogTrigger className="b-btn ly-share">
          <ShareNetworkIcon size={15} weight="bold" aria-hidden />
          Share episode
        </DialogTrigger>
        <DialogBackdrop className="ly-backdrop" />
        <DialogContent className="ly-dialog">
          <DialogTitle className="ly-dialog-title">Share episode</DialogTitle>
          <DialogDescription className="ly-dialog-desc">
            The player keeps going. Pause or skip it while you’re here.
          </DialogDescription>
          <div className="ly-link">
            <LinkIcon size={14} weight="bold" aria-hidden />
            <input
              className="ly-link-input"
              aria-label="Episode link"
              readOnly
              value={`changelog.fm/112?t=${elapsed}`}
            />
          </div>
          <div className="ly-actions">
            <DialogClose className="b-btn" data-variant="primary">
              Done
            </DialogClose>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  )
}

const notePopup = definePrimitive({
  name: "ly-note",
  options: { defaultOpen: "boolean", onOpenChange: "callback" },
  state: (options: {
    defaultOpen?: boolean
    onOpenChange?: (open: boolean, details: ChangeDetails<string>) => void
  }) => popupState(options),
  api: (context) => createPopup(context, { preset: popoverPreset }),
  slots: (slot) => ({
    trigger: popupTrigger(slot),
    content: popupContent(slot),
    title: popupTitle(slot),
    description: popupDescription(slot),
    close: popupClose(slot),
    arrow: popupArrow(slot),
  }),
})

const NotePopup = createParts(notePopup, {
  trigger: "button",
  content: "div",
  title: "h3",
  description: "p",
  close: "button",
  arrow: "div",
})

function Note() {
  const [note, setNote] = useState("Rotate after the 14:00 deploy.")
  const [draft, setDraft] = useState(note)
  const [asking, setAsking] = useState(false)
  const field = useRef<HTMLTextAreaElement>(null)

  return (
    <div className="ly-task">
      <div className="ly-task-head">
        <span className="ly-task-check" aria-hidden />
        <span className="ly-task-text">
          <span className="ly-task-title">Rotate the staging keys</span>
          <span className="ly-task-meta">Due today · Infra</span>
        </span>
      </div>
      <NotePopup.Root
        onOpenChange={(open, details) => {
          if (open) {
            setDraft(note)
            setAsking(false)
            return
          }
          if (isDismissal(details.reason) && draft !== note) {
            details.cancel()
            if (
              details.reason === "escape-key" ||
              details.reason === "outside-press"
            ) {
              setAsking(true)
            }
          }
        }}
      >
        <NotePopup.Trigger className="ly-note-trigger">
          <PencilSimpleIcon size={13} weight="bold" aria-hidden />
          <span className="ly-note-text">{note || "Add a note"}</span>
        </NotePopup.Trigger>
        <NotePopup.Content
          className="b-pop ly-note-pop"
          sideOffset={10}
          collisionPadding={12}
        >
          <NotePopup.Arrow className="ly-arrow" />
          <NotePopup.Title className="ly-note-title">Edit note</NotePopup.Title>
          <NotePopup.Description className="ly-note-desc">
            Only your team sees it.
          </NotePopup.Description>
          <textarea
            ref={field}
            className="b-input ly-note-field"
            aria-label="Note"
            rows={3}
            value={draft}
            onChange={(event) => {
              setDraft(event.target.value)
              setAsking(false)
            }}
          />
          {asking ? (
            <div className="ly-discard" role="alert">
              <span>Discard changes?</span>
              <span className="ly-discard-actions">
                <button
                  type="button"
                  className="b-btn"
                  onClick={() => {
                    setAsking(false)
                    field.current?.focus()
                  }}
                >
                  Keep editing
                </button>
                <NotePopup.Close className="b-btn ly-destroy">
                  Discard
                </NotePopup.Close>
              </span>
            </div>
          ) : (
            <div className="ly-note-actions">
              <NotePopup.Close className="b-btn">Cancel</NotePopup.Close>
              <NotePopup.Close
                className="b-btn"
                data-variant="primary"
                onClick={() => setNote(draft.trim())}
              >
                Save
              </NotePopup.Close>
            </div>
          )}
        </NotePopup.Content>
      </NotePopup.Root>
    </div>
  )
}

const receipt = definePrimitive({
  name: "ly-receipt",
  options: { defaultOpen: "boolean" },
  state: (options: { defaultOpen?: boolean }) => ({
    open: options.defaultOpen ?? false,
    exiting: false,
  }),
  api: ({ store, element }) => {
    let toggled = false
    return {
      toggle() {
        toggled = true
        const open = !store.get().open
        store.set({ open, exiting: !open })
      },
      opening: () => (toggled ? "animate" : "mount") as "animate" | "mount",
      trigger: () => element("trigger"),
    }
  },
  slots: (slot) => ({
    trigger: slot({
      attrs: (state, { id }) => ({
        type: "button",
        "aria-expanded": String(state.open),
        "aria-controls": `${id}-panel`,
        ...openAttrs(state.open),
      }),
      press: ({ api }) => api.toggle(),
    }),
    panel: slot({
      present: (state) => state.open || state.exiting,
      attrs: (state, { id }) => ({
        id: `${id}-panel`,
        ...hiddenAttrs(state.open || state.exiting, false),
        ...openAttrs(state.open),
      }),
      bind: (el, { api, store }) =>
        bindPanel(el, {
          size: (el, height) => el.style.setProperty("--ly-panel", height),
          isOpen: () => store.get().open,
          isExiting: () => store.get().exiting,
          isFindable: () => false,
          opening: api.opening,
          reveal: () => {},
          trigger: api.trigger,
          subscribe: store.subscribe,
          settled: () => {},
          exited: () => store.set({ exiting: false }),
        }),
    }),
  }),
})

const Receipt = createParts(receipt, { trigger: "button", panel: "div" })

const orders = [
  {
    id: "4821",
    date: "Nov 14",
    status: "Delivered",
    lines: [
      { item: "Ceramic pour-over", qty: 1, price: 38 },
      { item: "Paper filters", qty: 2, price: 7.5 },
      { item: "Single-origin, 250 g", qty: 1, price: 19 },
    ],
  },
  {
    id: "4790",
    date: "Oct 30",
    status: "Delivered",
    lines: [
      { item: "Gooseneck kettle", qty: 1, price: 64 },
      { item: "Descaler", qty: 1, price: 9 },
    ],
  },
  {
    id: "4702",
    date: "Oct 3",
    status: "Refunded",
    lines: [{ item: "Burr grinder", qty: 1, price: 129 }],
  },
  {
    id: "4655",
    date: "Sep 12",
    status: "Delivered",
    lines: [
      { item: "Glass carafe", qty: 1, price: 24 },
      { item: "Cork coasters", qty: 4, price: 3 },
    ],
  },
]

const money = (amount: number) => `$${amount.toFixed(2)}`

function Receipts() {
  return (
    <div className="ly-orders">
      <p className="ly-orders-head">Orders</p>
      {orders.map((order, index) => {
        const subtotal = order.lines.reduce(
          (sum, line) => sum + line.qty * line.price,
          0
        )
        const tax = subtotal * 0.08
        return (
          <Receipt.Root key={order.id} defaultOpen={index === 0}>
            <section className="ly-order">
              <Receipt.Trigger className="ly-order-trigger">
                <span className="ly-order-text">
                  <span className="ly-order-id">Order #{order.id}</span>
                  <span className="ly-order-meta">
                    {order.date} ·{" "}
                    <span data-status={order.status.toLowerCase()}>
                      {order.status}
                    </span>
                  </span>
                </span>
                <span className="ly-order-total">{money(subtotal + tax)}</span>
                <CaretDownIcon
                  size={14}
                  weight="bold"
                  className="ly-order-caret"
                  aria-hidden
                />
              </Receipt.Trigger>
              <Receipt.Panel className="ly-receipt">
                <dl className="ly-receipt-lines">
                  {order.lines.map((line) => (
                    <div key={line.item} className="ly-receipt-line">
                      <dt>
                        {line.item}
                        {line.qty > 1 && (
                          <span className="ly-receipt-qty"> × {line.qty}</span>
                        )}
                      </dt>
                      <dd>{money(line.qty * line.price)}</dd>
                    </div>
                  ))}
                  <div className="ly-receipt-line" data-kind="sum">
                    <dt>Subtotal</dt>
                    <dd>{money(subtotal)}</dd>
                  </div>
                  <div className="ly-receipt-line" data-kind="sum">
                    <dt>Tax</dt>
                    <dd>{money(tax)}</dd>
                  </div>
                  <div className="ly-receipt-line" data-kind="total">
                    <dt>Total</dt>
                    <dd>{money(subtotal + tax)}</dd>
                  </div>
                </dl>
              </Receipt.Panel>
            </section>
          </Receipt.Root>
        )
      })}
    </div>
  )
}

const stackOverlays = createOverlays()
const confirmRemoval = createConfirm({ overlays: stackOverlays })

const peopleSeed = [
  { name: "Amara Diallo", initials: "AD", role: "Can edit" },
  { name: "Kenji Sato", initials: "KS", role: "Can view" },
  { name: "Elif Kaya", initials: "EK", role: "Can view" },
]

function Stack() {
  const [people, setPeople] = useState(peopleSeed)
  const [sharing, setSharing] = useState(false)
  const [menuFor, setMenuFor] = useState<string | null>(null)
  const [asking, setAsking] = useState<string | null>(null)
  const content = useRef<HTMLDivElement>(null)
  const hint = useShortcutHint("mod+backspace")

  async function remove(name: string) {
    setAsking(name)
    const removed = await confirmRemoval({
      title: `Remove ${name}?`,
      description: "They lose access to Q3 report.pdf right away.",
      confirmLabel: "Remove",
    })
    setAsking(null)
    if (!removed) {
      return
    }
    const row = content.current?.querySelector(
      `[data-person="${CSS.escape(name)}"]`
    )
    const next = (
      row?.nextElementSibling ?? row?.previousElementSibling
    )?.querySelector("button")
    ;(next ?? content.current)?.focus()
    setPeople((list) => list.filter((person) => person.name !== name))
  }

  const removeRef = useRef(remove)
  useLayoutEffect(() => {
    removeRef.current = remove
  })

  useEffect(
    () =>
      registerShortcut(
        "mod+backspace",
        () => {
          const row =
            document.activeElement?.closest<HTMLElement>("[data-person]")
          if (row?.dataset.person) {
            removeRef.current(row.dataset.person)
          }
        },
        { layer: () => content.current, description: "Remove person" }
      ),
    []
  )

  const layers = [
    sharing && "Share popover",
    menuFor && `Role menu · ${menuFor.split(" ")[0]}`,
    asking && `Confirm · Remove ${asking.split(" ")[0]}?`,
  ].filter((layer): layer is string => !!layer)

  return (
    <div className="ly-stack">
      <div className="ly-file">
        <span className="ly-file-icon" aria-hidden>
          <FilePdfIcon size={20} weight="fill" />
        </span>
        <span className="ly-file-text">
          <span className="ly-file-name">Q3 report.pdf</span>
          <span className="ly-file-meta">
            Shared with {people.length}{" "}
            {people.length === 1 ? "person" : "people"}
          </span>
        </span>
        <Popover
          onOpenChange={(open) => {
            setSharing(open)
            if (!open) {
              setMenuFor(null)
            }
          }}
        >
          <PopoverTrigger className="b-btn ly-trigger" data-variant="primary">
            Share
          </PopoverTrigger>
          <PopoverContent
            ref={content}
            className="b-pop ly-share-pop"
            align="end"
            sideOffset={8}
            collisionPadding={12}
          >
            <PopoverTitle className="ly-share-title">Share report</PopoverTitle>
            <PopoverDescription className="ly-share-desc">
              {hint.text} removes the focused person.
            </PopoverDescription>
            {people.length ? (
              <ul className="ly-people">
                {people.map((person) => (
                  <li
                    key={person.name}
                    className="ly-person"
                    data-person={person.name}
                  >
                    <span className="ly-avatar" aria-hidden>
                      {person.initials}
                    </span>
                    <span className="ly-person-name">{person.name}</span>
                    <Menu
                      onOpenChange={(open) =>
                        setMenuFor(open ? person.name : null)
                      }
                      onAction={(value) => {
                        if (value === "remove") {
                          remove(person.name)
                          return
                        }
                        setPeople((list) =>
                          list.map((other) =>
                            other === person ? { ...other, role: value } : other
                          )
                        )
                      }}
                    >
                      <MenuTrigger
                        className="ly-role"
                        aria-label={`${person.name}: ${person.role}`}
                        aria-keyshortcuts={hint.ariaKeyshortcuts}
                      >
                        {person.role}
                        <CaretDownIcon size={12} weight="bold" aria-hidden />
                      </MenuTrigger>
                      <MenuContent
                        className="b-pop ly-menu"
                        align="end"
                        sideOffset={4}
                        collisionPadding={12}
                      >
                        <MenuItem value="Can edit" className="b-item">
                          Can edit
                        </MenuItem>
                        <MenuItem value="Can view" className="b-item">
                          Can view
                        </MenuItem>
                        <MenuSeparator className="ly-menu-sep" />
                        <MenuItem value="remove" className="b-item ly-danger">
                          <TrashIcon size={14} weight="bold" aria-hidden />
                          Remove access
                        </MenuItem>
                      </MenuContent>
                    </Menu>
                  </li>
                ))}
              </ul>
            ) : (
              <button
                type="button"
                className="b-btn ly-people-reset"
                onClick={() => setPeople(peopleSeed)}
              >
                Invite everyone back
              </button>
            )}
            <div className="ly-host">
              <OverlayHost overlays={stackOverlays} />
            </div>
          </PopoverContent>
        </Popover>
      </div>
      <div className="ly-layers">
        <p className="ly-layers-label">Open layers</p>
        <ol className="ly-layers-list" aria-live="polite">
          {layers.length ? (
            layers.map((layer, index) => (
              <li
                key={layer}
                className="ly-layer"
                data-top={index === layers.length - 1 || undefined}
              >
                {layer}
                {index === layers.length - 1 && (
                  <kbd className="ly-esc">Esc</kbd>
                )}
              </li>
            ))
          ) : (
            <li className="ly-layer" data-empty>
              Nothing open
            </li>
          )}
        </ol>
      </div>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "overlays-theme",
    title: "Light, dark or system.",
    blurb: "Themes its preview, and remembers after a reload.",
    size: "large",
    tone: "paper",
    Demo: Theme,
  },
  {
    id: "overlays-split",
    title: "The split remembers.",
    blurb: "Drag it, reload, and it stays put.",
    Demo: Split,
  },
  {
    id: "overlays-player",
    title: "Plays under a modal.",
    blurb: "The player stays live above the dialog.",
    tone: "ink",
    Demo: Player,
  },
  {
    id: "overlays-discard",
    title: "Discard changes?",
    blurb: "Escape asks first. Save and Cancel don’t.",
    tone: "butter",
    Demo: Note,
  },
  {
    id: "overlays-receipt",
    title: "Expand the receipt.",
    blurb: "Each panel animates to its own height.",
    size: "tall",
    Demo: Receipts,
  },
  {
    id: "overlays-stack",
    title: "Escape closes the top.",
    blurb: "Stacked layers close from the top down.",
    size: "wide",
    tone: "sky",
    Demo: Stack,
  },
]
