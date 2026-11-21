import {
  ArrowCounterClockwiseIcon,
  CaretDownIcon,
  CheckIcon,
  ClipboardTextIcon,
  PlayIcon,
  RobotIcon,
  XIcon,
} from "@phosphor-icons/react"
import { isRTL } from "@ui-sdk/core"
import { createAgent, type AgentComponent } from "@ui-sdk/core/agent"
import {
  addDays,
  addMonths,
  addYears,
  clampDate,
  daysBetween,
  endOfMonth,
  endOfWeek,
  firstDayOfWeek,
  formatDate,
  formatDateRange,
  isSameMonth,
  monthGrid,
  startOfMonth,
  startOfWeek,
  todayDate,
  weekdayNames,
} from "@ui-sdk/core/date"
import {
  decimalsOf,
  formatNumber,
  formatNumberRange,
  formatNumberToParts,
  fractionOf,
  isPartialNumber,
  parseNumber,
  roundTo,
  scrubTo,
  stepBy,
  stepFromKey,
  valueAt,
  wrap,
} from "@ui-sdk/core/number"
import { elementOf } from "@ui-sdk/core/observe"
import { Button } from "@ui-sdk/react/button"
import { Field, FieldLabel } from "@ui-sdk/react/field"
import { Input } from "@ui-sdk/react/input"
import { Radio, RadioGroup, RadioIndicator } from "@ui-sdk/react/radio"
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from "@ui-sdk/react/segmented-control"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectItemIndicator,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@ui-sdk/react/select"
import { Switch, SwitchThumb } from "@ui-sdk/react/switch"
import {
  createTokenInputController,
  TokenInput,
  TokenInputEditor,
  TokenInputToken,
  TokenInputTokenRemove,
  fromText,
  toText,
  type TokenCodec,
  type TokenInputValue,
} from "@ui-sdk/react/token-input"
import {
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react"
import type { Tile } from "../bento/types"
import "./locale.css"

function LocaleSwitch({
  locales,
  value,
  onChange,
}: {
  locales: readonly string[]
  value: string
  onChange: (locale: string) => void
}) {
  return (
    <SegmentedControl
      aria-label="Locale"
      className="lc-locales"
      value={value}
      onValueChange={(next) => next && onChange(next)}
    >
      <SegmentedControlIndicator className="lc-locales-indicator" />
      {locales.map((tag) => (
        <SegmentedControlItem key={tag} value={tag} className="lc-locale">
          {tag}
        </SegmentedControlItem>
      ))}
    </SegmentedControl>
  )
}

/* 1. An amount typed the local way */

const AMOUNT_LOCALES = ["en-US", "de-DE", "fr-FR", "hi-IN"] as const
const CURRENCY_OF: Record<string, string> = {
  "en-US": "USD",
  "de-DE": "EUR",
  "fr-FR": "EUR",
  "hi-IN": "INR",
}

function separatorsOf(locale: string) {
  const parts = formatNumberToParts(1234567.5, locale)
  const shown = (type: string) => {
    const value = parts.find((part) => part.type === type)?.value ?? ""
    return value.trim() ? value : "space"
  }
  return { group: shown("group"), decimal: shown("decimal") }
}

function Amount() {
  const [locale, setLocale] = useState("de-DE")
  const [text, setText] = useState("1.234,5")
  const [rejected, setRejected] = useState<string | null>(null)
  const value = parseNumber(text, locale)
  const { group, decimal } = separatorsOf(locale)
  const currency = CURRENCY_OF[locale]
  const id = useId()

  return (
    <div className="lc-amount">
      <LocaleSwitch
        locales={AMOUNT_LOCALES}
        value={locale}
        onChange={(next) => {
          setLocale(next)
          setRejected(null)
          if (value !== null) {
            setText(formatNumber(value, next, { maximumFractionDigits: 2 }))
          }
        }}
      />
      <label className="lc-amount-field" data-rejected={rejected ?? undefined}>
        <span className="b-label">Transfer amount</span>
        <span className="lc-amount-box">
          <input
            className="lc-amount-input"
            inputMode="decimal"
            value={text}
            spellCheck={false}
            autoComplete="off"
            aria-describedby={`${id}-out`}
            onChange={(event) => {
              const next = event.target.value
              if (isPartialNumber(next, locale, {}, { allowNegative: false })) {
                setText(next)
                setRejected(null)
              } else {
                setRejected(next.slice(-1) || null)
              }
            }}
            onBlur={() => {
              if (value !== null) {
                setText(
                  formatNumber(value, locale, { maximumFractionDigits: 2 })
                )
              }
            }}
          />
          <span className="lc-amount-code">{currency}</span>
        </span>
      </label>
      <p className="lc-amount-seps">
        {rejected ? (
          <span>
            “{rejected}” has no place in a {locale} number
          </span>
        ) : (
          <>
            Groups <kbd>{group}</kbd> Decimal <kbd>{decimal}</kbd>
          </>
        )}
      </p>
      <p className="lc-amount-out" id={`${id}-out`} aria-live="polite">
        <code>{value ?? "—"}</code>
        <span>
          {value === null
            ? "Keep typing"
            : formatNumber(value, locale, { style: "currency", currency })}
        </span>
      </p>
    </div>
  )
}

/* 2. A price styled from its parts */

const PRICES = [
  { locale: "en-US", currency: "USD", night: 1249.99, low: 120, high: 340 },
  { locale: "de-DE", currency: "EUR", night: 1149.5, low: 110, high: 315 },
  { locale: "ja-JP", currency: "JPY", night: 186400, low: 17900, high: 50800 },
  { locale: "hi-IN", currency: "INR", night: 104250, low: 9990, high: 28400 },
]

function Price() {
  const [locale, setLocale] = useState("en-US")
  const price = PRICES.find((entry) => entry.locale === locale) ?? PRICES[0]
  const format = { style: "currency", currency: price.currency } as const
  const parts = formatNumberToParts(price.night, locale, format)
  let afterDecimal = false
  return (
    <div className="lc-price">
      <LocaleSwitch
        locales={PRICES.map((entry) => entry.locale)}
        value={locale}
        onChange={setLocale}
      />
      <div className="lc-price-card">
        <p className="lc-price-room">Ocean suite</p>
        <p
          className="lc-price-amount"
          aria-label={formatNumber(price.night, locale, format)}
        >
          {parts.map((part, index) => {
            afterDecimal ||= part.type === "decimal"
            return (
              <span
                key={index}
                aria-hidden
                data-part={part.type}
                data-small={
                  afterDecimal || part.type === "currency" ? "" : undefined
                }
              >
                {part.value}
              </span>
            )
          })}
          <span className="lc-price-per">/ night</span>
        </p>
        <p className="lc-price-range">
          Other rooms{" "}
          <strong>
            {formatNumberRange(price.low, price.high, locale, {
              ...format,
              maximumFractionDigits: 0,
            })}
          </strong>
        </p>
      </div>
    </div>
  )
}

/* 3. A dial that snaps to quarter hours */

const HOURS = { min: 0, max: 8, step: 0.25, largeStep: 1 }
const SWEEP = 270
const TICKS = Array.from(
  { length: HOURS.max / HOURS.step + 1 },
  (_, index) => index * HOURS.step
)

function Hours() {
  const [value, setValue] = useState(6.75)
  const knob = useRef<HTMLDivElement>(null)
  const turn = fractionOf(value, HOURS) * SWEEP - SWEEP / 2
  const hours = Math.floor(value)
  const minutes = roundTo((value - hours) * 60, 0)

  function moveTo(event: ReactPointerEvent<HTMLDivElement>) {
    const box = event.currentTarget.getBoundingClientRect()
    const x = event.clientX - box.left - box.width / 2
    const y = event.clientY - box.top - box.height / 2
    const angle = wrap((Math.atan2(x, -y) * 180) / Math.PI, -180, 180)
    if (Math.abs(angle) > SWEEP / 2) {
      return
    }
    const raw = valueAt(angle / SWEEP + 0.5, HOURS)
    setValue(scrubTo(raw, HOURS).value)
  }

  return (
    <div className="lc-hours">
      <p className="lc-hours-task">
        Lantern redesign <span>Tue</span>
      </p>
      <div
        ref={knob}
        className="lc-dial"
        role="slider"
        tabIndex={0}
        aria-label="Hours on Lantern redesign"
        aria-valuemin={HOURS.min}
        aria-valuemax={HOURS.max}
        aria-valuenow={value}
        aria-valuetext={`${hours} hours ${minutes} minutes`}
        style={{ "--turn": `${turn}deg` } as CSSProperties}
        onKeyDown={(event) => {
          const move = stepFromKey(event, {
            dir: isRTL(knob.current) ? "rtl" : "ltr",
          })
          if (move) {
            event.preventDefault()
            setValue(stepBy(value, move, HOURS))
          }
        }}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId)
          moveTo(event)
        }}
        onPointerMove={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            moveTo(event)
          }
        }}
      >
        {TICKS.map((tick) => (
          <span
            key={tick}
            className="lc-dial-tick"
            data-hour={Number.isInteger(tick) ? "" : undefined}
            data-on={tick <= value ? "" : undefined}
            style={
              {
                "--at": `${fractionOf(tick, HOURS) * SWEEP - SWEEP / 2}deg`,
              } as CSSProperties
            }
          />
        ))}
        <span className="lc-dial-pointer" aria-hidden />
        <span className="lc-dial-face" aria-hidden>
          <span className="lc-dial-read">
            {hours}
            <small>h</small>
            {String(minutes).padStart(2, "0")}
            <small>m</small>
          </span>
        </span>
      </div>
      <p className="lc-hours-note">
        {value.toFixed(decimalsOf(HOURS.step))} of 8.00 h
      </p>
    </div>
  )
}

