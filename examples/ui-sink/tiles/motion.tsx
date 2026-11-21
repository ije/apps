import {
  ArchiveIcon,
  ArrowClockwiseIcon,
  BookmarkSimpleIcon,
  CalendarBlankIcon,
  CaretLeftIcon,
  CoffeeIcon,
  CornersOutIcon,
  DotsSixVerticalIcon,
  FileTextIcon,
  FolderSimpleIcon,
  FrameCornersIcon,
  GradientIcon,
  ImageIcon,
  LightbulbIcon,
  MagnifyingGlassMinusIcon,
  MagnifyingGlassPlusIcon,
  MapPinIcon,
  NavigationArrowIcon,
  PackageIcon,
  PenNibIcon,
  PersonSimpleWalkIcon,
  PlusIcon,
  RectangleIcon,
  SealPercentIcon,
  ShareNetworkIcon,
  TextTIcon,
  TrashIcon,
  XIcon,
  type Icon,
} from "@phosphor-icons/react"
import {
  bindDrag,
  bindDragPreview,
  createArea,
  createAutoScroll,
  createChange,
  createConnect,
  createLasso,
  createMove,
  createPreview,
  createReorder,
  createResize,
  createRovingFocus,
  createSelection,
  createSpans,
  createSwipeActions,
  createTransfer,
  createTreeReorder,
  createViewport,
  css,
  definePrimitive,
  dragThreshold,
  isSpanFree,
  live,
  stackSpans,
  type ConnectDraft,
  type DragPreview,
  type LassoDraft,
  type MoveDraft,
  type MovePoint,
  type ReorderDraft,
  type ResizeBox,
  type ResizeDraft,
  type ResizeHandle,
  type Span,
  type SpansDraft,
  type SwipeDraft,
  type SwipeOpen,
  type TransferDraft,
  type TransferLists,
  type TreeReorderDraft,
  type TreeReorderRow,
} from "@ui-sdk/core"
import {
  createVelocityTracker,
  projectDistance,
  rubberBand,
  sectorOf,
  solveSpring,
} from "@ui-sdk/core/gesture"
import { clamp } from "@ui-sdk/core/number"
import { createParts } from "@ui-sdk/react/primitive"
import {
  Resizable,
  ResizableHandle,
  ResizablePanel,
} from "@ui-sdk/react/resizable"
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react"
import type { Tile } from "../bento/types"
import "./motion.css"

/* Shared helpers */

const reduced = () =>
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches

/** Animates elements marked `data-flip` from where they were to where they are, so the others make room. */
function useFlip(ref: RefObject<HTMLElement | null>) {
  const last = useRef(new Map<string, { x: number; y: number }>())
  useLayoutEffect(() => {
    const root = ref.current
    if (!root) {
      return
    }
    const next = new Map<string, { x: number; y: number }>()
    const calm = reduced()
    for (const el of root.querySelectorAll<HTMLElement>("[data-flip]")) {
      const id = el.dataset.flip!
      const at = { x: el.offsetLeft, y: el.offsetTop }
      const was = last.current.get(id)
      next.set(id, at)
      if (!was || calm || el.hasAttribute("data-grabbed")) {
        continue
      }
      const dx = was.x - at.x
      const dy = was.y - at.y
      if (dx || dy) {
        el.animate(
          [{ translate: `${dx}px ${dy}px` }, { translate: "0px 0px" }],
          {
            duration: 220,
            easing: "cubic-bezier(0.22, 1, 0.36, 1)",
          }
        )
      }
    }
    last.current = next
  })
}

/** Measures a root element into the store's `size`, for bounds that follow the tile. */
function observeSize(
  el: HTMLElement,
  write: (size: { w: number; h: number }) => void
) {
  const observer = new ResizeObserver(() =>
    write({ w: el.clientWidth, h: el.clientHeight })
  )
  observer.observe(el)
  write({ w: el.clientWidth, h: el.clientHeight })
  return () => observer.disconnect()
}

/** Runs a 2D spring from `from` to `to`, calling `paint` each frame. Returns a cancel. */
function runSpring(
  from: MovePoint,
  to: MovePoint,
  velocity: MovePoint,
  paint: (point: MovePoint) => void,
  done?: () => void,
  { response = 0.42, damping = 0.7, tolerance = 0.4 } = {}
) {
  if (reduced()) {
    paint(to)
    done?.()
    return () => {}
  }
  const sx = solveSpring(response, damping, from.x - to.x, velocity.x)
  const sy = solveSpring(response, damping, from.y - to.y, velocity.y)
  const start = performance.now()
  let frame = 0
  const tick = (now: number) => {
    const t = now - start
    const x = sx.position(t)
    const y = sy.position(t)
    const still =
      Math.abs(x) < tolerance &&
      Math.abs(y) < tolerance &&
      Math.abs(sx.velocity(t)) < 0.05 * tolerance &&
      Math.abs(sy.velocity(t)) < 0.05 * tolerance
    if (still) {
      paint(to)
      done?.()
      return
    }
    paint({ x: to.x + x, y: to.y + y })
    frame = requestAnimationFrame(tick)
  }
  frame = requestAnimationFrame(tick)
  return () => cancelAnimationFrame(frame)
}

/* 1. Sortable list: poster layers */

const POSTER: Record<string, { name: string; icon: Icon }> = {
  headline: { name: "Sale headline", icon: TextTIcon },
  shot: { name: "Product shot", icon: ImageIcon },
  badge: { name: "Price badge", icon: SealPercentIcon },
  fill: { name: "Gradient fill", icon: GradientIcon },
}

const layerList = definePrimitive({
  name: "mo-layer-list",
  state: () => ({
    order: Object.keys(POSTER),
    moving: null as ReorderDraft | null,
    focused: null as string | null,
  }),
  api: (context) => {
    const change = createChange(context)
    const reorder = createReorder(context, {
      slot: "item",
      key: "order",
      draft: "moving",
      set: (value, details) => change("order", value, details),
      label: (key) => POSTER[key]?.name,
    })
    const roving = createRovingFocus(context, { slot: "item", key: "focused" })
    return { reorder, roving }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => ({ order: api.reorder.order(state) }),
      attrs: () => ({
        role: "listbox",
        "aria-label": "Poster layers",
        "aria-roledescription": "sortable list",
      }),
    }),
    item: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        role: "option",
        "aria-roledescription": "sortable layer",
        "aria-selected": String(part.api.reorder.moving(state, part.key)),
        tabindex: part.api.roving.tabIndex(state, part),
        "data-flip": part.key,
        "data-grabbed": part.api.reorder.dragging(state, part.key)
          ? ""
          : undefined,
        "data-moving": part.api.reorder.moving(state, part.key)
          ? ""
          : undefined,
      }),
      on: {
        keydown: (event, { el, api, key }) => {
          if (!api.reorder.keydown(event, key) && el) {
            api.roving.keydown(event, el)
          }
        },
        pointerdown: (event, { api, key }) =>
          api.reorder.pointerdown(event, key),
      },
      bind: (el, part) => {
        const cleanups = [
          part.api.roving.bind(el, part),
          part.api.reorder.bind(el, part),
        ]
        return () => cleanups.forEach((cleanup) => cleanup())
      },
    }),
  }),
})

const LayerParts = createParts(layerList, { root: "ul", item: "li" })

function LayerRows({
  order,
  flip,
}: {
  order: string[]
  flip: RefObject<HTMLUListElement | null>
}) {
  useFlip(flip)
  return order.map((key) => {
    const { name, icon: LayerIcon } = POSTER[key]
    return (
      <LayerParts.Item key={key} value={key} className="mo-layer">
        <DotsSixVerticalIcon
          className="mo-grip"
          size={16}
          weight="bold"
          aria-hidden
        />
        <span className="mo-layer-icon">
          <LayerIcon size={16} weight="fill" aria-hidden />
        </span>
        <span className="mo-layer-name">{name}</span>
      </LayerParts.Item>
    )
  })
}

function SortableLayers() {
  const ref = useRef<HTMLUListElement>(null)
  return (
    <div className="mo-layers-wrap">
      <LayerParts.Root ref={ref} className="mo-layers">
        {({ order }) => <LayerRows order={order} flip={ref} />}
      </LayerParts.Root>
    </div>
  )
}

/* 2. Sortable grid with a selection that moves together */

const SWATCHES: Record<string, string> = {
  terracotta: "oklch(0.62 0.14 40)",
  sand: "oklch(0.86 0.06 85)",
  olive: "oklch(0.6 0.09 120)",
  pine: "oklch(0.42 0.07 160)",
  lagoon: "oklch(0.66 0.1 200)",
  denim: "oklch(0.48 0.1 255)",
  charcoal: "oklch(0.3 0.01 260)",
  blush: "oklch(0.82 0.07 20)",
}

const swatchGrid = definePrimitive({
  name: "mo-swatch-grid",
  state: () => ({
    order: Object.keys(SWATCHES),
    moving: null as ReorderDraft | null,
    selected: ["olive", "lagoon"] as string[],
    focused: null as string | null,
  }),
  api: (context) => {
    const change = createChange(context)
    const selection = createSelection(context, {
      key: "selected",
      set: (value, details) => change("selected", value, details),
      order: () => context.store.get().order,
      multiple: () => true,
    })
    const reorder = createReorder(context, {
      slot: "swatch",
      key: "order",
      draft: "moving",
      orientation: () => "grid",
      preview: true,
      createAutoScroll: false,
      group: (key) => {
        const { selected } = context.store.get()
        return selected.includes(key) ? selected : [key]
      },
      set: (value, details) => change("order", value, details),
      label: (key) => key,
    })
    const roving = createRovingFocus(context, {
      slot: "swatch",
      key: "focused",
      orientation: () => "horizontal",
    })
    return {
      selection,
      reorder,
      roving,
      press(event: PointerEvent, key: string) {
        const why = { reason: "item-press", event }
        if (event.shiftKey) {
          selection.extend(key, { ...why, from: context.store.get().focused })
        } else if (event.metaKey || event.ctrlKey) {
          selection.toggle(key, why)
        } else if (!selection.isSelected(key)) {
          selection.choose(key, why)
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => ({
        order: api.reorder.order(state),
        count: state.selected.length,
      }),
      attrs: () => ({
        role: "listbox",
        "aria-label": "Brand palette",
        "aria-multiselectable": "true",
        "aria-roledescription": "sortable grid",
      }),
    }),
    swatch: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        role: "option",
        "aria-label": part.key,
        "aria-selected": String(state.selected.includes(part.key)),
        tabindex: part.api.roving.tabIndex(state, part),
        "data-flip": part.key,
        "data-selected": state.selected.includes(part.key) ? "" : undefined,
        "data-moving": part.api.reorder.moving(state, part.key)
          ? ""
          : undefined,
        style: css({ "background-color": SWATCHES[part.key] }),
      }),
      on: {
        keydown: (event, { el, api, key }) => {
          if (api.reorder.keydown(event, key) || !el) {
            return
          }
          if (event.key === "Escape") {
            return api.selection.choose(key, {
              reason: "list-navigation",
              event,
            })
          }
          const moved = api.roving.keydown(event, el)
          if (moved?.shift) {
            api.selection.extend(moved.key, {
              from: moved.from,
              reason: "list-navigation",
              event,
            })
          }
        },
        pointerdown: (event, { api, key }) => {
          if (event.button !== 0) {
            return
          }
          api.press(event, key)
          api.reorder.pointerdown(event, key)
        },
      },
      bind: (el, part) => {
        const cleanups = [
          part.api.roving.bind(el, part),
          part.api.reorder.bind(el, part),
        ]
        return () => cleanups.forEach((cleanup) => cleanup())
      },
    }),
  }),
})

