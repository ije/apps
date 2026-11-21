import {
  ArrowCounterClockwiseIcon,
  ArrowsLeftRightIcon,
  BackspaceIcon,
  CalendarBlankIcon,
  CaretLeftIcon,
  CaretRightIcon,
  CopyIcon,
  DesktopIcon,
  MagnifyingGlassIcon,
  MinusIcon,
  MoonIcon,
  PlusIcon,
  StarIcon,
  SunIcon,
  XIcon,
} from "@phosphor-icons/react"
import {
  animateIn,
  animateOut,
  announce,
  ariaKeyshortcuts,
  cellsToText,
  createCellRange,
  createChange,
  createFocusRecovery,
  createGridFocus,
  createPreview,
  createRovingFocus,
  createTypeahead,
  definePrimitive,
  formatShortcut,
  isRTL,
  isTypeaheadKey,
  keepFocus,
  labelOf,
  matchesShortcut,
  nearestInDirection,
  pageLocale,
  spatialMove,
  textToCells,
  toLocale,
  userLocale,
  type CellRange,
  type ChangeDetails,
  type GridCell,
} from "@ui-sdk/core"
import {
  addDays,
  datePattern,
  daysBetween,
  firstDayOfWeek,
  formatDate,
  formatDateRange,
  monthGrid,
  monthNames,
  parseDate,
  weekdayNames,
} from "@ui-sdk/core/date"
import { formatNumber, numberFormatter } from "@ui-sdk/core/number"
import { fuzzyScore, matchRanges, matchSegments } from "@ui-sdk/core/text-match"
import {
  Calendar,
  CalendarDay,
  CalendarGrid,
  CalendarHeading,
  CalendarNext,
  CalendarPrevious,
  CalendarWeekday,
  CalendarWeekdays,
  CalendarWeeks,
} from "@ui-sdk/react/calendar"
import {
  CalendarDay as PickerDay,
  CalendarGrid as PickerGrid,
  CalendarHeading as PickerHeading,
  CalendarNext as PickerNext,
  CalendarPrevious as PickerPrevious,
  CalendarWeekday as PickerWeekday,
  CalendarWeekdays as PickerWeekdays,
  CalendarWeeks as PickerWeeks,
  DatePicker,
  DatePickerContent,
  DatePickerHint,
  DatePickerInput,
  DatePickerTrigger,
} from "@ui-sdk/react/date-picker"
import { createParts } from "@ui-sdk/react/primitive"
import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react"
import { useShortcutHint } from "@ui-sdk/react/shortcuts"
import type { Tile } from "../bento/types"
import "./language.css"

/* Shared bits */

function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="lg-kbd">{children}</kbd>
}

const subscribeMinute = (onChange: () => void) => {
  const id = setInterval(onChange, 1000)
  return () => clearInterval(id)
}
const thisMinute = () => Math.floor(Date.now() / 60_000) * 60_000
const noTime = () => null
const subscribeNever = () => () => {}
const noLocale = () => undefined

/** The current minute, or null while hydrating, since the page is rendered ahead of time. */
function useMinute() {
  return useSyncExternalStore(subscribeMinute, thisMinute, noTime)
}

function useUserLocale() {
  return useSyncExternalStore(subscribeNever, userLocale, noLocale)
}

function useMedia(query: string) {
  return useSyncExternalStore(
    (onChange) => {
      const list = matchMedia(query)
      list.addEventListener("change", onChange)
      return () => list.removeEventListener("change", onChange)
    },
    () => matchMedia(query).matches,
    () => false
  )
}

function clockIn(zone: string, now: number | null, locale?: string) {
  if (now === null) {
    return { time: "--:--", offset: "" }
  }
  const parts = new Intl.DateTimeFormat(locale, {
    timeZone: zone,
    hour: "2-digit",
    minute: "2-digit",
    timeZoneName: "shortOffset",
  }).formatToParts(now)
  const time = parts
    .filter((part) => part.type !== "timeZoneName")
    .map((part) => part.value)
    .join("")
    .trim()
  return {
    time,
    offset: parts.find((part) => part.type === "timeZoneName")?.value ?? "",
  }
}

/* 1. Fuzzy city search */

const CITIES = [
  { name: "Santiago", country: "Chile", zone: "America/Santiago" },
  { name: "San Juan", country: "Puerto Rico", zone: "America/Puerto_Rico" },
  { name: "Santorini", country: "Greece", zone: "Europe/Athens" },
  { name: "Busan", country: "South Korea", zone: "Asia/Seoul" },
  { name: "Santa Marta", country: "Colombia", zone: "America/Bogota" },
  { name: "Saint Petersburg", country: "Russia", zone: "Europe/Moscow" },
  { name: "Kyoto", country: "Japan", zone: "Asia/Tokyo" },
  { name: "Reykjavík", country: "Iceland", zone: "Atlantic/Reykjavik" },
  { name: "São Paulo", country: "Brazil", zone: "America/Sao_Paulo" },
  { name: "Kraków", country: "Poland", zone: "Europe/Warsaw" },
  { name: "Marrakesh", country: "Morocco", zone: "Africa/Casablanca" },
  { name: "Tbilisi", country: "Georgia", zone: "Asia/Tbilisi" },
  { name: "Vancouver", country: "Canada", zone: "America/Vancouver" },
  { name: "Cape Town", country: "South Africa", zone: "Africa/Johannesburg" },
  { name: "Kathmandu", country: "Nepal", zone: "Asia/Kathmandu" },
  { name: "Hanoi", country: "Vietnam", zone: "Asia/Ho_Chi_Minh" },
  { name: "Auckland", country: "New Zealand", zone: "Pacific/Auckland" },
  { name: "Istanbul", country: "Türkiye", zone: "Europe/Istanbul" },
  { name: "Oaxaca", country: "Mexico", zone: "America/Mexico_City" },
  { name: "Tallinn", country: "Estonia", zone: "Europe/Tallinn" },
  { name: "Zanzibar", country: "Tanzania", zone: "Africa/Dar_es_Salaam" },
  { name: "Muscat", country: "Oman", zone: "Asia/Muscat" },
  { name: "Honolulu", country: "Hawaii", zone: "Pacific/Honolulu" },
  { name: "Montevideo", country: "Uruguay", zone: "America/Montevideo" },
]

const HQ = {
  name: "Vercel HQ",
  country: "San Francisco ▲",
  zone: "America/Los_Angeles",
}
const citiesFor = (query: string) => {
  const typed = query.trim().toLowerCase()
  return typed.length >= 3 && "vercel".startsWith(typed)
    ? [HQ, ...CITIES]
    : CITIES
}

type MatchMode = "fuzzy" | "startsWith" | "contains"
const MODES: { value: MatchMode; label: string }[] = [
  { value: "fuzzy", label: "Fuzzy" },
  { value: "startsWith", label: "Starts with" },
  { value: "contains", label: "Contains" },
]

function CitySearch() {
  const [query, setQuery] = useState("san")
  const [mode, setMode] = useState<MatchMode>("fuzzy")
  const [active, setActive] = useState(0)
  const [picked, setPicked] = useState("Kyoto")
  const now = useMinute()
  const id = useId()
  const locale = useUserLocale()

  const results = citiesFor(query)
    .map((city) => ({
      city,
      score: fuzzyScore(city.name, query),
      ranges: matchRanges(city.name, query, mode),
    }))
    .filter(({ score, ranges }) =>
      !query ? true : mode === "fuzzy" ? score > 0 : ranges.length > 0
    )
    .sort((a, b) => b.score - a.score)
    .slice(0, 7)

  const pickedCity = [HQ, ...CITIES].find((city) => city.name === picked)
  const pickedClock = pickedCity ? clockIn(pickedCity.zone, now, locale) : null

  function onKeyDown(event: ReactKeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault()
      const by = event.key === "ArrowDown" ? 1 : -1
      setActive((index) =>
        Math.max(0, Math.min(results.length - 1, index + by))
      )
    } else if (event.key === "Enter" && results[active]) {
      event.preventDefault()
      setPicked(results[active].city.name)
    }
  }

  return (
    <div className="lg-find">
      <div className="lg-find-top">
        <div className="lg-find-modes" role="group" aria-label="Match mode">
          {MODES.map((option) => (
            <button
              key={option.value}
              type="button"
              className="lg-find-mode"
              aria-pressed={mode === option.value}
              onClick={() => {
                setMode(option.value)
                setActive(0)
              }}
            >
              {option.label}
            </button>
          ))}
        </div>
        {pickedClock && (
          <p className="lg-find-picked" aria-live="polite">
            Next stop <strong>{picked}</strong> <span>{pickedClock.time}</span>
          </p>
        )}
      </div>
      <label className="lg-find-field">
        <MagnifyingGlassIcon size={16} weight="bold" aria-hidden />
        <input
          className="lg-find-input"
          role="combobox"
          aria-label="Search cities"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={
            results[active] ? `${id}-${active}` : undefined
          }
          value={query}
          spellCheck={false}
          onChange={(event) => {
            setQuery(event.target.value)
            setActive(0)
          }}
          onKeyDown={onKeyDown}
        />
        <span className="lg-find-count">
          {results.length} of {CITIES.length}
        </span>
      </label>
      <ul
        className="lg-find-list"
        role="listbox"
        id={`${id}-list`}
        aria-label="Cities"
      >
        {results.map(({ city, score, ranges }, index) => {
          const clock = clockIn(city.zone, now, locale)
          return (
            <li
              key={city.name}
              id={`${id}-${index}`}
              role="option"
              aria-selected={index === active}
              className="lg-find-row"
              data-highlighted={index === active ? "" : undefined}
              onPointerMove={() => setActive(index)}
              onClick={() => setPicked(city.name)}
            >
              <span className="lg-find-name">
                {matchSegments(city.name, ranges).map((segment, key) =>
                  segment.match ? (
                    <mark key={key}>{segment.text}</mark>
                  ) : (
                    <span key={key}>{segment.text}</span>
                  )
                )}
                <span className="lg-find-country">{city.country}</span>
              </span>
              <span
                className="lg-find-score"
                aria-hidden
                style={{ "--score": score } as CSSProperties}
              />
              <span className="lg-find-time">
                {clock.time}
                <small>{clock.offset}</small>
              </span>
            </li>
          )
        })}
        {!results.length && (
          <li className="lg-find-empty">No city matches “{query}”.</li>
        )}
      </ul>
    </div>
  )
}