/* 4. Quick date ranges */

const RANGE_LOCALES = ["en-US", "en-GB", "ar-EG"] as const

const PRESETS = [
  {
    label: "This week",
    range: (today: string, first: number) => [
      clampDate(startOfWeek(today, first), today),
      endOfWeek(today, first),
    ],
  },
  {
    label: "Next week",
    range: (today: string, first: number) => [
      startOfWeek(addDays(today, 7), first),
      endOfWeek(addDays(today, 7), first),
    ],
  },
  {
    label: "This month",
    range: (today: string) => [today, endOfMonth(today)],
  },
  {
    label: "Next month",
    range: (today: string) => [
      startOfMonth(addMonths(today, 1)),
      endOfMonth(addMonths(today, 1)),
    ],
  },
  {
    label: "A year out",
    range: (today: string, first: number) => [
      startOfWeek(addYears(today, 1), first),
      endOfWeek(addYears(today, 1), first),
    ],
  },
]

const subscribeNever = () => () => {}
const noDate = () => null

function useToday() {
  return useSyncExternalStore(subscribeNever, todayDate, noDate)
}

function Ranges() {
  const [locale, setLocale] = useState("en-GB")
  const [preset, setPreset] = useState("Next week")
  const today = useToday()
  const first = firstDayOfWeek(locale)
  const rtl = locale.startsWith("ar")
  const chosen = PRESETS.find((entry) => entry.label === preset) ?? PRESETS[0]
  const [start, end] = today ? chosen.range(today, first) : [null, null]
  const weeks = start
    ? monthGrid(start, { weekStartsOn: first, fixedWeeks: true })
    : []

  return (
    <div className="lc-ranges">
      <LocaleSwitch
        locales={RANGE_LOCALES}
        value={locale}
        onChange={setLocale}
      />
      <div className="lc-chips" role="group" aria-label="Out of office">
        {PRESETS.map((entry) => (
          <button
            key={entry.label}
            type="button"
            className="lc-chip"
            aria-pressed={entry.label === preset}
            onClick={() => setPreset(entry.label)}
          >
            {entry.label}
          </button>
        ))}
      </div>
      <div className="lc-ranges-out" aria-live="polite">
        <p className="lc-ranges-dates" dir={rtl ? "rtl" : undefined}>
          {start && end
            ? formatDateRange(start, end, locale, {
                weekday: "short",
                day: "numeric",
                month: "short",
              })
            : " "}
        </p>
        <p className="lc-ranges-meta">
          {start && end && today
            ? [
                `${daysBetween(start, end) + 1} days`,
                isSameMonth(start, end)
                  ? formatDate(start, locale, { month: "long" })
                  : "Across two months",
                start === today ? "From today" : null,
              ]
                .filter(Boolean)
                .join(" · ")
            : " "}
        </p>
      </div>
      <div className="lc-month" dir={rtl ? "rtl" : "ltr"} aria-hidden>
        {weekdayNames(locale, "narrow", first).map((name, index) => (
          <span key={index} className="lc-month-weekday">
            {name}
          </span>
        ))}
        {weeks.flat().map((day) => (
          <span
            key={day}
            className="lc-month-day"
            data-outside={start && !isSameMonth(day, start) ? "" : undefined}
            data-in={
              start && end && day >= start && day <= end ? "" : undefined
            }
            data-edge={day === start || day === end ? "" : undefined}
            data-today={day === today ? "" : undefined}
          >
            {formatDate(day, locale, { day: "numeric" })}
          </span>
        ))}
      </div>
    </div>
  )
}