const SwatchParts = createParts(swatchGrid, { root: "div", swatch: "div" })

function SwatchCells({
  order,
  flip,
}: {
  order: string[]
  flip: RefObject<HTMLDivElement | null>
}) {
  useFlip(flip)
  return order.map((key) => (
    <SwatchParts.Swatch key={key} value={key} className="mo-swatch" />
  ))
}

function SwatchGrid() {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <SwatchParts.Root ref={ref} className="mo-palette">
      {({ order, count }) => (
        <>
          <div className="mo-palette-head">
            <span className="b-label">Brand palette</span>
            <span className="mo-palette-count">
              {count ? `${count} selected` : "Shift-click to pick more"}
            </span>
          </div>
          <div className="mo-swatches">
            <SwatchCells order={order} flip={ref} />
          </div>
        </>
      )}
    </SwatchParts.Root>
  )
}

/* 3. Transfer: a two-column board */

const CARDS: Record<string, { title: string; tag: string }> = {
  icons: { title: "Export app icons", tag: "Assets" },
  kerning: { title: "Fix nav kerning", tag: "Type" },
  legend: { title: "Map legend colors", tag: "Maps" },
  onboarding: { title: "Onboarding flow v3", tag: "Flows" },
}
const COLUMNS: Record<string, string> = {
  backlog: "Backlog",
  week: "This week",
}

const board = definePrimitive({
  name: "mo-board",
  state: () => ({
    lists: {
      backlog: ["icons", "legend", "onboarding"],
      week: ["kerning"],
    } as TransferLists,
    moving: null as TransferDraft | null,
  }),
  api: (context) => {
    const change = createChange(context)
    const transfer = createTransfer(context, {
      key: "lists",
      draft: "moving",
      set: (value, details) => change("lists", value, details),
      order: () => ["backlog", "week"],
      label: (key) => CARDS[key]?.title,
      listLabel: (list) => COLUMNS[list],
      createAutoScroll: false,
    })
    return { transfer }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => ({ lists: api.transfer.lists(state) }),
    }),
    column: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => ({
        role: "list",
        "aria-label": COLUMNS[key],
      }),
      bind: (el, part) => part.api.transfer.bindList(el, part.key),
    }),
    card: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        role: "listitem",
        tabindex: "0",
        "aria-roledescription": "draggable card",
        "data-flip": part.key,
        "data-moving": part.api.transfer.moving(state, part.key)
          ? ""
          : undefined,
        "data-dragging": part.api.transfer.dragging(state, part.key)
          ? ""
          : undefined,
      }),
      on: {
        keydown: (event, { api, key }) => void api.transfer.keydown(event, key),
        pointerdown: (event, { api, key }) =>
          api.transfer.pointerdown(event, key),
      },
      bind: (el, part) => part.api.transfer.bindItem(el, part.key),
    }),
  }),
})

const BoardParts = createParts(board, {
  root: "div",
  column: "div",
  card: "div",
})

function BoardColumns({
  lists,
  flip,
}: {
  lists: TransferLists
  flip: RefObject<HTMLDivElement | null>
}) {
  useFlip(flip)
  return Object.keys(COLUMNS).map((list) => (
    <section key={list} className="mo-col">
      <header className="mo-col-head">
        <span>{COLUMNS[list]}</span>
        <span className="mo-col-count">{lists[list]?.length ?? 0}</span>
      </header>
      <BoardParts.Column value={list} className="mo-col-list">
        {(lists[list] ?? []).map((key) => (
          <BoardParts.Card key={key} value={key} className="mo-card">
            <DotsSixVerticalIcon
              className="mo-grip"
              size={14}
              weight="bold"
              aria-hidden
            />
            <span className="mo-card-title">{CARDS[key].title}</span>
            <span className="mo-card-tag">{CARDS[key].tag}</span>
          </BoardParts.Card>
        ))}
      </BoardParts.Column>
    </section>
  ))
}

function Board() {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <BoardParts.Root ref={ref} className="mo-board">
      {({ lists }) => <BoardColumns lists={lists} flip={ref} />}
    </BoardParts.Root>
  )
}

/* 4. Tree reorder: a page outline */

const OUTLINE: Record<string, string> = {
  navbar: "Navbar",
  logo: "Logo",
  search: "Search field",
  footer: "Footer",
  legal: "Legal links",
}

const outline = definePrimitive({
  name: "mo-outline",
  state: () => ({
    rows: [
      { key: "navbar", depth: 0 },
      { key: "logo", depth: 1 },
      { key: "search", depth: 1 },
      { key: "footer", depth: 0 },
      { key: "legal", depth: 1 },
    ] as TreeReorderRow[],
    moving: null as TreeReorderDraft | null,
    focused: null as string | null,
  }),
  api: (context) => {
    const change = createChange(context)
    const tree = createTreeReorder(context, {
      key: "rows",
      draft: "moving",
      set: (value, details) => change("rows", value, details),
      indent: () => 18,
      maxDepth: () => 2,
      preview: true,
      createAutoScroll: false,
      label: (key) => OUTLINE[key],
    })
    const roving = createRovingFocus(context, { slot: "row", key: "focused" })
    return { tree, roving }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => ({ rows: api.tree.rows(state) }),
      attrs: () => ({ role: "tree", "aria-label": "Page outline" }),
    }),
    row: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => {
        const rows = part.api.tree.rows(state)
        const index = rows.findIndex((row) => row.key === part.key)
        const depth = rows[index]?.depth ?? 0
        const parent = rows[index + 1] && rows[index + 1].depth > depth
        return {
          role: "treeitem",
          "aria-level": String(depth + 1),
          "aria-selected": String(part.api.tree.moving(state, part.key)),
          tabindex: part.api.roving.tabIndex(state, part),
          "data-flip": part.key,
          "data-parent": parent ? "" : undefined,
          "data-moving": part.api.tree.moving(state, part.key) ? "" : undefined,
          style: css({ "--depth": depth }),
        }
      },
      on: {
        keydown: (event, { el, api, key }) => {
          if (!api.tree.keydown(event, key) && el) {
            api.roving.keydown(event, el)
          }
        },
        pointerdown: (event, { api, key }) => api.tree.pointerdown(event, key),
      },
      bind: (el, part) => {
        const cleanups = [
          part.api.roving.bind(el, part),
          part.api.tree.bind(el, part.key),
        ]
        return () => cleanups.forEach((cleanup) => cleanup())
      },
    }),
  }),
})

const OutlineParts = createParts(outline, { root: "div", row: "div" })

function OutlineRows({
  rows,
  flip,
}: {
  rows: TreeReorderRow[]
  flip: RefObject<HTMLDivElement | null>
}) {
  useFlip(flip)
  return rows.map(({ key }, index) => {
    const parent = rows[index + 1] && rows[index + 1].depth > rows[index].depth
    const RowIcon = parent ? FolderSimpleIcon : RectangleIcon
    return (
      <OutlineParts.Row key={key} value={key} className="mo-node-row">
        <DotsSixVerticalIcon
          className="mo-grip"
          size={14}
          weight="bold"
          aria-hidden
        />
        <RowIcon className="mo-node-icon" size={14} weight="fill" aria-hidden />
        <span>{OUTLINE[key]}</span>
      </OutlineParts.Row>
    )
  })
}

function Outline() {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <OutlineParts.Root ref={ref} className="mo-outline">
      {({ rows }) => <OutlineRows rows={rows} flip={ref} />}
    </OutlineParts.Root>
  )
}

/* 5. Drag a copy: tag a task */

const LABELS = ["Urgent", "Research", "Copy"]

