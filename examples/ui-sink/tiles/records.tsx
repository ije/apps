import {
  ArrowClockwiseIcon,
  ArrowCounterClockwiseIcon,
  CaretLeftIcon,
  CaretRightIcon,
  SparkleIcon,
} from "@phosphor-icons/react"
import { createInputHistory, isComposing } from "@ui-sdk/core"
import { createAgent } from "@ui-sdk/react/agent"
import {
  createHistory,
  describeEntry,
  stageChanges,
  type HistoryEntry,
} from "@ui-sdk/react/history"
import { elementOf, observe, type ObservedChange } from "@ui-sdk/react/observe"
import {
  Pagination,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationList,
  PaginationNext,
  PaginationPrevious,
  paginationRange,
} from "@ui-sdk/react/pagination"
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from "@ui-sdk/react/segmented-control"
import {
  Slider,
  SliderControl,
  SliderIndicator,
  SliderThumb,
  SliderTrack,
  SliderValue,
} from "@ui-sdk/react/slider"
import { Switch, SwitchThumb } from "@ui-sdk/react/switch"
import {
  createTimelineController,
  Timeline,
  TimelineConnector,
  TimelineItem,
  TimelineMarker,
  TimelineTime,
  TimelineTitle,
  type TimelineStatus,
} from "@ui-sdk/react/timeline"
import {
  visibleWaterfallRows,
  Waterfall,
  WaterfallAxis,
  WaterfallBar,
  WaterfallCell,
  WaterfallColumnHeader,
  WaterfallHeaderRow,
  WaterfallIndicator,
  WaterfallMark,
  WaterfallRow,
  WaterfallRowHeader,
  type WaterfallData,
} from "@ui-sdk/react/waterfall"
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
  type PointerEvent,
} from "react"
import type { Tile } from "../bento/types"
import "./records.css"

/* Release history */

const releases: {
  version: string
  dateTime: string
  date: string
  name: string
  summary: string
  commits: number
  status: TimelineStatus
}[] = [
  {
    version: "3.0",
    dateTime: "2026-03-04",
    date: "Mar 4",
    name: "The rewrite",
    summary: "A new sync engine and a sidebar you can collapse.",
    commits: 1204,
    status: "complete",
  },
  {
    version: "3.1",
    dateTime: "2026-04-15",
    date: "Apr 15",
    name: "Shared links",
    summary: "Links remember the folder they were opened from.",
    commits: 318,
    status: "complete",
  },
  {
    version: "3.2",
    dateTime: "2026-06-02",
    date: "Jun 2",
    name: "Search in files",
    summary: "Search matches what's inside a file, not only its name.",
    commits: 402,
    status: "complete",
  },
  {
    version: "3.3",
    dateTime: "2026-07-21",
    date: "Jul 21",
    name: "Resumable uploads",
    summary: "Uploads pick up where they stopped after the network drops.",
    commits: 276,
    status: "complete",
  },
  {
    version: "3.4",
    dateTime: "2026-09-08",
    date: "Sep 8",
    name: "Dark mode",
    summary: "Follows your system, with sharper avatars and icons.",
    commits: 189,
    status: "complete",
  },
  {
    version: "3.5",
    dateTime: "2026-11-12",
    date: "Nov 12",
    name: "Offline drafts",
    summary: "Write without a connection. Drafts sync when you're back.",
    commits: 441,
    status: "current",
  },
  {
    version: "3.6",
    dateTime: "2027-01-20",
    date: "Jan 20",
    name: "Comments",
    summary: "Threads on any line, with mentions and email replies.",
    commits: 97,
    status: "upcoming",
  },
]

const latestRelease = "3.5"