/* 5. Pasted emails become tokens */

const EMAIL = /^(?:"?([^"<]*?)"?\s*<)?([^\s<>@,;]+@[^\s<>@,;]+\.[a-z]{2,})>?$/i

const emailCodec: TokenCodec = {
  format: (token) =>
    token.label === token.value
      ? token.value
      : `${token.label} <${token.value}>`,
  escape: (text, next) => (text.trim() ? text : next ? ", " : ""),
  parse(text) {
    const value: TokenInputValue = []
    for (const piece of text.split(/[,;\n\t]+/)) {
      const trimmed = piece.trim()
      const match = EMAIL.exec(trimmed)
      if (match) {
        const [, name, address] = match
        value.push(
          { type: "email", value: address, label: name?.trim() || address },
          " "
        )
      } else if (trimmed) {
        value.push(`${trimmed} `)
      }
    }
    return value
  },
}

const SHEET = `Grace Hopper <grace@navy.mil>
Katherine Johnson <kj@nasa.gov>, alan@bletchley.uk
not-an-email; margaret@apollo.org`

function Recipients() {
  const [recipients] = useState(createTokenInputController)
  const [value, setValue] = useState<TokenInputValue>(() =>
    fromText("Ada Lovelace <ada@engine.org>", emailCodec)
  )
  const tokens = value.filter((part) => typeof part !== "string")
  const stray = value
    .filter((part) => typeof part === "string")
    .join("")
    .trim()

  return (
    <div className="lc-mail">
      <div className="lc-mail-sheet">
        <pre className="lc-mail-text" dir="ltr">
          {SHEET}
        </pre>
        <button
          type="button"
          className="b-btn lc-mail-paste"
          onMouseDown={(event) => event.preventDefault()}
          onClick={() =>
            recipients.insert(fromText(SHEET, emailCodec), { at: "end" })
          }
        >
          <ClipboardTextIcon size={14} weight="bold" aria-hidden />
          Paste these
        </button>
      </div>
      <TokenInput
        controller={recipients}
        value={value}
        onValueChange={setValue}
        codec={emailCodec}
        renderToken={(token) => (
          <TokenInputToken className="lc-mail-token" title={token.value}>
            {token.label}
            <TokenInputTokenRemove className="lc-mail-remove">
              <XIcon size={11} weight="bold" aria-hidden />
            </TokenInputTokenRemove>
          </TokenInputToken>
        )}
      >
        <div className="lc-mail-box">
          <span className="lc-mail-to" aria-hidden>
            To
          </span>
          <TokenInputEditor
            aria-label="Recipients"
            placeholder="Paste names and emails"
            className="lc-mail-editor"
          />
        </div>
      </TokenInput>
      <p className="lc-mail-foot" aria-live="polite">
        <span>
          {tokens.length} {tokens.length === 1 ? "recipient" : "recipients"}
          {stray && <em> · “{stray}” isn’t an email</em>}
        </span>
        <button
          type="button"
          className="lc-mail-clear"
          onClick={() => recipients.clear()}
          disabled={!value.length}
        >
          Clear
        </button>
      </p>
      <p className="lc-mail-sends" dir="ltr">
        {toText(value, emailCodec) || " "}
      </p>
    </div>
  )
}