function TagDrop() {
  const [tags, setTags] = useState<string[]>(["Web"])
  const card = useRef<HTMLDivElement>(null)
  const stop = useRef<(() => void) | undefined>(undefined)
  useEffect(() => () => stop.current?.(), [])

  const add = (tag: string) =>
    setTags((current) => (current.includes(tag) ? current : [...current, tag]))

  const down = (event: ReactPointerEvent<HTMLButtonElement>, tag: string) => {
    if (event.button !== 0 || tags.includes(tag)) {
      return
    }
    const el = event.currentTarget
    const from = { x: event.clientX, y: event.clientY }
    let preview: DragPreview | undefined
    let over = false
    const clear = () => {
      preview?.remove()
      preview = undefined
      el.removeAttribute("data-dragging")
      card.current?.removeAttribute("data-over")
    }
    stop.current?.()
    const end = bindDrag(el, event.nativeEvent, {
      threshold: dragThreshold,
      cancelOnEscape: true,
      onStart: () => {
        preview = bindDragPreview(el, from)
        el.setAttribute("data-dragging", "")
      },
      onMove: ({ x, y }) => {
        preview?.move(x, y)
        const rect = card.current?.getBoundingClientRect()
        over =
          !!rect &&
          x >= rect.left &&
          x <= rect.right &&
          y >= rect.top &&
          y <= rect.bottom
        card.current?.toggleAttribute("data-over", over)
        preview?.el.toggleAttribute("data-over", over)
      },
      onEnd: (_event, canceled) => {
        if (preview && over && !canceled) {
          add(tag)
        }
        clear()
      },
    })
    stop.current = () => {
      end()
      clear()
    }
  }

  return (
    <div className="mo-tags">
      <div className="mo-chips" role="group" aria-label="Labels">
        {LABELS.map((tag) => (
          <button
            key={tag}
            type="button"
            className="mo-chip"
            data-used={tags.includes(tag) ? "" : undefined}
            aria-disabled={tags.includes(tag) || undefined}
            aria-label={`${tag}, drag onto the task or press Enter`}
            onPointerDown={(event) => down(event, tag)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault()
                add(tag)
              }
            }}
          >
            <DotsSixVerticalIcon size={14} weight="bold" aria-hidden />
            {tag}
          </button>
        ))}
      </div>
      <div ref={card} className="mo-task">
        <p className="mo-task-title">Redesign pricing page</p>
        <ul className="mo-task-tags" aria-label="Task labels">
          {tags.map((tag) => (
            <li key={tag} className="mo-task-tag">
              {tag}
              <button
                type="button"
                aria-label={`Remove ${tag}`}
                onClick={() =>
                  setTags((current) => current.filter((t) => t !== tag))
                }
              >
                <XIcon size={14} weight="bold" />
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}

/* 6. Auto-scroll: drop an event on a date */

const DAYS = Array.from({ length: 14 }, (_, i) => {
  const date = 9 + i
  const weekday = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i % 7]
  return { id: `nov-${date}`, label: `${weekday} ${date}` }
})

function DropOnDate() {
  const [day, setDay] = useState<string | null>(null)
  const list = useRef<HTMLDivElement>(null)
  const stop = useRef<(() => void) | undefined>(undefined)
  useEffect(() => () => stop.current?.(), [])

  const down = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) {
      return
    }
    const el = event.currentTarget
    let preview: DragPreview | undefined
    let target: string | null = null
    let last = { x: event.clientX, y: event.clientY }
    const find = () => {
      const hit = document
        .elementFromPoint(last.x, last.y)
        ?.closest<HTMLElement>("[data-day]")
      const next = hit && list.current?.contains(hit) ? hit.dataset.day! : null
      if (next === target) {
        return
      }
      list.current?.querySelector(`[data-over]`)?.removeAttribute("data-over")
      if (hit && next) {
        hit.setAttribute("data-over", "")
      }
      target = next
    }
    const scroll = createAutoScroll({
      edge: 32,
      speed: 8,
      ignore: () => preview?.el,
      onScroll: find,
    })
    const clear = () => {
      scroll.stop()
      preview?.remove()
      preview = undefined
      el.removeAttribute("data-dragging")
      list.current?.querySelector(`[data-over]`)?.removeAttribute("data-over")
    }
    stop.current?.()
    const end = bindDrag(el, event.nativeEvent, {
      threshold: 4,
      cancelOnEscape: true,
      onStart: () => {
        preview = bindDragPreview(el, last)
        el.setAttribute("data-dragging", "")
      },
      onMove: ({ x, y }) => {
        if (!preview) {
          return
        }
        last = { x, y }
        preview.move(x, y)
        scroll.update(x, y)
        find()
      },
      onEnd: (_event, canceled) => {
        if (preview && target && !canceled) {
          setDay(target)
        }
        clear()
      },
    })
    stop.current = () => {
      end()
      clear()
    }
  }

  return (
    <div className="mo-dates">
      <div className="mo-dates-side">
        <span className="b-label">Unscheduled</span>
        <button
          type="button"
          className="mo-event-chip"
          onPointerDown={down}
          aria-label="Studio walkthrough, drag onto a date"
        >
          <DotsSixVerticalIcon size={14} weight="bold" aria-hidden />
          <span>
            Studio walkthrough
            <small>1 h · Room 2</small>
          </span>
        </button>
      </div>
      <div ref={list} className="mo-days" role="list" aria-label="November">
        {DAYS.map(({ id, label }) => (
          <div key={id} role="listitem" className="mo-day" data-day={id}>
            <button
              type="button"
              className="mo-day-hit"
              onClick={() => setDay(id)}
              aria-label={`Schedule on ${label}`}
            >
              {label}
            </button>
            {day === id && <span className="mo-day-event">Walkthrough</span>}
          </div>
        ))}
      </div>
    </div>
  )
}

/* 7. Move: stickies on a board */

const NOTES: Record<string, string> = {
  kickoff: "Kickoff notes",
  lena: "Ask Lena about fonts",
  friday: "Ship on Friday",
}
const NOTE = { w: 108, h: 62 }

const stickies = definePrimitive({
  name: "mo-stickies",
  state: () => ({
    positions: {
      kickoff: { x: 12, y: 12 },
      lena: { x: 132, y: 40 },
      friday: { x: 36, y: 100 },
    } as Record<string, MovePoint>,
    moving: null as MoveDraft | null,
    size: { w: 260, h: 180 },
  }),
  live: ["moving"],
  api: (context) => {
    const change = createChange(context)
    const move = createMove(context, {
      key: "positions",
      draft: "moving",
      set: (value, details) => change("positions", value, details),
      step: () => 4,
      bounds: () => {
        const { size } = context.store.get()
        return {
          minX: 0,
          minY: 0,
          maxX: Math.max(0, size.w - NOTE.w),
          maxY: Math.max(0, size.h - NOTE.h),
        }
      },
      label: (key) => NOTES[key],
    })
    return { move }
  },
  slots: (slot) => ({
    root: slot({
      attrs: () => ({ role: "group", "aria-label": "Sticky notes" }),
      bind: (el, part) => observeSize(el, (size) => part.store.set({ size })),
    }),
    note: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { key }) => ({
        role: "button",
        tabindex: "0",
        "aria-roledescription": "movable note",
        "aria-keyshortcuts":
          "Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight",
        style: css({
          left: `${state.positions[key].x}px`,
          top: `${state.positions[key].y}px`,
        }),
      }),
      live: live((state, { api, key }) => ({
        "data-dragging": api.move.dragging(state, key) ? "" : undefined,
        style: api.move.style(state, key),
      })),
      on: {
        keydown: (event, { api, key }) => void api.move.keydown(event, key),
        pointerdown: (event, { el, api, key }) =>
          api.move.pointerdown(event, key, el!),
      },
    }),
  }),
})

const StickyParts = createParts(stickies, { root: "div", note: "div" })

function Stickies() {
  return (
    <div className="mo-fill">
      <StickyParts.Root className="mo-board-surface">
        {Object.entries(NOTES).map(([key, text]) => (
          <StickyParts.Note
            key={key}
            value={key}
            className="mo-sticky"
            data-note={key}
          >
            {text}
          </StickyParts.Note>
        ))}
        <p className="mo-hint">Alt + arrows to nudge</p>
      </StickyParts.Root>
    </div>
  )
}

/* 8. Resize: an artboard with handles */

const HANDLES: Record<Exclude<ResizeHandle, "move">, string> = {
  nw: "top left",
  n: "top",
  ne: "top right",
  e: "right",
  se: "bottom right",
  s: "bottom",
  sw: "bottom left",
  w: "left",
}

const artboard = definePrimitive({
  name: "mo-artboard",
  state: () => ({
    boxes: { frame: { x: 40, y: 34, width: 176, height: 108 } } as Record<
      string,
      ResizeBox
    >,
    draft: null as ResizeDraft | null,
    size: { w: 260, h: 180 },
  }),
  api: (context) => {
    const change = createChange(context)
    const resize = createResize(context, {
      boxes: (state) => state.boxes,
      draft: "draft",
      set: (value, details) => change("boxes", value, details),
      min: () => ({ width: 72, height: 48 }),
      bounds: () => {
        const { size } = context.store.get()
        return { left: 0, top: 0, right: size.w, bottom: size.h }
      },
      step: () => 2,
      label: () => "Hero banner",
    })
    return { resize }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => {
        const box = api.resize.box(state, "frame")
        return { width: Math.round(box.width), height: Math.round(box.height) }
      },
      bind: (el, part) => observeSize(el, (size) => part.store.set({ size })),
    }),
    box: slot({
      attrs: (state, { api }) => {
        const box = api.resize.box(state, "frame")
        return {
          "data-dragging": api.resize.dragging(state, "frame") ? "" : undefined,
          style: css({
            left: `${box.x}px`,
            top: `${box.y}px`,
            width: `${box.width}px`,
            height: `${box.height}px`,
          }),
        }
      },
      on: {
        pointerdown: (event, { el, api }) => {
          if ((event.target as Element).closest("[data-resize-handle]")) {
            return
          }
          api.resize.pointerdown(event, "frame", "move", el!)
        },
      },
    }),
    handle: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { api, key }) => ({
        ...api.resize.handleAttrs(key as ResizeHandle),
        type: "button",
        "aria-label": `Resize ${HANDLES[key as keyof typeof HANDLES]}`,
      }),
      on: {
        pointerdown: (event, { el, api, key }) =>
          api.resize.pointerdown(event, "frame", key as ResizeHandle, el!),
        keydown: (event, { api, key }) =>
          void api.resize.keydown(event, "frame", key as ResizeHandle),
      },
    }),
  }),
})

const ArtboardParts = createParts(artboard, {
  root: "div",
  box: "div",
  handle: "button",
})

function Artboard() {
  return (
    <div className="mo-fill">
      <ArtboardParts.Root className="mo-artboard-surface">
        {({ width, height }) => (
          <ArtboardParts.Box className="mo-frame">
            <span className="mo-frame-name">
              <FrameCornersIcon size={14} weight="fill" aria-hidden />
              Hero banner
            </span>
            <span className="mo-frame-size">
              {width} × {height}
            </span>
            {Object.keys(HANDLES).map((handle) => (
              <ArtboardParts.Handle
                key={handle}
                value={handle}
                className="mo-handle"
                data-at={handle}
              />
            ))}
          </ArtboardParts.Box>
        )}
      </ArtboardParts.Root>
    </div>
  )
}

/* 9. Swipe actions: rows that reveal archive and delete */

const INBOX: Record<string, { title: string; meta: string }> = {
  flight: { title: "Flight to Lisbon", meta: "Fri 7:40 · Gate 14" },
  invoice: { title: "Invoice from Atlas Print", meta: "Due Monday" },
  clusters: { title: "Review pin clusters", meta: "Map team · 3 comments" },
}