function ReleaseHistory() {
  const [controller] = useState(() => createTimelineController())
  const [selected, setSelected] = useState(latestRelease)
  const release =
    releases.find((candidate) => candidate.version === selected) ?? releases[0]

  const scrub = (event: PointerEvent<HTMLDivElement>) => {
    if (!event.buttons) {
      return
    }
    const item = document
      .elementFromPoint(event.clientX, event.clientY)
      ?.closest<HTMLElement>('[data-slot="timeline-item"]')
    if (item && item !== document.activeElement) {
      item.focus({ preventScroll: true })
    }
  }

  return (
    <div className="rc-releases">
      <div className="rc-release" data-status={release.status}>
        <div className="rc-release-head">
          <span className="rc-release-version">v{release.version}</span>
          <span className="rc-release-status">
            {release.status === "current"
              ? "Latest"
              : release.status === "upcoming"
                ? "Planned"
                : "Shipped"}
          </span>
          <button
            type="button"
            className="b-btn rc-release-latest"
            data-variant="ghost"
            hidden={selected === latestRelease}
            onClick={() => controller.focus(latestRelease)}
          >
            Jump to latest
          </button>
        </div>
        <p className="rc-release-name" key={release.version}>
          {release.name}
        </p>
        <p className="rc-release-summary">{release.summary}</p>
        <p className="rc-release-meta">
          {release.date} · {release.commits.toLocaleString("en-US")} commits
        </p>
      </div>
      <Timeline
        orientation="horizontal"
        loop={false}
        controller={controller}
        aria-label="Releases"
        className="rc-timeline"
        onPointerDown={scrub}
        onPointerMove={scrub}
      >
        {releases.map((item) => (
          <TimelineItem
            key={item.version}
            value={item.version}
            status={item.status}
            className="rc-tl-item"
            data-selected={item.version === selected || undefined}
            onFocus={() => setSelected(item.version)}
          >
            <TimelineMarker className="rc-tl-marker" />
            <TimelineConnector className="rc-tl-connector" />
            <TimelineTitle className="rc-tl-title">
              {item.version}
            </TimelineTitle>
            <TimelineTime dateTime={item.dateTime} className="rc-tl-time">
              {item.date}
            </TimelineTime>
          </TimelineItem>
        ))}
      </Timeline>
    </div>
  )
}

/* Request waterfall */

const requests: WaterfallData = [
  { value: "document", label: "dashboard", start: 0, duration: 168 },
  { value: "styles", label: "app.css", start: 176, duration: 92 },
  {
    value: "script",
    label: "app.js",
    start: 180,
    duration: 386,
    children: [
      { value: "charts", label: "charts.js", start: 590, duration: 174 },
      { value: "table", label: "table.js", start: 598, duration: 122 },
    ],
  },
  { value: "font", label: "geist.woff2", start: 284, duration: 196 },
  {
    value: "projects",
    label: "api/projects",
    start: 612,
    duration: 248,
    children: [
      { value: "avatar-1", label: "ada.webp", start: 872, duration: 96 },
      { value: "avatar-2", label: "linus.webp", start: 878, duration: 132 },
    ],
  },
  { value: "usage", label: "api/usage", start: 620, duration: 412 },
  { value: "analytics", label: "insights.js", start: 1046, duration: 150 },
]

const requestInfo: Record<
  string,
  { type: string; size: string; status: number; initiator: string }
> = {
  document: { type: "doc", size: "14 kB", status: 200, initiator: "Other" },
  styles: { type: "css", size: "9 kB", status: 200, initiator: "dashboard" },
  script: { type: "js", size: "212 kB", status: 200, initiator: "dashboard" },
  charts: { type: "js", size: "88 kB", status: 200, initiator: "app.js" },
  table: { type: "js", size: "41 kB", status: 200, initiator: "app.js" },
  font: { type: "font", size: "48 kB", status: 200, initiator: "app.css" },
  projects: { type: "api", size: "3 kB", status: 200, initiator: "app.js" },
  "avatar-1": { type: "img", size: "6 kB", status: 200, initiator: "app.js" },
  "avatar-2": { type: "img", size: "7 kB", status: 304, initiator: "app.js" },
  usage: { type: "api", size: "1 kB", status: 200, initiator: "app.js" },
  analytics: { type: "js", size: "22 kB", status: 200, initiator: "app.js" },
}

const milliseconds = {
  style: "unit",
  unit: "millisecond",
  unitDisplay: "narrow",
} as const

const requestFilters = [
  { value: "all", label: "All" },
  { value: "js", label: "JS" },
  { value: "api", label: "Fetch" },
  { value: "img", label: "Img" },
]

const requestsOfType = (type: string) =>
  type === "all"
    ? requests
    : requests.flatMap((item) => {
        const children = item.children?.filter(
          (child) => requestInfo[child.value].type === type
        )
        const kept = requestInfo[item.value].type === type || children?.length
        return kept
          ? [{ ...item, children: children?.length ? children : undefined }]
          : []
      })