/* 2. Typeahead: a currency list */

const currencyList = definePrimitive({
  name: "lg-currencies",
  state: () => ({
    focused: null as string | null,
    chosen: null as string | null,
  }),
  api: (context) => {
    const roving = createRovingFocus(context, { slot: "item", key: "focused" })
    const typeahead = createTypeahead()
    return {
      roving,
      choose: (code: string) => context.store.set({ chosen: code }),
      keydown(event: KeyboardEvent, el: HTMLElement) {
        if (
          event.defaultPrevented ||
          roving.keydown(event, el) ||
          !isTypeaheadKey(event)
        ) {
          return
        }
        if (event.key === " " && typeahead.active()) {
          event.preventDefault()
        }
        const items = roving.items()
        const index = typeahead(
          event.key,
          items.map((item) => labelOf(item.el)),
          items.findIndex((item) => item.el === el)
        )
        if (index !== -1) {
          roving.focus(items[index].key)
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot({ attrs: () => ({ role: "listbox" }) }),
    item: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        type: "button",
        role: "option",
        "aria-selected": String(state.chosen === part.key),
        tabindex: part.api.roving.tabIndex(state, part),
      }),
      press: ({ api, key }) => api.choose(key),
      on: {
        keydown: (event, { el, api }) => {
          if (el) {
            api.keydown(event, el)
          }
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
  }),
})

const { Root: CurrencyList, Item: CurrencyItem } = createParts(currencyList, {
  root: "div",
  item: "button",
})

const CURRENCIES = [
  ["Argentine peso", "ARS"],
  ["Bulgarian lev", "BGN"],
  ["Danish krone", "DKK"],
  ["Hungarian forint", "HUF"],
  ["Indian rupee", "INR"],
  ["Japanese yen", "JPY"],
  ["Korean won", "KRW"],
  ["Norwegian krone", "NOK"],
  ["Polish złoty", "PLN"],
  ["Singapore dollar", "SGD"],
  ["Swedish krona", "SEK"],
  ["Swiss franc", "CHF"],
  ["Thai baht", "THB"],
  ["Turkish lira", "TRY"],
]

function Typeahead() {
  return (
    <div className="lg-ta">
      <CurrencyList className="lg-ta-list" aria-label="Currencies">
        {CURRENCIES.map(([name, code]) => (
          <CurrencyItem key={code} value={code} className="lg-ta-item">
            <span>{name}</span>
            <span className="lg-ta-code" aria-hidden="true">
              {code}
            </span>
          </CurrencyItem>
        ))}
      </CurrencyList>
      <p className="lg-hint">
        Focus the list, type <Kbd>S</Kbd>
        <Kbd>W</Kbd>
        <Kbd>I</Kbd>
      </p>
    </div>
  )
}

/* 3. Roving focus: a number format picker */

const formatPicker = definePrimitive({
  name: "lg-number-format",
  options: { defaultFormat: "string" },
  state: (options: { defaultFormat?: string }) => ({
    format: options.defaultFormat ?? "en-US",
    focused: null as string | null,
  }),
  api: (context) => {
    const roving = createRovingFocus(context, {
      slot: "option",
      key: "focused",
      orientation: () => "horizontal",
      loop: () => true,
      remember: false,
      preferred: (item) => item.key === context.store.get().format,
      claims: (state, item) => item.key === state.format,
    })
    return {
      roving,
      select: (format: string) => context.store.set({ format }),
      keydown(event: KeyboardEvent, el: HTMLElement) {
        const moved = roving.keydown(event, el)
        if (moved) {
          context.store.set({ format: moved.key })
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot<object, { format: string }>({
      state: (state) => ({ format: state.format }),
    }),
    group: slot({ attrs: () => ({ role: "radiogroup" }) }),
    option: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        type: "button",
        role: "radio",
        "aria-checked": String(state.format === part.key),
        "data-checked": state.format === part.key ? "" : undefined,
        tabindex: part.api.roving.tabIndex(state, part),
      }),
      press: ({ api, key }) => api.select(key),
      on: {
        keydown: (event, { el, api }) => {
          if (el) {
            api.keydown(event, el)
          }
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
  }),
})

const {
  Root: FormatRoot,
  Group: FormatGroup,
  Option: FormatOption,
} = createParts(formatPicker, { root: "div", group: "div", option: "button" })

const FORMATS = [
  {
    value: "en-US",
    label: "US",
    name: "United States",
    currency: "USD",
    rule: "Commas, then a point",
  },
  {
    value: "de-DE",
    label: "DE",
    name: "Germany",
    currency: "EUR",
    rule: "Points, then a comma",
  },
  {
    value: "fr-FR",
    label: "FR",
    name: "France",
    currency: "EUR",
    rule: "Thin spaces, then a comma",
  },
  {
    value: "en-IN",
    label: "IN",
    name: "India",
    currency: "INR",
    rule: "Lakh and crore grouping",
  },
  {
    value: "de-CH",
    label: "CH",
    name: "Switzerland",
    currency: "CHF",
    rule: "Apostrophes, then a point",
  },
]

function NumberFormats() {
  return (
    <FormatRoot className="lg-nf">
      {({ format }) => {
        const current =
          FORMATS.find((item) => item.value === format) ?? FORMATS[0]
        const amount = formatNumber(1234567.89, current.value, {
          style: "currency",
          currency: current.currency,
        })
        return (
          <>
            <p className="lg-nf-amount">{amount}</p>
            <p className="lg-nf-rule">{current.rule}</p>
            <FormatGroup className="lg-nf-group" aria-label="Number format">
              {FORMATS.map((item) => (
                <FormatOption
                  key={item.value}
                  value={item.value}
                  aria-label={item.name}
                  className="lg-nf-option"
                >
                  {item.label}
                </FormatOption>
              ))}
            </FormatGroup>
          </>
        )
      }}
    </FormatRoot>
  )
}

/* 4. Grid focus: a seat map */

const SEAT_ROWS = 12
const FIRST_ROW = 10
const SEAT_LETTERS = ["A", "B", "C", "D", "E", "F"]
const TAKEN = new Set([
  "0:0",
  "0:1",
  "1:4",
  "1:5",
  "2:2",
  "3:0",
  "3:5",
  "4:3",
  "4:4",
  "5:1",
  "5:2",
  "6:4",
  "7:0",
  "7:3",
  "8:5",
  "9:2",
  "9:3",
  "10:1",
  "10:5",
  "11:0",
  "11:4",
])
const seatName = (row: number, col: number) =>
  `${row + FIRST_ROW}${SEAT_LETTERS[col]}`
const seatKind = (col: number) =>
  col === 0 || col === 5
    ? "Window"
    : col === 2 || col === 3
      ? "Aisle"
      : "Middle"

const seatMap = definePrimitive({
  name: "lg-seat-map",
  // The tab stop starts on the chosen seat; left null, it would sit on 10A, which is taken.
  state: () => ({
    active: { row: 4, col: 2 } as GridCell | null,
    seat: { row: 4, col: 2 } as GridCell | null,
  }),
  api: (context) => {
    const grid = createGridFocus(context, {
      key: "active",
      rows: () => SEAT_ROWS,
      cols: () => SEAT_LETTERS.length,
      disabled: (cell) => TAKEN.has(`${cell.row}:${cell.col}`),
    })
    return {
      grid,
      choose(cell: GridCell) {
        if (!TAKEN.has(`${cell.row}:${cell.col}`)) {
          context.store.set({ seat: { row: cell.row, col: cell.col } })
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot<object, { seat: GridCell | null }>({
      state: (state) => ({ seat: state.seat }),
    }),
    cabin: slot({ attrs: () => ({ role: "grid" }) }),
    seat: slot<{ row: number; col: number }>({
      // Declaring `id` makes it the part's key, so each cell is its own part.
      props: { id: "string", row: "number", col: "number" },
      attrs: (state, { props, api }) => {
        const taken = TAKEN.has(`${props.row}:${props.col}`)
        const mine =
          state.seat?.row === props.row && state.seat?.col === props.col
        return {
          type: "button",
          role: "gridcell",
          tabindex: api.grid.tabIndex(state, props),
          "aria-selected": String(mine),
          "aria-disabled": taken ? "true" : undefined,
          "aria-label": `Seat ${seatName(props.row, props.col)}, ${seatKind(props.col)}${taken ? ", taken" : ""}`,
          "data-taken": taken ? "" : undefined,
          "data-mine": mine ? "" : undefined,
          "data-active": api.grid.isActive(state, props) ? "" : undefined,
        }
      },
      press: ({ api, props }) => api.choose(props),
      on: {
        keydown: (event, { api }) => {
          api.grid.keydown(event)
        },
        pointerdown: (event, { api, props }) => {
          api.grid.focus(props, { reason: "item-press", event })
        },
      },
      bind: (el, { api, props }) => api.grid.bind(el, props),
    }),
  }),
})

const {
  Root: SeatRoot,
  Cabin: SeatCabin,
  Seat,
} = createParts(seatMap, { root: "div", cabin: "div", seat: "button" })

function Seats() {
  return (
    <SeatRoot className="lg-seats">
      {({ seat }) => (
        <>
          <div className="lg-seats-head">
            <p className="lg-seats-flight">AY 131</p>
            <p className="lg-seats-route">Helsinki → Singapore</p>
          </div>
          <div className="lg-seats-letters" aria-hidden="true">
            <span />
            {SEAT_LETTERS.map((letter) => (
              <span key={letter}>{letter}</span>
            ))}
          </div>
          <SeatCabin className="lg-seats-cabin" aria-label="Cabin seats">
            {Array.from({ length: SEAT_ROWS }, (_, row) => (
              <div key={row} role="row" className="lg-seats-row">
                <span className="lg-seats-num" aria-hidden="true">
                  {row + FIRST_ROW}
                </span>
                {SEAT_LETTERS.map((letter, col) => (
                  <Seat
                    key={letter}
                    id={`lg-seat-${row}-${col}`}
                    row={row}
                    col={col}
                    className="lg-seat"
                  />
                ))}
              </div>
            ))}
          </SeatCabin>
          <div className="lg-seats-foot">
            <p className="lg-seats-pick">
              {seat ? (
                <>
                  <strong>{seatName(seat.row, seat.col)}</strong>{" "}
                  {seatKind(seat.col)}
                </>
              ) : (
                "Choose a seat"
              )}
            </p>
            <p className="lg-hint">
              <Kbd>←</Kbd>
              <Kbd>→</Kbd>
              <Kbd>↑</Kbd>
              <Kbd>↓</Kbd> then <Kbd>Enter</Kbd>
            </p>
          </div>
        </>
      )}
    </SeatRoot>
  )
}

/* 5. Cell range: a trip budget sheet */

const SHEET_COLS = ["A", "B", "C", "D"]
const SHEET_START = [
  ["Stop", "Nights", "Hotel €", "Meals €"],
  ["Porto", "3", "276", "144"],
  ["Bologna", "2", "236", "122"],
  ["Ljubljana", "2", "172", "78"],
  ["Split", "4", "416", "220"],
]

type SheetState = {
  active: GridCell | null
  range: CellRange | null
  cells: string[][]
  note: string
}

function sheetBounds(state: SheetState) {
  const { range, active } = state
  if (range) {
    return {
      top: Math.min(range.anchor.row, range.end.row),
      bottom: Math.max(range.anchor.row, range.end.row),
      left: Math.min(range.anchor.col, range.end.col),
      right: Math.max(range.anchor.col, range.end.col),
    }
  }
  return active
    ? {
        top: active.row,
        bottom: active.row,
        left: active.col,
        right: active.col,
      }
    : null
}

const sheet = definePrimitive({
  name: "lg-sheet",
  state: (): SheetState => ({
    active: null,
    range: { anchor: { row: 1, col: 2 }, end: { row: 3, col: 3 } },
    cells: SHEET_START.map((row) => [...row]),
    note: "",
  }),
  api: (context) => {
    const rows = () => context.store.get().cells.length
    const cols = () => SHEET_COLS.length
    const grid = createGridFocus(context, { key: "active", rows, cols })
    const range = createCellRange(context, { key: "range", grid, rows, cols })
    return {
      grid,
      range,
      copy(event: ClipboardEvent) {
        const state = context.store.get()
        const text = range.text(state, ({ row, col }) => state.cells[row][col])
        if (!text) {
          return
        }
        event.preventDefault()
        event.clipboardData?.setData("text/plain", text)
        context.store.set({ note: `Copied ${text.split("\n").length} rows` })
      },
      paste(event: ClipboardEvent) {
        const rowsIn = textToCells(
          event.clipboardData?.getData("text/plain") ?? ""
        )
        const state = context.store.get()
        const bounds = range.bounds(state)
        if (!rowsIn.length || !bounds) {
          return
        }
        event.preventDefault()
        const cells = state.cells.map((row) => [...row])
        let bottom = bounds.top
        let right = bounds.left
        rowsIn.forEach((row, i) =>
          row.forEach((value, j) => {
            const r = bounds.top + i
            const c = bounds.left + j
            if (r >= cells.length || c >= SHEET_COLS.length) {
              return
            }
            cells[r][c] = value
            bottom = Math.max(bottom, r)
            right = Math.max(right, c)
          })
        )
        context.store.set({ cells, note: "Pasted into the sheet" })
        range.select({
          anchor: { row: bounds.top, col: bounds.left },
          end: { row: bottom, col: right },
        })
      },
      copySheet() {
        const text = cellsToText(context.store.get().cells)
        navigator.clipboard?.writeText(text).catch(() => undefined)
        context.store.set({ note: `Sheet copied, ${text.length} characters` })
      },
      reset() {
        context.store.set({
          cells: SHEET_START.map((row) => [...row]),
          note: "",
        })
      },
    }
  },
  slots: (slot) => ({
    root: slot<
      object,
      { label: string; sum: number; count: number; note: string }
    >({
      state: (state) => {
        const b = sheetBounds(state)
        if (!b) {
          return { label: "–", sum: 0, count: 0, note: state.note }
        }
        let sum = 0
        let count = 0
        for (let r = b.top; r <= b.bottom; r++) {
          for (let c = b.left; c <= b.right; c++) {
            const value = Number(state.cells[r][c])
            if (r > 0 && state.cells[r][c] !== "" && Number.isFinite(value)) {
              sum += value
              count++
            }
          }
        }
        const from = `${SHEET_COLS[b.left]}${b.top + 1}`
        const to = `${SHEET_COLS[b.right]}${b.bottom + 1}`
        return {
          label: from === to ? from : `${from}:${to}`,
          sum,
          count,
          note: state.note,
        }
      },
    }),
    board: slot({
      attrs: () => ({ role: "grid" }),
      on: {
        copy: (event, { api }) => api.copy(event),
        paste: (event, { api }) => api.paste(event),
      },
    }),
    cell: slot<{ row: number; col: number }, { text: string }>({
      // Declaring `id` makes it the part's key, so each cell is its own part.
      props: { id: "string", row: "number", col: "number" },
      state: (state, { props }) => ({
        text: state.cells[props.row]?.[props.col] ?? "",
      }),
      attrs: (state, { props, api }) => {
        const text = state.cells[props.row]?.[props.col] ?? ""
        return {
          role: "gridcell",
          tabindex: api.grid.tabIndex(state, props),
          ...api.range.cellAttrs(state, props),
          "data-head": props.row === 0 ? "" : undefined,
          "data-number": props.row > 0 && /^-?\d/.test(text) ? "" : undefined,
          "data-active": api.grid.isActive(state, props) ? "" : undefined,
        }
      },
      on: {
        keydown: (event, { api }) => {
          api.range.keydown(event)
        },
        pointerdown: (event, { api, props }) =>
          api.range.pointerdown(event, props),
      },
      bind: (el, { api, props }) => api.range.bind(el, props),
    }),
    copy: slot({
      attrs: () => ({ type: "button" }),
      press: ({ api }) => api.copySheet(),
    }),
    reset: slot({
      attrs: () => ({ type: "button" }),
      press: ({ api }) => api.reset(),
    }),
  }),
})

const {
  Root: SheetRoot,
  Board: SheetBoard,
  Cell: SheetCell,
  Copy: SheetCopy,
  Reset: SheetReset,
} = createParts(sheet, {
  root: "div",
  board: "div",
  cell: "div",
  copy: "button",
  reset: "button",
})

function Sheet() {
  return (
    <SheetRoot className="lg-sheet">
      {({ label, sum, count, note }) => (
        <>
          <div className="lg-sheet-main">
            <div className="lg-sheet-letters" aria-hidden="true">
              <span />
              {SHEET_COLS.map((letter) => (
                <span key={letter}>{letter}</span>
              ))}
            </div>
            <SheetBoard className="lg-sheet-board" aria-label="Trip budget">
              {SHEET_START.map((_, row) => (
                <div key={row} role="row" className="lg-sheet-row">
                  <span className="lg-sheet-num" aria-hidden="true">
                    {row + 1}
                  </span>
                  {SHEET_COLS.map((letter, col) => (
                    <SheetCell
                      key={letter}
                      id={`lg-cell-${row}-${col}`}
                      row={row}
                      col={col}
                      className="lg-sheet-cell"
                    >
                      {({ text }) => text}
                    </SheetCell>
                  ))}
                </div>
              ))}
            </SheetBoard>
            <p className="lg-hint lg-sheet-hint">
              <Kbd>Shift</Kbd>+<Kbd>arrows</Kbd> select, then copy or paste
            </p>
          </div>
          <aside className="lg-sheet-side" aria-live="polite">
            <p className="lg-sheet-ref">{label}</p>
            <dl className="lg-sheet-stats">
              <div>
                <dt>Sum</dt>
                <dd>{formatNumber(sum, "en-GB")}</dd>
              </div>
              <div>
                <dt>Count</dt>
                <dd>{count}</dd>
              </div>
            </dl>
            <p className="lg-sheet-note">
              {note || "Tab-separated, like any sheet"}
            </p>
            <div className="lg-sheet-actions">
              <SheetCopy className="b-btn lg-sheet-btn">
                <CopyIcon size={14} weight="bold" aria-hidden />
                Copy all
              </SheetCopy>
              <SheetReset
                className="b-btn lg-sheet-btn"
                data-variant="ghost"
                aria-label="Reset sheet"
              >
                <ArrowCounterClockwiseIcon
                  size={14}
                  weight="bold"
                  aria-hidden
                />
              </SheetReset>
            </div>
          </aside>
        </>
      )}
    </SheetRoot>
  )
}

/* 6. Calendar: a range for a stay */

function CalendarMonth({ offset }: { offset: number }) {
  return (
    <CalendarGrid offset={offset} className="lg-cal-grid">
      <CalendarWeekdays>
        {(index) => (
          <CalendarWeekday index={index} className="lg-cal-weekday" />
        )}
      </CalendarWeekdays>
      <CalendarWeeks>
        {(cell) => <CalendarDay value={cell.date} className="lg-cal-day" />}
      </CalendarWeeks>
    </CalendarGrid>
  )
}

function Stay() {
  const [range, setRange] = useState<[string, string] | null>([
    "2026-11-12",
    "2026-11-16",
  ])
  const narrow = useMedia("(max-width: 640px)")
  const nights = range ? daysBetween(range[0], range[1]) : 0
  return (
    <div className="lg-stay">
      <Calendar
        mode="range"
        months={narrow ? 1 : 2}
        locale="en-GB"
        weekStartsOn={0}
        weekdayFormat="narrow"
        fixedWeeks={false}
        defaultMonth="2026-11"
        value={range}
        onValueChange={(value) =>
          setRange(
            Array.isArray(value) && value.length === 2
              ? [value[0], value[1]]
              : null
          )
        }
        className="lg-cal"
      >
        <div className="lg-cal-head">
          <CalendarPrevious className="lg-cal-nav" aria-label="Previous month">
            <CaretLeftIcon size={14} weight="bold" aria-hidden />
          </CalendarPrevious>
          <CalendarHeading className="lg-cal-heading" />
          <CalendarNext className="lg-cal-nav" aria-label="Next month">
            <CaretRightIcon size={14} weight="bold" aria-hidden />
          </CalendarNext>
        </div>
        <div className="lg-cal-months">
          <CalendarMonth offset={0} />
          {!narrow && <CalendarMonth offset={1} />}
        </div>
      </Calendar>
      <div className="lg-stay-sum" aria-live="polite">
        <p className="lg-stay-place">Alfama, Lisbon</p>
        <p className="lg-stay-nights">
          {nights}
          <span>{nights === 1 ? " night" : " nights"}</span>
        </p>
        {/* Server and browser ICU space a range differently, like "12 – 16" and "12–16". */}
        <p className="lg-stay-dates" suppressHydrationWarning>
          {range
            ? formatDateRange(range[0], range[1], "en-GB")
            : "Pick check-in, then check-out"}
        </p>
      </div>
    </div>
  )
}

/* 7. Date Picker */

function Departure() {
  const labelId = useId()
  return (
    <DatePicker locale="de-CH" defaultValue="2026-12-18" className="lg-dp">
      <p className="lg-dp-label" id={labelId}>
        Train from Zürich
      </p>
      <div className="lg-dp-field">
        <DatePickerInput aria-labelledby={labelId} className="lg-dp-input" />
        <DatePickerTrigger className="lg-dp-trigger" aria-label="Open calendar">
          <CalendarBlankIcon size={16} weight="bold" aria-hidden />
        </DatePickerTrigger>
      </div>
      <DatePickerHint className="lg-dp-hint">
        {(state) => `Type it like ${state.example}`}
      </DatePickerHint>
      <DatePickerContent className="b-pop lg-dp-pop">
        <div className="lg-cal-head">
          <PickerPrevious className="lg-cal-nav" aria-label="Previous month">
            <CaretLeftIcon size={16} weight="bold" aria-hidden />
          </PickerPrevious>
          <PickerHeading className="lg-cal-heading" />
          <PickerNext className="lg-cal-nav" aria-label="Next month">
            <CaretRightIcon size={16} weight="bold" aria-hidden />
          </PickerNext>
        </div>
        <PickerGrid className="lg-cal-grid">
          <PickerWeekdays>
            {(index) => (
              <PickerWeekday index={index} className="lg-cal-weekday" />
            )}
          </PickerWeekdays>
          <PickerWeeks>
            {(cell) => (
              <PickerDay value={cell.date} className="lg-cal-day lg-dp-day" />
            )}
          </PickerWeeks>
        </PickerGrid>
      </DatePickerContent>
    </DatePicker>
  )
}

/* 8. Typing a date */

const PARSE_LOCALES = ["en-US", "de-DE", "fr-FR", "ko-KR"]

function ParseDate() {
  const [locale, setLocale] = useState("en-US")
  const [text, setText] = useState("03.04.2027")
  const id = useId()
  const iso = parseDate(text, locale, { referenceDate: "2026-11-02" })
  const pattern = datePattern(locale)
  return (
    <div className="lg-parse">
      <div className="lg-parse-locales" role="group" aria-label="Locale">
        {PARSE_LOCALES.map((tag) => (
          <button
            key={tag}
            type="button"
            className="lg-parse-locale"
            aria-pressed={tag === locale}
            onClick={() => setLocale(tag)}
          >
            {tag}
          </button>
        ))}
      </div>
      <label className="b-label" htmlFor={id}>
        Return flight
      </label>
      <input
        id={id}
        className="b-input lg-parse-input"
        value={text}
        placeholder={pattern.placeholder}
        spellCheck={false}
        aria-describedby={`${id}-out`}
        onChange={(event) => setText(event.target.value)}
      />
      <p
        className="lg-parse-out"
        id={`${id}-out`}
        aria-live="polite"
        data-invalid={iso ? undefined : ""}
      >
        {iso ? (
          <>
            <code>{iso}</code>
            <span>
              {formatDate(iso, locale, {
                weekday: "short",
                day: "numeric",
                month: "short",
                year: "numeric",
              })}
            </span>
          </>
        ) : (
          <span>Not a date in {pattern.placeholder}</span>
        )}
      </p>
    </div>
  )
}

/* 9. Month grids in three places */

const WEEK_PLACES = [
  { city: "New York", locale: "en-US" },
  { city: "Berlin", locale: "de-DE" },
  { city: "Cairo", locale: "ar-EG" },
]
const WEEK_MONTH = "2026-12-01"

function Weeks() {
  const [place, setPlace] = useState(WEEK_PLACES[1])
  const { locale } = place
  const first = firstDayOfWeek(locale)
  const weeks = monthGrid(WEEK_MONTH, {
    weekStartsOn: first,
    fixedWeeks: false,
  })
  const names = weekdayNames(locale, "narrow", first)
  const digits = numberFormatter(locale)
  const rtl = locale.startsWith("ar")
  return (
    <div className="lg-weeks">
      <div className="lg-weeks-places" role="group" aria-label="Place">
        {WEEK_PLACES.map((item) => (
          <button
            key={item.city}
            type="button"
            className="lg-weeks-place"
            aria-pressed={item === place}
            onClick={() => setPlace(item)}
          >
            {item.city}
          </button>
        ))}
      </div>
      <div className="lg-weeks-cal" lang={locale} dir={rtl ? "rtl" : "ltr"}>
        <div className="lg-weeks-top">
          <p className="lg-weeks-month">
            {monthNames(locale)[11]}{" "}
            {new Intl.DateTimeFormat(locale, { year: "numeric" }).format(
              new Date(2026, 11, 1)
            )}
          </p>
          <p className="lg-weeks-start" lang="en" dir="ltr">
            Starts {weekdayNames("en-US", "long")[first]}
          </p>
        </div>
        <table className="lg-weeks-grid">
          <thead>
            <tr>
              {names.map((name, index) => (
                <th key={index} scope="col">
                  {name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {weeks.map((week) => (
              <tr key={week[0]}>
                {week.map((date) => (
                  <td
                    key={date}
                    data-outside={date.slice(5, 7) !== "12" ? "" : undefined}
                  >
                    {digits.format(Number(date.slice(8)))}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

/* 10. A stepper primitive with date math */

type StepperReason = "increment-press" | "decrement-press" | "keyboard"
type StepperOptions = {
  value?: number
  defaultValue?: number
  min?: number
  max?: number
  onValueChange?: (value: number, details: ChangeDetails<StepperReason>) => void
}

const stepper = definePrimitive({
  name: "lg-stepper",
  options: {
    value: "number",
    defaultValue: "number",
    min: "number",
    max: "number",
    onValueChange: "callback",
  },
  controlled: ["value"],
  state: (options: StepperOptions) => ({
    value: options.value ?? options.defaultValue ?? 1,
    min: options.min ?? 1,
    max: options.max ?? 99,
  }),
  sync: (options) => ({
    ...(options.value === undefined ? {} : { value: options.value }),
    min: options.min ?? 1,
    max: options.max ?? 99,
  }),
  api: (context) => {
    const change = createChange(context)
    function set(next: number, reason: StepperReason, event?: Event) {
      const { min, max, value } = context.store.get()
      const clamped = Math.min(max, Math.max(min, next))
      return clamped !== value && change("value", clamped, { reason, event })
    }
    return {
      set,
      step: (by: number, event?: Event) =>
        set(
          context.store.get().value + by,
          by > 0 ? "increment-press" : "decrement-press",
          event
        ),
      keydown(event: KeyboardEvent) {
        const { value, min, max } = context.store.get()
        const next = (
          {
            ArrowUp: value + 1,
            ArrowDown: value - 1,
            PageUp: value + 7,
            PageDown: value - 7,
            Home: min,
            End: max,
          } as Record<string, number>
        )[event.key]
        if (next === undefined) {
          return
        }
        event.preventDefault()
        set(next, "keyboard", event)
      },
    }
  },
  slots: (slot) => ({
    root: slot({ attrs: () => ({ role: "group" }) }),
    count: slot<object, { value: number }>({
      state: (state) => ({ value: state.value }),
      attrs: (state) => ({
        role: "spinbutton",
        tabindex: "0",
        "aria-valuenow": String(state.value),
        "aria-valuemin": String(state.min),
        "aria-valuemax": String(state.max),
      }),
      on: { keydown: (event, { api }) => api.keydown(event) },
    }),
    decrement: slot({
      attrs: (state) => ({
        type: "button",
        tabindex: "-1",
        "aria-disabled": state.value <= state.min ? "true" : undefined,
        "data-disabled": state.value <= state.min ? "" : undefined,
      }),
      press: ({ api }, event) => api.step(-1, event),
    }),
    increment: slot({
      attrs: (state) => ({
        type: "button",
        tabindex: "-1",
        "aria-disabled": state.value >= state.max ? "true" : undefined,
        "data-disabled": state.value >= state.max ? "" : undefined,
      }),
      press: ({ api }, event) => api.step(1, event),
    }),
  }),
  agent: { actions: ["set"] },
})

const {
  Root: StepperRoot,
  Count: StepperCount,
  Decrement: StepperDecrement,
  Increment: StepperIncrement,
} = createParts(stepper, {
  root: "div",
  count: "output",
  decrement: "button",
  increment: "button",
})

const ARRIVAL = "2026-10-03"

function Nights() {
  const [days, setDays] = useState(30)
  const leave = addDays(ARRIVAL, days)
  const short = (date: string) =>
    formatDate(date, "en-GB", {
      weekday: "short",
      day: "numeric",
      month: "short",
    })
  return (
    <div className="lg-nights">
      <p className="lg-nights-label">Days in the Schengen area</p>
      <StepperRoot
        value={days}
        onValueChange={setDays}
        min={1}
        max={90}
        aria-label="Schengen days"
        className="lg-stepper"
      >
        <StepperDecrement className="lg-stepper-btn" aria-label="One day fewer">
          <MinusIcon size={16} weight="bold" aria-hidden />
        </StepperDecrement>
        <StepperCount
          className="lg-stepper-count"
          aria-label="Days"
          aria-valuetext={`${days} days`}
        >
          {({ value }) => value}
        </StepperCount>
        <StepperIncrement className="lg-stepper-btn" aria-label="One day more">
          <PlusIcon size={16} weight="bold" aria-hidden />
        </StepperIncrement>
      </StepperRoot>
      <p className="lg-nights-out">
        In {short(ARRIVAL)} <span aria-hidden="true">→</span> out {short(leave)}
      </p>
    </div>
  )
}

/* 11. A star rating primitive */

type RatingReason = "item-press" | "list-navigation"
type RatingOptions = {
  value?: number
  defaultValue?: number
  onValueChange?: (value: number, details: ChangeDetails<RatingReason>) => void
}

const rating = definePrimitive({
  name: "lg-rating",
  options: {
    value: "number",
    defaultValue: "number",
    onValueChange: "callback",
  },
  controlled: ["value"],
  state: (options: RatingOptions) => ({
    value: options.value ?? options.defaultValue ?? 0,
    preview: null as string | null,
    focused: null as string | null,
  }),
  sync: (options) =>
    options.value === undefined ? {} : { value: options.value },
  api: (context) => {
    const change = createChange(context)
    const rate = (value: number, event?: Event) =>
      change("value", value, { reason: "item-press", event })
    const roving = createRovingFocus(context, {
      slot: "star",
      key: "focused",
      remember: false,
      preferred: (item) => Number(item.key) === context.store.get().value,
      claims: (state, item) =>
        state.value ? Number(item.key) === state.value : item.key === "1",
      move(event, { from, items, rtl }) {
        if (event.ctrlKey || event.metaKey || event.altKey) {
          return
        }
        if (event.key === "Home") {
          return items[0]?.key
        }
        if (event.key === "End") {
          return items[items.length - 1]?.key
        }
        const up =
          event.key === "ArrowUp" ||
          event.key === (rtl ? "ArrowLeft" : "ArrowRight")
        const down =
          event.key === "ArrowDown" ||
          event.key === (rtl ? "ArrowRight" : "ArrowLeft")
        if (!up && !down) {
          return
        }
        const index = items.indexOf(from) + (up ? 1 : -1)
        return items[Math.max(0, Math.min(items.length - 1, index))]?.key
      },
    })
    const preview = createPreview(context, {
      key: "preview",
      slot: "star",
      value: () => null,
      choose: (key, details) => rate(Number(key), details.event),
    })
    return {
      roving,
      preview,
      rate,
      keydown(event: KeyboardEvent, el: HTMLElement) {
        const moved = roving.keydown(event, el)
        if (moved) {
          change("value", Number(moved.key), {
            reason: "list-navigation",
            event,
          })
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot({
      attrs: (state) => ({
        role: "radiogroup",
        "data-previewing": state.preview === null ? undefined : "",
      }),
    }),
    star: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => {
        const index = Number(part.key)
        const shown =
          state.preview === null ? state.value : Number(state.preview)
        return {
          type: "button",
          role: "radio",
          "aria-checked": String(index === state.value),
          tabindex: part.api.roving.tabIndex(state, part),
          "data-filled": index <= shown ? "" : undefined,
          style: `--i: ${index}`,
        }
      },
      press: ({ api, key }, event) => {
        api.rate(Number(key), event)
      },
      on: {
        pointerenter: (event, { api, key }) => {
          if (event.pointerType === "mouse") {
            api.preview.show(key, { reason: "item-hover", event })
          }
        },
        pointerleave: (event, { api }) => {
          api.preview.clear({ reason: "item-hover", event })
        },
        keydown: (event, { el, api }) => {
          if (el) {
            api.keydown(event, el)
          }
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
  }),
  agent: { actions: ["rate"] },
})

const { Root: RatingRoot, Star: RatingStar } = createParts(rating, {
  root: "div",
  star: "button",
})

const VERDICTS = ["Not rated yet", "Poor", "Fair", "Good", "Great", "Wonderful"]

function Rating() {
  const [value, setValue] = useState(4)
  return (
    <div className="lg-rate">
      <p className="lg-rate-place">Ryokan Kikuya, Kanazawa</p>
      <RatingRoot
        value={value}
        onValueChange={setValue}
        aria-label="Rate your stay"
        className="lg-rate-stars"
      >
        {[1, 2, 3, 4, 5].map((star) => (
          <RatingStar
            key={star}
            value={String(star)}
            aria-label={`${star} ${star === 1 ? "star" : "stars"}, ${VERDICTS[star]}`}
            className="lg-rate-star"
          >
            <StarIcon size={30} weight="fill" aria-hidden />
          </RatingStar>
        ))}
      </RatingRoot>
      <p className="lg-rate-word" aria-live="polite">
        {VERDICTS[value]}
      </p>
    </div>
  )
}

/* 12. nearestInDirection: a map where focus stays put */

const PINS = [
  { name: "Anchorage", lon: -149.9, lat: 61.2, zone: "America/Anchorage" },
  { name: "Mexico City", lon: -99.1, lat: 19.4, zone: "America/Mexico_City" },
  { name: "Lima", lon: -77.0, lat: -12.0, zone: "America/Lima" },
  { name: "Dakar", lon: -17.4, lat: 14.7, zone: "Africa/Dakar" },
  { name: "Oslo", lon: 10.7, lat: 59.9, zone: "Europe/Oslo" },
  { name: "Nairobi", lon: 36.8, lat: -1.3, zone: "Africa/Nairobi" },
  { name: "Mumbai", lon: 72.9, lat: 19.1, zone: "Asia/Kolkata" },
  { name: "Seoul", lon: 127.0, lat: 37.6, zone: "Asia/Seoul" },
  { name: "Perth", lon: 115.9, lat: -32.0, zone: "Australia/Perth" },
].map((pin) => ({ ...pin, x: (pin.lon + 165) / 315, y: (70 - pin.lat) / 110 }))

const DIRECTIONS: Record<string, readonly [number, number]> = {
  ArrowRight: [1, 0],
  ArrowLeft: [-1, 0],
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
}

function CityMap() {
  const [current, setCurrent] = useState("Dakar")
  const id = useId()
  const now = useMinute()
  const pin = PINS.find((item) => item.name === current) ?? PINS[0]
  const clock = clockIn(pin.zone, now, "en-GB")
  const rect = (item: (typeof PINS)[number]) => {
    const x = item.x * 1000
    const y = item.y * 1000
    return { left: x, right: x, top: y, bottom: y }
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const direction = DIRECTIONS[event.key]
    if (!direction) {
      return
    }
    event.preventDefault()
    const next = nearestInDirection(
      rect(pin),
      PINS.filter((item) => item !== pin).map((item) => ({
        key: item.name,
        rect: rect(item),
      })),
      direction
    )
    if (next) {
      setCurrent(next)
    }
  }

  return (
    <div className="lg-map">
      <div
        className="lg-map-board"
        role="listbox"
        tabIndex={0}
        aria-label="World cities"
        aria-activedescendant={`${id}-${pin.name}`}
        onKeyDown={onKeyDown}
      >
        {PINS.map((item) => (
          <div
            key={item.name}
            id={`${id}-${item.name}`}
            role="option"
            aria-selected={item === pin}
            className="lg-map-pin"
            data-side={item.x > 0.62 ? "start" : undefined}
            style={{ left: `${item.x * 100}%`, top: `${item.y * 100}%` }}
            onClick={() => setCurrent(item.name)}
          >
            <span className="lg-map-dot" />
            <span className="lg-map-name">{item.name}</span>
          </div>
        ))}
      </div>
      <div className="lg-map-foot" aria-live="polite">
        <p className="lg-map-city">
          {pin.name} <span>{clock.time}</span>
        </p>
        <p className="lg-hint">
          <Kbd>←</Kbd>
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd>
          <Kbd>→</Kbd>
        </p>
      </div>
    </div>
  )
}

/* 13. spatialMove: a QWERTZ keyboard */

const KEY_ROWS = ["QWERTZUIOP", "ASDFGHJKLÖ", "YXCVBNM"]

const keyboard = definePrimitive({
  name: "lg-keyboard",
  state: () => ({ focused: null as string | null, typed: "Grüß" }),
  api: (context) => {
    const roving = createRovingFocus(context, {
      slot: "key",
      key: "focused",
      move: spatialMove(),
    })
    return {
      roving,
      press(key: string) {
        const { typed } = context.store.get()
        if (key === "back") {
          context.store.set({ typed: typed.slice(0, -1) })
        } else {
          context.store.set({
            typed: (typed.length >= 12 ? "" : typed) + key.toLowerCase(),
          })
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot<object, { typed: string }>({
      state: (state) => ({ typed: state.typed }),
    }),
    board: slot({ attrs: () => ({ role: "group" }) }),
    key: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        type: "button",
        tabindex: part.api.roving.tabIndex(state, part),
      }),
      press: ({ api, key }) => api.press(key),
      on: {
        keydown: (event, { el, api }) => {
          if (el) {
            api.roving.keydown(event, el)
          }
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
  }),
})

const {
  Root: KeysRoot,
  Board: KeysBoard,
  Key,
} = createParts(keyboard, { root: "div", board: "div", key: "button" })

function Keyboard() {
  return (
    <KeysRoot className="lg-keys">
      {({ typed }) => (
        <>
          <p className="lg-keys-typed" aria-live="polite">
            {typed}
            <span className="lg-keys-caret" aria-hidden="true" />
          </p>
          <KeysBoard className="lg-keys-board" aria-label="QWERTZ keyboard">
            {KEY_ROWS.map((row, index) => (
              <div key={row} className="lg-keys-row" data-row={index}>
                {[...row].map((letter) => (
                  <Key key={letter} value={letter} className="lg-key">
                    {letter}
                  </Key>
                ))}
                {index === 2 && (
                  <Key
                    value="back"
                    className="lg-key"
                    data-wide=""
                    aria-label="Backspace"
                  >
                    <BackspaceIcon size={14} weight="fill" aria-hidden />
                  </Key>
                )}
              </div>
            ))}
          </KeysBoard>
          <p className="lg-hint">
            Focus <Kbd>B</Kbd> and press <Kbd>↑</Kbd>
          </p>
        </>
      )}
    </KeysRoot>
  )
}

/* 14. Focus recovery: delete a stop */

const stopList = definePrimitive({
  name: "lg-stops",
  options: { onRemove: "callback" },
  state: (options: { onRemove?: (key: string) => void }) => ({
    focused: null as string | null,
    removable: options.onRemove !== undefined,
  }),
  sync: (options) => ({ removable: options.onRemove !== undefined }),
  api: (context) => {
    const roving = createRovingFocus(context, { slot: "stop", key: "focused" })
    const recovery = createFocusRecovery<{ value: string; index: number }>({
      fallback: (lost) => {
        const items = roving.items()
        return (items[lost.props.index] ?? items.at(-1))?.el
      },
    })
    return {
      roving,
      recovery,
      remove: (key: string) => context.options().onRemove?.(key),
      keydown(event: KeyboardEvent, el: HTMLElement, key: string) {
        if (event.key === "Delete" || event.key === "Backspace") {
          event.preventDefault()
          context.options().onRemove?.(key)
          return
        }
        roving.keydown(event, el)
      },
    }
  },
  slots: (slot) => ({
    root: slot({ attrs: () => ({ role: "listbox" }) }),
    stop: slot<{ value: string; index: number }>({
      props: { value: "string", index: "number" },
      attrs: (state, part) => ({
        role: "option",
        "aria-selected": "false",
        "aria-keyshortcuts": state.removable ? "Delete" : undefined,
        tabindex: part.api.roving.tabIndex(state, part),
      }),
      on: {
        keydown: (event, { el, api, key }) => {
          if (el) {
            api.keydown(event, el, key)
          }
        },
      },
      bind: (el, part) => {
        const unbindFocus = part.api.roving.bind(el, part)
        const unbindRecovery = part.api.recovery.bind(el, part)
        return () => {
          unbindFocus()
          unbindRecovery()
        }
      },
    }),
  }),
})

const { Root: StopsRoot, Stop } = createParts(stopList, {
  root: "div",
  stop: "div",
})

const STOPS = [
  { name: "Bruges", day: "Mon" },
  { name: "Ghent", day: "Tue" },
  { name: "Antwerp", day: "Thu" },
  { name: "Delft", day: "Fri" },
  { name: "Utrecht", day: "Sun" },
]

function Stops() {
  const [stops, setStops] = useState(STOPS)
  const remove = (name: string) =>
    setStops((list) => list.filter((stop) => stop.name !== name))
  return (
    <div className="lg-stops">
      <StopsRoot
        className="lg-stops-list"
        aria-label="Itinerary"
        onRemove={remove}
      >
        {stops.map((stop, index) => (
          <Stop
            key={stop.name}
            value={stop.name}
            index={index}
            className="lg-stops-item"
          >
            <span className="lg-stops-day">{stop.day}</span>
            <span className="lg-stops-name">{stop.name}</span>
            <span
              className="lg-stops-x"
              aria-hidden="true"
              onClick={() => remove(stop.name)}
            >
              <XIcon size={14} weight="bold" />
            </span>
          </Stop>
        ))}
      </StopsRoot>
      {stops.length ? (
        <p className="lg-hint">
          Focus a stop, press <Kbd>Delete</Kbd>
        </p>
      ) : (
        <button
          type="button"
          className="b-btn lg-stops-reset"
          onClick={() => setStops(STOPS)}
        >
          <ArrowCounterClockwiseIcon size={14} weight="bold" aria-hidden />
          Restore the route
        </button>
      )}
      {!!stops.length && stops.length < STOPS.length && (
        <button
          type="button"
          className="lg-stops-undo"
          onClick={() => setStops(STOPS)}
        >
          Restore
        </button>
      )}
    </div>
  )
}

/* 15. keepFocus: words move between lists */

const WORDS = [
  { word: "Obrigado", language: "Portuguese" },
  { word: "Kiitos", language: "Finnish" },
  { word: "Gracias", language: "Spanish" },
  { word: "Grazie", language: "Italian" },
  { word: "Takk", language: "Norwegian" },
]

function Words() {
  const [known, setKnown] = useState<string[]>(["Gracias", "Grazie"])
  const base = useId()
  const idOf = (word: string) => `${base}-${word}`

  function move(word: string) {
    setKnown((list) =>
      list.includes(word)
        ? list.filter((item) => item !== word)
        : [...list, word]
    )
    keepFocus(() => document.getElementById(idOf(word)) ?? undefined)
  }

  const column = (title: string, items: typeof WORDS) => (
    <section className="lg-words-col" aria-label={title}>
      <p className="lg-words-title">
        {title} <span>{items.length}</span>
      </p>
      <ul>
        {items.map(({ word, language }) => (
          <li key={word}>
            <button
              type="button"
              id={idOf(word)}
              className="lg-words-word"
              lang=""
              onClick={() => move(word)}
            >
              <span>{word}</span>
              <small>{language}</small>
            </button>
          </li>
        ))}
      </ul>
    </section>
  )

  return (
    <div className="lg-words">
      <div className="lg-words-cols">
        {column(
          "Learning",
          WORDS.filter(({ word }) => !known.includes(word))
        )}
        {column(
          "Known",
          WORDS.filter(({ word }) => known.includes(word))
        )}
      </div>
      <p className="lg-hint">
        <Kbd>Enter</Kbd> moves a word, focus follows
      </p>
    </div>
  )
}

/* 16. Shortcuts per platform */

const SHORTCUTS = [
  { keys: "mod+d", label: "Fill down" },
  { keys: "mod+;", label: "Insert today" },
  { keys: "mod+shift+l", label: "Toggle filters" },
]

function capsOf(shortcut: string, apple: boolean) {
  // The label is wrapped in direction isolates so it reads left to right; keycaps don't need them.
  const label = formatShortcut(shortcut, apple).replace(
    /[\u200e\u200f\u2066-\u2069]/g,
    ""
  )
  return (apple ? [...label] : label.split("+")).filter(Boolean)
}

function Shortcuts() {
  const [chosen, setApple] = useState<boolean | null>(null)
  const detected = useShortcutHint("mod+k").ariaKeyshortcuts.startsWith("Meta")
  const apple = chosen ?? detected
  const [hit, setHit] = useState<{ label: string; at: number } | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])

  function flash(label: string) {
    setHit({ label, at: Date.now() })
    clearTimeout(timer.current)
    timer.current = setTimeout(() => setHit(null), 900)
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const match = SHORTCUTS.find((item) =>
      matchesShortcut(event.nativeEvent, item.keys, apple)
    )
    if (!match) {
      return
    }
    event.preventDefault()
    flash(match.label)
  }

  return (
    <div className="lg-sc" onKeyDown={onKeyDown}>
      <div className="lg-sc-platforms" role="group" aria-label="Platform">
        <button
          type="button"
          className="lg-sc-platform"
          aria-pressed={apple}
          onClick={() => setApple(true)}
        >
          Mac
        </button>
        <button
          type="button"
          className="lg-sc-platform"
          aria-pressed={!apple}
          onClick={() => setApple(false)}
        >
          Windows
        </button>
      </div>
      <ul className="lg-sc-list">
        {SHORTCUTS.map((item) => (
          <li key={item.keys}>
            <button
              type="button"
              className="lg-sc-row"
              aria-keyshortcuts={ariaKeyshortcuts(item.keys, apple)}
              data-hit={hit?.label === item.label ? "" : undefined}
              onClick={() => flash(item.label)}
            >
              <span>{item.label}</span>
              <span className="lg-sc-caps" aria-hidden="true">
                {capsOf(item.keys, apple).map((cap, index) => (
                  <Kbd key={index}>{cap}</Kbd>
                ))}
              </span>
            </button>
          </li>
        ))}
      </ul>
      <p className="lg-hint" aria-live="polite">
        {hit ? `${hit.label} matched` : "Focus a row, then press its keys"}
      </p>
    </div>
  )
}

/* 17. announce, with lines that animate in and out */

const RATE = 10.38
const AMOUNTS = [50, 100, 250]

function LogLine({ text }: { text: string }) {
  const ref = useRef<HTMLLIElement>(null)
  useLayoutEffect(() => (ref.current ? animateIn(ref.current) : undefined), [])
  return (
    <li ref={ref} className="lg-say-line">
      {text}
    </li>
  )
}

function Announce() {
  const [fromPounds, setFromPounds] = useState(true)
  const [amount, setAmount] = useState(100)
  const [log, setLog] = useState<{ id: number; text: string }[]>([])
  const logRef = useRef<HTMLUListElement>(null)
  const nextId = useRef(0)

  const from = fromPounds ? "GBP" : "HKD"
  const to = fromPounds ? "HKD" : "GBP"
  const money = (value: number, currency: string) =>
    formatNumber(value, "en-GB", {
      style: "currency",
      currency,
      maximumFractionDigits: 0,
    })
  const converted = (value: number, pounds: boolean) =>
    pounds ? value * RATE : value / RATE

  function say(value: number, pounds: boolean) {
    const text = `${money(value, pounds ? "GBP" : "HKD")} is about ${money(converted(value, pounds), pounds ? "HKD" : "GBP")}`
    announce(text)
    nextId.current += 1
    setLog((lines) => [...lines, { id: nextId.current, text }].slice(-2))
  }

  function clear() {
    const lines = [...(logRef.current?.children ?? [])] as HTMLElement[]
    let left = lines.length
    if (!left) {
      return
    }
    for (const line of lines) {
      animateOut(line, () => {
        left -= 1
        if (!left) {
          setLog([])
        }
      })
    }
  }

  return (
    <div className="lg-say">
      <p className="lg-say-value">
        {money(converted(amount, fromPounds), to)}
        <span>from {money(amount, from)}</span>
      </p>
      <div className="lg-say-controls">
        {AMOUNTS.map((value) => (
          <button
            key={value}
            type="button"
            className="lg-say-amount"
            aria-pressed={value === amount}
            onClick={() => {
              setAmount(value)
              say(value, fromPounds)
            }}
          >
            {money(value, from)}
          </button>
        ))}
        <button
          type="button"
          className="lg-say-swap"
          aria-label={`Convert from ${to} instead`}
          onClick={() => {
            setFromPounds(!fromPounds)
            say(amount, !fromPounds)
          }}
        >
          <ArrowsLeftRightIcon size={14} weight="bold" aria-hidden />
        </button>
      </div>
      <div className="lg-say-log">
        <p className="lg-say-head">
          <span>Screen reader hears</span>
          {!!log.length && (
            <button type="button" className="lg-say-clear" onClick={clear}>
              Clear
            </button>
          )}
        </p>
        <ul ref={logRef}>
          {log.map((line) => (
            <LogLine key={line.id} text={line.text} />
          ))}
          {!log.length && (
            <li className="lg-say-empty">Press an amount to hear it</li>
          )}
        </ul>
      </div>
    </div>
  )
}

/* 18. Direction and page language: a phrasebook */

const PHRASES = [
  {
    tag: "ar_EG",
    name: "العربية",
    text: "أين المحطة؟",
    dir: "rtl",
    currency: "EGP",
    fare: 30,
  },
  {
    tag: "pt_BR",
    name: "Português",
    text: "Onde fica a estação?",
    dir: "ltr",
    currency: "BRL",
    fare: 5.2,
  },
  {
    tag: "he_IL",
    name: "עברית",
    text: "איפה התחנה?",
    dir: "rtl",
    currency: "ILS",
    fare: 8.5,
  },
  {
    tag: "ja_JP",
    name: "日本語",
    text: "駅はどこですか？",
    dir: "ltr",
    currency: "JPY",
    fare: 210,
  },
  {
    tag: "en_GB",
    name: "English",
    text: "Where is the station?",
    dir: "ltr",
    currency: "GBP",
    fare: 2.8,
  },
]

function Phrasebook() {
  const [index, setIndex] = useState(0)
  const card = useRef<HTMLDivElement>(null)
  const fare = useRef<HTMLSpanElement>(null)
  const phrase = PHRASES[index]
  const locale = toLocale(phrase.tag)
  const go = (by: number) =>
    setIndex((current) => (current + by + PHRASES.length) % PHRASES.length)

  // pageLocale reads the nearest lang attribute, so the fare follows the card's language.
  useLayoutEffect(() => {
    if (!fare.current) {
      return
    }
    fare.current.textContent = formatNumber(
      phrase.fare,
      pageLocale(fare.current),
      {
        style: "currency",
        currency: phrase.currency,
      }
    )
  }, [phrase])

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
      return
    }
    event.preventDefault()
    const forward = isRTL(event.currentTarget) ? "ArrowLeft" : "ArrowRight"
    go(event.key === forward ? 1 : -1)
  }

  return (
    <div className="lg-phrase">
      <div
        ref={card}
        className="lg-phrase-card"
        lang={locale}
        dir={phrase.dir}
        tabIndex={0}
        role="group"
        aria-roledescription="phrase"
        aria-label={`Phrase ${index + 1} of ${PHRASES.length}`}
        onKeyDown={onKeyDown}
      >
        <p className="lg-phrase-lang">{phrase.name}</p>
        <p className="lg-phrase-text">{phrase.text}</p>
        <p className="lg-phrase-fare">
          <span ref={fare} />
        </p>
      </div>
      <div className="lg-phrase-nav">
        <button
          type="button"
          className="lg-phrase-btn"
          aria-label="Previous phrase"
          onClick={() => go(-1)}
        >
          <CaretLeftIcon size={14} weight="bold" aria-hidden />
        </button>
        <p className="lg-phrase-tag">
          <code>{phrase.tag}</code> <span aria-hidden="true">→</span>{" "}
          <code>{locale}</code>
          <span className="lg-phrase-dir">
            {phrase.dir === "rtl" ? "RTL" : "LTR"}
          </span>
        </p>
        <button
          type="button"
          className="lg-phrase-btn"
          aria-label="Next phrase"
          onClick={() => go(1)}
        >
          <CaretRightIcon size={14} weight="bold" aria-hidden />
        </button>
      </div>
    </div>
  )
}

/* 19. A light, dark or system scope, remembered on this device */

type SchemeReason = "item-press" | "list-navigation"
type Scheme = "light" | "dark" | "system"

const schemePicker = definePrimitive({
  name: "lg-scheme",
  options: {
    value: "string",
    defaultValue: "string",
    onValueChange: "callback",
  },
  controlled: ["value"],
  state: (options: {
    value?: string
    defaultValue?: string
    onValueChange?: (
      value: string,
      details: ChangeDetails<SchemeReason>
    ) => void
  }) => ({
    value: options.value ?? options.defaultValue ?? "system",
    focused: null as string | null,
  }),
  sync: (options) =>
    options.value === undefined ? {} : { value: options.value },
  api: (context) => {
    const change = createChange(context)
    const roving = createRovingFocus(context, {
      slot: "option",
      key: "focused",
      orientation: () => "horizontal",
      loop: () => true,
      remember: false,
      preferred: (item) => item.key === context.store.get().value,
      claims: (state, item) => item.key === state.value,
    })
    return {
      roving,
      pick: (value: string, event?: Event) =>
        change("value", value, { reason: "item-press", event }),
      keydown(event: KeyboardEvent, el: HTMLElement) {
        const moved = roving.keydown(event, el)
        if (moved) {
          change("value", moved.key, { reason: "list-navigation", event })
        }
      },
    }
  },
  slots: (slot) => ({
    root: slot({ attrs: () => ({ role: "radiogroup" }) }),
    option: slot<{ value: string }>({
      props: { value: "string" },
      attrs: (state, part) => ({
        type: "button",
        role: "radio",
        "aria-checked": String(state.value === part.key),
        "data-checked": state.value === part.key ? "" : undefined,
        tabindex: part.api.roving.tabIndex(state, part),
      }),
      press: ({ api, key }, event) => {
        api.pick(key, event)
      },
      on: {
        keydown: (event, { el, api }) => {
          if (el) {
            api.keydown(event, el)
          }
        },
      },
      bind: (el, part) => part.api.roving.bind(el, part),
    }),
  }),
})

const { Root: SchemeRoot, Option: SchemeOption } = createParts(schemePicker, {
  root: "div",
  option: "button",
})

const SCHEME_KEY = "lg-scheme"
const SCHEMES = [
  { value: "light", label: "Light", icon: SunIcon },
  { value: "dark", label: "Dark", icon: MoonIcon },
  { value: "system", label: "Auto", icon: DesktopIcon },
] as const

function readScheme(): Scheme {
  try {
    const saved = localStorage.getItem(SCHEME_KEY)
    return saved === "light" || saved === "dark" || saved === "system"
      ? saved
      : "dark"
  } catch {
    return "dark"
  }
}

function SchemeScope() {
  const [scheme, setScheme] = useState<Scheme>(readScheme)
  const systemDark = useMedia("(prefers-color-scheme: dark)")
  const resolved =
    scheme === "system" ? (systemDark ? "dark" : "light") : scheme

  function choose(next: string) {
    setScheme(next as Scheme)
    try {
      localStorage.setItem(SCHEME_KEY, next)
    } catch {
      // Storage can be unavailable, like in a private window. The choice still applies.
    }
  }

  return (
    <div className="lg-scheme" data-scheme={resolved}>
      <div className="lg-scheme-ticket">
        <p className="lg-scheme-train">Nightjet 466</p>
        <p className="lg-scheme-route">Wien Hbf → Venezia</p>
        <p className="lg-scheme-meta">Couchette 4 · 21:27</p>
      </div>
      <SchemeRoot
        value={scheme}
        onValueChange={choose}
        aria-label="Color scheme"
        className="lg-scheme-picker"
      >
        {SCHEMES.map(({ value, label, icon: Icon }) => (
          <SchemeOption key={value} value={value} className="lg-scheme-option">
            <Icon size={14} weight="fill" aria-hidden />
            {label}
          </SchemeOption>
        ))}
      </SchemeRoot>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "language-city-search",
    title: "Find any city.",
    blurb: "Ranked as you type, matches marked, accents ignored.",
    size: "large",
    Demo: CitySearch,
  },
  {
    id: "language-typeahead",
    title: "Type to jump.",
    blurb: "Letters find the currency.",
    tone: "paper",
    Demo: Typeahead,
  },
  {
    id: "language-number-format",
    title: "Pick a format.",
    blurb: "Arrows move, Tab leaves the group.",
    Demo: NumberFormats,
  },
  {
    id: "language-seats",
    title: "Choose a seat.",
    blurb: "A keyboard grid that skips taken seats.",
    size: "tall",
    tone: "blueprint",
    Demo: Seats,
  },
  {
    id: "language-sheet",
    title: "Copy a block.",
    blurb: "Shift-select cells, paste back as rows.",
    size: "wide",
    Demo: Sheet,
  },
  {
    id: "language-stay",
    title: "Book the stay.",
    blurb: "A two-month range with a live night count.",
    size: "wide",
    tone: "sky",
    Demo: Stay,
  },
  {
    id: "language-departure",
    title: "Type or pick.",
    blurb: "Swiss date order, calendar on demand.",
    Demo: Departure,
  },
  {
    id: "language-parse-date",
    title: "Same digits, new date.",
    blurb: "Each locale reads its own order.",
    tone: "paper",
    Demo: ParseDate,
  },
  {
    id: "language-weeks",
    title: "Weeks start elsewhere.",
    blurb: "One month, three local grids.",
    tone: "sage",
    Demo: Weeks,
  },
  {
    id: "language-nights",
    title: "Count the days.",
    blurb: "A stepper primitive doing date math.",
    tone: "signal",
    Demo: Nights,
  },
  {
    id: "language-rating",
    title: "Rate the ryokan.",
    blurb: "Stars built from scratch.",
    tone: "butter",
    Demo: Rating,
  },
  {
    id: "language-map",
    title: "Arrows follow the map.",
    blurb: "The nearest city that way wins.",
    tone: "paper",
    Demo: CityMap,
  },
  {
    id: "language-keyboard",
    title: "Keys by position.",
    blurb: "Up from B lands where it looks.",
    tone: "ink",
    Demo: Keyboard,
  },
  {
    id: "language-stops",
    title: "Delete, stay put.",
    blurb: "Focus lands on the next stop.",
    Demo: Stops,
  },
  {
    id: "language-words",
    title: "Focus follows.",
    blurb: "Move a word, keep your place.",
    tone: "paper",
    Demo: Words,
  },
  {
    id: "language-shortcuts",
    title: "Shortcuts per platform.",
    blurb: "⌘ on a Mac, Ctrl elsewhere.",
    Demo: Shortcuts,
  },
  {
    id: "language-announce",
    title: "Heard, not seen.",
    blurb: "What a screen reader is told.",
    tone: "ink",
    Demo: Announce,
  },
  {
    id: "language-phrasebook",
    title: "Read right to left.",
    blurb: "Arrows follow each language's direction.",
    tone: "paper",
    Demo: Phrasebook,
  },
  {
    id: "language-scheme",
    title: "Light, dark or auto.",
    blurb: "One tile's scheme, remembered here.",
    Demo: SchemeScope,
  },
]