const swipeList = definePrimitive({
  name: "mo-swipe-list",
  state: () => ({
    rows: Object.keys(INBOX),
    open: null as SwipeOpen | null,
    swiping: null as SwipeDraft | null,
  }),
  live: ["swiping"],
  api: (context) => {
    const change = createChange(context)
    const remove = (key: string) => {
      change(
        "rows",
        context.store.get().rows.filter((row) => row !== key),
        { reason: "item-press" }
      )
      context.store.set({ open: null })
    }
    const swipe = createSwipeActions(context, {
      key: "open",
      draft: "swiping",
      fullSwipe: () => 0.6,
      onFullSwipe: (key) => {
        remove(key)
        return true
      },
    })
    return {
      swipe,
      remove,
      restore: () =>
        change("rows", Object.keys(INBOX), { reason: "item-press" }),
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({ rows: state.rows }),
      attrs: () => ({ role: "list", "aria-label": "Inbox" }),
    }),
    row: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => ({
        role: "listitem",
        tabindex: "0",
        "aria-label": INBOX[key]?.title,
      }),
      on: {
        pointerdown: (event, { el, api, key }) =>
          api.swipe.pointerdown(event, key, el!),
        keydown: (event, { api, key }) => void api.swipe.keydown(event, key),
      },
      bind: (el, part) => part.api.swipe.bindRow(el, part.key),
    }),
    content: slot<{ value: string }>({
      props: { value: "string" },
      attrs: () => ({
        style: css({ transform: "translateX(var(--swipe-x, 0px))" }),
      }),
      live: live((state, { api, key }) => api.swipe.contentAttrs(state, key)),
    }),
    actions: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { api, key }) => api.swipe.actionsAttrs(state, key, "end"),
      bind: (el, part) => part.api.swipe.bindActions(el, part.key, "end"),
    }),
    archive: slot<{ value: string }>({
      props: { value: "string" },
      attrs: () => ({ type: "button" }),
      press: ({ api, key }) => api.remove(key),
    }),
    remove: slot<{ value: string }>({
      props: { value: "string" },
      attrs: () => ({ type: "button" }),
      press: ({ api, key }) => api.remove(key),
    }),
    restore: slot({
      attrs: () => ({ type: "button" }),
      press: ({ api }) => api.restore(),
    }),
  }),
})

const SwipeParts = createParts(swipeList, {
  root: "div",
  row: "div",
  content: "div",
  actions: "div",
  archive: "button",
  remove: "button",
  restore: "button",
})

function SwipeRows() {
  return (
    <SwipeParts.Root className="mo-inbox">
      {({ rows }) =>
        rows.length ? (
          rows.map((key) => (
            <SwipeParts.Row key={key} value={key} className="mo-swipe-row">
              <SwipeParts.Actions value={key} className="mo-swipe-actions">
                <SwipeParts.Archive
                  value={key}
                  className="mo-swipe-btn"
                  data-kind="archive"
                >
                  <ArchiveIcon size={16} weight="fill" aria-hidden />
                  Archive
                </SwipeParts.Archive>
                <SwipeParts.Remove
                  value={key}
                  className="mo-swipe-btn"
                  data-kind="delete"
                >
                  <TrashIcon size={16} weight="fill" aria-hidden />
                  Delete
                </SwipeParts.Remove>
              </SwipeParts.Actions>
              <SwipeParts.Content value={key} className="mo-swipe-content">
                <span className="mo-swipe-text">
                  <span className="mo-swipe-title">{INBOX[key].title}</span>
                  <span className="mo-swipe-meta">{INBOX[key].meta}</span>
                </span>
                <CaretLeftIcon
                  className="mo-swipe-cue"
                  size={16}
                  weight="bold"
                  aria-hidden
                />
              </SwipeParts.Content>
            </SwipeParts.Row>
          ))
        ) : (
          <div className="mo-inbox-empty">
            <span>All caught up</span>
            <SwipeParts.Restore className="b-btn">Restore</SwipeParts.Restore>
          </div>
        )
      }
    </SwipeParts.Root>
  )
}

/* 10. Area: a saturation and brightness pad */

const HUE = 24

function hsvToHex(h: number, s: number, v: number) {
  const f = (n: number) => {
    const k = (n + h / 60) % 6
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1))
  }
  return `#${[f(5), f(3), f(1)]
    .map((c) =>
      Math.round(c * 255)
        .toString(16)
        .padStart(2, "0")
    )
    .join("")}`.toUpperCase()
}

const colorPad = definePrimitive({
  name: "mo-color-pad",
  state: () => ({ color: { s: 72, v: 86 } }),
  api: (context) => {
    const change = createChange(context)
    const area = createArea(context, {
      value: (state) => ({ x: state.color.s, y: state.color.v }),
      set: (value, details) =>
        change("color", { s: value.x, v: value.y }, details),
      x: () => ({ min: 0, max: 100, step: 1, label: "Saturation" }),
      y: () => ({ min: 0, max: 100, step: 1, label: "Brightness" }),
      valueText: ({ x, y }) => `${x}% saturation, ${y}% brightness`,
    })
    return { area }
  },
  slots: (slot) => ({
    root: slot({ state: (state) => ({ color: state.color }) }),
    pad: slot({
      attrs: () => ({ style: css({ "--hue": HUE }) }),
      on: {
        pointerdown: (event, { el, api }) => api.area.pointerdown(event, el!),
      },
    }),
    thumb: slot({
      attrs: (state, { api }) => ({
        ...api.area.thumbAttrs(state),
        "aria-label": "Accent color",
        style: `${api.area.thumbStyle(state)};background:${hsvToHex(HUE, state.color.s / 100, state.color.v / 100)}`,
      }),
      on: { keydown: (event, { api }) => void api.area.keydown(event) },
    }),
  }),
})

const PadParts = createParts(colorPad, {
  root: "div",
  pad: "div",
  thumb: "div",
})

function ColorPad() {
  return (
    <PadParts.Root className="mo-color">
      {({ color }) => {
        const hex = hsvToHex(HUE, color.s / 100, color.v / 100)
        return (
          <>
            <PadParts.Pad className="mo-pad">
              <PadParts.Thumb className="mo-pad-thumb" />
            </PadParts.Pad>
            <div className="mo-color-side">
              <span className="mo-color-swatch" style={{ background: hex }} />
              <span className="b-label">Accent</span>
              <span className="mo-color-hex">{hex}</span>
              <span className="mo-color-sv">
                S {color.s} · B {color.v}
              </span>
            </div>
          </>
        )
      }}
    </PadParts.Root>
  )
}

/* 11. Spans on a timeline, with isSpanFree */

type Booking = Span & { title: string }
const ROOMS: Record<string, string> = {
  a: "Studio A",
  b: "Studio B",
  booth: "Booth",
}
const OPEN = 9 * 60
const CLOSE = 17 * 60
const clock = (minutes: number) =>
  `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")}`

const timeline = definePrimitive({
  name: "mo-timeline",
  state: () => ({
    bookings: [
      {
        id: "review",
        track: "a",
        start: 9 * 60,
        end: 11 * 60,
        title: "Client review",
      },
      {
        id: "shoot",
        track: "b",
        start: 10 * 60,
        end: 13 * 60,
        title: "Photo shoot",
      },
      {
        id: "demo",
        track: "a",
        start: 13 * 60 + 30,
        end: 15 * 60 + 30,
        title: "Sprint demo",
      },
      {
        id: "vendor",
        track: "booth",
        start: 11 * 60,
        end: 12 * 60 + 30,
        title: "Vendor call",
      },
    ] as Booking[],
    draft: null as SpansDraft<Booking> | null,
  }),
  api: (context) => {
    const change = createChange(context)
    let made = 0
    type State = ReturnType<typeof context.store.get>
    const spans = createSpans<State, Booking>(context, {
      key: "bookings",
      draft: "draft",
      set: (value, details) => change("bookings", value, details),
      axis: "x",
      min: () => OPEN,
      max: () => CLOSE,
      step: () => 30,
      minSize: () => 30,
      overlap: false,
      tracks: () => Object.keys(ROOMS),
      create: (span) => ({ ...span, id: `hold-${++made}`, title: "Hold" }),
      label: (b) =>
        `${b.title}, ${ROOMS[b.track]} ${clock(b.start)} to ${clock(b.end)}`,
      createAutoScroll: false,
    })
    const freeSlot = (bookings: Booking[]) => {
      for (let start = OPEN; start + 60 <= CLOSE; start += 30) {
        for (const track of Object.keys(ROOMS)) {
          const probe = { id: "probe", track, start, end: start + 60 }
          if (isSpanFree(bookings, probe)) {
            return probe
          }
        }
      }
      return null
    }
    return {
      spans,
      freeSlot,
      addHold: () => {
        const { bookings } = context.store.get()
        const slot = freeSlot(bookings)
        if (slot) {
          change(
            "bookings",
            [...bookings, { ...slot, id: `hold-${++made}`, title: "Hold" }],
            { reason: "imperative" }
          )
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => {
        const free = api.freeSlot(state.bookings)
        return {
          spans: api.spans.spans(state),
          free: free && `${ROOMS[free.track]} at ${clock(free.start)}`,
        }
      },
    }),
    track: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => ({ "aria-label": ROOMS[key] }),
      on: {
        pointerdown: (event, { el, api, key }) => {
          if (event.target === el) {
            api.spans.trackPointerdown(event, key)
          }
        },
      },
      bind: (el, part) => part.api.spans.bindTrack(el, part.key),
    }),
    booking: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { api, key }) => {
        const booking = api.spans.spans(state).find((b) => b.id === key)
        if (!booking) {
          return {}
        }
        return {
          ...api.spans.spanAttrs(state, key),
          tabindex: "0",
          role: "button",
          "aria-roledescription": "booking",
          "aria-label": `${booking.title}, ${clock(booking.start)} to ${clock(booking.end)}`,
          style: api.spans.spanStyle(booking),
        }
      },
      on: {
        pointerdown: (event, { api, key }) =>
          api.spans.spanPointerdown(event, key),
        keydown: (event, { api, key }) => void api.spans.keydown(event, key),
      },
      bind: (el, part) => part.api.spans.bindSpan(el, part.key),
    }),
    add: slot({
      attrs: (state, { api }) => ({
        type: "button",
        disabled: api.freeSlot(state.bookings) ? undefined : "",
      }),
      press: ({ api }) => api.addHold(),
    }),
  }),
})