function RequestWaterfall() {
  const [filter, setFilter] = useState("all")
  const [expanded, setExpanded] = useState(["script", "projects"])
  const [selected, setSelected] = useState<string | null>("usage")
  const shown = useMemo(() => requestsOfType(filter), [filter])
  const rows = visibleWaterfallRows(shown, expanded)
  const chosen = rows.find((row) => row.item.value === selected)?.item
  const info = chosen && requestInfo[chosen.value]

  return (
    <div className="rc-network">
      <div className="rc-wf-toolbar">
        <span className="rc-wf-title">Network</span>
        <SegmentedControl
          aria-label="Request type"
          value={filter}
          onValueChange={(value) => value && setFilter(value)}
          className="rc-seg"
        >
          <SegmentedControlIndicator className="rc-seg-indicator" />
          {requestFilters.map((option) => (
            <SegmentedControlItem
              key={option.value}
              value={option.value}
              className="rc-seg-item"
            >
              {option.label}
            </SegmentedControlItem>
          ))}
        </SegmentedControl>
      </div>
      <Waterfall
        aria-label="Network requests"
        items={shown}
        range={[0, 1200]}
        expanded={expanded}
        onExpandedChange={setExpanded}
        value={selected}
        onValueChange={setSelected}
        format={milliseconds}
        locale="en-US"
        getBarLabel={(request, { start, duration }) =>
          `${requestInfo[request.value].size}, starts at ${start}, takes ${duration}`
        }
        className="rc-wf"
      >
        <WaterfallHeaderRow className="rc-wf-head">
          <WaterfallColumnHeader className="rc-wf-name">
            Name
          </WaterfallColumnHeader>
          <WaterfallColumnHeader className="rc-wf-size">
            Size
          </WaterfallColumnHeader>
          <WaterfallColumnHeader aria-label="Timeline" className="rc-wf-axis">
            <WaterfallAxis maxTicks={5} className="rc-wf-ticks" />
            <WaterfallMark time={572} className="rc-wf-mark" data-kind="dcl">
              DCL
            </WaterfallMark>
            <WaterfallMark time={1032} className="rc-wf-mark" data-kind="load">
              Load
            </WaterfallMark>
          </WaterfallColumnHeader>
        </WaterfallHeaderRow>
        {rows.map(({ item }) => (
          <WaterfallRow
            key={item.value}
            value={item.value}
            className="rc-wf-row"
            data-type={requestInfo[item.value].type}
          >
            <WaterfallRowHeader className="rc-wf-name">
              {item.children && (
                <WaterfallIndicator className="rc-wf-caret">
                  <CaretRightIcon size={11} weight="bold" aria-hidden />
                </WaterfallIndicator>
              )}
              <span className="rc-wf-type" aria-hidden />
              <span className="rc-wf-label">{item.label}</span>
            </WaterfallRowHeader>
            <WaterfallCell className="rc-wf-size">
              {requestInfo[item.value].size}
            </WaterfallCell>
            <WaterfallBar className="rc-wf-bar" />
          </WaterfallRow>
        ))}
      </Waterfall>
      <p className="rc-wf-detail" aria-live="polite">
        {chosen && info ? (
          <>
            <span className="rc-wf-detail-name">{chosen.label}</span>
            <span data-status={info.status}>{info.status}</span>
            <span>{info.type}</span>
            <span>{chosen.duration} ms</span>
            <span className="rc-wf-detail-from">from {info.initiator}</span>
          </>
        ) : (
          <span>Select a request.</span>
        )}
      </p>
      <p className="rc-wf-summary">
        <span>
          {shown.flatMap((item) => [item, ...(item.children ?? [])]).length}{" "}
          requests
        </span>
        <span data-kind="dcl">DOMContentLoaded 572 ms</span>
        <span data-kind="load">Load 1.03 s</span>
      </p>
    </div>
  )
}

/* Invoices */

const INVOICES = 1269
const PER_PAGE = 6
const PAGE_COUNT = Math.ceil(INVOICES / PER_PAGE)

const customers = [
  "Northwind",
  "Acme Corp",
  "Globex",
  "Initech",
  "Umbrella",
  "Hooli",
  "Stark & Co",
  "Wayne Ent.",
  "Soylent",
  "Tyrell",
  "Cyberdyne",
]

const invoiceStatuses = ["Paid", "Paid", "Paid", "Due", "Overdue"] as const