/* 6. An agent fills the form through snapshot and act */

const TEAMS = [
  { value: "research", label: "Research" },
  { value: "design", label: "Design" },
  { value: "platform", label: "Platform" },
  { value: "growth", label: "Growth" },
]

const ROLES = [
  { value: "viewer", label: "Viewer" },
  { value: "editor", label: "Editor" },
  { value: "admin", label: "Admin" },
]

const TASK = "Invite ada@engine.org to Research as an editor, with a welcome."

const SCRIPT = [
  { label: "Teammate email", action: "setValue", args: ["ada@engine.org"] },
  { label: "Team", action: "setValue", args: ["Research"] },
  { label: "Role", action: "setValue", args: ["Editor"] },
  { label: "Send welcome email", action: "setChecked", args: [true] },
  { label: "Send invite", action: "press", args: [] },
]

type LogLine = {
  kind: "task" | "call" | "seen" | "done" | "error"
  text: string
}

let pageAgent: ReturnType<typeof createAgent> | undefined
const agentOf = () => (pageAgent ??= createAgent())
const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))
const quoted = (value: unknown) =>
  typeof value === "string" ? `"${value}"` : String(value)

function snapshotLine(component: AgentComponent) {
  const state =
    "checked" in component
      ? quoted(component.checked)
      : "value" in component
        ? quoted(component.value)
        : null
  return [component.ref, component.component, quoted(component.label), state]
    .filter(Boolean)
    .join(" ")
}