const TimelineParts = createParts(timeline, {
  root: "div",
  track: "div",
  booking: "div",
  add: "button",
})

function Timeline() {
  const hours = Array.from({ length: (CLOSE - OPEN) / 60 + 1 }, (_, i) => 9 + i)
  return (
    <TimelineParts.Root className="mo-timeline">
      {({ spans, free }) => (
        <>
          <div className="mo-tl-hours" aria-hidden>
            {hours.map((hour, i) => (
              <span
                key={hour}
                style={{ left: `${(i / (hours.length - 1)) * 100}%` }}
              >
                {hour}
              </span>
            ))}
          </div>
          {Object.entries(ROOMS).map(([track, name]) => (
            <div key={track} className="mo-tl-row">
              <span className="mo-tl-room">{name}</span>
              <TimelineParts.Track value={track} className="mo-tl-track">
                {spans
                  .filter((b) => b.track === track)
                  .map((b) => (
                    <TimelineParts.Booking
                      key={b.id}
                      value={b.id}
                      className="mo-booking"
                      data-hold={b.title === "Hold" ? "" : undefined}
                    >
                      <span data-span-edge="start" className="mo-edge" />
                      <span className="mo-booking-title">{b.title}</span>
                      <span data-span-edge="end" className="mo-edge" />
                    </TimelineParts.Booking>
                  ))}
              </TimelineParts.Track>
            </div>
          ))}
          <div className="mo-tl-foot">
            <span className="mo-tl-free">
              {free ? `Next free hour: ${free}` : "Fully booked"}
            </span>
            <TimelineParts.Add className="b-btn" data-size="sm">
              <PlusIcon size={14} weight="bold" aria-hidden />
              Add hold
            </TimelineParts.Add>
          </div>
        </>
      )}
    </TimelineParts.Root>
  )
}

/* 12. Spans down a day, stacked in lanes */

type Plan = Span & { title: string }
const DAY_START = 8 * 60
const DAY_END = 19 * 60

const dayPlan = definePrimitive({
  name: "mo-day-plan",
  state: () => ({
    plans: [
      {
        id: "standup",
        track: "thu",
        start: 9 * 60,
        end: 9 * 60 + 30,
        title: "Standup",
      },
      {
        id: "kickoff",
        track: "thu",
        start: 10 * 60,
        end: 11 * 60 + 30,
        title: "Atlas kickoff",
      },
      {
        id: "lunch",
        track: "thu",
        start: 12 * 60 + 30,
        end: 13 * 60 + 30,
        title: "Lunch with Priya",
      },
      {
        id: "crit",
        track: "thu",
        start: 14 * 60,
        end: 15 * 60 + 30,
        title: "Design crit",
      },
      {
        id: "pins",
        track: "thu",
        start: 14 * 60 + 45,
        end: 16 * 60,
        title: "Pin review",
      },
    ] as Plan[],
    draft: null as SpansDraft<Plan> | null,
  }),
  api: (context) => {
    const change = createChange(context)
    let made = 0
    type State = ReturnType<typeof context.store.get>
    const spans = createSpans<State, Plan>(context, {
      key: "plans",
      draft: "draft",
      set: (value, details) => change("plans", value, details),
      axis: "y",
      min: () => DAY_START,
      max: () => DAY_END,
      step: () => 15,
      minSize: () => 30,
      size: () => 60,
      overlap: true,
      tracks: () => ["thu"],
      create: (span) => ({ ...span, id: `new-${++made}`, title: "New event" }),
      label: (p) => `${p.title}, ${clock(p.start)} to ${clock(p.end)}`,
      createAutoScroll: false,
    })
    return { spans }
  },
  slots: (slot) => ({
    root: slot({
      state: (state, { api }) => ({ plans: api.spans.spansOn(state, "thu") }),
    }),
    track: slot({
      attrs: () => ({ "aria-label": "Thursday" }),
      on: {
        pointerdown: (event, { el, api }) => {
          if (event.target === el) {
            api.spans.trackPointerdown(event, "thu")
          }
        },
      },
      bind: (el, part) => part.api.spans.bindTrack(el, "thu"),
    }),
    plan: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { api, key }) => {
        const all = api.spans.spansOn(state, "thu")
        const plan = all.find((p) => p.id === key)
        if (!plan) {
          return {}
        }
        const lanes = stackSpans(all)
        return {
          ...api.spans.spanAttrs(state, key),
          tabindex: "0",
          role: "button",
          "aria-roledescription": "event",
          "aria-label": `${plan.title}, ${clock(plan.start)} to ${clock(plan.end)}`,
          "data-short": plan.end - plan.start < 45 ? "" : undefined,
          style: api.spans.spanStyle(plan, lanes.get(plan.id)),
        }
      },
      on: {
        pointerdown: (event, { api, key }) =>
          api.spans.spanPointerdown(event, key),
        keydown: (event, { api, key }) => void api.spans.keydown(event, key),
      },
      bind: (el, part) => part.api.spans.bindSpan(el, part.key),
    }),
  }),
})

const DayParts = createParts(dayPlan, {
  root: "div",
  track: "div",
  plan: "div",
})

function DayCalendar() {
  const hours = Array.from(
    { length: (DAY_END - DAY_START) / 60 },
    (_, i) => 8 + i
  )
  return (
    <DayParts.Root className="mo-day-cal">
      {({ plans }) => (
        <>
          <header className="mo-day-head">
            <CalendarBlankIcon size={16} weight="fill" aria-hidden />
            <span className="mo-day-name">Thursday</span>
            <span className="mo-day-date">Nov 5</span>
          </header>
          <div className="mo-day-body">
            <div className="mo-day-hours" aria-hidden>
              {hours.map((hour) => (
                <span
                  key={hour}
                  style={{
                    top: `${((hour * 60 - DAY_START) / (DAY_END - DAY_START)) * 100}%`,
                  }}
                >
                  {hour}:00
                </span>
              ))}
            </div>
            <DayParts.Track className="mo-day-track">
              {plans.map((plan) => (
                <DayParts.Plan
                  key={plan.id}
                  value={plan.id}
                  className="mo-plan"
                  data-new={plan.title === "New event" ? "" : undefined}
                >
                  <span className="mo-plan-body">
                    <span className="mo-plan-title">{plan.title}</span>
                    <span className="mo-plan-time">
                      {clock(plan.start)}–{clock(plan.end)}
                    </span>
                  </span>
                  <span data-span-edge="end" className="mo-plan-edge" />
                </DayParts.Plan>
              ))}
            </DayParts.Track>
          </div>
        </>
      )}
    </DayParts.Root>
  )
}

/* 13. Preview: hover a theme before choosing it */

const THEMES: Record<string, string> = {
  clay: "oklch(0.6 0.14 42)",
  moss: "oklch(0.55 0.1 135)",
  harbor: "oklch(0.5 0.11 245)",
  saffron: "oklch(0.76 0.15 80)",
  slate: "oklch(0.45 0.02 260)",
}
const themeName = (key: string) => key[0].toUpperCase() + key.slice(1)

const themePicker = definePrimitive({
  name: "mo-theme-picker",
  state: () => ({
    value: "harbor",
    preview: null as string | null,
    focused: null as string | null,
  }),
  api: (context) => {
    const change = createChange(context)
    const preview = createPreview(context, {
      key: "preview",
      slot: "swatch",
      value: () => context.store.get().value,
      choose: (key, details) => change("value", key, details),
      label: themeName,
    })
    const roving = createRovingFocus(context, {
      slot: "swatch",
      key: "focused",
      orientation: () => "horizontal",
      preferred: (item) => item.key === context.store.get().value,
    })
    return { preview, roving }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({
        shown: state.preview ?? state.value,
        previewing: state.preview !== null,
      }),
      on: {
        focusout: (event, { el, api }) => {
          if (
            api.preview.dragging() ||
            el?.contains(event.relatedTarget as Node)
          ) {
            return
          }
          api.preview.clear({ reason: "focus-out", event })
        },
      },
    }),
    group: slot({
      attrs: () => ({ role: "radiogroup", "aria-label": "Event color" }),
    }),
    swatch: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        role: "radio",
        "aria-label": themeName(part.key),
        "aria-checked": String(state.value === part.key),
        tabindex: part.api.roving.tabIndex(state, part),
        "data-previewing": state.preview === part.key ? "" : undefined,
        style: css({ "--swatch": THEMES[part.key] }),
      }),
      on: {
        pointerenter: (event, { api, key }) =>
          void api.preview.show(key, { reason: "item-hover", event }),
        pointerleave: (event, { api }) =>
          void api.preview.clear({ reason: "item-hover", event }),
        click: (event, { api, key }) => void api.preview.choose(key, { event }),
        keydown: (event, { el, api, key }) => {
          if (api.preview.keydown(event, key) || !el) {
            return
          }
          const moved = api.roving.keydown(event, el)
          if (moved) {
            api.preview.show(moved.key, { reason: "list-navigation", event })
          }
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
  }),
})

const ThemeParts = createParts(themePicker, {
  root: "div",
  group: "div",
  swatch: "button",
})

function ThemePreview() {
  return (
    <ThemeParts.Root className="mo-theme">
      {({ shown, previewing }) => (
        <>
          <div
            className="mo-theme-card"
            style={{ "--swatch": THEMES[shown] } as CSSProperties}
          >
            <span className="mo-theme-bar" />
            <span className="mo-theme-text">
              <span className="mo-theme-title">Sunday market run</span>
              <span className="mo-theme-meta">Sat 9:30 · Mercado</span>
            </span>
          </div>
          <ThemeParts.Group className="mo-theme-swatches">
            {Object.keys(THEMES).map((key) => (
              <ThemeParts.Swatch
                key={key}
                value={key}
                className="mo-theme-swatch"
              />
            ))}
          </ThemeParts.Group>
          <p className="mo-theme-status" aria-live="polite">
            {previewing
              ? `Trying ${themeName(shown)}`
              : `${themeName(shown)}, saved`}
          </p>
        </>
      )}
    </ThemeParts.Root>
  )
}

/* 14. Lasso and selection: pins on a map */