function invoicesOn(page: number) {
  const first = (page - 1) * PER_PAGE
  return Array.from(
    { length: Math.min(PER_PAGE, INVOICES - first) },
    (_, offset) => {
      const index = first + offset
      return {
        number: `INV-${String(24180 - index).padStart(5, "0")}`,
        customer: customers[(index * 7) % customers.length],
        amount: 120 + ((index * 937) % 4800),
        status: invoiceStatuses[(index * 3 + page) % invoiceStatuses.length],
      }
    }
  )
}

const currency = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 0,
})

function Invoices() {
  const [page, setPage] = useState(4)
  const range = paginationRange({
    page,
    pageCount: PAGE_COUNT,
    siblings: 0,
  })
  const first = (page - 1) * PER_PAGE + 1

  return (
    <div className="rc-invoices">
      <div className="rc-inv-head">
        <span className="rc-inv-title">Invoices</span>
        <span className="rc-inv-count">
          {first}–{Math.min(first + PER_PAGE - 1, INVOICES)} of{" "}
          {INVOICES.toLocaleString("en-US")}
        </span>
      </div>
      <ul
        className="rc-inv-list"
        key={page}
        aria-label={`Invoices, page ${page}`}
      >
        {invoicesOn(page).map((invoice) => (
          <li key={invoice.number} className="rc-inv-row">
            <span className="rc-inv-who">
              <span className="rc-inv-customer">{invoice.customer}</span>
              <span className="rc-inv-number">{invoice.number}</span>
            </span>
            <span className="rc-inv-right">
              <span className="rc-inv-amount">
                {currency.format(invoice.amount)}
              </span>
              <span className="rc-inv-status" data-status={invoice.status}>
                {invoice.status}
              </span>
            </span>
          </li>
        ))}
      </ul>
      <p className="rc-inv-page" aria-hidden>
        Page {page} of {PAGE_COUNT}
      </p>
      <Pagination
        aria-label="Invoice pages"
        pageCount={PAGE_COUNT}
        page={page}
        onPageChange={setPage}
        siblings={0}
        className="rc-pages"
      >
        <PaginationList className="rc-pages-list">
          <PaginationItem>
            <PaginationPrevious aria-label="Previous page" className="rc-page">
              <CaretLeftIcon size={13} weight="bold" aria-hidden />
            </PaginationPrevious>
          </PaginationItem>
          {range.map((item) => (
            <PaginationItem key={item}>
              {typeof item === "number" ? (
                <PaginationLink page={item} className="rc-page" />
              ) : (
                <PaginationEllipsis className="rc-page-gap">
                  …
                </PaginationEllipsis>
              )}
            </PaginationItem>
          ))}
          <PaginationItem>
            <PaginationNext aria-label="Next page" className="rc-page">
              <CaretRightIcon size={13} weight="bold" aria-hidden />
            </PaginationNext>
          </PaginationItem>
        </PaginationList>
      </Pagination>
    </div>
  )
}

/* Board history */

let sharedAgent: ReturnType<typeof createAgent> | undefined
const agentOf = () => (sharedAgent ??= createAgent())

const columns = ["To do", "Doing", "Done"] as const
const priorities = ["Low", "Medium", "High"] as const

const otherCards: Record<(typeof columns)[number], string> = {
  "To do": "Empty states",
  Doing: "Keyboard map",
  Done: "Dark mode",
}

const subscribeNever = () => () => {}