function AgentFill() {
  const form = useRef<HTMLFormElement>(null)
  const logEnd = useRef<HTMLOListElement>(null)
  const run = useRef(0)
  const [formKey, setFormKey] = useState(0)
  const [log, setLog] = useState<LogLine[]>([])
  const [target, setTarget] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const [sent, setSent] = useState<string | null>(null)
  const id = useId()

  useEffect(() => {
    const id = run
    return () => {
      id.current++
    }
  }, [])

  useEffect(() => {
    const list = logEnd.current
    if (list) {
      list.scrollTop = list.scrollHeight
    }
  }, [log])

  async function start() {
    const id = ++run.current
    const say = (line: LogLine) => setLog((lines) => [...lines, line])
    const stale = () => run.current !== id
    setRunning(true)
    setSent(null)
    setLog([{ kind: "task", text: TASK }])
    setFormKey((key) => key + 1)
    await pause(500)
    const agent = agentOf()
    say({ kind: "call", text: "snapshot()" })
    await pause(350)
    const mine = new Map<string, AgentComponent>()
    const keep = (found: AgentComponent[]) => {
      for (const component of found) {
        const el = elementOf(component.ref)
        if (el && form.current?.contains(el) && component.label) {
          mine.set(component.label, component)
          say({ kind: "seen", text: snapshotLine(component) })
        }
      }
    }
    const page = agent.snapshot()
    keep(page)
    const more = page.find((component) => component.ref === "more")
    if (more) {
      say({ kind: "seen", text: `…${more.more} more on the page` })
    }
    for (const { label } of SCRIPT) {
      if (!mine.has(label)) {
        say({ kind: "call", text: `query({ text: ${quoted(label)} })` })
        const found = await agent.query({ text: label })
        keep(Array.isArray(found) ? found : [])
      }
    }
    let completed = true
    for (const step of SCRIPT) {
      await pause(700)
      if (stale()) {
        return
      }
      const ref = mine.get(step.label)?.ref ?? step.label
      setTarget(step.label)
      say({
        kind: "call",
        text: `act ${ref}.${step.action}(${step.args.map(quoted).join(", ")})`,
      })
      const result = await agent.act({
        steps: [{ ref, action: step.action, args: step.args }],
      })
      if (!result.ok) {
        say({ kind: "error", text: result.error?.message ?? "Failed" })
        completed = false
        break
      }
      const changes = result.steps[0]?.changes ?? []
      say({
        kind: "seen",
        text: changes.length
          ? changes
              .map((change) => `${change.key} → ${quoted(change.value)}`)
              .join(", ")
          : "pressed",
      })
    }
    await pause(500)
    if (stale()) {
      return
    }
    if (completed) {
      say({ kind: "done", text: "Done. The invite is on its way." })
    }
    setTarget(null)
    setRunning(false)
  }

  return (
    <div className="lc-agent">
      <form
        key={formKey}
        ref={form}
        className="lc-agent-form"
        onSubmit={(event) => event.preventDefault()}
      >
        <p className="lc-agent-title">Invite a teammate</p>
        <Field
          name="email"
          className="lc-agent-field"
          data-target={target === "Teammate email" ? "" : undefined}
        >
          <FieldLabel className="b-label">Teammate email</FieldLabel>
          <Input
            className="b-input"
            type="email"
            placeholder="name@company.com"
            autoComplete="off"
          />
        </Field>
        <div
          className="lc-agent-field"
          data-target={target === "Team" ? "" : undefined}
        >
          <Select name="team" items={TEAMS} placeholder="Choose a team">
            <SelectLabel className="b-label">Team</SelectLabel>
            <SelectTrigger className="b-input lc-agent-trigger">
              <SelectValue />
              <CaretDownIcon size={14} weight="bold" aria-hidden />
            </SelectTrigger>
            <SelectContent className="b-pop lc-agent-pop" sideOffset={6}>
              {TEAMS.map((team) => (
                <SelectItem
                  key={team.value}
                  value={team.value}
                  className="b-item lc-agent-option"
                >
                  {team.label}
                  <SelectItemIndicator className="lc-agent-check">
                    <CheckIcon size={14} weight="bold" />
                  </SelectItemIndicator>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div
          className="lc-agent-field"
          data-target={target === "Role" ? "" : undefined}
        >
          <span className="b-label" id={`${id}-role`}>
            Role
          </span>
          <RadioGroup
            name="role"
            defaultValue="viewer"
            aria-labelledby={`${id}-role`}
            className="lc-agent-roles"
          >
            {ROLES.map((role) => (
              <label key={role.value} className="lc-agent-role">
                <Radio value={role.value} className="lc-agent-radio">
                  <RadioIndicator className="lc-agent-radio-dot" />
                </Radio>
                {role.label}
              </label>
            ))}
          </RadioGroup>
        </div>
        <label
          className="lc-agent-field lc-agent-welcome"
          data-target={target === "Send welcome email" ? "" : undefined}
        >
          Send welcome email
          <Switch name="welcome" className="lc-agent-switch">
            <SwitchThumb className="lc-agent-thumb" />
          </Switch>
        </label>
        <div
          className="lc-agent-submit"
          data-target={target === "Send invite" ? "" : undefined}
        >
          <Button
            className="b-btn"
            data-variant="primary"
            onClick={() => {
              const data = new FormData(form.current ?? undefined)
              const email = String(data.get("email") ?? "")
              setSent(email ? `Sent to ${email}` : "Add an email first")
            }}
          >
            Send invite
          </Button>
          <span className="lc-agent-sent" aria-live="polite">
            {sent}
          </span>
        </div>
      </form>
      <section className="lc-agent-console" aria-label="Agent">
        <header className="lc-agent-head">
          <RobotIcon size={16} weight="fill" aria-hidden />
          <span>Scripted agent</span>
          <button
            type="button"
            className="lc-agent-run"
            onClick={start}
            disabled={running}
          >
            {log.length ? (
              <ArrowCounterClockwiseIcon size={13} weight="bold" aria-hidden />
            ) : (
              <PlayIcon size={13} weight="fill" aria-hidden />
            )}
            {running ? "Running" : log.length ? "Run again" : "Run"}
          </button>
        </header>
        <ol className="lc-agent-log" ref={logEnd} dir="ltr" aria-live="polite">
          {log.length ? (
            log.map((line, index) => (
              <li key={index} data-kind={line.kind}>
                {line.text}
              </li>
            ))
          ) : (
            <li data-kind="hint">
              Run reads the form with snapshot(), then fills it with act().
            </li>
          )}
        </ol>
      </section>
    </div>
  )
}

const tiles: Tile[] = [
  {
    id: "language-amount",
    title: "Type 1.234,5 in German.",
    blurb: "Each locale reads its own separators.",
    Demo: Amount,
  },
  {
    id: "language-price",
    title: "Big dollars, small cents.",
    blurb: "A price styled from its parts.",
    tone: "butter",
    Demo: Price,
  },
  {
    id: "language-hours",
    title: "Snap to a quarter.",
    blurb: "A dial on the step grid, by drag or arrows.",
    tone: "ink",
    Demo: Hours,
  },
  {
    id: "language-ranges",
    title: "This week, next month.",
    blurb: "Quick ranges that start where the locale's week does.",
    size: "tall",
    tone: "sage",
    Demo: Ranges,
  },
  {
    id: "language-recipients",
    title: "Paste a list of emails.",
    blurb: "A codec turns pasted text into tokens.",
    size: "wide",
    tone: "sky",
    Demo: Recipients,
  },
  {
    id: "language-agent",
    title: "Let the agent fill it.",
    blurb: "A scripted agent reads the form and acts through its api.",
    size: "large",
    Demo: AgentFill,
  },
]

export { tiles }