const PINS: Record<string, { name: string; x: number; y: number }> = {
  lumen: { name: "Café Lumen", x: 18, y: 28 },
  copper: { name: "Copper Kettle", x: 40, y: 18 },
  dock: { name: "Dockside Roasters", x: 66, y: 34 },
  fig: { name: "Fig & Bean", x: 30, y: 62 },
  north: { name: "North Pour", x: 56, y: 70 },
  owl: { name: "Night Owl Espresso", x: 82, y: 60 },
}

const pinMap = definePrimitive({
  name: "mo-pin-map",
  state: () => ({
    selected: ["dock"] as string[],
    lasso: null as LassoDraft | null,
  }),
  live: ["lasso"],
  api: (context) => {
    const change = createChange(context)
    const lasso = createLasso(context, {
      slot: "pin",
      draft: "lasso",
      selected: () => context.store.get().selected,
      set: (keys, details) => change("selected", keys, details),
      message: (count) => `${count} ${count === 1 ? "pin" : "pins"} selected`,
    })
    const selection = createSelection(context, {
      key: "selected",
      set: (value, details) => change("selected", value, details),
      order: () => Object.keys(PINS),
      multiple: () => true,
    })
    return { lasso, selection }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({ count: state.selected.length }),
      attrs: () => ({ role: "group", "aria-label": "Coffee map" }),
      on: {
        pointerdown: (event, { el, api }) => {
          if (el && event.target === el) {
            api.lasso.pointerdown(event, el)
          }
        },
      },
    }),
    pin: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { key }) => ({
        type: "button",
        "aria-label": PINS[key].name,
        "aria-pressed": String(state.selected.includes(key)),
        "data-selected": state.selected.includes(key) ? "" : undefined,
        style: css({ left: `${PINS[key].x}%`, top: `${PINS[key].y}%` }),
      }),
      live: live((state, { api, key }) => ({
        "data-lassoed": api.lasso.touches(state, key) ? "" : undefined,
      })),
      on: {
        click: (event, { api, key }) => {
          const why = { reason: "item-press", event }
          if (event.shiftKey || event.metaKey || event.ctrlKey) {
            api.selection.toggle(key, why)
          } else {
            api.selection.choose(key, why)
          }
        },
      },
    }),
    rect: slot({
      attrs: () => ({ "aria-hidden": "true" }),
      live: live((state, { api }) => ({ style: api.lasso.style(state) })),
    }),
  }),
})

const PinParts = createParts(pinMap, {
  root: "div",
  pin: "button",
  rect: "div",
})

function PinLasso() {
  return (
    <div className="mo-fill">
      <PinParts.Root className="mo-map">
        {({ count }) => (
          <>
            <span className="mo-map-road" data-road="a" aria-hidden />
            <span className="mo-map-road" data-road="b" aria-hidden />
            <span className="mo-map-river" aria-hidden />
            {Object.keys(PINS).map((key) => (
              <PinParts.Pin key={key} value={key} className="mo-pin">
                <CoffeeIcon size={14} weight="fill" aria-hidden />
              </PinParts.Pin>
            ))}
            <PinParts.Rect className="mo-lasso" />
            <p className="mo-map-status" aria-live="polite">
              {count
                ? `${count} ${count === 1 ? "pin" : "pins"} selected`
                : "Drag across the map"}
            </p>
          </>
        )}
      </PinParts.Root>
    </div>
  )
}

/* 15. Canvas: viewport, movable nodes and connections */

const NODES: Record<
  string,
  { title: string; kind: string; icon: Icon; ports: ("in" | "out")[] }
> = {
  brief: {
    title: "Creative brief",
    kind: "Doc",
    icon: FileTextIcon,
    ports: ["out"],
  },
  mood: {
    title: "Moodboard",
    kind: "Board",
    icon: LightbulbIcon,
    ports: ["in", "out"],
  },
  wire: {
    title: "Wireframes",
    kind: "Frames",
    icon: PenNibIcon,
    ports: ["in", "out"],
  },
  handoff: {
    title: "Hand-off",
    kind: "Package",
    icon: PackageIcon,
    ports: ["in"],
  },
}
const NODE = { w: 156, h: 52 }
type Edge = { from: string; to: string }

const portPoint = (port: string, at: Record<string, MovePoint>) => {
  const [node, side] = port.split(":")
  const p = at[node]
  return { x: p.x + (side === "out" ? NODE.w : 0), y: p.y + NODE.h / 2 }
}
/** Positions with the drag offset applied to the nodes being moved. */
const placed = (
  positions: Record<string, MovePoint>,
  moving: MoveDraft | null
) => {
  const at: Record<string, MovePoint> = {}
  for (const [key, p] of Object.entries(positions)) {
    at[key] = moving?.keys.includes(key)
      ? { x: p.x + moving.dx, y: p.y + moving.dy }
      : p
  }
  return at
}
const curve = (a: MovePoint, b: MovePoint) => {
  const bend = Math.max(40, Math.abs(b.x - a.x) / 2)
  return `M${a.x},${a.y} C${a.x + bend},${a.y} ${b.x - bend},${b.y} ${b.x},${b.y}`
}

const flow = definePrimitive({
  name: "mo-flow",
  state: () => ({
    viewport: { x: 0, y: 0, zoom: 1 },
    positions: {
      brief: { x: 0, y: 136 },
      mood: { x: 216, y: 16 },
      wire: { x: 216, y: 256 },
      handoff: { x: 432, y: 136 },
    } as Record<string, MovePoint>,
    moving: null as MoveDraft | null,
    edges: [
      { from: "brief:out", to: "mood:in" },
      { from: "brief:out", to: "wire:in" },
      { from: "mood:out", to: "handoff:in" },
    ] as Edge[],
    connecting: null as ConnectDraft | null,
  }),
  live: ["viewport", "connecting"],
  api: (context) => {
    const change = createChange(context)
    const viewport = createViewport(context, {
      key: "viewport",
      surface: () => context.element("root") ?? null,
      minZoom: () => 0.4,
      maxZoom: () => 2,
      message: (zoom) => `Canvas at ${Math.round(zoom * 100)} percent`,
    })
    const move = createMove(context, {
      key: "positions",
      draft: "moving",
      set: (value, details) => change("positions", value, details),
      scale: () => context.store.get().viewport.zoom,
      step: () => 8,
      label: (key) => NODES[key]?.title,
    })
    const connect = createConnect(context, {
      slot: "port",
      draft: "connecting",
      canConnect: (from, to) =>
        from.endsWith(":out") &&
        to.endsWith(":in") &&
        from.split(":")[0] !== to.split(":")[0],
      point: (x, y) => viewport.toCanvas(x, y),
      label: (key) =>
        `${NODES[key.split(":")[0]]?.title} ${key.endsWith(":out") ? "output" : "input"}`,
      connect: (from, to, details) => {
        const { edges } = context.store.get()
        const exists = edges.some(
          (edge) => edge.from === from && edge.to === to
        )
        const next = exists
          ? edges.filter((edge) => edge.from !== from || edge.to !== to)
          : [...edges, { from, to }]
        return change("edges", next, details)
      },
    })
    const fit = () => {
      const { positions } = context.store.get()
      const all = Object.values(positions)
      viewport.fit(
        {
          left: Math.min(...all.map((p) => p.x)),
          top: Math.min(...all.map((p) => p.y)),
          right: Math.max(...all.map((p) => p.x)) + NODE.w,
          bottom: Math.max(...all.map((p) => p.y)) + NODE.h,
        },
        { padding: 28, maxZoom: 1 }
      )
      // On a phone, keep nodes readable and let the canvas pan instead.
      if (viewport.get().zoom < 0.7) {
        viewport.zoomTo(0.7)
      }
    }
    return { viewport, move, connect, fit }
  },
  slots: (slot) => ({
    root: slot({
      state: (state) => ({
        positions: state.positions,
        moving: state.moving,
        edges: state.edges,
        connecting: state.connecting,
      }),
      attrs: () => ({
        tabindex: "0",
        role: "application",
        "aria-label": "Design pipeline canvas",
        "aria-roledescription": "canvas",
      }),
      on: {
        wheel: (event, { api }) => api.viewport.wheel(event),
        pointerdown: (event, { el, api }) => {
          if (event.target === el) {
            api.viewport.pointerdown(event)
          }
        },
        keydown: (event, { el, api }) => {
          if (event.target === el) {
            api.viewport.keydown(event)
          }
        },
      },
      bind: (_el, part) => {
        const frame = requestAnimationFrame(() => part.api.fit())
        return () => cancelAnimationFrame(frame)
      },
    }),
    world: slot({
      attrs: () => ({
        style: css({ position: "absolute", left: "0", top: "0" }),
      }),
      live: live((state, { api }) => ({ style: api.viewport.style(state) })),
    }),
    node: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, { api, key }) => {
        const p = state.positions[key]
        const drag = state.moving?.keys.includes(key)
        const x = p.x + (drag ? state.moving!.dx : 0)
        const y = p.y + (drag ? state.moving!.dy : 0)
        const links = state.edges.filter(
          (edge) =>
            edge.from.startsWith(`${key}:`) || edge.to.startsWith(`${key}:`)
        ).length
        return {
          tabindex: "0",
          role: "button",
          "aria-roledescription": "node",
          "aria-label": `${NODES[key].title}, ${links} ${links === 1 ? "connection" : "connections"}`,
          "aria-keyshortcuts": NODES[key].ports.includes("out")
            ? "Alt+ArrowRight C"
            : "Alt+ArrowRight",
          "data-dragging": api.move.dragging(state, key) ? "" : undefined,
          style: css({
            left: `${x}px`,
            top: `${y}px`,
            width: `${NODE.w}px`,
            height: `${NODE.h}px`,
          }),
        }
      },
      on: {
        pointerdown: (event, { el, api, key }) => {
          if ((event.target as Element).closest("[data-port]")) {
            return
          }
          api.move.pointerdown(event, key, el!)
        },
        keydown: (event, { api, key }) => {
          if (api.connect.active()) {
            return void api.connect.keydown(event)
          }
          if (
            event.key.toLowerCase() === "c" &&
            NODES[key].ports.includes("out")
          ) {
            event.preventDefault()
            api.connect.start(`${key}:out`)
            return
          }
          api.move.keydown(event, key)
        },
        focus: (_event, { el, api }) => {
          if (el) {
            api.viewport.reveal(api.viewport.rectOf(el), { padding: 24 })
          }
        },
      },
    }),
    port: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => ({
        "data-port": key.split(":")[1],
        "aria-hidden": "true",
      }),
      live: live((state, { api, key }) => ({
        "data-connect": api.connect.role(state, key),
      })),
      on: {
        pointerdown: (event, { el, api, key }) => {
          if (el) {
            api.connect.pointerdown(event, key, el)
          }
        },
      },
    }),
    zoom: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (_state, { key }) => ({
        type: "button",
        "aria-label":
          key === "in"
            ? "Zoom in"
            : key === "out"
              ? "Zoom out"
              : "Fit to screen",
      }),
      press: ({ api, key }) => {
        if (key === "in") {
          api.viewport.zoomBy(1.25)
        } else if (key === "out") {
          api.viewport.zoomBy(0.8)
        } else {
          api.fit()
        }
      },
    }),
    readout: slot({
      state: (state) => ({ percent: Math.round(state.viewport.zoom * 100) }),
    }),
  }),
})