function BoardHistory() {
  const rootRef = useRef<HTMLDivElement>(null)
  const historyRef = useRef<ReturnType<typeof createHistory>>(null)
  const [status, setStatus] = useState<string>("Doing")
  const [priority, setPriority] = useState<string>("Medium")
  const [blocked, setBlocked] = useState(false)
  const [log, setLog] = useState<{
    entries: readonly HistoryEntry[]
    position: number
  }>({ entries: [], position: 0 })
  const undoKeys = useSyncExternalStore(
    subscribeNever,
    () => (/Mac|iPhone|iPad/.test(navigator.platform) ? "⌘Z" : "Ctrl+Z"),
    () => "⌘Z"
  )
  const [stagedAt, setStagedAt] = useState<number>()

  useEffect(() => {
    const history = createHistory(agentOf(), {
      within: () => rootRef.current,
    })
    historyRef.current = history
    const stop = history.subscribe(() =>
      setLog({ entries: history.entries(), position: history.position() })
    )
    return () => {
      stop()
      history.destroy()
    }
  }, [])

  const { entries, position } = log
  const seek = (to: number) => historyRef.current?.seek(to)
  const isStaged = stagedAt !== undefined && position > stagedAt

  const suggest = async () => {
    const before = position
    const { applied } = await stageChanges(agentOf(), [
      { ref: "Card status", value: "Done" },
      { ref: "Card priority", value: "High" },
    ])
    if (applied.length) {
      setStagedAt(before)
    }
  }

  const discard = () => {
    if (stagedAt !== undefined) {
      seek(stagedAt)
    }
    setStagedAt(undefined)
  }

  const onKeyDown = (event: KeyboardEvent) => {
    if (
      event.key.toLowerCase() !== "z" ||
      !(event.metaKey || event.ctrlKey) ||
      event.altKey
    ) {
      return
    }
    event.preventDefault()
    seek(event.shiftKey ? position + 1 : position - 1)
  }

  return (
    <div className="rc-board" ref={rootRef} onKeyDown={onKeyDown}>
      <div className="rc-columns" aria-hidden>
        {columns.map((column) => (
          <div
            key={column}
            className="rc-column"
            data-current={column === status || undefined}
          >
            <span className="rc-column-name">{column}</span>
            <span className="rc-card rc-card-quiet">{otherCards[column]}</span>
            {column === status && (
              <span
                className="rc-card"
                data-priority={priority}
                data-blocked={blocked || undefined}
              >
                <span className="rc-card-title">Offline drafts</span>
                <span className="rc-card-meta">
                  <span className="rc-card-priority">{priority}</span>
                  {blocked && <span className="rc-card-blocked">Blocked</span>}
                </span>
              </span>
            )}
          </div>
        ))}
      </div>

      <div className="rc-controls">
        <SegmentedControl
          aria-label="Card status"
          defaultValue="Doing"
          onValueChange={(value) => value && setStatus(value)}
          className="rc-seg"
        >
          <SegmentedControlIndicator className="rc-seg-indicator" />
          {columns.map((column) => (
            <SegmentedControlItem
              key={column}
              value={column}
              className="rc-seg-item"
            >
              {column}
            </SegmentedControlItem>
          ))}
        </SegmentedControl>
        <SegmentedControl
          aria-label="Card priority"
          defaultValue="Medium"
          onValueChange={(value) => value && setPriority(value)}
          className="rc-seg"
        >
          <SegmentedControlIndicator className="rc-seg-indicator" />
          {priorities.map((level) => (
            <SegmentedControlItem
              key={level}
              value={level}
              className="rc-seg-item"
            >
              {level}
            </SegmentedControlItem>
          ))}
        </SegmentedControl>
        <label className="rc-switch-row">
          <Switch
            aria-label="Blocked"
            onCheckedChange={setBlocked}
            className="rc-switch"
          >
            <SwitchThumb className="rc-switch-thumb" />
          </Switch>
          <span aria-hidden>Blocked</span>
        </label>
      </div>

      <div className="rc-history">
        <div className="rc-history-bar">
          <span className="rc-history-title">History</span>
          <kbd className="rc-kbd">{undoKeys}</kbd>
          <button
            type="button"
            className="b-btn rc-icon-btn"
            aria-label="Undo"
            disabled={position === 0}
            onClick={() => seek(position - 1)}
          >
            <ArrowCounterClockwiseIcon size={15} weight="bold" aria-hidden />
          </button>
          <button
            type="button"
            className="b-btn rc-icon-btn"
            aria-label="Redo"
            disabled={position === entries.length}
            onClick={() => seek(position + 1)}
          >
            <ArrowClockwiseIcon size={15} weight="bold" aria-hidden />
          </button>
        </div>
        <ol className="rc-entries" aria-label="Changes">
          {!entries.length && (
            <li className="rc-entries-hint">
              Change the card, then press {undoKeys}.
            </li>
          )}
          <li>
            <button
              type="button"
              className="rc-entry"
              data-applied=""
              aria-current={position === 0 || undefined}
              onClick={() => seek(0)}
            >
              Opened the card
            </button>
          </li>
          {entries.map((entry, index) => (
            <li key={entry.id}>
              <button
                type="button"
                className="rc-entry"
                data-applied={index < position || undefined}
                data-agent={entry.source === "agent" || undefined}
                aria-current={index + 1 === position || undefined}
                onClick={() => seek(index + 1)}
              >
                {describeEntry(entry)}
              </button>
            </li>
          ))}
        </ol>
        <div className="rc-suggest" data-staged={isStaged || undefined}>
          <SparkleIcon
            size={14}
            weight="fill"
            aria-hidden
            className="rc-suggest-icon"
          />
          {isStaged ? (
            <>
              <span className="rc-suggest-text">Agent moved it to Done.</span>
              <button
                type="button"
                className="b-btn"
                data-variant="ghost"
                onClick={discard}
              >
                Discard
              </button>
              <button
                type="button"
                className="b-btn"
                data-variant="primary"
                onClick={() => setStagedAt(undefined)}
              >
                Keep
              </button>
            </>
          ) : (
            <>
              <span className="rc-suggest-text">
                Looks shipped. Close it out?
              </span>
              <button
                type="button"
                className="b-btn"
                disabled={status === "Done" && priority === "High"}
                onClick={suggest}
              >
                Preview
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

/* Command line */

const seededCommands = [
  "git status",
  "pnpm test --filter core",
  "deploy --env preview",
]

function respond(command: string) {
  const [name] = command.split(/\s+/)
  if (name === "git") {
    return "On branch main. Nothing to commit."
  }
  if (name === "pnpm") {
    return "✓ 312 tests passed in 4.1s"
  }
  if (name === "deploy") {
    return command.includes("prod")
      ? "Queued build #483 → production"
      : "Queued build #483 → preview"
  }
  if (name === "help") {
    return "Try git, pnpm, deploy or clear."
  }
  return `command not found: ${name}`
}

function CommandLine() {
  const inputRef = useRef<HTMLInputElement>(null)
  const commandsRef = useRef<readonly string[]>(seededCommands)
  const [lines, setLines] = useState(() => [
    {
      id: 0,
      command: "deploy --env preview",
      output: respond("deploy --env preview"),
    },
  ])
  const [history] = useState(() =>
    createInputHistory({
      entries: () => commandsRef.current,
      read: () => inputRef.current?.value ?? "",
      write: (value) => {
        if (inputRef.current) {
          inputRef.current.value = value
        }
      },
    })
  )

  const run = () => {
    const field = inputRef.current
    const command = field?.value.trim()
    if (!field || !command) {
      return
    }
    field.value = ""
    commandsRef.current = [...commandsRef.current, command]
    if (command === "clear") {
      setLines([])
      return
    }
    setLines((current) =>
      [
        ...current,
        { id: commandsRef.current.length, command, output: respond(command) },
      ].slice(-3)
    )
  }

  return (
    <div className="rc-term">
      <div className="rc-term-bar" aria-hidden>
        <span className="rc-term-dots">
          <i />
          <i />
          <i />
        </span>
        <span>~/atlas</span>
      </div>
      <ol className="rc-term-log" aria-live="polite" aria-label="Output">
        {lines.map((line) => (
          <li key={line.id}>
            <span className="rc-term-echo">
              <span className="rc-term-prompt" aria-hidden>
                ›
              </span>
              {line.command}
            </span>
            <span className="rc-term-out">{line.output}</span>
          </li>
        ))}
      </ol>
      <div className="rc-term-input">
        <span className="rc-term-prompt" aria-hidden>
          ›
        </span>
        <input
          ref={inputRef}
          type="text"
          aria-label="Command"
          placeholder="Type a command"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          className="rc-term-field"
          onKeyDown={(event) => {
            if (history.keydown(event.nativeEvent)) {
              return
            }
            if (event.key === "Enter" && !isComposing(event)) {
              event.preventDefault()
              run()
            }
          }}
        />
      </div>
      <p className="rc-term-hint" aria-hidden>
        <kbd>↑</kbd>
        <kbd>↓</kbd> history <kbd>⏎</kbd> run
      </p>
    </div>
  )
}

/* Activity feed */

const shownValue = (change: ObservedChange, value: unknown) =>
  typeof value === "boolean"
    ? value
      ? "on"
      : "off"
    : change.component === "slider" && typeof value === "number"
      ? quietTime(value)
      : String(value)

const quietTime = (hour: number) =>
  `${Math.floor(hour) % 24}:${hour % 1 ? "30" : "00"}`

function ActivityFeed() {
  const rootRef = useRef<HTMLDivElement>(null)
  const digestLabel = useId()
  const [changes, setChanges] = useState<(ObservedChange & { at: number })[]>(
    []
  )

  useEffect(() => {
    let count = 0
    const timers = new Set<number>()
    const stop = observe(
      (change) => {
        setChanges((current) =>
          [{ ...change, at: ++count }, ...current].slice(0, 5)
        )
        const element = elementOf(change.ref)
        if (!element) {
          return
        }
        element.setAttribute("data-rc-flash", "")
        const timer = window.setTimeout(() => {
          element.removeAttribute("data-rc-flash")
          timers.delete(timer)
        }, 700)
        timers.add(timer)
      },
      { within: () => rootRef.current }
    )
    return () => {
      stop()
      timers.forEach((timer) => window.clearTimeout(timer))
    }
  }, [])

  return (
    <div className="rc-activity" ref={rootRef}>
      <div className="rc-prefs">
        <div className="rc-pref">
          <span className="rc-pref-name" id={digestLabel}>
            Email digest
          </span>
          <Switch
            aria-labelledby={digestLabel}
            defaultChecked
            className="rc-switch"
          >
            <SwitchThumb className="rc-switch-thumb" />
          </Switch>
        </div>
        <div className="rc-pref">
          <span className="rc-pref-name">Send</span>
          <SegmentedControl
            aria-label="Send"
            defaultValue="Daily"
            className="rc-seg"
          >
            <SegmentedControlIndicator className="rc-seg-indicator" />
            <SegmentedControlItem value="Daily" className="rc-seg-item">
              Daily
            </SegmentedControlItem>
            <SegmentedControlItem value="Weekly" className="rc-seg-item">
              Weekly
            </SegmentedControlItem>
          </SegmentedControl>
        </div>
        <Slider
          defaultValue={21}
          min={18}
          max={24}
          step={0.5}
          format={quietTime}
          className="rc-quiet"
        >
          <div className="rc-pref">
            <span className="rc-pref-name">Quiet after</span>
            <SliderValue className="rc-quiet-value" />
          </div>
          <SliderControl className="rc-quiet-control">
            <SliderTrack className="rc-quiet-track">
              <SliderIndicator className="rc-quiet-fill" />
              <SliderThumb
                aria-label="Quiet after"
                className="rc-quiet-thumb"
              />
            </SliderTrack>
          </SliderControl>
        </Slider>
      </div>
      <div className="rc-feed">
        <span className="rc-feed-title">
          <span className="rc-feed-live" aria-hidden />
          Changes
        </span>
        {changes.length ? (
          <ol className="rc-feed-list">
            {changes.map((change) => (
              <li key={change.at} className="rc-feed-item">
                <span className="rc-feed-label">{change.label}</span>
                <span className="rc-feed-values">
                  <s>{shownValue(change, change.previous)}</s>
                  <span aria-label="to">→</span>
                  <strong>{shownValue(change, change.value)}</strong>
                </span>
                <span className="rc-feed-source">
                  {change.source} · {change.reason}
                </span>
              </li>
            ))}
          </ol>
        ) : (
          <p className="rc-feed-empty">Change a setting.</p>
        )}
      </div>
    </div>
  )
}

const tiles: Tile[] = [
  {
    id: "collections-release-history",
    title: "Scrub the release history.",
    blurb: "Arrow keys or a drag move through every version.",
    size: "wide",
    tone: "paper",
    Demo: ReleaseHistory,
  },
  {
    id: "collections-request-waterfall",
    title: "Read the request waterfall.",
    blurb:
      "Every request on one clock, with the ones it started tucked inside.",
    size: "large",
    tone: "white",
    Demo: RequestWaterfall,
  },
  {
    id: "collections-invoice-pages",
    title: "Page 4 of 212.",
    blurb:
      "The page list keeps its length, so nothing shifts under the pointer.",
    size: "tall",
    tone: "sky",
    Demo: Invoices,
  },
  {
    id: "collections-board-history",
    title: "Undo 'Moved card to Done'.",
    blurb:
      "Every change in words, from you or an agent, one step back at a time.",
    size: "large",
    tone: "butter",
    Demo: BoardHistory,
  },
  {
    id: "collections-command-recall",
    title: "Recall the last command.",
    blurb: "Arrow up brings it back, and leaves an IME alone.",
    size: "s",
    tone: "ink",
    Demo: CommandLine,
  },
  {
    id: "collections-activity-feed",
    title: "Watch what changed.",
    blurb: "One stream of every change, who made it and how.",
    size: "tall",
    tone: "sage",
    Demo: ActivityFeed,
  },
]

export { tiles }