const FlowParts = createParts(flow, {
  root: "div",
  world: "div",
  node: "div",
  port: "span",
  zoom: "button",
  readout: "span",
})

function FlowCanvas() {
  return (
    <div className="mo-fill">
      <div className="mo-flow-frame">
        <FlowParts.Root className="mo-flow">
          {({ positions, moving, edges, connecting }) => {
            const at = placed(positions, moving)
            return (
              <>
                <FlowParts.World>
                  <svg className="mo-flow-edges" aria-hidden>
                    {edges.map((edge) => (
                      <path
                        key={`${edge.from}-${edge.to}`}
                        d={curve(
                          portPoint(edge.from, at),
                          portPoint(edge.to, at)
                        )}
                      />
                    ))}
                    {connecting && (
                      <path
                        data-draft=""
                        d={curve(
                          portPoint(connecting.from, at),
                          connecting.to
                            ? portPoint(connecting.to, at)
                            : { x: connecting.x, y: connecting.y }
                        )}
                      />
                    )}
                  </svg>
                  {Object.entries(NODES).map(([key, node]) => (
                    <FlowParts.Node
                      key={key}
                      value={key}
                      className="mo-flow-node"
                    >
                      <span className="mo-flow-icon">
                        <node.icon size={16} weight="fill" aria-hidden />
                      </span>
                      <span className="mo-flow-text">
                        <span className="mo-flow-title">{node.title}</span>
                        <span className="mo-flow-kind">{node.kind}</span>
                      </span>
                      {node.ports.map((side) => (
                        <FlowParts.Port
                          key={side}
                          value={`${key}:${side}`}
                          className="mo-port"
                        />
                      ))}
                    </FlowParts.Node>
                  ))}
                </FlowParts.World>
                <p className="mo-flow-hint">
                  Drag from a port to connect · C on a node
                </p>
                <div className="mo-flow-tools">
                  <FlowParts.Zoom value="out" className="mo-tool">
                    <MagnifyingGlassMinusIcon size={16} weight="bold" />
                  </FlowParts.Zoom>
                  <FlowParts.Readout className="mo-zoom-readout">
                    {({ percent }) => `${percent}%`}
                  </FlowParts.Readout>
                  <FlowParts.Zoom value="in" className="mo-tool">
                    <MagnifyingGlassPlusIcon size={16} weight="bold" />
                  </FlowParts.Zoom>
                  <FlowParts.Zoom value="fit" className="mo-tool">
                    <CornersOutIcon size={16} weight="bold" />
                  </FlowParts.Zoom>
                </div>
              </>
            )
          }}
        </FlowParts.Root>
      </div>
    </div>
  )
}

/* 16. Throw: velocity, projection and a spring */

const PLACES: Record<string, { name: string; kind: string }> = {
  graca: { name: "Miradouro da Graça", kind: "Viewpoint · Lisbon" },
  market: { name: "Time Out Market", kind: "Food hall · Lisbon" },
  lello: { name: "Livraria Lello", kind: "Bookshop · Porto" },
}

function ThrowCard() {
  const [deck, setDeck] = useState(Object.keys(PLACES))
  const top = useRef<HTMLDivElement>(null)
  const pos = useRef({ x: 0, y: 0 })
  const cancel = useRef<() => void>(() => {})
  const stop = useRef<(() => void) | undefined>(undefined)
  useEffect(
    () => () => {
      cancel.current()
      stop.current?.()
    },
    []
  )

  const paint = (point: MovePoint) => {
    pos.current = point
    const el = top.current
    if (!el) {
      return
    }
    el.style.translate = `${point.x}px ${point.y}px`
    el.style.rotate = `${point.x / 16}deg`
  }

  const fling = (direction: number, velocity: MovePoint) => {
    const el = top.current
    const width = el?.parentElement?.clientWidth ?? 260
    const stage = el?.closest(".tile-stage")?.clientWidth ?? 0
    const from = Math.abs(pos.current.x)
    const to = Math.max(width * 1.2, stage / 2 + width * 0.7)
    cancel.current()
    cancel.current = runSpring(
      pos.current,
      { x: direction * to, y: pos.current.y + velocity.y * 80 },
      velocity,
      (point) => {
        paint(point)
        if (el) {
          const gone = (Math.abs(point.x) - from) / (to - from)
          el.style.opacity = String(clamp(1 - gone, 0, 1))
        }
      },
      () => {
        if (el) {
          el.style.translate = ""
          el.style.rotate = ""
          requestAnimationFrame(() => {
            el.style.opacity = ""
          })
        }
        pos.current = { x: 0, y: 0 }
        setDeck((current) => [...current.slice(1), current[0]])
      },
      { response: 0.42, damping: 1, tolerance: 12 }
    )
  }

  const down = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) {
      return
    }
    cancel.current()
    const el = event.currentTarget
    const start = { x: event.clientX, y: event.clientY }
    const from = { ...pos.current }
    const tx = createVelocityTracker()
    const ty = createVelocityTracker()
    tx.add(event.timeStamp, from.x)
    ty.add(event.timeStamp, from.y)
    el.setAttribute("data-dragging", "")
    stop.current?.()
    stop.current = bindDrag(el, event.nativeEvent, {
      cancelOnEscape: true,
      onMove: ({ x, y, event: move }) => {
        const next = { x: from.x + x - start.x, y: from.y + y - start.y }
        tx.add(move.timeStamp, next.x)
        ty.add(move.timeStamp, next.y)
        paint(next)
      },
      onEnd: (up, canceled) => {
        el.removeAttribute("data-dragging")
        const velocity = canceled
          ? { x: 0, y: 0 }
          : { x: tx.velocity(up.timeStamp), y: ty.velocity(up.timeStamp) }
        const landing = pos.current.x + projectDistance(velocity.x, 0.994)
        const width = el.parentElement?.clientWidth ?? 260
        if (!canceled && Math.abs(landing) > width * 0.55) {
          fling(Math.sign(landing), velocity)
        } else {
          cancel.current = runSpring(
            pos.current,
            { x: 0, y: 0 },
            velocity,
            paint
          )
        }
      },
    })
  }

  const key = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
      return
    }
    event.preventDefault()
    const direction = event.key === "ArrowLeft" ? -1 : 1
    fling(direction, { x: direction * 2, y: 0 })
  }

  return (
    <div className="mo-deck">
      {Object.keys(PLACES).map((id) => {
        const depth = deck.indexOf(id)
        return (
          <div
            key={id}
            ref={depth === 0 ? top : undefined}
            className="mo-place"
            data-depth={depth}
            style={{ "--depth": depth } as CSSProperties}
            tabIndex={depth === 0 ? 0 : -1}
            role={depth === 0 ? "button" : undefined}
            aria-hidden={depth === 0 ? undefined : true}
            aria-label={
              depth === 0
                ? `${PLACES[id].name}. Throw it, or press Left or Right to skip.`
                : undefined
            }
            onPointerDown={depth === 0 ? down : undefined}
            onKeyDown={depth === 0 ? key : undefined}
          >
            <span className="mo-place-map" aria-hidden>
              <MapPinIcon size={16} weight="fill" />
            </span>
            <span className="mo-place-name">{PLACES[id].name}</span>
            <span className="mo-place-kind">{PLACES[id].kind}</span>
          </div>
        )
      })}
    </div>
  )
}

/* 17. Pull to refresh: rubber banding and a spring */

const NEARBY = [
  { name: "Café Lumen", far: "120 m" },
  { name: "Tram 28 stop", far: "240 m" },
  { name: "Jardim da Estrela", far: "450 m" },
]
const FRESH = [
  { name: "Feira da Ladra", far: "80 m" },
  { name: "LX Factory", far: "300 m" },
  { name: "Mercado de Campo de Ourique", far: "520 m" },
]
const PULL_AT = 44

function PullToRefresh() {
  const [rows, setRows] = useState(NEARBY)
  const [phase, setPhase] = useState<"idle" | "ready" | "refreshing">("idle")
  const [updated, setUpdated] = useState("Updated 4 min ago")
  const sheet = useRef<HTMLDivElement>(null)
  const offset = useRef(0)
  const cancel = useRef<() => void>(() => {})
  const stop = useRef<(() => void) | undefined>(undefined)
  const timer = useRef(0)
  const fresh = useRef(0)
  useEffect(
    () => () => {
      cancel.current()
      stop.current?.()
      clearTimeout(timer.current)
    },
    []
  )

  const paint = (y: number) => {
    offset.current = y
    const el = sheet.current
    if (!el) {
      return
    }
    el.style.setProperty("--pull", `${y}px`)
    el.style.setProperty("--pull-progress", String(Math.min(y / PULL_AT, 1)))
  }
  const springTo = (y: number, done?: () => void) => {
    cancel.current()
    cancel.current = runSpring(
      { x: 0, y: offset.current },
      { x: 0, y },
      { x: 0, y: 0 },
      (p) => paint(p.y),
      done,
      {
        response: 0.35,
        damping: 0.9,
      }
    )
  }
  const refresh = () => {
    setPhase("refreshing")
    springTo(PULL_AT)
    clearTimeout(timer.current)
    timer.current = window.setTimeout(() => {
      const next = FRESH[fresh.current++ % FRESH.length]
      setRows((current) =>
        [next, ...current.filter((row) => row.name !== next.name)].slice(0, 3)
      )
      setUpdated("Updated just now")
      setPhase("idle")
      springTo(0)
    }, 900)
  }

  const down = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || phase === "refreshing") {
      return
    }
    cancel.current()
    const el = event.currentTarget
    const startY = event.clientY
    const height = el.clientHeight
    stop.current?.()
    stop.current = bindDrag(el, event.nativeEvent, {
      threshold: 2,
      cancelOnEscape: true,
      onMove: ({ y }) => {
        const shown = rubberBand(Math.max(0, y - startY), height)
        paint(shown)
        setPhase(shown >= PULL_AT ? "ready" : "idle")
      },
      onEnd: (_event, canceled) => {
        if (!canceled && offset.current >= PULL_AT) {
          refresh()
        } else {
          setPhase("idle")
          springTo(0)
        }
      },
    })
  }

  return (
    <div className="mo-pull">
      <header className="mo-pull-head">
        <span className="mo-pull-title">Nearby</span>
        <button
          type="button"
          className="mo-pull-refresh"
          aria-label="Refresh"
          onClick={refresh}
          disabled={phase === "refreshing"}
        >
          <ArrowClockwiseIcon size={16} weight="bold" />
        </button>
      </header>
      <div className="mo-pull-viewport">
        <div
          className="mo-pull-indicator"
          data-phase={phase}
          aria-live="polite"
        >
          <ArrowClockwiseIcon size={16} weight="bold" aria-hidden />
          <span>
            {phase === "refreshing"
              ? "Refreshing"
              : phase === "ready"
                ? "Release to refresh"
                : updated}
          </span>
        </div>
        <div
          ref={sheet}
          className="mo-pull-sheet"
          onPointerDown={down}
          data-phase={phase}
        >
          <span className="mo-pull-grabber" aria-hidden />
          <ul className="mo-pull-list">
            {rows.map((row) => (
              <li key={row.name}>
                <MapPinIcon size={16} weight="fill" aria-hidden />
                <span className="mo-pull-name">{row.name}</span>
                <span className="mo-pull-far">{row.far}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </div>
  )
}

/* 18. Radial menu: sectorOf */

const ACTIONS = [
  { label: "Save", done: "Saved to Weekend list", icon: BookmarkSimpleIcon },
  { label: "Share", done: "Link copied", icon: ShareNetworkIcon },
  {
    label: "Street view",
    done: "Opening street view",
    icon: PersonSimpleWalkIcon,
  },
  {
    label: "Directions",
    done: "Route from your location",
    icon: NavigationArrowIcon,
  },
]

function RadialMenu() {
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(-1)
  const [result, setResult] = useState<string | null>(null)
  const pin = useRef<HTMLButtonElement>(null)
  const root = useRef<HTMLDivElement>(null)
  const stop = useRef<(() => void) | undefined>(undefined)
  useEffect(() => () => stop.current?.(), [])

  useEffect(() => {
    if (!open) {
      return
    }
    const away = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener("pointerdown", away)
    return () => document.removeEventListener("pointerdown", away)
  }, [open])

  const choose = (index: number) => {
    setResult(ACTIONS[index].done)
    setOpen(false)
    setActive(-1)
  }

  const down = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) {
      return
    }
    const el = event.currentTarget
    const rect = el.getBoundingClientRect()
    const center = {
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2,
    }
    let current = -1
    let strayed = false
    setOpen(true)
    setActive(-1)
    stop.current?.()
    stop.current = bindDrag(el, event.nativeEvent, {
      cancelOnEscape: true,
      onMove: ({ x, y }) => {
        current = sectorOf(x - center.x, y - center.y, ACTIONS.length, {
          deadZone: 24,
        })
        if (current >= 0) {
          strayed = true
        }
        setActive(current)
      },
      onEnd: (_event, canceled) => {
        if (canceled) {
          setOpen(false)
        } else if (current >= 0) {
          choose(current)
        } else if (strayed) {
          setOpen(false)
        }
        // A press without a stroke leaves the menu open to click.
      },
    })
  }

  const key = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    const arrows: Record<string, number> = {
      ArrowUp: 0,
      ArrowRight: 1,
      ArrowDown: 2,
      ArrowLeft: 3,
    }
    if (event.key in arrows) {
      event.preventDefault()
      setOpen(true)
      setActive(arrows[event.key])
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault()
      if (open && active >= 0) {
        choose(active)
      } else {
        setOpen((value) => !value)
      }
    } else if (event.key === "Escape") {
      setOpen(false)
      setActive(-1)
    }
  }

  return (
    <div ref={root} className="mo-radial" data-open={open ? "" : undefined}>
      <div className="mo-radial-ring">
        {ACTIONS.map(({ label, icon: ActionIcon }, index) => (
          <button
            key={label}
            type="button"
            className="mo-radial-item"
            data-index={index}
            data-active={active === index ? "" : undefined}
            aria-label={label}
            tabIndex={open ? 0 : -1}
            aria-hidden={!open || undefined}
            onClick={() => choose(index)}
          >
            <ActionIcon size={16} weight="fill" />
          </button>
        ))}
        <button
          ref={pin}
          type="button"
          className="mo-radial-pin"
          aria-label="Pastéis de Belém, actions"
          aria-expanded={open}
          onPointerDown={down}
          onKeyDown={key}
        >
          <MapPinIcon size={16} weight="fill" />
        </button>
      </div>
      <p className="mo-radial-caption" aria-live="polite">
        {open
          ? active >= 0
            ? ACTIONS[active].label
            : "Swipe toward an action"
          : (result ?? "Pastéis de Belém")}
      </p>
    </div>
  )
}

/* 19. Resizable: split panes in a design tool */

function SplitPanes() {
  return (
    <Resizable className="mo-split">
      <ResizablePanel defaultSize={34} minSize={24} className="mo-split-panel">
        <span className="mo-split-label">Layers</span>
        <ul className="mo-split-layers">
          <li>
            <FrameCornersIcon size={14} weight="fill" aria-hidden />
            <span>Page</span>
          </li>
          <li>
            <TextTIcon size={14} weight="fill" aria-hidden />
            <span>Title</span>
          </li>
          <li>
            <ImageIcon size={14} weight="fill" aria-hidden />
            <span>Cover</span>
          </li>
        </ul>
      </ResizablePanel>
      <ResizableHandle aria-label="Resize layers" className="mo-split-handle">
        <span className="mo-split-grip" />
      </ResizableHandle>
      <ResizablePanel
        defaultSize={30}
        minSize={20}
        className="mo-split-panel mo-split-canvas"
      >
        <span className="mo-split-board" />
      </ResizablePanel>
      <ResizableHandle
        aria-label="Resize inspector"
        className="mo-split-handle"
      >
        <span className="mo-split-grip" />
      </ResizableHandle>
      <ResizablePanel defaultSize={36} minSize={26} className="mo-split-panel">
        <span className="mo-split-label">Inspector</span>
        <dl className="mo-split-props">
          <div>
            <dt>W</dt>
            <dd>1440</dd>
          </div>
          <div>
            <dt>H</dt>
            <dd>900</dd>
          </div>
        </dl>
      </ResizablePanel>
    </Resizable>
  )
}

export const tiles: Tile[] = [
  {
    id: "motion-canvas",
    title: "Wire the flow.",
    blurb: "Pan, zoom, move nodes, connect ports.",
    size: "large",
    tone: "blueprint",
    Demo: FlowCanvas,
  },
  {
    id: "motion-layers",
    title: "Rank the layers.",
    blurb: "Drag a row, or pick it up with Space.",
    Demo: SortableLayers,
  },
  {
    id: "motion-throw",
    title: "Throw it.",
    blurb: "Flick to skip, or let it spring back.",
    tone: "signal",
    Demo: ThrowCard,
  },
  {
    id: "motion-board",
    title: "Plan the week.",
    blurb: "Drag cards across columns, or use the arrows.",
    size: "wide",
    tone: "paper",
    Demo: Board,
  },
  {
    id: "motion-stickies",
    title: "Stick it anywhere.",
    blurb: "Drag notes, or nudge with Alt and arrows.",
    tone: "butter",
    Demo: Stickies,
  },
  {
    id: "motion-day",
    title: "Stack the day.",
    blurb: "Overlapping events share the width.",
    size: "tall",
    tone: "sky",
    Demo: DayCalendar,
  },
  {
    id: "motion-palette",
    title: "Move a handful.",
    blurb: "Select a few swatches, then drag one.",
    tone: "paper",
    Demo: SwatchGrid,
  },
  {
    id: "motion-frame",
    title: "Size the frame.",
    blurb: "Eight handles, plus the arrow keys.",
    tone: "blueprint",
    Demo: Artboard,
  },
  {
    id: "motion-swipe",
    title: "Swipe it away.",
    blurb: "Archive and delete wait behind each row.",
    Demo: SwipeRows,
  },
  {
    id: "motion-color",
    title: "Pick a shade.",
    blurb: "Saturation across, brightness up.",
    tone: "ink",
    Demo: ColorPad,
  },
  {
    id: "motion-timeline",
    title: "Book the studio.",
    blurb: "Draw, move and stretch bookings.",
    size: "wide",
    Demo: Timeline,
  },
  {
    id: "motion-outline",
    title: "Nest a layer.",
    blurb: "Drag sideways to change its depth.",
    Demo: Outline,
  },
  {
    id: "motion-tags",
    title: "Tag the card.",
    blurb: "Drop a label onto the task.",
    Demo: TagDrop,
  },
  {
    id: "motion-theme",
    title: "Try before choosing.",
    blurb: "Hover a color to preview it.",
    tone: "paper",
    Demo: ThemePreview,
  },
  {
    id: "motion-lasso",
    title: "Lasso the pins.",
    blurb: "Drag a box, or Shift-click to add.",
    Demo: PinLasso,
  },
  {
    id: "motion-dates",
    title: "Drop on a date.",
    blurb: "Hold near the edge to scroll.",
    tone: "paper",
    Demo: DropOnDate,
  },
  {
    id: "motion-pull",
    title: "Pull to refresh.",
    blurb: "The list resists the farther you pull.",
    Demo: PullToRefresh,
  },
  {
    id: "motion-radial",
    title: "Pick by direction.",
    blurb: "Press the pin, swipe toward an action.",
    tone: "sage",
    Demo: RadialMenu,
  },
  {
    id: "motion-split",
    title: "Split the workspace.",
    blurb: "Drag the dividers between panels.",
    tone: "paper",
    Demo: SplitPanes,
  },
]
